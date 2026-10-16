//! Upload new transcript usage: lock → scan → batch → upload → advance offsets.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, FixedOffset, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::claude_json::{self, ClaudeState, OAuthAccount, UsageSnapshot};
use crate::client::{ApiClient, ApiError};
use crate::credentials::{self, Credentials};
use crate::paths::Paths;
use crate::proxy::{self, AuthCache, LogClock, PoolLog, PoolResolver, ProxyMode, SessionProxy, Verdict};
use crate::scan::{self, Offsets, Position, TranscriptFile};
use crate::sessions::{self, Inferred, SessionStates};
use crate::state::{lock_file, read_json, write_json_atomic};
use crate::timeline::{self, Timeline};
use crate::transcript::{self, TitleLine, UsageEntry};
use crate::wire::{API_KEY_ACCOUNT, Device, IngestAccount, IngestEntry, IngestRequest, IngestSession, LimitSnapshotIn};

pub const BATCH_ENTRIES: usize = 1500;
/// Largest serialized ingest request sent (the server accepts up to 1 MiB).
pub const MAX_BATCH_BYTES: usize = 900_000;
/// Server limits per ingest request (validateIngest): distinct sessions and
/// accounts, the latter including the limit-snapshot account.
pub const MAX_BATCH_SESSIONS: usize = 500;
pub const MAX_BATCH_ACCOUNTS: usize = 20;

pub struct SyncOptions {
    /// Sync only this session transcript (and its subagents) instead of sweeping.
    pub transcript: Option<PathBuf>,
    /// How long to wait for another sync to finish. Zero means skip immediately.
    pub wait_for_lock: Duration,
}

#[derive(Debug, Default, PartialEq)]
pub struct SyncReport {
    pub files_read: usize,
    pub entries_sent: usize,
    pub accepted: i64,
    pub duplicates: i64,
    pub batches: usize,
    pub skipped_locked: bool,
    /// Sessions the server deterministically rejected during this run; their
    /// entries were dropped and they will not be sent again.
    pub quarantined: Vec<String>,
    /// Session titles sent on their own (without new usage).
    pub titles_sent: usize,
    /// Title-only updates that failed (also in `sync.log`).
    pub title_errors: Vec<String>,
}

/// Appends a timestamped line to `sync.log` (best effort).
pub fn log(paths: &Paths, msg: &str) {
    let _ = fs::create_dir_all(&paths.state_dir);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(paths.sync_log()) {
        let _ = writeln!(f, "{} {msg}", Utc::now().to_rfc3339());
    }
}

/// Sessions never to upload again (`quarantine.json`), per cctrack identity:
/// `{"<server>|<user_id>": [session ids]}`. A session refused for one user
/// (e.g. it belongs to someone else) may be fine for another.
#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(transparent)]
pub struct Quarantine(pub BTreeMap<String, BTreeSet<String>>);

impl Quarantine {
    pub fn load(path: &Path) -> Self {
        read_json(path).unwrap_or_default()
    }
    /// The key for the identity in `creds`.
    pub fn scope(creds: &Credentials) -> String {
        format!("{}|{}", creds.server, creds.user.id)
    }
    pub fn contains(&self, scope: &str, session_id: &str) -> bool {
        self.0.get(scope).is_some_and(|s| s.contains(session_id))
    }
    fn insert(&mut self, scope: &str, session_id: &str) {
        self.0.entry(scope.to_string()).or_default().insert(session_id.to_string());
    }
}

/// Errors that will recur no matter how often the same request is retried.
/// 413 is not one of them: a smaller request will do. Nor is 426 (this
/// cctrack is out of date): the usage uploads once cctrack is updated.
fn deterministic(e: &ApiError) -> Option<u16> {
    match e {
        ApiError::Http(code @ (400 | 403 | 409 | 422), _) => Some(*code),
        _ => None,
    }
}

/// Converts a fatal upload error; a 401 also marks the credentials invalid
/// (if they still hold the rejected `token`).
fn fatal(paths: &Paths, token: &str, e: ApiError) -> anyhow::Error {
    match e {
        ApiError::Unauthorized(_) => match credentials::mark_invalid(paths, token) {
            Ok(()) => anyhow!("the server rejected your token (revoked or expired; HTTP 401); run `cctrack login`"),
            Err(m) => anyhow!("the server rejected your token (HTTP 401; also failed to record it: {m}); run `cctrack login`"),
        },
        e => e.into(),
    }
}

pub fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

fn to_wire(a: &OAuthAccount) -> IngestAccount {
    IngestAccount {
        account_uuid: a.account_uuid.clone(),
        org_uuid: a.organization_uuid.clone(),
        email: a.email_address.clone(),
        org_type: a.organization_type.clone(),
        rate_limit_tier: a.organization_rate_limit_tier.clone(),
    }
}

fn api_key_account() -> IngestAccount {
    IngestAccount { account_uuid: API_KEY_ACCOUNT.into(), org_uuid: None, email: None, org_type: None, rate_limit_tier: None }
}

/// How a proxied entry's pool account was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolHow {
    /// A CLIProxyAPI log line for the same session (first character) and
    /// model, started shortly before the entry.
    Matched,
    /// No line matched: the pool account that served most of the session.
    SessionUsual,
}

/// The CLIProxyAPI pool account that served an entry.
#[derive(Clone, Debug, PartialEq)]
pub struct PoolHit {
    pub account: IngestAccount,
    /// Its auth file name in the proxy home.
    pub auth: String,
    pub how: PoolHow,
    /// The auth file is gone: its identity came from the cache.
    pub cached: bool,
}

/// Decides each entry's account: the pool account that served it when the
/// session ran through a CLIProxyAPI pool, else the account timeline at the
/// entry's timestamp, falling back to the current `oauthAccount`, else API key.
pub struct Attributor<'a> {
    pub timeline: &'a Timeline,
    pub claude: &'a ClaudeState,
    /// Pool accounts by message id.
    pub pool: HashMap<String, PoolHit>,
    /// Entries of proxied sessions left with the Claude account because the
    /// pool account the log names could not be identified, by message id: why.
    pub notes: HashMap<String, String>,
}

impl<'a> Attributor<'a> {
    pub fn new(timeline: &'a Timeline, claude: &'a ClaudeState) -> Self {
        Self { timeline, claude, pool: HashMap::new(), notes: HashMap::new() }
    }

    pub fn account(&self, e: &UsageEntry) -> IngestAccount {
        match self.pool.get(&e.message_id) {
            Some(hit) => hit.account.clone(),
            None => self.claude_account(e).0,
        }
    }

    fn claude_account(&self, e: &UsageEntry) -> (IngestAccount, &'static str) {
        match self.timeline.account_at(e.ts).unwrap_or(self.claude.account.as_ref()) {
            Some(a) => (to_wire(a), "Claude account (~/.claude.json)"),
            None => (api_key_account(), "API key"),
        }
    }

    /// The account and how it was chosen.
    pub fn explain(&self, e: &UsageEntry) -> (IngestAccount, String) {
        if let Some(hit) = self.pool.get(&e.message_id) {
            let how = match hit.how {
                PoolHow::Matched => "CLIProxyAPI log",
                PoolHow::SessionUsual => "session's usual pool account",
            };
            let how = if hit.cached { format!("{how}; auth file {} gone, identity from cache", hit.auth) } else { how.to_string() };
            return (hit.account.clone(), how);
        }
        let (account, how) = self.claude_account(e);
        match self.notes.get(&e.message_id) {
            Some(note) => (account, format!("{how}; {note}")),
            None => (account, how.to_string()),
        }
    }
}

/// Builds one ingest request. Each entry's account comes from the timeline at
/// the entry's timestamp, falling back to the current account.
pub fn build_request(
    device: &Device,
    entries: &[UsageEntry],
    timeline: &Timeline,
    claude: &ClaudeState,
    snapshot: Option<&UsageSnapshot>,
) -> IngestRequest {
    build_request_with(device, entries, &Attributor::new(timeline, claude), &BTreeMap::new(), snapshot)
}

/// Builds one ingest request, attributing entries with `attr` and attaching
/// `titles` (by session id) to the sessions in it.
pub fn build_request_with(
    device: &Device,
    entries: &[UsageEntry],
    attr: &Attributor,
    titles: &BTreeMap<String, String>,
    snapshot: Option<&UsageSnapshot>,
) -> IngestRequest {
    let claude = attr.claude;
    let mut accounts: BTreeMap<String, IngestAccount> = BTreeMap::new();
    let mut sessions: BTreeMap<String, IngestSession> = BTreeMap::new();
    let mut wire_entries = Vec::with_capacity(entries.len());

    for e in entries {
        let account = attr.account(e);
        let account_uuid = account.account_uuid.clone();
        accounts.entry(account_uuid.clone()).or_insert(account);
        sessions
            .entry(e.session_id.clone())
            .and_modify(|s| {
                if e.ts < s.started_at {
                    s.started_at = e.ts;
                }
                if e.ts >= s.ended_at {
                    s.ended_at = e.ts;
                    if e.cwd.is_some() {
                        s.cwd = e.cwd.clone();
                    }
                    if e.git_branch.is_some() {
                        s.git_branch = e.git_branch.clone();
                    }
                }
            })
            .or_insert_with(|| IngestSession {
                session_id: e.session_id.clone(),
                cwd: e.cwd.clone(),
                git_branch: e.git_branch.clone(),
                started_at: e.ts,
                ended_at: e.ts,
                title: titles.get(&e.session_id).cloned(),
            });
        wire_entries.push(IngestEntry {
            session_id: e.session_id.clone(),
            account_uuid,
            message_id: e.message_id.clone(),
            request_id: e.request_id.clone(),
            model: e.model.clone(),
            ts: e.ts,
            input: e.input,
            output: e.output,
            cache_write_5m: e.cache_write_5m,
            cache_write_1h: e.cache_write_1h,
            cache_read: e.cache_read,
            speed: e.speed.clone(),
            is_subagent: e.is_subagent,
        });
    }

    let limit_snapshot = snapshot.map(|s| {
        if let Some(a) = claude.account.as_ref().filter(|a| a.account_uuid == s.account_uuid) {
            accounts.entry(a.account_uuid.clone()).or_insert_with(|| to_wire(a));
        }
        LimitSnapshotIn {
            account_uuid: s.account_uuid.clone(),
            captured_at: s.fetched_at,
            five_hour_pct: s.five_hour_pct,
            five_hour_resets_at: s.five_hour_resets_at,
            seven_day_pct: s.seven_day_pct,
            seven_day_resets_at: s.seven_day_resets_at,
        }
    });

    IngestRequest {
        device: device.clone(),
        accounts: accounts.into_values().collect(),
        sessions: sessions.into_values().collect(),
        entries: wire_entries,
        limit_snapshot,
    }
}

#[derive(Default)]
struct Batch {
    entries: Vec<UsageEntry>,
    /// Files whose offsets commit once this batch is accepted (or its
    /// refused sessions quarantined): files whose last pending entry is in
    /// it, and files whose new entries all duplicate entries already queued.
    completes: Vec<(PathBuf, Position)>,
    /// Distinct session ids and account uuids, bounded by the server's limits.
    sessions: HashSet<String>,
    accounts: HashSet<String>,
}

impl Batch {
    /// Whether adding an entry for `session`/`account` stays within the limits.
    /// The byte cap is enforced when sending (see [`Uploader::send_split`]).
    fn fits(&self, session: &str, account: &str) -> bool {
        self.entries.len() < BATCH_ENTRIES
            && (self.sessions.contains(session) || self.sessions.len() < MAX_BATCH_SESSIONS)
            && (self.accounts.contains(account) || self.accounts.len() < MAX_BATCH_ACCOUNTS)
    }
}

/// The server's message (apps/server/internal/httpapi/ingest.go) when a
/// session in the request is owned by another cctrack user.
pub const FOREIGN_SESSION_ERROR: &str = "belongs to another user";

/// A part of a batch the server refused deterministically.
struct Rejected {
    entries: Vec<UsageEntry>,
    had_snapshot: bool,
    status: u16,
    error: String,
    /// HTTP 403 with the server's ownership error: definitive for a session
    /// sent alone. Any other 403 (a proxy, a WAF) is not.
    foreign: bool,
}

struct Uploader<'a> {
    paths: &'a Paths,
    creds: &'a Credentials,
    client: &'a ApiClient,
    device: Device,
    attr: &'a Attributor<'a>,
    /// Per message id: whether the proxy log had a running request matching it.
    evidence: &'a HashMap<String, bool>,
    /// Titles to send, by session id.
    titles: &'a BTreeMap<String, String>,
    states: SessionStates,
    now: DateTime<Utc>,
    scope: String,
    quarantine: Quarantine,
    report: SyncReport,
}

impl Uploader<'_> {
    fn fatal(&self, e: ApiError) -> anyhow::Error {
        fatal(self.paths, &self.creds.token, e)
    }

    /// Sends `entries` (with `snap`), halving the request while its JSON
    /// exceeds [`MAX_BATCH_BYTES`] or the server answers 413. Deterministic
    /// rejections are collected in `rejected`; returns whether any part was
    /// accepted. A single entry refused with 413 is an error (never quarantined).
    fn send_split(&mut self, entries: &[UsageEntry], snap: Option<&UsageSnapshot>, rejected: &mut Vec<Rejected>) -> Result<bool, ApiError> {
        let req = build_request_with(&self.device, entries, self.attr, self.titles, snap);
        let size = serde_json::to_vec(&req).map_or(usize::MAX, |v| v.len());
        if size > MAX_BATCH_BYTES && entries.len() > 1 {
            return self.split(entries, snap, rejected);
        }
        match self.client.ingest(&req) {
            Ok(resp) => {
                self.report.batches += 1;
                self.report.entries_sent += entries.len();
                self.report.accepted += resp.accepted;
                self.report.duplicates += resp.duplicates;
                Ok(true)
            }
            Err(ApiError::Http(413, m)) if entries.len() > 1 => {
                log(self.paths, &format!("ingest of {} entries ({size} bytes) refused as too large (HTTP 413: {m}); splitting", entries.len()));
                self.split(entries, snap, rejected)
            }
            Err(ApiError::Http(413, m)) => {
                Err(ApiError::Http(413, format!("the server refused a {size}-byte upload of {} entries as too large: {m}", entries.len())))
            }
            Err(e) => match deterministic(&e) {
                Some(status) => {
                    let foreign = matches!(&e, ApiError::Http(403, m) if m.contains(FOREIGN_SESSION_ERROR));
                    rejected.push(Rejected { entries: entries.to_vec(), had_snapshot: snap.is_some(), status, error: e.to_string(), foreign });
                    Ok(false)
                }
                None => Err(e),
            },
        }
    }

    fn split(&mut self, entries: &[UsageEntry], snap: Option<&UsageSnapshot>, rejected: &mut Vec<Rejected>) -> Result<bool, ApiError> {
        let (a, b) = entries.split_at(entries.len() / 2);
        let first = self.send_split(a, snap, rejected)?;
        let second = self.send_split(b, None, rejected)?;
        Ok(first || second)
    }

    /// Uploads one batch. Sessions the server refuses on their own are
    /// quarantined, unless every refused session failed with the same status
    /// and nothing at all was accepted: that looks like a rejection of this
    /// device or client (clock skew, schema drift, a proxy's 403), so the sync
    /// stops instead. A 403 with the server's ownership error
    /// ([`FOREIGN_SESSION_ERROR`]) is always quarantined.
    fn send_batch(&mut self, entries: Vec<UsageEntry>, snap: Option<&UsageSnapshot>) -> Result<()> {
        // Sessions quarantined earlier in this run are not sent again.
        let entries: Vec<UsageEntry> = entries.into_iter().filter(|e| !self.quarantine.contains(&self.scope, &e.session_id)).collect();
        if entries.is_empty() && snap.is_none() {
            return Ok(());
        }
        let mut rejected = Vec::new();
        let mut accepted_any = self.send_split(&entries, snap, &mut rejected).map_err(|e| self.fatal(e))?;
        if rejected.is_empty() {
            return Ok(());
        }
        let snapshot_rejected = rejected.iter().any(|r| r.had_snapshot);
        // One bad session (e.g. HTTP 403: it belongs to another cctrack user
        // after someone else logged in on this OS account) must not wedge
        // every sync: retry per session.
        let mut by_session: BTreeMap<String, Vec<UsageEntry>> = BTreeMap::new();
        for r in rejected {
            log(self.paths, &format!("ingest of {} entries rejected with HTTP {} ({}); retrying per session", r.entries.len(), r.status, r.error));
            for e in r.entries {
                by_session.entry(e.session_id.clone()).or_default().push(e);
            }
        }
        let mut failed: Vec<(String, usize, Rejected)> = Vec::new();
        for (session_id, entries) in by_session {
            let mut r = Vec::new();
            accepted_any |= self.send_split(&entries, None, &mut r).map_err(|e| self.fatal(e))?;
            if let Some(first) = r.into_iter().next() {
                failed.push((session_id, entries.len(), first));
            }
        }
        if snapshot_rejected && let Some(s) = snap {
            let mut r = Vec::new();
            accepted_any |= self.send_split(&[], Some(s), &mut r).map_err(|e| self.fatal(e))?;
            if let Some(r) = r.first() {
                log(self.paths, &format!("dropped limit snapshot captured at {}: HTTP {} ({})", s.fetched_at.to_rfc3339(), r.status, r.error));
            }
        }
        // A session refused alone with the server's ownership 403 is
        // definitive, so it is always quarantined, even when it is the only
        // session: otherwise a lone foreign session would be retried forever
        // and block every later batch of a sweep.
        let (foreign, others): (Vec<_>, Vec<_>) = failed.into_iter().partition(|(_, _, r)| r.foreign);
        let any_foreign = !foreign.is_empty();
        self.quarantine_sessions(foreign)?;
        if let Some((_, _, first)) = others.first()
            && !accepted_any
            && others.iter().all(|(_, _, r)| r.status == first.status)
        {
            let msg = format!(
                "the server rejected every session in this upload with HTTP {} ({}); {} and the usage will be retried. \
                 Check this machine's clock, any proxy or firewall in front of the server, and that cctrack is up to date",
                first.status,
                first.error,
                if any_foreign { "only sessions owned by another user were quarantined" } else { "nothing was quarantined" },
            );
            log(self.paths, &msg);
            bail!(msg);
        }
        self.quarantine_sessions(others)
    }

    /// Quarantines refused sessions: logs, reports and persists them.
    fn quarantine_sessions(&mut self, failed: Vec<(String, usize, Rejected)>) -> Result<()> {
        if failed.is_empty() {
            return Ok(());
        }
        for (session_id, n, r) in failed {
            log(self.paths, &format!("quarantined session {session_id}: dropped {n} entries, HTTP {} ({})", r.status, r.error));
            self.quarantine.insert(&self.scope, &session_id);
            self.report.quarantined.push(session_id);
        }
        write_json_atomic(&self.paths.quarantine(), &self.quarantine, None)?;
        Ok(())
    }

    /// Records that the entries of an accepted batch (minus sessions
    /// quarantined meanwhile) were uploaded: the titles attached to them, and
    /// the pool accounts and proxy-log evidence of their entries (counted
    /// only now, so a failed upload that is retried does not count twice).
    fn note_accepted(&mut self, sent: &[(String, DateTime<Utc>, String)]) {
        for (session_id, ts, message_id) in sent {
            if self.quarantine.contains(&self.scope, session_id) {
                continue;
            }
            self.states.note_uploaded(session_id, *ts, self.now);
            if let Some(t) = self.titles.get(session_id) {
                self.states.touch(session_id, self.now).sent_title = Some(t.clone());
            }
            if let Some(hit) = self.attr.pool.get(message_id).filter(|h| h.how == PoolHow::Matched) {
                self.states.touch(session_id, self.now).note_pool(&hit.auth, *ts);
            }
            if let Some(&had) = self.evidence.get(message_id) {
                self.states.touch(session_id, self.now).note_evidence(had);
            }
        }
    }

    fn title_failed(&mut self, msg: String) {
        log(self.paths, &msg);
        self.report.title_errors.push(msg);
    }

    fn title_sent(&mut self, s: &IngestSession) {
        if let Some(t) = &s.title {
            self.states.touch(&s.session_id, self.now).sent_title = Some(t.clone());
            self.report.titles_sent += 1;
        }
    }

    /// Sends title changes of sessions without new usage, as sessions with
    /// no entries over the time range already uploaded (which leaves the
    /// server's range and metadata as they are). A title the server refuses
    /// is logged and not retried; a network or server error leaves the
    /// titles for the next sync. A rejected token or an outdated cctrack
    /// (401, 426) fails the sync, also leaving the titles for later.
    fn send_titles(&mut self, sessions: Vec<IngestSession>) -> Result<()> {
        let request = |s: &[IngestSession], device: &Device| IngestRequest {
            device: device.clone(),
            accounts: vec![],
            sessions: s.to_vec(),
            entries: vec![],
            limit_snapshot: None,
        };
        for chunk in sessions.chunks(MAX_BATCH_SESSIONS) {
            match self.client.ingest(&request(chunk, &self.device)) {
                Ok(_) => chunk.iter().for_each(|s| self.title_sent(s)),
                Err(e @ (ApiError::Unauthorized(_) | ApiError::Outdated { .. })) => return Err(self.fatal(e)),
                Err(e) if deterministic(&e).is_some() => {
                    for s in chunk {
                        match self.client.ingest(&request(std::slice::from_ref(s), &self.device)) {
                            Ok(_) => self.title_sent(s),
                            Err(e @ (ApiError::Unauthorized(_) | ApiError::Outdated { .. })) => return Err(self.fatal(e)),
                            Err(e) if deterministic(&e).is_some() => {
                                self.title_failed(format!("title of session {} refused ({e}); not retried", s.session_id));
                                if let Some(t) = &s.title {
                                    self.states.touch(&s.session_id, self.now).sent_title = Some(t.clone());
                                }
                            }
                            Err(e) => {
                                self.title_failed(format!("title update failed ({e}); retrying next sync"));
                                return Ok(());
                            }
                        }
                    }
                }
                Err(e) => {
                    self.title_failed(format!("title update failed ({e}); retrying next sync"));
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

/// New usage read from transcripts.
struct Scanned {
    files_read: usize,
    /// Files with new uploadable usage. A file's entries may be empty when
    /// they all duplicate entries queued from an earlier file (e.g. a copied
    /// transcript): its offset then commits with the batch carrying those.
    pending: Vec<(PathBuf, Position, Vec<UsageEntry>)>,
}

/// Reads new lines of `files` (from the start with `rescan`), recording
/// titles of main transcripts in `states`. Offsets of files with nothing to
/// upload are advanced in `offsets`.
fn scan_files(
    files: &[TranscriptFile],
    offsets: &mut Offsets,
    rescan: bool,
    creds: &Credentials,
    quarantined: impl Fn(&str) -> bool,
    states: &mut SessionStates,
    now: DateTime<Utc>,
) -> Scanned {
    let mut out = Scanned { files_read: 0, pending: Vec::new() };
    let mut seen = HashSet::new();
    for f in files {
        let stored = if rescan { Position::default() } else { offsets.get(&f.path) };
        let Ok(new) = scan::read_new_lines(&f.path, &stored) else { continue };
        if new.lines.is_empty() {
            if new.end != stored && !rescan {
                offsets.set(&f.path, new.end); // rewritten, or an older offsets format
            }
            continue;
        }
        if !f.subagent {
            for t in new.lines.iter().filter_map(|l| transcript::parse_title(l, &f.session_hint)) {
                if !quarantined(title_session(&t)) {
                    states.note_title(t, now);
                }
            }
        }
        out.files_read += 1;
        let candidates: Vec<UsageEntry> = new
            .lines
            .iter()
            .filter_map(|l| transcript::parse_line(l, f.subagent, &f.session_hint))
            .filter(|e| e.ts >= creds.logged_in_at)
            .filter(|e| !quarantined(&e.session_id))
            .collect();
        if candidates.is_empty() {
            offsets.set(&f.path, new.end); // nothing to upload from these bytes
            continue;
        }
        let entries = candidates.into_iter().filter(|e| seen.insert(e.message_id.clone())).collect();
        out.pending.push((f.path.clone(), new.end, entries));
    }
    out
}

fn title_session(t: &TitleLine) -> &str {
    match t {
        TitleLine::Ai { session_id, .. } | TitleLine::Custom { session_id, .. } => session_id,
    }
}

/// One-time pass when `sessions.json` does not exist yet (the first sync of
/// a version that keeps it): titles in the bytes of main transcripts that
/// earlier syncs already read are recorded, with the time range of the
/// usage in those bytes that was uploaded (since login), so a title-only
/// update can be sent for sessions uploaded before. Reads only; offsets
/// stay as they are and no entry is queued.
fn backfill_titles(
    files: &[TranscriptFile],
    offsets: &Offsets,
    creds: &Credentials,
    quarantined: impl Fn(&str) -> bool,
    states: &mut SessionStates,
    now: DateTime<Utc>,
) {
    for f in files.iter().filter(|f| !f.subagent) {
        let stored = offsets.get(&f.path);
        if stored.offset == 0 {
            continue; // never read: scan_files reads it from the start
        }
        let mut titles = Vec::new();
        // parse_title skips lines without `-title"` before parsing JSON.
        let read = scan::for_each_line_before(&f.path, &stored, |l| titles.extend(transcript::parse_title(l, &f.session_hint)));
        titles.retain(|t| !quarantined(title_session(t)));
        if !matches!(read, Ok(true)) || titles.is_empty() {
            continue;
        }
        let ids: HashSet<String> = titles.iter().map(|t| title_session(t).to_string()).collect();
        let mut range: HashMap<String, (DateTime<Utc>, DateTime<Utc>)> = HashMap::new();
        let _ = scan::for_each_line_before(&f.path, &stored, |l| {
            if let Some(e) = transcript::parse_line(l, false, &f.session_hint)
                && e.ts >= creds.logged_in_at
                && ids.contains(&e.session_id)
            {
                let r = range.entry(e.session_id).or_insert((e.ts, e.ts));
                r.0 = r.0.min(e.ts);
                r.1 = r.1.max(e.ts);
            }
        });
        for t in titles {
            states.note_title(t, now);
        }
        for (id, (first, last)) in range {
            states.note_uploaded(&id, first, now);
            states.note_uploaded(&id, last, now);
        }
    }
}

/// What the proxy logs say about one run's entries.
#[derive(Default)]
struct PoolOutcome {
    hits: HashMap<String, PoolHit>,
    /// Message ids left with the Claude account although their session is
    /// proxied (the pool account named could not be identified): why.
    notes: HashMap<String, String>,
    /// Per message id compared with a proxy log: whether a request of a
    /// session with the same first character and model was running then.
    evidence: HashMap<String, bool>,
    /// Per session: Ok(why) when attributed from a log, Err(why not).
    decisions: BTreeMap<String, Result<String, String>>,
    /// Sessions credited to the Claude account that look proxied: a note.
    looks_proxied: BTreeMap<String, String>,
    warnings: Vec<String>,
    /// To save after a real (not dry) run.
    auths: AuthCache,
    clock: LogClock,
}

fn load_log<Tz: TimeZone>(dir: &Path, times: &[DateTime<Utc>], chars: &HashSet<char>, offset: Option<FixedOffset>, tz: &Tz) -> PoolLog {
    let windows = proxy::read_windows(times);
    match offset {
        Some(o) => PoolLog::load_windows(dir, &windows, chars, &o),
        None => PoolLog::load_windows(dir, &windows, chars, tz),
    }
}

fn inferred_verdict(dir: PathBuf, inf: &Inferred, session_id: &str) -> Verdict {
    let c = session_id.chars().next().unwrap_or('?');
    let what = format!(
        "{} of {} entries ran during a request of a session starting with '{c}' (no hook record and no ANTHROPIC_BASE_URL; decided {})",
        inf.matched,
        inf.total,
        inf.decided_at.format("%Y-%m-%d %H:%M UTC")
    );
    if inf.proxied {
        let log = dir.join(proxy::MAIN_LOG);
        Verdict::Proxied(SessionProxy { why: format!("inferred from the proxy log: {what}; pool account read from {}", log.display()), dir })
    } else {
        Verdict::NotProxied {
            why: format!("inferred from the proxy log as not proxied: only {what}, below {}%", proxy::INFER_PERCENT),
            dir: Some(dir),
        }
    }
}

/// Per session, whether its usage is attributed from a CLIProxyAPI log, and
/// the pool account of each entry so attributed (by message id).
///
/// Sessions with no hook record and no `ANTHROPIC_BASE_URL` are decided from
/// the log ([`proxy::infer`]) and the decision is kept in `states`. Every
/// session is compared with a log when one exists (not in mode off), to flag
/// sessions that look proxied but are credited otherwise. When a session
/// known to be proxied matches nothing, the log's clock is calibrated
/// ([`proxy::calibrate`]). Pool account counts and affinity in `states` are
/// only read here; they are updated once an upload is accepted.
fn pool_attribution<Tz: TimeZone>(
    paths: &Paths,
    mode: ProxyMode,
    entries: &[&UsageEntry],
    states: &mut SessionStates,
    now: DateTime<Utc>,
    tz: &Tz,
) -> PoolOutcome {
    let mut out = PoolOutcome { auths: AuthCache::load(paths), clock: LogClock::load(paths), ..Default::default() };
    let mut cwd: HashMap<&str, &str> = HashMap::new();
    for e in entries {
        if let Some(c) = e.cwd.as_deref() {
            cwd.entry(&e.session_id).or_insert(c);
        }
    }
    let mut verdicts: BTreeMap<String, Verdict> = BTreeMap::new();
    for e in entries {
        if !verdicts.contains_key(&e.session_id) {
            let v = proxy::session_verdict(paths, mode, &e.session_id, cwd.get(e.session_id.as_str()).copied(), now);
            verdicts.insert(e.session_id.clone(), v);
        }
    }
    // Only sessions a hook recorded as running through a CLIProxyAPI whose
    // log was there calibrate a log's clock: not mode on alone, nor a
    // settings URL (which may be another local proxy).
    let hooked: HashMap<String, PathBuf> = verdicts
        .iter()
        .filter(|(_, v)| matches!(v, Verdict::Proxied(_)))
        .filter_map(|(id, _)| {
            let m = proxy::session_marker(paths, id).filter(|m| m.proxy_url && m.log_found)?;
            Some((id.clone(), m.cliproxy_dir))
        })
        .collect();
    // An earlier inference is kept.
    for (id, v) in verdicts.iter_mut() {
        if let Verdict::Infer { dir } = v
            && let Some(inf) = states.get(id).and_then(|s| s.inferred.clone())
        {
            *v = inferred_verdict(dir.clone(), &inf, id);
        }
    }

    // Which log each session's entries are compared with.
    let log_dir = |v: &Verdict| match v {
        Verdict::Proxied(sp) => Some(sp.dir.clone()),
        Verdict::NotProxied { dir, .. } => dir.clone(),
        Verdict::Infer { dir } => Some(dir.clone()),
    };
    let mut dirs: BTreeMap<PathBuf, (Vec<DateTime<Utc>>, HashSet<char>)> = BTreeMap::new();
    let mut entry_dirs: Vec<(&UsageEntry, PathBuf)> = Vec::new();
    for e in entries {
        let Some(dir) = verdicts.get(&e.session_id).and_then(log_dir) else { continue };
        let (times, chars) = dirs.entry(dir.clone()).or_default();
        times.push(e.ts);
        if let Some(c) = e.session_id.chars().next() {
            chars.insert(c.to_ascii_lowercase());
        }
        entry_dirs.push((e, dir));
    }
    let mut logs: HashMap<PathBuf, PoolLog> = HashMap::new();
    for (dir, (times, chars)) in &dirs {
        logs.insert(dir.clone(), load_log(dir, times, chars, out.clock.offset(dir), tz));
    }
    let evidence_for = |logs: &HashMap<PathBuf, PoolLog>, dir: &Path, out: &mut PoolOutcome| {
        for (e, d) in &entry_dirs {
            if d == dir {
                let had = logs.get(d).is_some_and(|l| proxy::has_running_candidate(&l.selections, &e.session_id, &e.model, e.ts));
                out.evidence.insert(e.message_id.clone(), had);
            }
        }
    };
    for dir in dirs.keys() {
        evidence_for(&logs, dir, &mut out);
    }

    // A hook-recorded proxied session whose entries all match nothing: the
    // log's times may not be this machine's local time. At least two such
    // sessions are needed (a per-turn sync of one session never calibrates).
    for (dir, (times, chars)) in &dirs {
        let probes: Vec<&UsageEntry> =
            entry_dirs.iter().filter(|(e, d)| d == dir && hooked.get(&e.session_id) == Some(d)).map(|(e, _)| *e).collect();
        let mut per_session: BTreeMap<&str, usize> = BTreeMap::new();
        for e in &probes {
            *per_session.entry(e.session_id.as_str()).or_default() += usize::from(out.evidence[&e.message_id]);
        }
        if per_session.len() < 2 || !per_session.values().any(|&m| m == 0) || !out.clock.may_calibrate(dir, now) {
            continue;
        }
        let current: Vec<bool> = probes.iter().map(|e| out.evidence[&e.message_id]).collect();
        let wide: Vec<(DateTime<Utc>, DateTime<Utc>)> = proxy::read_windows(times)
            .into_iter()
            .map(|(f, t)| (f - chrono::Duration::hours(14), t + chrono::Duration::hours(14)))
            .collect();
        let naive = PoolLog::load_windows(dir, &wide, chars, &Utc);
        let probe: Vec<(&str, &str, DateTime<Utc>)> = probes.iter().map(|e| (e.session_id.as_str(), e.model.as_str(), e.ts)).collect();
        // The new offset must not cost any session compared with this log
        // (hook-recorded or not) a match it has now.
        let found = proxy::calibrate(&naive.selections, &probe, &current).and_then(|(o, n)| {
            let log = load_log(dir, times, chars, FixedOffset::east_opt(o * 60), tz);
            let mut per: HashMap<&str, (usize, usize)> = HashMap::new();
            for (e, _) in entry_dirs.iter().filter(|(_, d)| d == dir) {
                let c = per.entry(e.session_id.as_str()).or_default();
                c.0 += usize::from(out.evidence[&e.message_id]);
                c.1 += usize::from(proxy::has_running_candidate(&log.selections, &e.session_id, &e.model, e.ts));
            }
            per.values().all(|&(was, new)| new >= was).then_some((o, n, log))
        });
        out.clock.record(dir, found.as_ref().map(|&(o, n, _)| (o, n)), now);
        if let Some((_, _, log)) = found {
            logs.insert(dir.clone(), log);
            evidence_for(&logs, dir, &mut out);
        }
    }
    for (dir, log) in &logs {
        if let Some(w) = out.clock.warning(dir) {
            out.warnings.push(w);
        }
        if log.dst_ambiguous + log.dst_nonexistent > 0 {
            out.warnings.push(format!(
                "{} lines of {} have a local time repeated by a DST change and {} one skipped by it; they were read with the earliest valid UTC time",
                log.dst_ambiguous,
                dir.join(proxy::MAIN_LOG).display(),
                log.dst_nonexistent
            ));
        }
    }

    // Evidence per session: earlier uploads plus this run.
    let mut run_evidence: HashMap<&str, (u64, u64)> = HashMap::new();
    for (e, _) in &entry_dirs {
        let c = run_evidence.entry(e.session_id.as_str()).or_default();
        c.0 += u64::from(out.evidence[&e.message_id]);
        c.1 += 1;
    }
    let evidence = |states: &SessionStates, id: &str| {
        let prior = states.get(id).and_then(|s| s.log_evidence).unwrap_or_default();
        let (m, n) = run_evidence.get(id).copied().unwrap_or_default();
        (prior.matched + m, prior.total + n)
    };
    for (id, v) in verdicts.iter_mut() {
        let Verdict::Infer { dir } = v else { continue };
        let (m, n) = evidence(states, id);
        *v = match proxy::infer(m, n) {
            Some(proxied) => {
                let inf = Inferred { proxied, matched: m, total: n, decided_at: now };
                states.touch(id, now).inferred = Some(inf.clone());
                inferred_verdict(dir.clone(), &inf, id)
            }
            None => Verdict::NotProxied {
                why: format!(
                    "no hook record and no ANTHROPIC_BASE_URL, and only {n} entries so far ({} needed to infer from the proxy log); credited to the Claude account",
                    proxy::INFER_MIN_ENTRIES
                ),
                dir: Some(dir.clone()),
            },
        };
    }
    for (id, v) in &verdicts {
        match v {
            Verdict::Proxied(sp) => {
                out.decisions.insert(id.clone(), Ok(sp.why.clone()));
                if mode != ProxyMode::Off {
                    states.touch(id, now).not_proxied = None;
                }
            }
            Verdict::NotProxied { why, dir } => {
                out.decisions.insert(id.clone(), Err(why.clone()));
                if mode == ProxyMode::Off {
                    continue;
                }
                states.touch(id, now).not_proxied = Some(why.clone());
                let (m, n) = evidence(states, id);
                if dir.is_some() && proxy::infer(m, n) == Some(true) {
                    out.looks_proxied.insert(
                        id.clone(),
                        format!("looks proxied: {m} of {n} entries ran during a matching request in the proxy log, but it is credited to the Claude account ({why})"),
                    );
                }
            }
            Verdict::Infer { .. } => {}
        }
    }

    // Attribution, per session in time order (for affinity).
    let mut resolver = PoolResolver::with_cache(std::mem::take(&mut out.auths));
    let mut by_session: BTreeMap<&str, Vec<&UsageEntry>> = BTreeMap::new();
    for e in entries {
        if matches!(verdicts.get(&e.session_id), Some(Verdict::Proxied(_))) {
            by_session.entry(e.session_id.as_str()).or_default().push(e);
        }
    }
    let mut run_counts: HashMap<&str, BTreeMap<String, u64>> = HashMap::new();
    let mut unmatched: Vec<(&UsageEntry, PathBuf)> = Vec::new();
    for (id, mut es) in by_session {
        let Some(Verdict::Proxied(sp)) = verdicts.get(id) else { continue };
        let Some(log) = logs.get(&sp.dir) else { continue };
        es.sort_by_key(|e| e.ts);
        let mut affinity = states.get(id).and_then(|s| s.last_pool_auth.clone());
        for e in es {
            let Some(sel) = proxy::match_entry(&log.selections, id, &e.model, e.ts, affinity.as_deref()) else {
                unmatched.push((e, sp.dir.clone()));
                continue;
            };
            let auth = sel.auth.clone();
            match resolver.resolve(&sp.dir, &auth) {
                Some((account, cached)) => {
                    *run_counts.entry(id).or_default().entry(auth.clone()).or_default() += 1;
                    out.hits.insert(e.message_id.clone(), PoolHit { account, auth: auth.clone(), how: PoolHow::Matched, cached });
                }
                // Never another account's: the log says which one served it.
                None => {
                    out.notes.insert(
                        e.message_id.clone(),
                        format!("the proxy log names pool auth {auth}, whose auth file is gone and whose identity was never cached"),
                    );
                }
            }
            affinity = Some(auth);
        }
    }
    // No log line: the pool account that served most of the session.
    for (e, dir) in unmatched {
        let mut counts = states.get(&e.session_id).map(|s| s.pool_auths.clone()).unwrap_or_default();
        for (auth, n) in run_counts.get(e.session_id.as_str()).into_iter().flatten() {
            *counts.entry(auth.clone()).or_default() += n;
        }
        let Some(auth) = sessions::top_auth(&counts).map(String::from) else { continue };
        match resolver.resolve(&dir, &auth) {
            Some((account, cached)) => {
                out.hits.insert(e.message_id.clone(), PoolHit { account, auth, how: PoolHow::SessionUsual, cached });
            }
            None => {
                out.notes.insert(
                    e.message_id.clone(),
                    format!("no proxy log line matched, and the session's usual pool auth {auth} has no auth file and no cached identity"),
                );
            }
        }
    }
    out.auths = resolver.cache;
    out
}

/// Titles to send, by session id: changed since last sent, not quarantined.
fn pending_titles(states: &SessionStates, quarantine: &Quarantine, scope: &str) -> BTreeMap<String, String> {
    states
        .sessions
        .iter()
        .filter(|(id, _)| !quarantine.contains(scope, id))
        .filter_map(|(id, s)| s.pending_title().map(|t| (id.clone(), t.to_string())))
        .collect()
}

/// Title-only session updates: titles of sessions with no new usage in
/// this run whose usage was uploaded before (a title alone never creates a
/// session on the server).
fn title_only(states: &SessionStates, titles: &BTreeMap<String, String>, with_usage: &HashSet<&str>) -> Vec<IngestSession> {
    titles
        .iter()
        .filter(|(id, _)| !with_usage.contains(id.as_str()))
        .filter_map(|(id, t)| {
            let (started_at, ended_at) = states.get(id)?.uploaded_range()?;
            Some(IngestSession { session_id: id.clone(), cwd: None, git_branch: None, started_at, ended_at, title: Some(t.clone()) })
        })
        .collect()
}

/// A transcript named with `--transcript` must exist.
fn check_transcript(opts: &SyncOptions) -> Result<()> {
    match &opts.transcript {
        Some(t) if !t.is_file() => bail!("no transcript at {}", t.display()),
        _ => Ok(()),
    }
}

fn files_to_scan(paths: &Paths, creds: &Credentials, opts: &SyncOptions) -> Vec<TranscriptFile> {
    match &opts.transcript {
        Some(t) => scan::session_files(t),
        None => sweep_files(paths, creds),
    }
}

/// Every transcript written since shortly before login.
fn sweep_files(paths: &Paths, creds: &Credentials) -> Vec<TranscriptFile> {
    let since = SystemTime::from(creds.logged_in_at).checked_sub(Duration::from_secs(3600));
    scan::discover(&paths.projects_dir(), since)
}

pub fn run(paths: &Paths, creds: &Credentials, client: &ApiClient, opts: &SyncOptions, now: DateTime<Utc>) -> Result<SyncReport> {
    run_with(paths, creds, client, opts, proxy::env_mode().as_deref(), now)
}

/// [`run`] with `env_mode` as the value of `CCTRACK_PROXY_MODE`.
pub fn run_with(
    paths: &Paths,
    creds: &Credentials,
    client: &ApiClient,
    opts: &SyncOptions,
    env_mode: Option<&str>,
    now: DateTime<Utc>,
) -> Result<SyncReport> {
    check_transcript(opts)?;
    let mut report = SyncReport::default();
    let Some(_lock) = lock_file(&paths.sync_lock(), opts.wait_for_lock)? else {
        report.skipped_locked = true;
        return Ok(report);
    };

    // A missing or torn ~/.claude.json must not be recorded as API-key auth.
    let checked = claude_json::read_checked(&paths.claude_json);
    let timeline = match &checked {
        Some(c) => timeline::record(paths, now, c.account.clone()),
        None => Timeline::load(&paths.timeline()),
    };
    let claude = checked.unwrap_or_default();

    let files = files_to_scan(paths, creds, opts);
    let scope = Quarantine::scope(creds);
    let quarantine = Quarantine::load(&paths.quarantine());
    let mut offsets = Offsets::load(&paths.offsets());
    let fresh = !paths.sessions().exists();
    let mut states = SessionStates::load(&paths.sessions());
    if fresh {
        backfill_titles(&sweep_files(paths, creds), &offsets, creds, |id| quarantine.contains(&scope, id), &mut states, now);
    }
    let scanned = scan_files(&files, &mut offsets, false, creds, |id| quarantine.contains(&scope, id), &mut states, now);
    report.files_read = scanned.files_read;
    let pending = scanned.pending;
    // Titles first: the offsets below may move past the lines that held them.
    states.save(&paths.sessions(), now)?;
    offsets.save(&paths.offsets())?;

    let all: Vec<&UsageEntry> = pending.iter().flat_map(|(_, _, es)| es.iter()).collect();
    if let Some(w) = proxy::mode_env_warning(env_mode) {
        log(paths, &w);
    }
    let (pmode, _) = proxy::mode(paths, env_mode);
    let outcome = pool_attribution(paths, pmode, &all, &mut states, now, &Local);
    // Inferred decisions and auth identities are kept now; pool account
    // counts only once their upload is accepted.
    states.save(&paths.sessions(), now)?;
    outcome.auths.save(paths)?;
    outcome.clock.save(paths)?;
    let evidence = outcome.evidence;
    let attr = Attributor { timeline: &timeline, claude: &claude, pool: outcome.hits, notes: outcome.notes };
    let titles = pending_titles(&states, &quarantine, &scope);
    let with_usage: HashSet<&str> = all.iter().map(|e| e.session_id.as_str()).collect();
    let title_updates = title_only(&states, &titles, &with_usage);
    drop(with_usage);
    drop(all);

    let last_sent: Option<DateTime<Utc>> = read_json(&paths.last_snapshot());
    let current_uuid = claude.account.as_ref().map(|a| a.account_uuid.as_str());
    let snapshot = claude
        .snapshot
        .as_ref()
        .filter(|s| Some(s.account_uuid.as_str()) == current_uuid)
        .filter(|s| last_sent.is_none_or(|l| s.fetched_at > l))
        .cloned();

    if pending.iter().all(|(_, _, entries)| entries.is_empty()) && snapshot.is_none() && title_updates.is_empty() {
        // Nothing to upload, so nothing would notice a revoked token: check it
        // (one cheap request) so the gate blocks before the next prompt. An
        // outdated CLI is reported too: later uploads would fail.
        if let Err(e @ (ApiError::Unauthorized(_) | ApiError::Outdated { .. })) = client.me() {
            return Err(fatal(paths, &creds.token, e));
        }
        return Ok(report);
    }

    // The first batch carries the snapshot, whose account counts toward its limit.
    let mut first = Batch::default();
    if let Some(s) = &snapshot {
        first.accounts.insert(s.account_uuid.clone());
    }
    let mut batches = vec![first];
    for (path, end, entries) in pending {
        for e in entries {
            let account = attr.account(&e).account_uuid;
            if !batches.last().unwrap().fits(&e.session_id, &account) {
                batches.push(Batch::default());
            }
            let b = batches.last_mut().unwrap();
            b.sessions.insert(e.session_id.clone());
            b.accounts.insert(account);
            b.entries.push(e);
        }
        // A file's offset commits only with the batch holding its last entry,
        // or (all duplicates) the latest batch so far, which is at or after
        // every batch holding the entries it duplicates.
        batches.last_mut().unwrap().completes.push((path, end));
    }

    let mut up = Uploader {
        paths,
        creds,
        client,
        device: Device { hostname: hostname(), os: std::env::consts::OS.to_string() },
        attr: &attr,
        evidence: &evidence,
        titles: &titles,
        states,
        now,
        scope,
        quarantine,
        report,
    };
    for (i, batch) in batches.into_iter().enumerate() {
        let snap = if i == 0 { snapshot.as_ref() } else { None };
        let sent: Vec<(String, DateTime<Utc>, String)> =
            batch.entries.iter().map(|e| (e.session_id.clone(), e.ts, e.message_id.clone())).collect();
        up.send_batch(batch.entries, snap)?;
        // Every entry of this batch is now accepted or quarantined.
        up.note_accepted(&sent);
        up.states.save(&paths.sessions(), now)?;
        for (path, end) in batch.completes {
            offsets.set(&path, end);
        }
        offsets.save(&paths.offsets())?;
        if let Some(s) = snap {
            write_json_atomic(&paths.last_snapshot(), &s.fetched_at, None)?;
        }
    }
    let title_updates: Vec<IngestSession> =
        title_updates.into_iter().filter(|s| !up.quarantine.contains(&up.scope, &s.session_id)).collect();
    if !title_updates.is_empty() {
        up.send_titles(title_updates)?;
        up.states.save(&paths.sessions(), now)?;
    }
    proxy::prune_markers(paths, Duration::from_secs(sessions::RETENTION.num_seconds().max(0) as u64));
    Ok(up.report)
}

/// One session in a dry run.
#[derive(Debug, Default, PartialEq)]
pub struct DryRunSession {
    pub session_id: String,
    pub title: Option<String>,
    /// Whether the title would be sent.
    pub title_pending: bool,
    pub entries: usize,
    /// Ok(why) when attributed from a CLIProxyAPI log, Err(why not) otherwise.
    pub proxy: Option<Result<String, String>>,
    /// Set when credited to the Claude account although its entries ran
    /// during matching requests in the proxy log.
    pub looks_proxied: Option<String>,
    /// Entries per (account label, how it was chosen).
    pub accounts: BTreeMap<(String, String), usize>,
}

#[derive(Debug, Default)]
pub struct DryRun {
    pub mode: Option<(ProxyMode, proxy::ModeSource)>,
    pub files_read: usize,
    pub sessions: Vec<DryRunSession>,
    /// An invalid `CCTRACK_PROXY_MODE`, a calibrated proxy log clock, …
    pub warnings: Vec<String>,
}

/// What a sync would upload, with each entry's account and why, without
/// uploading or changing any state. With `rescan`, transcripts are read from
/// the start (usage since login) instead of from the stored offsets.
pub fn dry_run(paths: &Paths, creds: &Credentials, opts: &SyncOptions, rescan: bool, now: DateTime<Utc>) -> Result<DryRun> {
    dry_run_with(paths, creds, opts, rescan, proxy::env_mode().as_deref(), now)
}

/// [`dry_run`] with `env_mode` as the value of `CCTRACK_PROXY_MODE`.
pub fn dry_run_with(
    paths: &Paths,
    creds: &Credentials,
    opts: &SyncOptions,
    rescan: bool,
    env_mode: Option<&str>,
    now: DateTime<Utc>,
) -> Result<DryRun> {
    check_transcript(opts)?;
    let timeline = Timeline::load(&paths.timeline());
    let claude = claude_json::read_checked(&paths.claude_json).unwrap_or_default();
    let files = files_to_scan(paths, creds, opts);
    let scope = Quarantine::scope(creds);
    let quarantine = Quarantine::load(&paths.quarantine());
    let mut offsets = Offsets::load(&paths.offsets());
    let fresh = !paths.sessions().exists();
    let mut states = SessionStates::load(&paths.sessions());
    if fresh {
        backfill_titles(&sweep_files(paths, creds), &offsets, creds, |id| quarantine.contains(&scope, id), &mut states, now);
    }
    if rescan {
        // As if nothing had been sent: every title is shown as pending, and
        // nothing counted from uploads (an inferred decision is kept).
        for s in states.sessions.values_mut() {
            s.sent_title = None;
            s.pool_auths.clear();
            s.last_pool_auth = None;
            s.last_pool_at = None;
            s.log_evidence = None;
        }
    }
    let scanned = scan_files(&files, &mut offsets, rescan, creds, |id| quarantine.contains(&scope, id), &mut states, now);
    let all: Vec<&UsageEntry> = scanned.pending.iter().flat_map(|(_, _, es)| es.iter()).collect();
    let mode = proxy::mode(paths, env_mode);
    let outcome = pool_attribution(paths, mode.0, &all, &mut states, now, &Local);
    let mut warnings: Vec<String> = proxy::mode_env_warning(env_mode).into_iter().collect();
    warnings.extend(outcome.warnings);
    let decisions = outcome.decisions;
    let mut looks_proxied = outcome.looks_proxied;
    let attr = Attributor { timeline: &timeline, claude: &claude, pool: outcome.hits, notes: outcome.notes };
    let titles = pending_titles(&states, &quarantine, &scope);

    let mut by_session: BTreeMap<String, DryRunSession> = BTreeMap::new();
    for e in &all {
        let s = by_session.entry(e.session_id.clone()).or_insert_with(|| DryRunSession {
            session_id: e.session_id.clone(),
            proxy: decisions.get(&e.session_id).cloned(),
            looks_proxied: looks_proxied.remove(&e.session_id),
            ..Default::default()
        });
        s.entries += 1;
        let (account, how) = attr.explain(e);
        *s.accounts.entry((account.email.unwrap_or(account.account_uuid), how)).or_default() += 1;
    }
    let with_usage: HashSet<&str> = all.iter().map(|e| e.session_id.as_str()).collect();
    for u in title_only(&states, &titles, &with_usage) {
        by_session.entry(u.session_id.clone()).or_insert_with(|| DryRunSession { session_id: u.session_id.clone(), ..Default::default() });
    }
    for s in by_session.values_mut() {
        let st = states.get(&s.session_id);
        s.title = st.and_then(|st| st.title()).map(String::from);
        s.title_pending = titles.contains_key(&s.session_id) && (s.entries > 0 || st.and_then(|st| st.uploaded_range()).is_some());
    }
    Ok(DryRun { mode: Some(mode), files_read: scanned.files_read, sessions: by_session.into_values().collect(), warnings })
}


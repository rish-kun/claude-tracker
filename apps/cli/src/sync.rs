//! Upload new transcript usage: lock → scan → batch → upload → advance offsets.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::claude_json::{self, ClaudeState, OAuthAccount, UsageSnapshot};
use crate::client::{ApiClient, ApiError};
use crate::credentials::{self, Credentials};
use crate::paths::Paths;
use crate::scan::{self, Offsets, Position};
use crate::state::{lock_file, read_json, write_json_atomic};
use crate::timeline::{self, Timeline};
use crate::transcript::{self, UsageEntry};
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
/// 413 is not one of them: a smaller request will do.
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

/// The account an entry is attributed to: the timeline at its timestamp,
/// falling back to the current account. None means API-key auth.
fn account_for<'a>(timeline: &'a Timeline, claude: &'a ClaudeState, ts: DateTime<Utc>) -> Option<&'a OAuthAccount> {
    timeline.account_at(ts).unwrap_or(claude.account.as_ref())
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
    let mut accounts: BTreeMap<String, IngestAccount> = BTreeMap::new();
    let mut sessions: BTreeMap<String, IngestSession> = BTreeMap::new();
    let mut wire_entries = Vec::with_capacity(entries.len());

    for e in entries {
        let account = account_for(timeline, claude, e.ts).cloned();
        let account_uuid = match account {
            Some(a) => {
                accounts.entry(a.account_uuid.clone()).or_insert_with(|| to_wire(&a));
                a.account_uuid
            }
            None => {
                accounts.entry(API_KEY_ACCOUNT.into()).or_insert_with(|| IngestAccount {
                    account_uuid: API_KEY_ACCOUNT.into(),
                    org_uuid: None,
                    email: None,
                    org_type: None,
                    rate_limit_tier: None,
                });
                API_KEY_ACCOUNT.into()
            }
        };
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
    timeline: &'a Timeline,
    claude: &'a ClaudeState,
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
        let req = build_request(&self.device, entries, self.timeline, self.claude, snap);
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
}

pub fn run(paths: &Paths, creds: &Credentials, client: &ApiClient, opts: &SyncOptions, now: DateTime<Utc>) -> Result<SyncReport> {
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

    let files = match &opts.transcript {
        Some(t) => scan::session_files(t),
        None => {
            let since = SystemTime::from(creds.logged_in_at).checked_sub(Duration::from_secs(3600));
            scan::discover(&paths.projects_dir(), since)
        }
    };

    let scope = Quarantine::scope(creds);
    let quarantine = Quarantine::load(&paths.quarantine());
    let mut offsets = Offsets::load(&paths.offsets());
    let mut seen = HashSet::new();
    // Files with new uploadable usage. A file's entries may be empty when
    // they all duplicate entries queued from an earlier file (e.g. a copied
    // transcript): its offset then commits with the batch carrying those.
    let mut pending: Vec<(PathBuf, Position, Vec<UsageEntry>)> = Vec::new();
    for f in &files {
        let stored = offsets.get(&f.path);
        let Ok(new) = scan::read_new_lines(&f.path, &stored) else { continue };
        if new.lines.is_empty() {
            if new.end != stored {
                offsets.set(&f.path, new.end); // rewritten, or an older offsets format
            }
            continue;
        }
        report.files_read += 1;
        let candidates: Vec<UsageEntry> = new
            .lines
            .iter()
            .filter_map(|l| transcript::parse_line(l, f.subagent, &f.session_hint))
            .filter(|e| e.ts >= creds.logged_in_at)
            .filter(|e| !quarantine.contains(&scope, &e.session_id))
            .collect();
        if candidates.is_empty() {
            offsets.set(&f.path, new.end); // nothing to upload from these bytes
            continue;
        }
        let entries = candidates.into_iter().filter(|e| seen.insert(e.message_id.clone())).collect();
        pending.push((f.path.clone(), new.end, entries));
    }
    offsets.save(&paths.offsets())?;

    let last_sent: Option<DateTime<Utc>> = read_json(&paths.last_snapshot());
    let current_uuid = claude.account.as_ref().map(|a| a.account_uuid.as_str());
    let snapshot = claude
        .snapshot
        .as_ref()
        .filter(|s| Some(s.account_uuid.as_str()) == current_uuid)
        .filter(|s| last_sent.is_none_or(|l| s.fetched_at > l))
        .cloned();

    if pending.iter().all(|(_, _, entries)| entries.is_empty()) && snapshot.is_none() {
        // Nothing to upload, so nothing would notice a revoked token: check it
        // (one cheap request) so the gate blocks before the next prompt.
        if let Err(e @ ApiError::Unauthorized(_)) = client.me() {
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
            let account = account_for(&timeline, &claude, e.ts).map_or(API_KEY_ACCOUNT, |a| a.account_uuid.as_str()).to_string();
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
        timeline: &timeline,
        claude: &claude,
        scope,
        quarantine,
        report,
    };
    for (i, batch) in batches.into_iter().enumerate() {
        let snap = if i == 0 { snapshot.as_ref() } else { None };
        up.send_batch(batch.entries, snap)?;
        // Every entry of this batch is now accepted or quarantined.
        for (path, end) in batch.completes {
            offsets.set(&path, end);
        }
        offsets.save(&paths.offsets())?;
        if let Some(s) = snap {
            write_json_atomic(&paths.last_snapshot(), &s.fetched_at, None)?;
        }
    }
    Ok(up.report)
}

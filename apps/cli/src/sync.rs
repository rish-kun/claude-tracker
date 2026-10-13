//! Upload new transcript usage: lock → scan → batch → upload → advance offsets.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::claude_json::{self, ClaudeState, OAuthAccount, UsageSnapshot};
use crate::client::{ApiClient, ApiError};
use crate::credentials::{self, Credentials};
use crate::paths::Paths;
use crate::scan::{self, Offsets, Position};
use crate::state::{read_json, write_json_atomic};
use crate::timeline::{self, Timeline};
use crate::transcript::{self, UsageEntry};
use crate::wire::{API_KEY_ACCOUNT, Device, IngestAccount, IngestEntry, IngestRequest, IngestSession, LimitSnapshotIn};

pub const BATCH_ENTRIES: usize = 1500;
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

/// A session the server refused with a deterministic 4xx even on its own.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct QuarantinedSession {
    pub at: DateTime<Utc>,
    pub status: u16,
    pub error: String,
}

/// Sessions never to upload again, keyed by session id (`quarantine.json`).
#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(transparent)]
pub struct Quarantine(pub BTreeMap<String, QuarantinedSession>);

impl Quarantine {
    pub fn load(path: &Path) -> Self {
        read_json(path).unwrap_or_default()
    }
    pub fn contains(&self, session_id: &str) -> bool {
        self.0.contains_key(session_id)
    }
}

/// Errors that will recur no matter how often the same request is retried.
fn deterministic(e: &ApiError) -> Option<u16> {
    match e {
        ApiError::Http(code @ (400 | 403 | 409 | 413 | 422), _) => Some(*code),
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

fn acquire(f: &File, wait: Duration) -> Result<bool> {
    let deadline = Instant::now() + wait;
    loop {
        match f.try_lock() {
            Ok(()) => return Ok(true),
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Ok(false);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(TryLockError::Error(e)) => return Err(e.into()),
        }
    }
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
    /// Files whose last pending entry is in this batch, with their new offsets.
    completes: Vec<(PathBuf, Position)>,
    /// Distinct session ids and account uuids, bounded by the server's limits.
    sessions: HashSet<String>,
    accounts: HashSet<String>,
}

impl Batch {
    /// Whether adding an entry for `session`/`account` stays within the limits.
    fn fits(&self, session: &str, account: &str) -> bool {
        self.entries.len() < BATCH_ENTRIES
            && (self.sessions.contains(session) || self.sessions.len() < MAX_BATCH_SESSIONS)
            && (self.accounts.contains(account) || self.accounts.len() < MAX_BATCH_ACCOUNTS)
    }
}

pub fn run(paths: &Paths, creds: &Credentials, client: &ApiClient, opts: &SyncOptions, now: DateTime<Utc>) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    fs::create_dir_all(&paths.state_dir)?;
    let lock = OpenOptions::new().create(true).write(true).truncate(false).open(paths.sync_lock())?;
    if !acquire(&lock, opts.wait_for_lock)? {
        report.skipped_locked = true;
        return Ok(report);
    }

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

    let mut quarantine = Quarantine::load(&paths.quarantine());
    let mut offsets = Offsets::load(&paths.offsets());
    let mut seen = HashSet::new();
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
        let entries: Vec<UsageEntry> = new
            .lines
            .iter()
            .filter_map(|l| transcript::parse_line(l, f.subagent, &f.session_hint))
            .filter(|e| e.ts >= creds.logged_in_at)
            .filter(|e| !quarantine.contains(&e.session_id))
            .filter(|e| seen.insert(e.message_id.clone()))
            .collect();
        if entries.is_empty() {
            offsets.set(&f.path, new.end); // nothing to upload from these bytes
        } else {
            pending.push((f.path.clone(), new.end, entries));
        }
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

    if pending.is_empty() && snapshot.is_none() {
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
        // A file's offset commits only with the batch holding its last entry.
        batches.last_mut().unwrap().completes.push((path, end));
    }

    let device = Device { hostname: hostname(), os: std::env::consts::OS.to_string() };
    for (i, batch) in batches.iter().enumerate() {
        let snap = if i == 0 { snapshot.as_ref() } else { None };
        let req = build_request(&device, &batch.entries, &timeline, &claude, snap);
        match client.ingest(&req) {
            Ok(resp) => {
                report.batches += 1;
                report.entries_sent += batch.entries.len();
                report.accepted += resp.accepted;
                report.duplicates += resp.duplicates;
            }
            Err(e) => {
                let Some(status) = deterministic(&e) else { return Err(fatal(paths, &creds.token, e)) };
                // One bad session (e.g. HTTP 403: it belongs to another cctrack user
                // after someone else logged in on this OS account) must not wedge
                // every sync: retry per session and quarantine the ones still refused.
                log(paths, &format!("ingest batch of {} entries rejected with HTTP {status} ({e}); retrying per session", batch.entries.len()));
                let mut by_session: BTreeMap<&str, Vec<UsageEntry>> = BTreeMap::new();
                for e in &batch.entries {
                    by_session.entry(e.session_id.as_str()).or_default().push(e.clone());
                }
                for (session_id, entries) in by_session {
                    let req = build_request(&device, &entries, &timeline, &claude, None);
                    match client.ingest(&req) {
                        Ok(resp) => {
                            report.batches += 1;
                            report.entries_sent += entries.len();
                            report.accepted += resp.accepted;
                            report.duplicates += resp.duplicates;
                        }
                        Err(e) => {
                            let Some(status) = deterministic(&e) else { return Err(fatal(paths, &creds.token, e)) };
                            let error = e.to_string();
                            log(paths, &format!("quarantined session {session_id}: dropped {} entries, HTTP {status} ({error})", entries.len()));
                            quarantine.0.insert(session_id.to_string(), QuarantinedSession { at: now, status, error });
                            write_json_atomic(&paths.quarantine(), &quarantine, None)?;
                            report.quarantined.push(session_id.to_string());
                        }
                    }
                }
                if let Some(s) = snap {
                    let req = build_request(&device, &[], &timeline, &claude, Some(s));
                    match client.ingest(&req) {
                        Ok(_) => {}
                        Err(e) => {
                            let Some(status) = deterministic(&e) else { return Err(fatal(paths, &creds.token, e)) };
                            log(paths, &format!("dropped limit snapshot captured at {}: HTTP {status} ({e})", s.fetched_at.to_rfc3339()));
                        }
                    }
                }
            }
        }
        // Every entry of this batch is now accepted or quarantined.
        for (path, end) in &batch.completes {
            offsets.set(path, end.clone());
        }
        offsets.save(&paths.offsets())?;
        if let Some(s) = snap {
            write_json_atomic(&paths.last_snapshot(), &s.fetched_at, None)?;
        }
    }
    Ok(report)
}

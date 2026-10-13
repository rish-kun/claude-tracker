//! Upload new transcript usage: lock → scan → batch → upload → advance offsets.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};

use crate::claude_json::{self, ClaudeState, OAuthAccount, UsageSnapshot};
use crate::client::{ApiClient, ApiError};
use crate::credentials::{self, Credentials};
use crate::paths::Paths;
use crate::scan::{self, Offsets};
use crate::state::{read_json, write_json_atomic};
use crate::timeline::Timeline;
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
    completes: Vec<(PathBuf, u64)>,
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
    let mut timeline = Timeline::load(&paths.timeline());
    if let Some(c) = &checked
        && timeline.observe(now, c.account.clone())
    {
        timeline.save(&paths.timeline())?;
    }
    let claude = checked.unwrap_or_default();

    let files = match &opts.transcript {
        Some(t) => scan::session_files(t),
        None => {
            let since = SystemTime::from(creds.logged_in_at).checked_sub(Duration::from_secs(3600));
            scan::discover(&paths.projects_dir(), since)
        }
    };

    let mut offsets = Offsets::load(&paths.offsets());
    let mut seen = HashSet::new();
    let mut pending: Vec<(PathBuf, u64, Vec<UsageEntry>)> = Vec::new();
    for f in &files {
        let start = offsets.get(&f.path);
        let Ok(new) = scan::read_new_lines(&f.path, start) else { continue };
        if new.end == start {
            continue;
        }
        report.files_read += 1;
        let entries: Vec<UsageEntry> = new
            .lines
            .iter()
            .filter_map(|l| transcript::parse_line(l, f.subagent, &f.session_hint))
            .filter(|e| e.ts >= creds.logged_in_at)
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
                for (path, end) in &batch.completes {
                    offsets.set(path, *end);
                }
                offsets.save(&paths.offsets())?;
                if let Some(s) = snap {
                    write_json_atomic(&paths.last_snapshot(), &s.fetched_at, None)?;
                }
            }
            Err(ApiError::Unauthorized(_)) => {
                credentials::mark_invalid(paths)?;
                bail!("the server rejected your token (revoked or expired); run `cctrack login`");
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(report)
}

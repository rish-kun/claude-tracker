//! Usage served through a local CLIProxyAPI pool.
//!
//! Some users point Claude Code at a local CLIProxyAPI (`ANTHROPIC_BASE_URL`
//! on loopback) that spreads requests over a pool of Claude accounts. Then
//! `~/.claude.json`'s `oauthAccount` is only the user's own login, not the
//! account that served the request. The proxy's log names that account:
//!
//! ```text
//! [2026-10-15 22:24:56] [84c01d98] [info ] [selector.go:1068] session-affinity: cache hit | session=claude:8... auth=claude-ef80fd69-x@example.org.json provider=mixed model=claude-opus-5-5
//! [2026-10-15 22:25:04] [84c01d98] [info ] [gin_logger.go:108] 200 |  7.605s | 127.0.0.1 | POST "/v1/messages?beta=true"
//! ```
//!
//! The selector line is written when the request starts (local time, second
//! precision) and the `gin_logger` line with the same request id when it ends.
//! `session=claude:X...` carries only the first character of Claude Code's
//! session id. `auth=` names a JSON file in the proxy's home whose
//! non-secret fields (`account_uuid`, `email`, `organization_uuid`) identify
//! the account; its tokens are never kept, logged or uploaded.
//!
//! Hooks record per session whether it runs through such a proxy (their
//! environment is Claude Code's); syncs, which may run from a terminal without
//! that environment, read the record.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;
use crate::state::{read_json, write_json_atomic};
use crate::wire::{API_KEY_ACCOUNT, IngestAccount};

/// CLIProxyAPI's current log, relative to its home.
pub const MAIN_LOG: &str = "logs/main.log";

/// Longest a request may run: a selector line up to this long before an
/// entry's timestamp may still be the request that produced it.
pub const REQUEST_WINDOW: Duration = Duration::minutes(15);

/// How long after a request's logged end its response may still be stamped
/// in the transcript (log times are truncated to the second).
const END_SLACK: Duration = Duration::seconds(5);

/// Most bytes read from one log file per sync, after seeking by time.
const MAX_SCAN_BYTES: u64 = 256 * 1024 * 1024;

/// Whether `url` points at this machine (`localhost`, 127.0.0.0/8, ::1).
pub fn is_loopback_url(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url.trim()) else { return false };
    if !matches!(u.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = u.host_str() else { return false };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Whether `url` points at a CLIProxyAPI: a loopback host, or the port
/// the proxy's `config.yaml` listens on.
pub fn url_is_proxy(url: &str, config_port: Option<u16>) -> bool {
    is_loopback_url(url)
        || config_port.is_some_and(|p| {
            reqwest::Url::parse(url.trim())
                .ok()
                .filter(|u| matches!(u.scheme(), "http" | "https"))
                .and_then(|u| u.port_or_known_default())
                == Some(p)
        })
}

/// `scheme://host:port` only: no credentials, path or query are kept.
fn display_url(url: &str) -> String {
    match reqwest::Url::parse(url.trim()) {
        Ok(u) => match (u.host_str(), u.port()) {
            (Some(h), Some(p)) => format!("{}://{h}:{p}", u.scheme()),
            (Some(h), None) => format!("{}://{h}", u.scheme()),
            _ => format!("{}:", u.scheme()),
        },
        Err(_) => "(not a URL)".to_string(),
    }
}

/// The listening port from CLIProxyAPI's `config.yaml`: `server.port`
/// (current versions) or a top-level `port` (older ones). No other key's
/// value is read.
pub fn config_port(dir: &Path) -> Option<u16> {
    let text = fs::read_to_string(dir.join("config.yaml")).ok()?;
    let value = |v: &str| -> Option<u16> { v.split('#').next()?.trim().trim_matches(['"', '\'']).parse().ok() };
    let mut section = "";
    let mut child_indent: Option<usize> = None;
    for l in text.lines() {
        let trimmed = l.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = l.len() - trimmed.len();
        if indent == 0 {
            if let Some(v) = trimmed.strip_prefix("port:") {
                return value(v);
            }
            section = trimmed.split(':').next().unwrap_or("");
            child_indent = None;
        } else if section == "server" {
            // Only direct children of `server:`.
            let first = *child_indent.get_or_insert(indent);
            if indent == first && let Some(v) = trimmed.strip_prefix("port:") {
                return value(v);
            }
        }
    }
    None
}

/// Whether usage is attributed to CLIProxyAPI pool accounts.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    /// Detect per session (the default).
    Auto,
    /// Treat every session as proxied.
    On,
    /// Never: always the Claude account from `~/.claude.json`.
    Off,
}

impl ProxyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ProxyMode::Auto => "auto",
            ProxyMode::On => "on",
            ProxyMode::Off => "off",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(ProxyMode::Auto),
            "on" => Some(ProxyMode::On),
            "off" => Some(ProxyMode::Off),
            _ => None,
        }
    }
}

/// Where the effective mode came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeSource {
    /// `CCTRACK_PROXY_MODE`, which beats the saved setting.
    Env,
    Saved,
    Default,
}

/// The environment variable that overrides the saved mode.
pub const MODE_ENV: &str = "CCTRACK_PROXY_MODE";

#[derive(Serialize, Deserialize)]
struct ProxyConfig {
    mode: ProxyMode,
}

/// `CCTRACK_PROXY_MODE` from this process's environment.
pub fn env_mode() -> Option<String> {
    std::env::var(MODE_ENV).ok()
}

/// The effective mode: a valid `env` value, else the saved one, else auto.
pub fn mode(paths: &Paths, env: Option<&str>) -> (ProxyMode, ModeSource) {
    if let Some(m) = env.and_then(ProxyMode::parse) {
        return (m, ModeSource::Env);
    }
    match read_json::<ProxyConfig>(&paths.proxy_config()) {
        Some(c) => (c.mode, ModeSource::Saved),
        None => (ProxyMode::Auto, ModeSource::Default),
    }
}

pub fn save_mode(paths: &Paths, mode: ProxyMode) -> anyhow::Result<()> {
    write_json_atomic(&paths.proxy_config(), &ProxyConfig { mode }, None)
}

/// Where `ANTHROPIC_BASE_URL` was found, in Claude Code's precedence order.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UrlSource {
    /// The hook's (Claude Code's) environment, e.g. set by a wrapper script.
    Env,
    ManagedSettings,
    ProjectLocalSettings,
    ProjectSettings,
    UserSettings,
}

impl UrlSource {
    pub fn label(self) -> &'static str {
        match self {
            UrlSource::Env => "Claude Code's environment",
            UrlSource::ManagedSettings => "managed settings",
            UrlSource::ProjectLocalSettings => "project .claude/settings.local.json",
            UrlSource::ProjectSettings => "project .claude/settings.json",
            UrlSource::UserSettings => "user settings.json",
        }
    }
}

/// Only `env.ANTHROPIC_BASE_URL` is parsed from a settings file; every other
/// key (settings can hold secrets) is skipped by the parser.
#[derive(Deserialize)]
struct SettingsEnv {
    #[serde(default)]
    env: Option<EnvBlock>,
}

#[derive(Deserialize)]
struct EnvBlock {
    #[serde(rename = "ANTHROPIC_BASE_URL", default)]
    base_url: Option<String>,
}

fn settings_base_url(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let s: SettingsEnv = serde_json::from_slice(&bytes).ok()?;
    s.env?.base_url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty())
}

/// The `ANTHROPIC_BASE_URL` in effect and where it came from: the
/// environment first, then managed, project-local, project and user settings.
pub fn find_base_url(paths: &Paths, cwd: Option<&Path>, env_url: Option<&str>) -> Option<(UrlSource, String)> {
    if let Some(u) = env_url.map(str::trim).filter(|u| !u.is_empty()) {
        return Some((UrlSource::Env, u.to_string()));
    }
    let mut files = vec![(UrlSource::ManagedSettings, paths.managed_settings.clone())];
    if let Some(cwd) = cwd {
        files.push((UrlSource::ProjectLocalSettings, cwd.join(".claude/settings.local.json")));
        files.push((UrlSource::ProjectSettings, cwd.join(".claude/settings.json")));
    }
    files.push((UrlSource::UserSettings, paths.claude_dir.join("settings.json")));
    files.into_iter().find_map(|(src, p)| settings_base_url(&p).map(|u| (src, u)))
}

/// What was detected for a session (stored per session by hooks).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Detection {
    pub detected_at: DateTime<Utc>,
    /// Where `ANTHROPIC_BASE_URL` came from; None when it is unset everywhere.
    #[serde(default)]
    pub source: Option<UrlSource>,
    /// The URL's scheme, host and port.
    #[serde(default)]
    pub base_url: Option<String>,
    /// The URL points at a CLIProxyAPI.
    #[serde(default)]
    pub proxy_url: bool,
    /// The proxy home in effect (`CCTRACK_CLIPROXY_DIR` may differ between
    /// Claude Code and a terminal).
    pub cliproxy_dir: PathBuf,
    /// Whether the proxy's log existed when detected.
    #[serde(default)]
    pub log_found: bool,
}

impl Detection {
    pub fn describe(&self) -> String {
        match (&self.source, &self.base_url) {
            (Some(src), Some(url)) if self.proxy_url && self.log_found => {
                format!("ANTHROPIC_BASE_URL={url} from {} points at a CLIProxyAPI (log found)", src.label())
            }
            (Some(src), Some(url)) if self.proxy_url => format!(
                "ANTHROPIC_BASE_URL={url} from {} points at a CLIProxyAPI, but there is no log at {}; usage is credited to the Claude account as before",
                src.label(),
                self.cliproxy_dir.join(MAIN_LOG).display()
            ),
            (Some(src), Some(url)) => format!("ANTHROPIC_BASE_URL={url} from {} is not a local proxy", src.label()),
            _ => "ANTHROPIC_BASE_URL is not set".to_string(),
        }
    }
}

/// Detects whether a session with working directory `cwd` and (hook)
/// environment value `env_url` runs through a CLIProxyAPI pool.
pub fn detect(paths: &Paths, cwd: Option<&Path>, env_url: Option<&str>, now: DateTime<Utc>) -> Detection {
    let found = find_base_url(paths, cwd, env_url);
    let proxy_url = found.as_ref().is_some_and(|(_, u)| is_loopback_url(u) || url_is_proxy(u, config_port(&paths.cliproxy_dir)));
    Detection {
        detected_at: now,
        source: found.as_ref().map(|(s, _)| *s),
        base_url: found.as_ref().map(|(_, u)| display_url(u)),
        proxy_url,
        cliproxy_dir: paths.cliproxy_dir.clone(),
        log_found: proxy_url && paths.cliproxy_dir.join(MAIN_LOG).is_file(),
    }
}

/// Session ids become file names: only plain ids are accepted.
fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && !id.starts_with('.')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn marker_path(paths: &Paths, session_id: &str) -> Option<PathBuf> {
    valid_session_id(session_id).then(|| paths.proxy_sessions_dir().join(format!("{session_id}.json")))
}

/// Which hook is recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookPoint {
    /// SessionStart (startup, resume, clear, compact) always records, so a
    /// session resumed without the proxy stops being treated as proxied.
    SessionStart,
    /// Stop/SessionEnd only add a missing record (e.g. a session started
    /// before this cctrack version): one stat when it exists.
    Later,
}

/// Records what `session_id` runs through, from the hook's environment and
/// Claude Code's settings. Nothing in `off` mode. Best effort: errors are
/// ignored (hooks must stay fast and never fail).
pub fn record_session(
    paths: &Paths,
    session_id: &str,
    cwd: Option<&str>,
    env_url: Option<&str>,
    point: HookPoint,
    mode: ProxyMode,
    now: DateTime<Utc>,
) {
    if mode == ProxyMode::Off {
        return;
    }
    let Some(path) = marker_path(paths, session_id) else { return };
    if point == HookPoint::Later && path.exists() {
        return;
    }
    let d = detect(paths, cwd.map(Path::new), env_url, now);
    let _ = write_json_atomic(&path, &d, None);
}

pub fn session_marker(paths: &Paths, session_id: &str) -> Option<Detection> {
    read_json(&marker_path(paths, session_id)?)
}

/// Removes session records older than `max_age` (best effort).
pub fn prune_markers(paths: &Paths, max_age: std::time::Duration) {
    let Ok(items) = fs::read_dir(paths.proxy_sessions_dir()) else { return };
    let Some(cutoff) = std::time::SystemTime::now().checked_sub(max_age) else { return };
    for item in items.flatten() {
        if item.metadata().and_then(|m| m.modified()).is_ok_and(|m| m < cutoff) {
            let _ = fs::remove_file(item.path());
        }
    }
}

/// A session whose usage is attributed from a proxy log.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionProxy {
    pub dir: PathBuf,
    pub why: String,
}

/// Whether a sync attributes `session_id` from a CLIProxyAPI log, and why
/// (or why not). In auto mode a session without a hook record is judged
/// from Claude Code's settings files for `cwd` (a sync's own environment is
/// not the session's).
pub fn session_proxy(paths: &Paths, mode: ProxyMode, session_id: &str, cwd: Option<&str>, now: DateTime<Utc>) -> Result<SessionProxy, String> {
    let marker = session_marker(paths, session_id);
    let (dir, why) = match mode {
        ProxyMode::Off => return Err("proxy mode is off".into()),
        ProxyMode::On => (marker.map_or_else(|| paths.cliproxy_dir.clone(), |m| m.cliproxy_dir), "proxy mode is on".to_string()),
        ProxyMode::Auto => {
            let d = marker.unwrap_or_else(|| detect(paths, cwd.map(Path::new), None, now));
            if !d.proxy_url {
                return Err(d.describe());
            }
            let why = format!("ANTHROPIC_BASE_URL from {}", d.source.map_or("settings", |s| s.label()));
            (d.cliproxy_dir, why)
        }
    };
    let log = dir.join(MAIN_LOG);
    if !log.is_file() {
        return Err(format!("{why}, but no CLIProxyAPI log at {}; credited to the Claude account as before", log.display()));
    }
    Ok(SessionProxy { why: format!("{why}; pool account read from {}", log.display()), dir })
}

/// Pool accounts in a proxy home: email (or account uuid) of each Claude auth file.
pub fn pool_accounts(dir: &Path) -> Vec<String> {
    let Ok(items) = fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<String> = items
        .flatten()
        .filter_map(|i| {
            let name = i.file_name().to_str()?.to_string();
            auth_path(dir, &name)?;
            let a = parse_auth(&fs::read(i.path()).ok()?)?;
            Some(a.email.unwrap_or(a.account_uuid))
        })
        .collect();
    out.sort();
    out
}

/// `cctrack proxy status`: the mode, the proxy home, what this shell would
/// detect, the pool accounts (by email) and recent sessions. Never secrets.
pub fn status_report(paths: &Paths, env_mode: Option<&str>, env_url: Option<&str>, cwd: Option<&Path>, now: DateTime<Utc>) -> String {
    let (mode, source) = mode(paths, env_mode);
    let mut out = Vec::new();
    let origin = match source {
        ModeSource::Env => format!("from {MODE_ENV}"),
        ModeSource::Saved => "saved".to_string(),
        ModeSource::Default => "default".to_string(),
    };
    out.push(format!("proxy mode: {} ({origin})", mode.as_str()));
    if let Some(v) = env_mode.filter(|v| ProxyMode::parse(v).is_none()) {
        out.push(format!("  ignoring {MODE_ENV}={:?}: expected auto, on or off", v.chars().take(20).collect::<String>()));
    }
    out.push(match mode {
        ProxyMode::Off => "  usage is credited to the Claude account in ~/.claude.json".to_string(),
        ProxyMode::On => "  every session is treated as running through the CLIProxyAPI pool".to_string(),
        ProxyMode::Auto => {
            "  sessions whose ANTHROPIC_BASE_URL points at a CLIProxyAPI are credited to the pool account that served them".to_string()
        }
    });
    let dir = &paths.cliproxy_dir;
    out.push(format!("CLIProxyAPI dir: {}", dir.display()));
    let log = dir.join(MAIN_LOG);
    out.push(match fs::metadata(&log) {
        Ok(m) => format!("  log: {} (found, {} KB)", log.display(), m.len() / 1024),
        Err(_) => format!("  log: {} (not found)", log.display()),
    });
    out.push(match config_port(dir) {
        Some(p) => format!("  config port: {p}"),
        None => "  config port: unknown".to_string(),
    });
    let here = detect(paths, cwd, env_url, now);
    out.push(format!("this shell: {}", here.describe()));
    let accounts = pool_accounts(dir);
    out.push(format!("pool accounts ({}):", accounts.len()));
    out.extend(accounts.iter().map(|a| format!("  {a}")));
    let mut recent: Vec<(String, Detection)> = fs::read_dir(paths.proxy_sessions_dir())
        .map(|items| {
            items
                .flatten()
                .filter_map(|i| {
                    let id = i.path().file_stem()?.to_str()?.to_string();
                    Some((id, read_json::<Detection>(&i.path())?))
                })
                .collect()
        })
        .unwrap_or_default();
    recent.sort_by(|a, b| b.1.detected_at.cmp(&a.1.detected_at).then(a.0.cmp(&b.0)));
    out.push(format!("recent sessions ({} recorded):", recent.len()));
    for (id, d) in recent.iter().take(10) {
        let verdict = match mode {
            ProxyMode::Off => "Claude account (mode off)",
            ProxyMode::On => "pool",
            ProxyMode::Auto if d.proxy_url && d.cliproxy_dir.join(MAIN_LOG).is_file() => "pool",
            ProxyMode::Auto => "Claude account",
        };
        out.push(format!("  {id} [{verdict}] {}: {}", d.detected_at.format("%Y-%m-%d %H:%M"), d.describe()));
    }
    out.join("\n")
}

/// One pool selection from the proxy's log.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    /// When the request started.
    pub at: DateTime<Utc>,
    /// When it finished (its `gin_logger` line), if seen.
    pub end: Option<DateTime<Utc>>,
    pub request_id: String,
    /// First character of the Claude Code session id (`session=claude:X...`);
    /// None for other clients' keys (`lcp:`, `codex:`, …).
    pub session_char: Option<char>,
    /// Auth file name of the pool account.
    pub auth: String,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LogLine {
    Selection(Selection),
    /// A request finished (`gin_logger`).
    Done { at: DateTime<Utc>, request_id: String },
}

/// Splits `[a] rest` into `a` and `rest`.
fn bracketed(s: &str) -> Option<(&str, &str)> {
    let (inner, rest) = s.strip_prefix('[')?.split_once(']')?;
    Some((inner, rest.strip_prefix(' ').unwrap_or(rest)))
}

fn line_time(line: &str) -> Option<NaiveDateTime> {
    let (ts, _) = bracketed(line)?;
    NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S").ok()
}

/// A local wall-clock time in `tz` as UTC (the earlier one when ambiguous).
fn to_utc<Tz: TimeZone>(t: NaiveDateTime, tz: &Tz) -> Option<DateTime<Utc>> {
    tz.from_local_datetime(&t).earliest().map(|d| d.with_timezone(&Utc))
}

/// Parses a selector or `gin_logger` line; None for any other line.
pub fn parse_log_line<Tz: TimeZone>(line: &str, tz: &Tz) -> Option<LogLine> {
    let (ts, rest) = bracketed(line)?;
    let at = to_utc(NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S").ok()?, tz)?;
    let (request_id, rest) = bracketed(rest)?;
    let request_id = request_id.trim();
    if request_id.is_empty() || request_id.contains(' ') {
        return None;
    }
    let (_level, rest) = bracketed(rest)?;
    let (source, msg) = bracketed(rest)?;
    if source.starts_with("gin_logger.go") {
        return Some(LogLine::Done { at, request_id: request_id.to_string() });
    }
    if !source.starts_with("selector.go") || !msg.starts_with("session-affinity:") {
        return None;
    }
    let (_, fields) = msg.split_once(" | ")?;
    let mut kv: HashMap<&str, &str> = HashMap::new();
    for f in fields.split_whitespace() {
        if let Some((k, v)) = f.split_once('=') {
            kv.entry(k).or_insert(v);
        }
    }
    let session = *kv.get("session")?;
    let auth = kv.get("auth").filter(|a| !a.is_empty())?;
    let model = kv.get("model").filter(|m| !m.is_empty())?;
    let session_char = session.strip_prefix("claude:").and_then(|s| s.chars().next()).filter(char::is_ascii_alphanumeric);
    Some(LogLine::Selection(Selection {
        at,
        end: None,
        request_id: request_id.to_string(),
        session_char: session_char.map(|c| c.to_ascii_lowercase()),
        auth: auth.to_string(),
        model: model.to_string(),
    }))
}

/// The first log timestamp in the lines that start after `pos` (and before
/// `limit`), skipping the partial line at `pos`.
fn first_time_after(f: &mut File, pos: u64, limit: u64) -> io::Result<Option<NaiveDateTime>> {
    f.seek(SeekFrom::Start(pos))?;
    let mut r = BufReader::new(f.take(limit.saturating_sub(pos)));
    let mut buf = Vec::new();
    if pos > 0 {
        r.read_until(b'\n', &mut buf)?;
    }
    for _ in 0..64 {
        buf.clear();
        if r.read_until(b'\n', &mut buf)? == 0 {
            return Ok(None);
        }
        if let Some(t) = line_time(&String::from_utf8_lossy(&buf)) {
            return Ok(Some(t));
        }
    }
    Ok(None)
}

/// An offset at or before the first line logged at or after `target`
/// (binary search: log lines are in time order).
fn seek_time(f: &mut File, len: u64, target: NaiveDateTime) -> io::Result<u64> {
    let (mut lo, mut hi) = (0u64, len);
    while hi - lo > 64 * 1024 {
        let mid = lo + (hi - lo) / 2;
        match first_time_after(f, mid, hi)? {
            Some(t) if t < target => lo = mid,
            _ => hi = mid,
        }
    }
    Ok(lo)
}

/// `main.log` and its rotated `main-<timestamp>.log` files.
fn log_files(dir: &Path) -> Vec<PathBuf> {
    let logs = dir.join("logs");
    let Ok(items) = fs::read_dir(&logs) else { return vec![] };
    let mut out: Vec<PathBuf> = items
        .flatten()
        .map(|i| i.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n == "main.log" || (n.starts_with("main-") && n.ends_with(".log")))
        })
        .collect();
    out.sort();
    out
}

/// Pool selections of Claude Code sessions from the proxy's logs.
#[derive(Debug, Default)]
pub struct PoolLog {
    pub selections: Vec<Selection>,
}

impl PoolLog {
    /// Reads the selections that started in `[from, to]` for sessions whose
    /// id starts with one of `chars`, and their end times. Each log file is
    /// entered by binary search on time and read for at most
    /// [`MAX_SCAN_BYTES`]; files last written before `from` are skipped.
    pub fn load<Tz: TimeZone>(dir: &Path, from: DateTime<Utc>, to: DateTime<Utc>, chars: &HashSet<char>, tz: &Tz) -> Self {
        let mut log = PoolLog::default();
        for path in log_files(dir) {
            let _ = log.read_file(&path, from, to, chars, tz);
        }
        log.selections.sort_by_key(|s| s.at);
        log
    }

    fn read_file<Tz: TimeZone>(&mut self, path: &Path, from: DateTime<Utc>, to: DateTime<Utc>, chars: &HashSet<char>, tz: &Tz) -> io::Result<()> {
        let mut f = File::open(path)?;
        let meta = f.metadata()?;
        if meta.modified().is_ok_and(|m| DateTime::<Utc>::from(m) < from) {
            return Ok(());
        }
        // An hour of margin covers clock changes between local and UTC.
        let target = (from - Duration::hours(1)).with_timezone(tz).naive_local();
        let start = seek_time(&mut f, meta.len(), target)?;
        f.seek(SeekFrom::Start(start))?;
        let mut r = BufReader::new((&mut f).take(MAX_SCAN_BYTES));
        let mut buf = Vec::new();
        if start > 0 {
            r.read_until(b'\n', &mut buf)?; // partial line
        }
        let stop = to + REQUEST_WINDOW; // end lines of the last requests
        let mut open: HashMap<String, usize> = HashMap::new();
        loop {
            buf.clear();
            if r.read_until(b'\n', &mut buf)? == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf);
            match parse_log_line(line.trim_end(), tz) {
                Some(LogLine::Selection(s)) => {
                    if s.at > stop {
                        break;
                    }
                    if s.at < from || s.at > to || !s.session_char.is_some_and(|c| chars.contains(&c)) {
                        continue;
                    }
                    // A request reselected later keeps its start; the last auth served it.
                    match open.get(&s.request_id) {
                        Some(&i) => self.selections[i].auth = s.auth,
                        None => {
                            open.insert(s.request_id.clone(), self.selections.len());
                            self.selections.push(s);
                        }
                    }
                }
                Some(LogLine::Done { at, request_id }) => {
                    if at > stop {
                        break;
                    }
                    if let Some(i) = open.remove(&request_id) {
                        self.selections[i].end = Some(at);
                    }
                }
                None => {}
            }
        }
        Ok(())
    }
}

/// Whether a transcript model and a log model name the same model
/// (allowing a dated suffix on either).
fn same_model(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    a == b || a.strip_prefix(&b).is_some_and(|r| r.starts_with('-')) || b.strip_prefix(&a).is_some_and(|r| r.starts_with('-'))
}

/// The selection that most likely served an entry of `session_id` with
/// `model`, stamped `ts` (the response time): a selection of a session with
/// the same first character and model that started at most
/// [`REQUEST_WINDOW`] before `ts`. Requests still running at `ts` are
/// preferred; among those (or else among all candidates) the latest start wins.
pub fn match_entry<'a>(selections: &'a [Selection], session_id: &str, model: &str, ts: DateTime<Utc>) -> Option<&'a Selection> {
    let c = session_id.chars().next()?.to_ascii_lowercase();
    // Log times are truncated to the second.
    let latest_start = ts + Duration::seconds(1);
    let candidates: Vec<&Selection> = selections
        .iter()
        .filter(|s| s.session_char == Some(c) && same_model(&s.model, model))
        .filter(|s| s.at <= latest_start && s.at >= ts - REQUEST_WINDOW)
        .collect();
    let in_flight = candidates.iter().copied().filter(|s| s.end.is_some_and(|e| e + END_SLACK >= ts));
    // max_by_key keeps the last of equal keys: the later log line.
    in_flight.max_by_key(|s| s.at).or_else(|| candidates.iter().copied().max_by_key(|s| s.at))
}

/// The non-secret fields of a CLIProxyAPI auth file. Every other field
/// (access/refresh/id tokens, …) is skipped by the parser and never kept.
#[derive(Deserialize)]
struct AuthFields {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    account_uuid: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    organization_uuid: Option<String>,
}

/// An auth file name from the log, as a plain file name in the proxy home.
fn auth_path(dir: &Path, auth: &str) -> Option<PathBuf> {
    let ok = auth.ends_with(".json")
        && auth.len() <= 255
        && !auth.starts_with('.')
        && !auth.contains(['/', '\\', '\0']);
    ok.then(|| dir.join(auth))
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s.len() <= 320 && !s.contains('\0'))
}

/// Parses an auth file's identity: Claude accounts only, with a usable
/// `account_uuid`. Plan fields stay empty: an admin sets each pool
/// account's plan in the dashboard.
pub fn parse_auth(bytes: &[u8]) -> Option<IngestAccount> {
    let f: AuthFields = serde_json::from_slice(bytes).ok()?;
    if f.kind.as_deref() != Some("claude") {
        return None;
    }
    let account_uuid = clean(f.account_uuid).filter(|u| u.len() <= 100 && !u.starts_with(API_KEY_ACCOUNT))?;
    Some(IngestAccount {
        account_uuid,
        org_uuid: clean(f.organization_uuid),
        email: clean(f.email),
        org_type: None,
        rate_limit_tier: None,
    })
}

/// Resolves pool accounts for the entries of proxied sessions within one
/// sync run, caching each proxy home's log and each auth file.
#[derive(Default)]
pub struct PoolResolver {
    logs: HashMap<PathBuf, PoolLog>,
    auths: HashMap<(PathBuf, String), Option<IngestAccount>>,
}

/// What [`PoolResolver::load_logs`] needs per proxy home.
#[derive(Default)]
pub struct LogWindow {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub chars: HashSet<char>,
}

impl LogWindow {
    pub fn include(&mut self, session_id: &str, ts: DateTime<Utc>) {
        self.from = Some(self.from.map_or(ts, |f| f.min(ts)));
        self.to = Some(self.to.map_or(ts, |t| t.max(ts)));
        if let Some(c) = session_id.chars().next() {
            self.chars.insert(c.to_ascii_lowercase());
        }
    }
}

impl PoolResolver {
    /// Reads each proxy home's log once for the window its entries need.
    pub fn load_logs<Tz: TimeZone>(&mut self, windows: &BTreeMap<PathBuf, LogWindow>, tz: &Tz) {
        for (dir, w) in windows {
            let (Some(from), Some(to)) = (w.from, w.to) else { continue };
            let log = PoolLog::load(dir, from - REQUEST_WINDOW, to + Duration::seconds(1), &w.chars, tz);
            self.logs.insert(dir.clone(), log);
        }
    }

    /// The auth file name of the selection that served an entry, if any.
    pub fn matched_auth(&self, dir: &Path, session_id: &str, model: &str, ts: DateTime<Utc>) -> Option<String> {
        let log = self.logs.get(dir)?;
        match_entry(&log.selections, session_id, model, ts).map(|s| s.auth.clone())
    }

    /// The account behind an auth file in `dir` (parsed once per run).
    pub fn account(&mut self, dir: &Path, auth: &str) -> Option<IngestAccount> {
        self.auths
            .entry((dir.to_path_buf(), auth.to_string()))
            .or_insert_with(|| auth_path(dir, auth).and_then(|p| fs::read(p).ok()).and_then(|b| parse_auth(&b)))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;
    use std::io::Write;

    fn ist() -> FixedOffset {
        FixedOffset::east_opt(5 * 3600 + 1800).unwrap()
    }

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn sel(local: &str, req: &str, variant: &str, session: &str, auth: &str, model: &str) -> String {
        format!("[{local}] [{req}] [info ] [selector.go:1068] session-affinity: {variant} | session={session} auth={auth} provider=mixed model={model}")
    }

    #[test]
    fn loopback_urls() {
        for ok in ["http://127.0.0.1:8317", "http://localhost:8317/", "https://127.0.0.2", "http://[::1]:8317/v1", " http://LOCALHOST ", "http://api.localhost:1"] {
            assert!(is_loopback_url(ok), "{ok}");
        }
        for bad in ["https://api.anthropic.com", "http://10.0.0.1:8317", "http://192.168.1.2", "127.0.0.1:8317", "", "ftp://127.0.0.1", "http://localhost.evil.com"] {
            assert!(!is_loopback_url(bad), "{bad}");
        }
    }

    #[test]
    fn parses_every_selector_variant() {
        let variants = [
            ("cache hit", "claude:8..."),
            ("fallback cache hit", "claude:8... fallback=claude:8..."),
            ("cache miss, new binding", "claude:8..."),
            ("cache hit but auth unavailable, reselected", "claude:8..."),
            ("LCP cache miss, new binding", "lcp:v1:7..."),
            ("LCP cache hit", "lcp:v1:a... prefix=2"),
        ];
        for (variant, session) in variants {
            let line = sel("2026-10-15 22:24:56", "84c01d98", variant, session, "claude-ef80fd69-a@example.org.json", "claude-opus-5-5");
            let Some(LogLine::Selection(s)) = parse_log_line(&line, &ist()) else { panic!("{line}") };
            assert_eq!(s.at, utc("2026-10-15T16:54:56Z"), "local IST converted to UTC");
            assert_eq!((s.request_id.as_str(), s.auth.as_str(), s.model.as_str()), ("84c01d98", "claude-ef80fd69-a@example.org.json", "claude-opus-5-5"));
            let expected = if session.starts_with("claude:") { Some('8') } else { None };
            assert_eq!(s.session_char, expected, "{variant}");
            assert_eq!(s.end, None);
        }
        // Other clients' keys parse but carry no Claude session.
        let Some(LogLine::Selection(s)) = parse_log_line(&sel("2026-10-15 22:24:56", "a", "cache hit", "codex:f...", "x.json", "gpt-6-sol"), &ist()) else { panic!() };
        assert_eq!(s.session_char, None);
        let Some(LogLine::Selection(s)) = parse_log_line(&sel("2026-10-15 22:24:56", "a", "cache hit", "claude:D...", "x.json", "m"), &ist()) else { panic!() };
        assert_eq!(s.session_char, Some('d'));
    }

    #[test]
    fn parses_request_end_lines() {
        let line = r#"[2026-10-15 22:25:04] [84c01d98] [info ] [gin_logger.go:108] 200 |        7.605s |       127.0.0.1 | POST    "/v1/messages?beta=true""#;
        assert_eq!(parse_log_line(line, &ist()), Some(LogLine::Done { at: utc("2026-10-15T16:55:04Z"), request_id: "84c01d98".into() }));
    }

    #[test]
    fn malformed_log_lines_are_ignored() {
        for bad in [
            "",
            "garbage",
            "[2026-10-15 22:24:56]",
            "[not a date] [a] [info ] [selector.go:1] session-affinity: cache hit | session=claude:8... auth=a.json model=m",
            "[2026-10-15 22:24:56] [] [info ] [selector.go:1] session-affinity: cache hit | session=claude:8... auth=a.json model=m",
            "[2026-10-15 22:24:56] [a] [info ] [selector.go:1] session-affinity: cache hit | session=claude:8... model=m",
            "[2026-10-15 22:24:56] [a] [info ] [selector.go:1] session-affinity: cache hit | session=claude:8... auth=a.json",
            "[2026-10-15 22:24:56] [a] [info ] [selector.go:1] session-affinity: cache hit | auth=a.json model=m",
            "[2026-10-15 22:24:56] [a] [info ] [selector.go:1] session-affinity: cache hit session=claude:8... auth=a.json model=m",
            "[2026-10-15 22:24:56] [a] [info ] [selector.go:1] something else | session=claude:8... auth=a.json model=m",
            "[2026-10-15 22:24:56] [a] [info ] [conductor.go:9] session-affinity: cache hit | session=claude:8... auth=a.json model=m",
            "[2026-10-15 22:24:56] [a] [info ] [selector.go:1] session-affinity: cache hit | session=claude:8... auth= model=m",
        ] {
            assert_eq!(parse_log_line(bad, &ist()), None, "{bad}");
        }
        // A claude: key without a usable first character carries no session.
        let Some(LogLine::Selection(s)) = parse_log_line(&sel("2026-10-15 22:24:56", "a", "cache hit", "claude:...", "a.json", "m"), &ist()) else { panic!() };
        assert_eq!(s.session_char, None);
    }

    fn s(at: &str, end: Option<&str>, c: char, auth: &str, model: &str) -> Selection {
        Selection { at: utc(at), end: end.map(utc), request_id: at.into(), session_char: Some(c), auth: auth.into(), model: model.into() }
    }

    #[test]
    fn matching_uses_first_char_model_and_time_window() {
        let sels = vec![
            s("2026-10-15T10:00:00Z", Some("2026-10-15T10:00:30Z"), '8', "a.json", "claude-opus-5-5"),
            s("2026-10-15T10:01:00Z", Some("2026-10-15T10:01:20Z"), '9', "other-session.json", "claude-opus-5-5"),
            s("2026-10-15T10:01:05Z", Some("2026-10-15T10:01:25Z"), '8', "haiku.json", "claude-haiku-5-5"),
            s("2026-10-15T10:02:00Z", Some("2026-10-15T10:02:40Z"), '8', "b.json", "claude-opus-5-5"),
        ];
        let m = |ts: &str, model: &str| match_entry(&sels, "8dfd5956-x", model, utc(ts)).map(|s| s.auth.as_str());
        assert_eq!(m("2026-10-15T10:00:20.500Z", "claude-opus-5-5"), Some("a.json"));
        assert_eq!(m("2026-10-15T10:01:15Z", "claude-opus-5-5"), Some("a.json"), "other session's char and other model are skipped");
        assert_eq!(m("2026-10-15T10:01:15Z", "claude-haiku-5-5"), Some("haiku.json"));
        assert_eq!(m("2026-10-15T10:02:10Z", "claude-opus-5-5"), Some("b.json"), "latest start at or before ts");
        assert_eq!(m("2026-10-15T10:02:10Z", "claude-opus-5-5-20261001"), Some("b.json"), "dated model names match");
        // Second precision: a request logged in the same second as the response counts.
        assert_eq!(m("2026-10-15T10:01:59.400Z", "claude-opus-5-5"), Some("b.json"));
        // Before any request, or more than the window after the last one: no match.
        assert_eq!(m("2026-10-15T09:59:00Z", "claude-opus-5-5"), None);
        assert_eq!(m("2026-10-15T10:17:01Z", "claude-opus-5-5"), None);
        assert_eq!(m("2026-10-15T10:16:59Z", "claude-opus-5-5"), Some("b.json"), "a 15-minute request still matches");
        assert_eq!(m("2026-10-15T10:02:10Z", "claude-sonnet-5-5"), None);
        assert_eq!(match_entry(&sels, "", "claude-opus-5-5", utc("2026-10-15T10:02:10Z")), None);
    }

    #[test]
    fn matching_prefers_requests_still_running() {
        // A long request (a.json) overlaps a short later one (b.json) that already ended.
        let sels = vec![
            s("2026-10-15T10:00:00Z", Some("2026-10-15T10:05:00Z"), '8', "a.json", "m"),
            s("2026-10-15T10:01:00Z", Some("2026-10-15T10:01:10Z"), '8', "b.json", "m"),
        ];
        let m = |ts: &str| match_entry(&sels, "8", "m", utc(ts)).map(|s| s.auth.as_str());
        assert_eq!(m("2026-10-15T10:01:05Z"), Some("b.json"), "both running: latest start");
        assert_eq!(m("2026-10-15T10:04:00Z"), Some("a.json"), "b ended long before");
        assert_eq!(m("2026-10-15T10:01:14Z"), Some("b.json"), "within the end slack");
        // Nothing running: the latest earlier request is the best guess.
        assert_eq!(m("2026-10-15T10:06:00Z"), Some("b.json"));
        // Unknown end (its end line not read): still a candidate.
        let open = vec![s("2026-10-15T10:00:00Z", None, '8', "c.json", "m")];
        assert_eq!(match_entry(&open, "8", "m", utc("2026-10-15T10:03:00Z")).map(|s| s.auth.as_str()), Some("c.json"));
    }

    fn local(t: DateTime<Utc>) -> String {
        t.with_timezone(&ist()).format("%Y-%m-%d %H:%M:%S").to_string()
    }

    #[test]
    fn log_is_read_by_time_with_rotation_and_reselection() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("logs")).unwrap();
        let base = utc("2026-10-15T10:00:00Z");
        // A big rotated file of old noise, then the requests that matter in main.log.
        let mut old = File::create(dir.path().join("logs/main-2026-10-12T12-44-49.634.log")).unwrap();
        for i in 0..20_000 {
            let t = base - Duration::days(2) + Duration::seconds(i);
            writeln!(old, "{}", sel(&local(t), &format!("o{i}"), "cache hit", "claude:8...", "old.json", "m")).unwrap();
        }
        // Older than the window: never read.
        drop(old);
        let lines = [
            sel(&local(base - Duration::hours(2)), "r0", "cache hit", "claude:8...", "too-old.json", "m"),
            "a line without a timestamp".to_string(),
            sel(&local(base), "r1", "cache hit", "claude:8...", "first.json", "m"),
            sel(&local(base), "r1", "cache hit but auth unavailable, reselected", "claude:8...", "second.json", "m"),
            sel(&local(base + Duration::seconds(1)), "r2", "cache hit", "claude:3...", "other.json", "m"),
            sel(&local(base + Duration::seconds(2)), "r3", "LCP cache hit", "lcp:v1:8...", "lcp.json", "m"),
            format!("[{}] [r1] [info ] [gin_logger.go:108] 200 |  9s | 127.0.0.1 | POST \"/v1/messages\"", local(base + Duration::seconds(9))),
            sel(&local(base + Duration::hours(3)), "r9", "cache hit", "claude:8...", "late.json", "m"),
        ];
        fs::write(dir.path().join("logs/main.log"), lines.join("\n") + "\n").unwrap();
        let chars: HashSet<char> = ['8'].into();
        let log = PoolLog::load(dir.path(), base - Duration::minutes(15), base + Duration::minutes(1), &chars, &ist());
        assert_eq!(log.selections.len(), 1, "{:?}", log.selections);
        let s = &log.selections[0];
        assert_eq!((s.request_id.as_str(), s.auth.as_str()), ("r1", "second.json"), "the reselected auth served it");
        assert_eq!(s.at, base);
        assert_eq!(s.end, Some(base + Duration::seconds(9)));
    }

    #[test]
    fn seek_skips_old_parts_of_a_large_log() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("logs")).unwrap();
        let base = utc("2026-10-15T10:00:00Z");
        let mut f = File::create(dir.path().join("logs/main.log")).unwrap();
        for i in 0..30_000i64 {
            let t = base - Duration::seconds(30_000 - i);
            writeln!(f, "{}", sel(&local(t), &format!("q{i}"), "cache hit", "claude:8...", "noise.json", "m")).unwrap();
        }
        writeln!(f, "{}", sel(&local(base), "hit", "cache hit", "claude:8...", "hit.json", "m")).unwrap();
        drop(f);
        let mut file = File::open(dir.path().join("logs/main.log")).unwrap();
        let len = file.metadata().unwrap().len();
        let target = (base - Duration::minutes(5)).with_timezone(&ist()).naive_local();
        let off = seek_time(&mut file, len, target).unwrap();
        assert!(off > len / 2, "binary search skipped most of the file: {off} of {len}");
        assert!(first_time_after(&mut file, off, len).unwrap().unwrap() <= target);

        let chars: HashSet<char> = ['8'].into();
        let log = PoolLog::load(dir.path(), base - Duration::minutes(1), base + Duration::seconds(1), &chars, &ist());
        assert_eq!(log.selections.len(), 61);
        assert_eq!(log.selections.last().unwrap().auth, "hit.json");
    }

    #[test]
    fn auth_files_yield_identity_only() {
        let a = parse_auth(br#"{"type":"claude","email":"pool@example.org","account_uuid":"acc-pool","organization_uuid":"org-1",
            "organization_name":"Org","access_token":"sk-ant-oat-SECRET","refresh_token":"sk-ant-ort-SECRET","id_token":"x","priority":1}"#).unwrap();
        assert_eq!(a, IngestAccount {
            account_uuid: "acc-pool".into(), org_uuid: Some("org-1".into()), email: Some("pool@example.org".into()),
            org_type: None, rate_limit_tier: None,
        });
        assert!(!format!("{a:?}").contains("SECRET"));
        for bad in [
            &br#"{"type":"codex","email":"x@y","account_id":"a","access_token":"t"}"#[..],
            br#"{"email":"x@y","account_uuid":"a"}"#,
            br#"{"type":"claude","email":"x@y"}"#,
            br#"{"type":"claude","account_uuid":"  "}"#,
            br#"{"type":"claude","account_uuid":"api_key:1:2"}"#,
            br#"not json"#,
        ] {
            assert_eq!(parse_auth(bad), None, "{}", String::from_utf8_lossy(bad));
        }
        assert_eq!(auth_path(Path::new("/p"), "claude-a@b.json"), Some(PathBuf::from("/p/claude-a@b.json")));
        for bad in ["../x.json", "a/b.json", ".hidden.json", "config.yaml", "x\\y.json"] {
            assert_eq!(auth_path(Path::new("/p"), bad), None, "{bad}");
        }
    }

    #[test]
    fn resolver_caches_auth_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("p.json"), r#"{"type":"claude","account_uuid":"acc-p","email":"p@x"}"#).unwrap();
        let mut r = PoolResolver::default();
        assert_eq!(r.account(dir.path(), "p.json").unwrap().account_uuid, "acc-p");
        fs::remove_file(dir.path().join("p.json")).unwrap();
        assert_eq!(r.account(dir.path(), "p.json").unwrap().account_uuid, "acc-p", "parsed once per run");
        assert_eq!(r.account(dir.path(), "missing.json"), None);
    }

    fn with_log(paths: &Paths) {
        fs::create_dir_all(paths.cliproxy_dir.join("logs")).unwrap();
        fs::write(paths.cliproxy_dir.join(MAIN_LOG), "").unwrap();
    }

    fn settings(path: &Path, url: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!(r#"{{"apiKeyHelper":"secret-helper","env":{{"ANTHROPIC_API_KEY":"sk-SECRET","ANTHROPIC_BASE_URL":"{url}"}}}}"#)).unwrap();
    }

    #[test]
    fn mode_env_beats_saved_beats_default() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        assert_eq!(mode(&paths, None), (ProxyMode::Auto, ModeSource::Default));
        save_mode(&paths, ProxyMode::Off).unwrap();
        assert_eq!(mode(&paths, None), (ProxyMode::Off, ModeSource::Saved));
        assert_eq!(mode(&paths, Some("ON")), (ProxyMode::On, ModeSource::Env));
        assert_eq!(mode(&paths, Some(" auto ")), (ProxyMode::Auto, ModeSource::Env));
        assert_eq!(mode(&paths, Some("bogus")), (ProxyMode::Off, ModeSource::Saved), "an invalid override is ignored");
        assert_eq!(mode(&paths, Some("")), (ProxyMode::Off, ModeSource::Saved));
    }

    #[test]
    fn base_url_sources_in_precedence_order() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let cwd = dir.path().join("proj");
        let find = |env: Option<&str>| find_base_url(&paths, Some(&cwd), env);
        assert_eq!(find(None), None);
        settings(&paths.claude_dir.join("settings.json"), "http://user.example");
        assert_eq!(find(None), Some((UrlSource::UserSettings, "http://user.example".into())));
        settings(&cwd.join(".claude/settings.json"), "http://project.example");
        assert_eq!(find(None), Some((UrlSource::ProjectSettings, "http://project.example".into())));
        settings(&cwd.join(".claude/settings.local.json"), "http://local.example");
        assert_eq!(find(None), Some((UrlSource::ProjectLocalSettings, "http://local.example".into())));
        settings(&paths.managed_settings, "http://managed.example");
        assert_eq!(find(None), Some((UrlSource::ManagedSettings, "http://managed.example".into())));
        assert_eq!(find(Some("http://127.0.0.1:8317")), Some((UrlSource::Env, "http://127.0.0.1:8317".into())), "the environment wins");
        assert_eq!(find(Some("  ")), Some((UrlSource::ManagedSettings, "http://managed.example".into())), "blank env is unset");
        // Without a cwd, project files are not consulted.
        fs::remove_file(&paths.managed_settings).unwrap();
        assert_eq!(find_base_url(&paths, None, None), Some((UrlSource::UserSettings, "http://user.example".into())));
        // A settings file without the key, or malformed, is skipped.
        fs::write(cwd.join(".claude/settings.local.json"), r#"{"env":{"OTHER":"x"}}"#).unwrap();
        fs::write(cwd.join(".claude/settings.json"), "{not json").unwrap();
        assert_eq!(find(None), Some((UrlSource::UserSettings, "http://user.example".into())));
    }

    #[test]
    fn proxy_urls_by_host_or_config_port() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(config_port(dir.path()), None);
        fs::write(dir.path().join("config.yaml"), "host: \"\"\napi-keys:\n  - port: 1\nport: 8317 # listen\nremote-management:\n  secret-key: x\n").unwrap();
        assert_eq!(config_port(dir.path()), Some(8317));
        // Current CLIProxyAPI: `server.port`; ports elsewhere are not it.
        fs::write(dir.path().join("config.yaml"), "# c\nmodels:\n  port: 1\nserver:\n  host: \"\"\n  tls:\n    port: 2\n  port: 8318\nother:\n  x: 1\n").unwrap();
        assert_eq!(config_port(dir.path()), Some(8318));
        fs::write(dir.path().join("config.yaml"), "server:\n  host: x\nmodels:\n  port: 1\n").unwrap();
        assert_eq!(config_port(dir.path()), None);
        assert!(url_is_proxy("http://127.0.0.1:9999", None));
        assert!(url_is_proxy("http://192.168.1.20:8317", Some(8317)), "the proxy's port on a LAN address");
        assert!(!url_is_proxy("http://192.168.1.20:8318", Some(8317)));
        assert!(!url_is_proxy("https://api.anthropic.com", Some(8317)));
        assert!(url_is_proxy("https://proxy.example", Some(443)));
        assert_eq!(display_url("http://user:pw@127.0.0.1:8317/v1?k=secret"), "http://127.0.0.1:8317");
    }

    #[test]
    fn detection_reasons() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let now = utc("2026-10-15T10:00:00Z");
        let d = detect(&paths, None, None, now);
        assert!(!d.proxy_url && d.source.is_none());
        assert_eq!(d.describe(), "ANTHROPIC_BASE_URL is not set");
        let d = detect(&paths, None, Some("http://127.0.0.1:8317"), now);
        assert!(d.proxy_url && !d.log_found);
        assert!(d.describe().contains("but there is no log"), "{}", d.describe());
        with_log(&paths);
        let d = detect(&paths, None, Some("http://127.0.0.1:8317"), now);
        assert!(d.proxy_url && d.log_found && d.describe().contains("Claude Code's environment"), "{}", d.describe());
        let d = detect(&paths, None, Some("https://api.anthropic.com"), now);
        assert!(!d.proxy_url && d.describe().contains("is not a local proxy"));
        // Via the proxy's configured port.
        fs::write(paths.cliproxy_dir.join("config.yaml"), "port: 9000\n").unwrap();
        settings(&paths.claude_dir.join("settings.json"), "http://10.0.0.5:9000");
        let d = detect(&paths, None, None, now);
        assert!(d.proxy_url && d.source == Some(UrlSource::UserSettings), "{d:?}");
        assert!(!serde_json::to_string(&d).unwrap().contains("SECRET"));
    }

    #[test]
    fn detection_is_recorded_per_session() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let now = utc("2026-10-15T10:00:00Z");
        let proxy = Some("http://127.0.0.1:8317");
        with_log(&paths);
        record_session(&paths, "s1", None, proxy, HookPoint::SessionStart, ProxyMode::Auto, now);
        let m = session_marker(&paths, "s1").unwrap();
        assert!(m.proxy_url && m.log_found && m.source == Some(UrlSource::Env));
        assert_eq!(session_proxy(&paths, ProxyMode::Auto, "s1", None, now).unwrap().dir, paths.cliproxy_dir);

        // Later hooks only add a missing record.
        record_session(&paths, "s1", None, None, HookPoint::Later, ProxyMode::Auto, now + Duration::hours(1));
        assert_eq!(session_marker(&paths, "s1"), Some(m));
        record_session(&paths, "s2", None, proxy, HookPoint::Later, ProxyMode::Auto, now);
        assert!(session_marker(&paths, "s2").unwrap().proxy_url);

        // Resumed without the proxy: SessionStart records the new state, and a
        // later terminal sync (no env) still knows.
        record_session(&paths, "s1", None, None, HookPoint::SessionStart, ProxyMode::Auto, now);
        assert!(!session_marker(&paths, "s1").unwrap().proxy_url);
        assert!(session_proxy(&paths, ProxyMode::Auto, "s1", None, now).is_err());
        assert!(session_proxy(&paths, ProxyMode::On, "s1", None, now).is_ok(), "on: every session");
        assert!(session_proxy(&paths, ProxyMode::Off, "s2", None, now).is_err(), "off: none");

        // Off mode records nothing.
        record_session(&paths, "s3", None, proxy, HookPoint::SessionStart, ProxyMode::Off, now);
        assert_eq!(session_marker(&paths, "s3"), None);

        // No record: a sync judges from the session's settings files.
        let cwd = dir.path().join("p");
        settings(&cwd.join(".claude/settings.json"), "http://localhost:8317");
        let sp = session_proxy(&paths, ProxyMode::Auto, "s4", Some(cwd.to_str().unwrap()), now).unwrap();
        assert!(sp.why.contains("project .claude/settings.json"), "{}", sp.why);

        // A proxy URL without a log keeps today's behaviour.
        fs::remove_file(paths.cliproxy_dir.join(MAIN_LOG)).unwrap();
        let err = session_proxy(&paths, ProxyMode::Auto, "s2", None, now).unwrap_err();
        assert!(err.contains("no CLIProxyAPI log"), "{err}");
        assert!(session_proxy(&paths, ProxyMode::On, "s2", None, now).is_err());

        // Odd session ids never become paths.
        record_session(&paths, "../evil", None, proxy, HookPoint::SessionStart, ProxyMode::Auto, now);
        assert!(!dir.path().join("state/evil.json").exists());
        assert_eq!(marker_path(&paths, "../evil"), None);
        assert_eq!(marker_path(&paths, ""), None);
    }

    #[test]
    fn status_report_shows_mode_detection_and_pool_without_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let now = utc("2026-10-15T10:00:00Z");
        let out = status_report(&paths, None, None, None, now);
        assert!(out.contains("proxy mode: auto (default)"), "{out}");
        assert!(out.contains("(not found)") && out.contains("config port: unknown") && out.contains("pool accounts (0)"), "{out}");
        assert!(out.contains("this shell: ANTHROPIC_BASE_URL is not set"), "{out}");

        with_log(&paths);
        fs::write(paths.cliproxy_dir.join("config.yaml"), "port: 8317\n").unwrap();
        fs::write(paths.cliproxy_dir.join("claude-a.json"),
            r#"{"type":"claude","email":"pool-a@example.org","account_uuid":"u-a","access_token":"sk-ant-SECRET","refresh_token":"SECRET2"}"#).unwrap();
        fs::write(paths.cliproxy_dir.join("codex-b.json"), r#"{"type":"codex","email":"codex@example.org","access_token":"SECRET3"}"#).unwrap();
        record_session(&paths, "sess-1", None, Some("http://127.0.0.1:8317"), HookPoint::SessionStart, ProxyMode::Auto, now);
        save_mode(&paths, ProxyMode::On).unwrap();
        let out = status_report(&paths, None, Some("http://127.0.0.1:8317"), None, now);
        assert!(out.contains("proxy mode: on (saved)"), "{out}");
        assert!(out.contains("(found, 0 KB)") && out.contains("config port: 8317"), "{out}");
        assert!(out.contains("this shell: ANTHROPIC_BASE_URL=http://127.0.0.1:8317 from Claude Code's environment points at a CLIProxyAPI (log found)"), "{out}");
        assert!(out.contains("pool accounts (1):\n  pool-a@example.org"), "{out}");
        assert!(out.contains("sess-1 [pool]"), "{out}");
        assert!(!out.contains("SECRET") && !out.contains("codex@"), "{out}");

        let out = status_report(&paths, Some("off"), None, None, now);
        assert!(out.contains("proxy mode: off (from CCTRACK_PROXY_MODE)") && out.contains("sess-1 [Claude account (mode off)]"), "{out}");
        let out = status_report(&paths, Some("maybe"), None, None, now);
        assert!(out.contains("proxy mode: on (saved)") && out.contains("ignoring CCTRACK_PROXY_MODE"), "{out}");
    }
}

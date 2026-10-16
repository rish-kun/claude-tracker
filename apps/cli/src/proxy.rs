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
//! The selector line is written when the request starts (the proxy host's
//! local time, second precision) and the `gin_logger` line with the same
//! request id when it ends. `session=claude:X...` carries only the first
//! character of Claude Code's session id. CLIProxyAPI keeps a session on one
//! pool account (session affinity) until that account becomes unavailable.
//! `auth=` names a JSON file in the proxy's home whose non-secret fields
//! (`account_uuid`, `email`, `organization_uuid`) identify the account; its
//! tokens are never kept, logged or uploaded. Those identity fields are
//! cached in the state dir so usage stays attributable after an auth file is
//! deleted or renamed.
//!
//! Hooks record per session whether it runs through such a proxy (their
//! environment is Claude Code's); syncs, which may run from a terminal without
//! that environment, read the record. A session with no record and no
//! `ANTHROPIC_BASE_URL` in any settings file (e.g. started before the hooks
//! were installed) is judged from the log itself (see [`infer`]).

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, FixedOffset, LocalResult, NaiveDateTime, Offset, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;
use crate::sessions::SessionStates;
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

/// Most bytes read from one log file for one group of entries, after
/// seeking to the group's time.
const MAX_SCAN_BYTES: u64 = 256 * 1024 * 1024;

/// Entries further apart than this are read from the log separately.
const GROUP_GAP: Duration = Duration::hours(1);
/// Longest time span of one group, so each read stays small.
const GROUP_SPAN: Duration = Duration::hours(6);

/// A session with no hook record and no `ANTHROPIC_BASE_URL` is inferred to
/// run through the proxy when at least this many of its entries were seen...
pub const INFER_MIN_ENTRIES: u64 = 3;
/// ...and at least this percentage of them ran during a request of a session
/// with the same first character and model.
pub const INFER_PERCENT: u64 = 80;

/// UTC offsets tried when calibrating a log's clock: ±14 h in 15-minute steps.
const CALIBRATION_RANGE_MINUTES: i32 = 14 * 60;
const CALIBRATION_STEP_MINUTES: i32 = 15;
/// A calibration that found no clear winner is not retried sooner than this.
const CALIBRATION_RETRY: Duration = Duration::hours(24);

/// Whether `url` points at this machine (`localhost`, 127.0.0.0/8, ::1).
pub fn is_loopback_url(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url.trim()) else { return false };
    if !matches!(u.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = u.host_str() else { return false };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost") || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The lowercase host and port (explicit or the scheme's default) of an http(s) URL.
fn url_host_port(url: &str) -> Option<(String, Option<u16>)> {
    let u = reqwest::Url::parse(url.trim()).ok().filter(|u| matches!(u.scheme(), "http" | "https"))?;
    let host = u.host_str()?.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    Some((host, u.port_or_known_default()))
}

/// Loopback, unspecified, private (RFC 1918, fc00::/7) or link-local
/// (169.254/16, fe80::/10) addresses.
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback() || v.is_unspecified() || v.is_private() || v.is_link_local(),
        IpAddr::V6(v) => {
            let first = v.segments()[0];
            v.is_loopback()
                || v.is_unspecified()
                || first & 0xfe00 == 0xfc00
                || first & 0xffc0 == 0xfe80
                || v.to_ipv4_mapped().is_some_and(|v4| is_private_ip(IpAddr::V4(v4)))
        }
    }
}

/// Whether `host` (lowercase) is this machine or on its private network:
/// loopback names, private or link-local addresses, or `hostname`
/// (with or without `.local`).
fn is_local_host(host: &str, hostname: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return is_private_ip(ip);
    }
    let bare = |s: &str| s.trim_end_matches('.').trim_end_matches(".local").to_string();
    !hostname.is_empty() && bare(host) == bare(&hostname.to_ascii_lowercase())
}

fn this_hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

/// Whether `url` points at a CLIProxyAPI: a loopback host, or the port the
/// proxy's `config.yaml` listens on at a host that is this machine or a
/// private/link-local address (never a public name such as
/// `localhost.evil.com`).
pub fn url_is_proxy(url: &str, config_port: Option<u16>) -> bool {
    url_is_proxy_on(url, config_port, &this_hostname())
}

fn url_is_proxy_on(url: &str, config_port: Option<u16>, hostname: &str) -> bool {
    is_loopback_url(url)
        || config_port.is_some_and(|p| url_host_port(url).is_some_and(|(h, port)| port == Some(p) && is_local_host(&h, hostname)))
}

/// `scheme://host:port` only: no credentials, path or query are kept.
fn display_url(url: &str) -> String {
    if url.trim().is_empty() {
        return String::new();
    }
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

/// A warning for an invalid `CCTRACK_PROXY_MODE` value (which is ignored).
pub fn mode_env_warning(env: Option<&str>) -> Option<String> {
    env.filter(|v| ProxyMode::parse(v).is_none())
        .map(|v| format!("ignoring {MODE_ENV}={:?}: expected auto, on or off", v.chars().take(20).collect::<String>()))
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

/// The file's `env.ANTHROPIC_BASE_URL`, trimmed; `Some("")` when it is
/// defined as an empty string.
fn settings_base_url(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let s: SettingsEnv = serde_json::from_slice(&bytes).ok()?;
    s.env?.base_url.map(|u| u.trim().to_string())
}

/// The `ANTHROPIC_BASE_URL` in effect and where it came from: the
/// environment first, then managed, project-local, project and user
/// settings. As in Claude Code, a value defined in a higher-priority file
/// wins even when it is empty (`""`: explicitly unset, so not proxied).
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
    /// The URL's scheme, host and port ("" when defined as empty).
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
    /// As recorded by a hook (its environment is Claude Code's).
    pub fn describe(&self) -> String {
        self.describe_as(UrlSource::Env.label(), self.log_found)
    }

    /// As detected in a terminal: an environment value is this shell's.
    pub fn describe_from_shell(&self) -> String {
        self.describe_as("this shell's environment", self.log_found)
    }

    /// As recorded by a hook, with whether the log exists now.
    pub fn describe_now(&self) -> String {
        self.describe_as(UrlSource::Env.label(), self.log_exists_now())
    }

    pub fn log_exists_now(&self) -> bool {
        self.cliproxy_dir.join(MAIN_LOG).is_file()
    }

    fn describe_as(&self, env_label: &str, log_found: bool) -> String {
        let label = |s: &UrlSource| if *s == UrlSource::Env { env_label } else { s.label() };
        match (&self.source, &self.base_url) {
            (Some(src), Some(url)) if url.is_empty() => format!(
                "ANTHROPIC_BASE_URL is set to \"\" in {}, which overrides any lower-priority setting: not proxied",
                label(src)
            ),
            (Some(src), Some(url)) if self.proxy_url && log_found => {
                format!("ANTHROPIC_BASE_URL={url} from {} points at a CLIProxyAPI (log found)", label(src))
            }
            (Some(src), Some(url)) if self.proxy_url => format!(
                "ANTHROPIC_BASE_URL={url} from {} points at a CLIProxyAPI, but there is no log at {}; usage is credited to the Claude account as before",
                label(src),
                self.cliproxy_dir.join(MAIN_LOG).display()
            ),
            (Some(src), Some(url)) => format!("ANTHROPIC_BASE_URL={url} from {} is not a local proxy", label(src)),
            _ => "ANTHROPIC_BASE_URL is not set".to_string(),
        }
    }
}

/// Detects whether a session with working directory `cwd` and (hook)
/// environment value `env_url` runs through a CLIProxyAPI pool.
pub fn detect(paths: &Paths, cwd: Option<&Path>, env_url: Option<&str>, now: DateTime<Utc>) -> Detection {
    let found = find_base_url(paths, cwd, env_url);
    let proxy_url = found.as_ref().is_some_and(|(_, u)| !u.is_empty() && url_is_proxy(u, config_port(&paths.cliproxy_dir)));
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

/// A session record (`<id>.json`), not a temp file left by an interrupted
/// atomic write (`<id>.tmp.<pid>`).
fn is_marker_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "json")
}

/// Removes session records older than `max_age` (best effort).
pub fn prune_markers(paths: &Paths, max_age: std::time::Duration) {
    let Ok(items) = fs::read_dir(paths.proxy_sessions_dir()) else { return };
    let Some(cutoff) = std::time::SystemTime::now().checked_sub(max_age) else { return };
    for item in items.flatten() {
        if is_marker_file(&item.path()) && item.metadata().and_then(|m| m.modified()).is_ok_and(|m| m < cutoff) {
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

/// How a sync treats a session.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Attributed from the proxy log in `dir`.
    Proxied(SessionProxy),
    /// Credited to the Claude account, and why. `dir` is a proxy home whose
    /// log can still be compared with the session (to flag sessions that look
    /// proxied).
    NotProxied { why: String, dir: Option<PathBuf> },
    /// No hook record and no `ANTHROPIC_BASE_URL` anywhere, and a log exists:
    /// decided from the log (see [`infer`]).
    Infer { dir: PathBuf },
}

/// Whether a sync attributes `session_id` from a CLIProxyAPI log, and why
/// (or why not). In auto mode a session without a hook record is judged
/// from Claude Code's settings files for `cwd` (a sync's own environment is
/// not the session's), else from the log. Mode `on` forces log attribution
/// for every session, even one a hook saw as not proxied.
pub fn session_verdict(paths: &Paths, mode: ProxyMode, session_id: &str, cwd: Option<&str>, now: DateTime<Utc>) -> Verdict {
    let marker = session_marker(paths, session_id);
    let (dir, why) = match mode {
        ProxyMode::Off => return Verdict::NotProxied { why: "proxy mode is off".into(), dir: None },
        ProxyMode::On => (marker.map_or_else(|| paths.cliproxy_dir.clone(), |m| m.cliproxy_dir), "proxy mode is on".to_string()),
        ProxyMode::Auto => {
            let recorded = marker.is_some();
            let d = marker.unwrap_or_else(|| detect(paths, cwd.map(Path::new), None, now));
            if !d.proxy_url {
                let log_exists = d.cliproxy_dir.join(MAIN_LOG).is_file();
                if !recorded && d.source.is_none() && log_exists {
                    return Verdict::Infer { dir: d.cliproxy_dir };
                }
                return Verdict::NotProxied { why: d.describe(), dir: log_exists.then_some(d.cliproxy_dir) };
            }
            let why = format!("ANTHROPIC_BASE_URL from {}", d.source.map_or("settings", |s| s.label()));
            (d.cliproxy_dir, why)
        }
    };
    let log = dir.join(MAIN_LOG);
    if !log.is_file() {
        return Verdict::NotProxied {
            why: format!("{why}, but no CLIProxyAPI log at {}; credited to the Claude account as before", log.display()),
            dir: None,
        };
    }
    Verdict::Proxied(SessionProxy { why: format!("{why}; pool account read from {}", log.display()), dir })
}

/// [`session_verdict`] as Ok (attributed from a log) or Err (why not; a
/// session to infer from the log is not decided yet).
pub fn session_proxy(paths: &Paths, mode: ProxyMode, session_id: &str, cwd: Option<&str>, now: DateTime<Utc>) -> Result<SessionProxy, String> {
    match session_verdict(paths, mode, session_id, cwd, now) {
        Verdict::Proxied(sp) => Ok(sp),
        Verdict::NotProxied { why, .. } => Err(why),
        Verdict::Infer { .. } => Err("no hook record and no ANTHROPIC_BASE_URL: decided from the proxy log by a sync".into()),
    }
}

/// Whether `matched` of `total` entries having a running candidate request
/// (see [`has_running_candidate`]) means the session runs through the proxy;
/// None while there are too few entries to tell.
///
/// Thresholds: a plain session only has a candidate by coincidence (another
/// proxied session with the same first character and model running at that
/// moment), so its share stays near 0; a proxied session's share is near
/// 100%, short of it only where log lines are missing (a deleted rotated
/// log, a request still running when the log was read). 80% tolerates such
/// gaps while a coincidental overlap that large is implausible, and at least
/// 3 entries keep one or two lucky overlaps from deciding.
pub fn infer(matched: u64, total: u64) -> Option<bool> {
    (total >= INFER_MIN_ENTRIES).then(|| matched * 100 >= total * INFER_PERCENT)
}

/// Pool accounts in a proxy home: email (or account uuid) of each Claude auth file.
pub fn pool_accounts(dir: &Path) -> Vec<String> {
    let Ok(items) = fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<String> = items
        .flatten()
        .filter_map(|i| {
            let name = i.file_name().to_str()?.to_string();
            let a = parse_auth(&read_auth_file(&auth_path(dir, &name)?)?)?;
            Some(a.email.unwrap_or(a.account_uuid))
        })
        .collect();
    out.sort();
    out
}

/// A UTC offset in minutes as `UTC+05:30`.
pub fn fmt_offset(minutes: i32) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    let m = minutes.unsigned_abs();
    format!("UTC{sign}{:02}:{:02}", m / 60, m % 60)
}

/// `cctrack proxy status`: the mode, the proxy home, what this shell would
/// detect, the pool accounts (by email) and recent sessions. Never secrets.
/// Times are UTC.
pub fn status_report(paths: &Paths, env_mode: Option<&str>, env_url: Option<&str>, cwd: Option<&Path>, now: DateTime<Utc>) -> String {
    let (mode, source) = mode(paths, env_mode);
    let mut out = Vec::new();
    let origin = match source {
        ModeSource::Env => format!("from {MODE_ENV}"),
        ModeSource::Saved => "saved".to_string(),
        ModeSource::Default => "default".to_string(),
    };
    out.push(format!("proxy mode: {} ({origin})", mode.as_str()));
    if let Some(w) = mode_env_warning(env_mode) {
        out.push(format!("  {w}"));
    }
    out.push(match mode {
        ProxyMode::Off => "  usage is credited to the Claude account in ~/.claude.json".to_string(),
        ProxyMode::On => "  every session is credited from the CLIProxyAPI log, even sessions a hook saw as not proxied".to_string(),
        ProxyMode::Auto => format!(
            "  sessions whose ANTHROPIC_BASE_URL points at a CLIProxyAPI are credited to the pool account that served them; \
             sessions with no hook record and no ANTHROPIC_BASE_URL are inferred from the proxy log \
             (at least {INFER_PERCENT}% of at least {INFER_MIN_ENTRIES} entries ran during a matching request)"
        ),
    });
    let dir = &paths.cliproxy_dir;
    out.push(format!("CLIProxyAPI dir: {}", dir.display()));
    let log = dir.join(MAIN_LOG);
    let log_exists = log.is_file();
    out.push(match fs::metadata(&log) {
        Ok(m) => format!("  log: {} (found, {} KB)", log.display(), m.len() / 1024),
        Err(_) => format!("  log: {} (not found)", log.display()),
    });
    let clock = LogClock::load(paths);
    out.push(match clock.warning(dir) {
        Some(w) => format!("  warning: {w}"),
        None => "  log times: read as this machine's local time".to_string(),
    });
    out.push(match config_port(dir) {
        Some(p) => format!("  config port: {p}"),
        None => "  config port: unknown".to_string(),
    });
    let here = detect(paths, cwd, env_url, now);
    out.push(format!("this shell: {}", here.describe_from_shell()));
    let accounts = pool_accounts(dir);
    out.push(format!("pool accounts ({}):", accounts.len()));
    out.extend(accounts.iter().map(|a| format!("  {a}")));
    let mut recent: Vec<(String, Detection)> = fs::read_dir(paths.proxy_sessions_dir())
        .map(|items| {
            items
                .flatten()
                .filter(|i| is_marker_file(&i.path()))
                .filter_map(|i| {
                    let id = i.path().file_stem()?.to_str()?.to_string();
                    Some((id, read_json::<Detection>(&i.path())?))
                })
                .collect()
        })
        .unwrap_or_default();
    recent.sort_by(|a, b| b.1.detected_at.cmp(&a.1.detected_at).then(a.0.cmp(&b.0)));
    let states = SessionStates::load(&paths.sessions());
    let inferred_pool = |id: &str| states.get(id).and_then(|s| s.inferred.as_ref()).is_some_and(|i| i.proxied);
    out.push(format!("recent sessions ({} recorded; times UTC):", recent.len()));
    for (id, d) in recent.iter().take(10) {
        let log_now = d.log_exists_now();
        let verdict = match mode {
            ProxyMode::Off => "Claude account (mode off)",
            ProxyMode::On if log_now => "pool (mode on)",
            ProxyMode::On => "Claude account (mode on, but no log)",
            ProxyMode::Auto if d.proxy_url && log_now => "pool",
            // A hook record beats an earlier inference from the log.
            ProxyMode::Auto if inferred_pool(id) => "Claude account (hook record overrides the log inference)",
            ProxyMode::Auto => "Claude account",
        };
        out.push(format!("  {id} [{verdict}] {}: {}", d.detected_at.format("%Y-%m-%d %H:%M UTC"), d.describe_now()));
    }
    let inferred: Vec<_> = states.sessions.iter().filter_map(|(id, s)| Some((id, s.inferred.as_ref()?))).collect();
    if !inferred.is_empty() {
        out.push(format!("sessions inferred from the proxy log ({}):", inferred.len()));
        for (id, i) in inferred {
            // A session that got a hook record since is credited per that record.
            let label = if mode == ProxyMode::Auto && session_marker(paths, id).is_some() {
                "superseded by a hook record"
            } else if i.proxied {
                "pool"
            } else {
                "Claude account"
            };
            out.push(format!(
                "  {id} [{label}] decided {}: {} of {} entries ran during a matching proxy request",
                i.decided_at.format("%Y-%m-%d %H:%M UTC"),
                i.matched,
                i.total
            ));
        }
    }
    if mode != ProxyMode::Off {
        let suspicious: Vec<String> = states
            .sessions
            .iter()
            .filter_map(|(id, s)| {
                let why = s.not_proxied.as_ref()?;
                let ev = s.log_evidence.as_ref().filter(|e| infer(e.matched, e.total) == Some(true))?;
                Some(format!("  {id}: {} of {} entries ran during a matching proxy request; {why}", ev.matched, ev.total))
            })
            .collect();
        if !suspicious.is_empty() {
            out.push(format!("sessions that look proxied but were credited to the Claude account ({}):", suspicious.len()));
            out.extend(suspicious);
        }
    }
    if !log_exists && mode != ProxyMode::Off {
        out.push("no CLIProxyAPI log: every session is credited to the Claude account".to_string());
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

impl LogLine {
    fn at(&self) -> DateTime<Utc> {
        match self {
            LogLine::Selection(s) => s.at,
            LogLine::Done { at, .. } => *at,
        }
    }
}

/// How a log line's local time mapped to UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DstNote {
    Normal,
    /// Repeated by a DST fall-back: the earlier instant is used.
    Ambiguous,
    /// Skipped by a DST spring-forward: the earliest of the readings with
    /// the offsets before and after the change is used.
    Nonexistent,
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

/// A local wall-clock time in `tz` as UTC, deterministically: the earlier
/// instant when ambiguous, and for a time in a DST gap the earliest of its
/// readings with the offsets in effect a day before and a day after.
fn local_to_utc<Tz: TimeZone>(t: NaiveDateTime, tz: &Tz) -> Option<(DateTime<Utc>, DstNote)> {
    match tz.from_local_datetime(&t) {
        LocalResult::Single(d) => Some((d.with_timezone(&Utc), DstNote::Normal)),
        LocalResult::Ambiguous(a, b) => Some((a.with_timezone(&Utc).min(b.with_timezone(&Utc)), DstNote::Ambiguous)),
        LocalResult::None => {
            let offset = |n: NaiveDateTime| tz.from_local_datetime(&n).earliest().map(|d| d.offset().fix());
            let utc = [offset(t - Duration::days(1)), offset(t + Duration::days(1))]
                .into_iter()
                .flatten()
                .map(|o| (t - Duration::seconds(i64::from(o.local_minus_utc()))).and_utc())
                .min()?;
            Some((utc, DstNote::Nonexistent))
        }
    }
}

/// Parses a selector or `gin_logger` line; None for any other line.
pub fn parse_log_line<Tz: TimeZone>(line: &str, tz: &Tz) -> Option<LogLine> {
    parse_noted(line, tz).map(|(l, _)| l)
}

fn parse_noted<Tz: TimeZone>(line: &str, tz: &Tz) -> Option<(LogLine, DstNote)> {
    let (ts, rest) = bracketed(line)?;
    let (at, note) = local_to_utc(NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S").ok()?, tz)?;
    let (request_id, rest) = bracketed(rest)?;
    let request_id = request_id.trim();
    if request_id.is_empty() || request_id.contains(' ') {
        return None;
    }
    let (_level, rest) = bracketed(rest)?;
    let (source, msg) = bracketed(rest)?;
    if source.starts_with("gin_logger.go") {
        return Some((LogLine::Done { at, request_id: request_id.to_string() }, note));
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
    let s = Selection {
        at,
        end: None,
        request_id: request_id.to_string(),
        session_char: session_char.map(|c| c.to_ascii_lowercase()),
        auth: auth.to_string(),
        model: model.to_string(),
    };
    Some((LogLine::Selection(s), note))
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

/// `main.log` and its rotated `main-<timestamp>.log` files, oldest first
/// (`main-…` sorts before `main.log`).
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

/// Groups entry times into log read windows `[first - REQUEST_WINDOW, last + 1 s]`:
/// a new group starts after a gap of more than [`GROUP_GAP`] or when a group
/// would span more than [`GROUP_SPAN`].
pub fn read_windows(times: &[DateTime<Utc>]) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    let mut ts = times.to_vec();
    ts.sort();
    let window = |(f, l): (DateTime<Utc>, DateTime<Utc>)| (f - REQUEST_WINDOW, l + Duration::seconds(1));
    let mut out = Vec::new();
    let mut group: Option<(DateTime<Utc>, DateTime<Utc>)> = None;
    for t in ts {
        group = match group {
            Some((f, l)) if t - l <= GROUP_GAP && t - f <= GROUP_SPAN => Some((f, t)),
            Some(g) => {
                out.push(window(g));
                Some((t, t))
            }
            None => Some((t, t)),
        };
    }
    out.extend(group.map(window));
    out
}

/// Pool selections of Claude Code sessions from the proxy's logs.
#[derive(Debug, Default)]
pub struct PoolLog {
    /// Sorted by start (log order for equal starts).
    pub selections: Vec<Selection>,
    /// Lines read whose local time was repeated (DST fall-back) or skipped
    /// (spring-forward) on this machine's clock; see [`local_to_utc`].
    pub dst_ambiguous: usize,
    pub dst_nonexistent: usize,
}

impl PoolLog {
    /// Reads the selections that started in `[from, to]` (see [`Self::load_windows`]).
    pub fn load<Tz: TimeZone>(dir: &Path, from: DateTime<Utc>, to: DateTime<Utc>, chars: &HashSet<char>, tz: &Tz) -> Self {
        Self::load_windows(dir, &[(from, to)], chars, tz)
    }

    /// Reads the selections that started in any of `windows` for sessions
    /// whose id starts with one of `chars`, and their end times, also when a
    /// request ends in a later (rotated) file than it started in. Each log
    /// file is entered by binary search on time once per window and read for
    /// at most [`MAX_SCAN_BYTES`] per window (never re-reading bytes); files
    /// last written before a window are skipped for it. Log times are read
    /// as local time in `tz`.
    pub fn load_windows<Tz: TimeZone>(dir: &Path, windows: &[(DateTime<Utc>, DateTime<Utc>)], chars: &HashSet<char>, tz: &Tz) -> Self {
        Self::load_capped(dir, windows, chars, tz, MAX_SCAN_BYTES)
    }

    fn load_capped<Tz: TimeZone>(
        dir: &Path,
        windows: &[(DateTime<Utc>, DateTime<Utc>)],
        chars: &HashSet<char>,
        tz: &Tz,
        cap: u64,
    ) -> Self {
        let mut windows = windows.to_vec();
        windows.sort();
        let mut log = PoolLog::default();
        // Request id → selection, across files: a request that starts in a
        // rotated file gets its end from the next one.
        let mut index: HashMap<String, usize> = HashMap::new();
        for path in log_files(dir) {
            let _ = log.read_file(&path, &windows, chars, tz, cap, &mut index);
        }
        log.selections.sort_by_key(|s| s.at);
        log
    }

    fn read_file<Tz: TimeZone>(
        &mut self,
        path: &Path,
        windows: &[(DateTime<Utc>, DateTime<Utc>)],
        chars: &HashSet<char>,
        tz: &Tz,
        cap: u64,
        index: &mut HashMap<String, usize>,
    ) -> io::Result<()> {
        let mut f = File::open(path)?;
        let meta = f.metadata()?;
        let len = meta.len();
        let modified = meta.modified().ok().map(DateTime::<Utc>::from);
        // Windows are sorted by start, and so by end.
        let in_window = |t: DateTime<Utc>| {
            let i = windows.partition_point(|w| w.1 < t);
            windows.get(i).is_some_and(|w| w.0 <= t)
        };
        // Bytes before this offset were read (it is a line start).
        let mut done = 0u64;
        for &(from, to) in windows {
            if modified.is_some_and(|m| m < from) {
                continue;
            }
            // An hour of margin covers clock changes between local and UTC.
            let target = (from - Duration::hours(1)).with_timezone(tz).naive_local();
            let start = seek_time(&mut f, len, target)?.max(done);
            if start >= len {
                continue;
            }
            f.seek(SeekFrom::Start(start))?;
            let mut r = BufReader::new((&mut f).take(cap));
            let mut pos = start;
            let mut buf = Vec::new();
            if start > 0 && start != done {
                pos += r.read_until(b'\n', &mut buf)? as u64; // partial line
            }
            let stop = to + REQUEST_WINDOW; // end lines of the last requests
            loop {
                buf.clear();
                let n = r.read_until(b'\n', &mut buf)?;
                // EOF, the read cap, or a line still being written.
                if n == 0 || buf.last() != Some(&b'\n') {
                    break;
                }
                let line = String::from_utf8_lossy(&buf);
                if let Some((parsed, note)) = parse_noted(line.trim_end(), tz) {
                    if parsed.at() > stop {
                        break; // left unread for a later window
                    }
                    match note {
                        DstNote::Normal => {}
                        DstNote::Ambiguous => self.dst_ambiguous += 1,
                        DstNote::Nonexistent => self.dst_nonexistent += 1,
                    }
                    match parsed {
                        LogLine::Selection(s) => {
                            if in_window(s.at) && s.session_char.is_some_and(|c| chars.contains(&c)) {
                                self.add(s, index);
                            }
                        }
                        LogLine::Done { at, request_id } => {
                            if let Some(&i) = index.get(&request_id) {
                                let s = &mut self.selections[i];
                                if s.end.is_none() && at >= s.at {
                                    s.end = Some(at);
                                }
                            }
                        }
                    }
                }
                pos += n as u64;
            }
            done = done.max(pos);
        }
        Ok(())
    }

    fn add(&mut self, s: Selection, index: &mut HashMap<String, usize>) {
        if let Some(&i) = index.get(&s.request_id) {
            let prev = &mut self.selections[i];
            // A request reselected later keeps its start; the last auth served it.
            if prev.end.is_none() && s.at >= prev.at && s.at - prev.at <= REQUEST_WINDOW {
                prev.auth = s.auth;
                return;
            }
        }
        index.insert(s.request_id.clone(), self.selections.len());
        self.selections.push(s);
    }
}

/// Whether a transcript model and a log model name the same model
/// (allowing a dated suffix on either).
fn same_model(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    a == b || a.strip_prefix(&b).is_some_and(|r| r.starts_with('-')) || b.strip_prefix(&a).is_some_and(|r| r.starts_with('-'))
}

/// Selections (sorted by start) of a session starting with `c` with
/// `model` that started at most [`REQUEST_WINDOW`] before `ts` (and not
/// after it: log times are truncated to the second).
fn candidates<'a>(selections: &'a [Selection], c: char, model: &str, ts: DateTime<Utc>) -> impl Iterator<Item = &'a Selection> {
    let lo = selections.partition_point(|s| s.at < ts - REQUEST_WINDOW);
    let hi = selections.partition_point(|s| s.at <= ts + Duration::seconds(1)).max(lo);
    selections[lo..hi].iter().filter(move |s| s.session_char == Some(c) && same_model(&s.model, model))
}

/// Still running at `ts`: no end seen, or ended at most [`END_SLACK`] before.
fn running(s: &Selection, ts: DateTime<Utc>) -> bool {
    s.end.is_none_or(|e| e + END_SLACK >= ts)
}

fn first_char(session_id: &str) -> Option<char> {
    session_id.chars().next().map(|c| c.to_ascii_lowercase())
}

/// Whether a request of a session with the same first character and model
/// was running at `ts` (`selections` sorted by start).
pub fn has_running_candidate(selections: &[Selection], session_id: &str, model: &str, ts: DateTime<Utc>) -> bool {
    first_char(session_id).is_some_and(|c| candidates(selections, c, model, ts).any(|s| running(s, ts)))
}

/// How well a running request's logged end fits an entry stamped `ts`
/// (higher is better): an end at or after `ts`, closest first (the
/// transcript time is typically 1-2 s before the logged end); then an end
/// shortly before `ts`, closest first; then an unknown end.
fn end_fit(s: &Selection, ts: DateTime<Utc>) -> (u8, Reverse<i64>) {
    match s.end {
        // Logged ends are truncated to the second.
        Some(e) if e + Duration::seconds(1) >= ts => (2, Reverse((e + Duration::seconds(1) - ts).num_milliseconds())),
        Some(e) => (1, Reverse((ts - e).num_milliseconds())),
        None => (0, Reverse(0)),
    }
}

/// The selection that most likely served an entry of `session_id` with
/// `model`, stamped `ts` (the response time), among the [`candidates`].
/// Requests still running at `ts` are preferred; among those (a) the auth
/// this session was attributed to most recently (`affinity`: CLIProxyAPI
/// keeps a session on one account), then (b) the best fitting logged end
/// (see [`end_fit`]), then (c) the latest start (the later log line on
/// ties). With none running, the affine auth, then the latest start.
/// `selections` must be sorted by start.
pub fn match_entry<'a>(selections: &'a [Selection], session_id: &str, model: &str, ts: DateTime<Utc>, affinity: Option<&str>) -> Option<&'a Selection> {
    let c = first_char(session_id)?;
    let cands: Vec<&Selection> = candidates(selections, c, model, ts).collect();
    let affine = |s: &Selection| affinity == Some(s.auth.as_str());
    // max_by_key keeps the last of equal keys: the later log line.
    cands
        .iter()
        .copied()
        .filter(|s| running(s, ts))
        .max_by_key(|s| (affine(s), end_fit(s, ts), s.at))
        .or_else(|| cands.iter().copied().max_by_key(|s| (affine(s), s.at)))
}

/// The UTC offset (minutes east) that best explains a log whose times are
/// not this machine's local time: `naive` holds selections read with their
/// times taken as UTC, `probes` the (session id, model, timestamp) of
/// entries of sessions recorded as proxied by a hook, and `current` whether
/// each probe matches with the offset in use. Returns the offset and its
/// matches only for a clear winner: at least 3 matches from at least 2
/// distinct sessions and half the probes, at least twice as many as any
/// other offset and as match now, and no session matching fewer entries
/// than it does now.
pub fn calibrate(naive: &[Selection], probes: &[(&str, &str, DateTime<Utc>)], current: &[bool]) -> Option<(i32, usize)> {
    let matches_at = |o: i32| -> Vec<bool> {
        // Local = UTC + o, so an entry at `ts` is logged at `ts + o`.
        let shift = Duration::minutes(i64::from(o));
        probes.iter().map(|(sid, model, ts)| has_running_candidate(naive, sid, model, *ts + shift)).collect()
    };
    let mut counts = Vec::new();
    let mut o = -CALIBRATION_RANGE_MINUTES;
    while o <= CALIBRATION_RANGE_MINUTES {
        counts.push((o, matches_at(o).into_iter().filter(|&m| m).count()));
        o += CALIBRATION_STEP_MINUTES;
    }
    let &(best_o, best) = counts.iter().max_by_key(|&&(o, n)| (n, Reverse(o.abs()), o))?;
    let second = counts.iter().filter(|&&(o, _)| o != best_o).map(|&(_, n)| n).max().unwrap_or(0);
    let now_matched = current.iter().filter(|&&m| m).count();
    if !(best >= 3 && 2 * best >= probes.len() && best >= 2 * second && best >= 2 * now_matched) {
        return None;
    }
    // Per session: entries matched now and with the winner.
    let mut per_session: HashMap<&str, (usize, usize)> = HashMap::new();
    for (((sid, _, _), &now), new) in probes.iter().zip(current).zip(matches_at(best_o)) {
        let c = per_session.entry(sid).or_default();
        c.0 += usize::from(now);
        c.1 += usize::from(new);
    }
    let sessions = per_session.values().filter(|&&(_, new)| new > 0).count();
    let regresses = per_session.values().any(|&(now, new)| new < now);
    (sessions >= 2 && !regresses).then_some((best_o, best))
}

/// A log's clock when it is not this machine's local time.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Calibration {
    /// Minutes east of UTC the log's times are read with; None: local time.
    #[serde(default)]
    pub offset_minutes: Option<i32>,
    /// When it was last calibrated (successfully or not).
    pub checked_at: DateTime<Utc>,
    /// Entries of proxied sessions matched with `offset_minutes`.
    #[serde(default)]
    pub matched: usize,
}

/// Calibrated clocks per proxy log (`proxy-log-clock.json`), keyed by the
/// log's `main.log` path and applied to its rotated files too. Written only
/// by syncs.
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct LogClock {
    #[serde(default)]
    pub logs: BTreeMap<String, Calibration>,
    #[serde(skip)]
    dirty: bool,
}

impl LogClock {
    pub fn load(paths: &Paths) -> Self {
        read_json(&paths.proxy_log_clock()).unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> anyhow::Result<()> {
        if self.dirty { write_json_atomic(&paths.proxy_log_clock(), self, None) } else { Ok(()) }
    }

    fn key(dir: &Path) -> String {
        dir.join(MAIN_LOG).display().to_string()
    }

    /// The calibrated offset of `dir`'s log, if any.
    pub fn offset(&self, dir: &Path) -> Option<FixedOffset> {
        let m = self.logs.get(&Self::key(dir))?.offset_minutes?;
        FixedOffset::east_opt(m * 60)
    }

    /// Whether calibrating `dir`'s log may be tried now.
    pub fn may_calibrate(&self, dir: &Path, now: DateTime<Utc>) -> bool {
        self.logs.get(&Self::key(dir)).is_none_or(|c| now - c.checked_at >= CALIBRATION_RETRY)
    }

    /// Records a calibration: the winning offset, or None to keep the current one.
    pub fn record(&mut self, dir: &Path, found: Option<(i32, usize)>, now: DateTime<Utc>) {
        let entry = self.logs.entry(Self::key(dir)).or_insert(Calibration { offset_minutes: None, checked_at: now, matched: 0 });
        entry.checked_at = now;
        if let Some((o, n)) = found {
            entry.offset_minutes = Some(o);
            entry.matched = n;
        }
        self.dirty = true;
    }

    /// A warning while `dir`'s log is read with a calibrated offset.
    pub fn warning(&self, dir: &Path) -> Option<String> {
        let key = Self::key(dir);
        let c = self.logs.get(&key)?;
        let o = c.offset_minutes?;
        Some(format!(
            "times in {key} (and its rotated files) are read as {} instead of this machine's local time: calibrated {} \
             because a proxied session's entries matched no request with the local offset ({} entries matched with {})",
            fmt_offset(o),
            c.checked_at.format("%Y-%m-%d %H:%M UTC"),
            c.matched,
            fmt_offset(o)
        ))
    }
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
        && !auth.contains(['/', '\\', ':', '\0']);
    ok.then(|| dir.join(auth))
}

/// Largest auth file read: a CLIProxyAPI auth file is a few KB.
const MAX_AUTH_BYTES: u64 = 1024 * 1024;

/// An auth file's bytes; None when it is unreadable or larger than [`MAX_AUTH_BYTES`].
fn read_auth_file(path: &Path) -> Option<Vec<u8>> {
    let f = File::open(path).ok()?;
    if f.metadata().ok()?.len() > MAX_AUTH_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    f.take(MAX_AUTH_BYTES + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX_AUTH_BYTES).then_some(bytes)
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

/// The identity (non-secret fields only) of a pool auth file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CachedAuth {
    pub account_uuid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_uuid: Option<String>,
}

/// Identities of the pool auth files read so far (`proxy-auths.json`, by
/// file name), so usage stays attributable after an auth file is deleted or
/// renamed. Written only by syncs.
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct AuthCache {
    #[serde(default)]
    pub auths: BTreeMap<String, CachedAuth>,
    #[serde(skip)]
    dirty: bool,
}

impl AuthCache {
    pub fn load(paths: &Paths) -> Self {
        read_json(&paths.proxy_auths()).unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> anyhow::Result<()> {
        if self.dirty { write_json_atomic(&paths.proxy_auths(), self, Some(0o600)) } else { Ok(()) }
    }

    fn remember(&mut self, auth: &str, a: &IngestAccount) {
        let c = CachedAuth { account_uuid: a.account_uuid.clone(), email: a.email.clone(), organization_uuid: a.org_uuid.clone() };
        if self.auths.get(auth) != Some(&c) {
            self.auths.insert(auth.to_string(), c);
            self.dirty = true;
        }
    }

    pub fn get(&self, auth: &str) -> Option<IngestAccount> {
        self.auths.get(auth).map(|c| IngestAccount {
            account_uuid: c.account_uuid.clone(),
            org_uuid: c.organization_uuid.clone(),
            email: c.email.clone(),
            org_type: None,
            rate_limit_tier: None,
        })
    }
}

/// Resolves pool auth files to accounts within one sync run: each file is
/// parsed once, its identity remembered in the [`AuthCache`], and the cache
/// used when the file is gone.
#[derive(Default)]
pub struct PoolResolver {
    auths: HashMap<(PathBuf, String), Option<IngestAccount>>,
    pub cache: AuthCache,
}

impl PoolResolver {
    pub fn with_cache(cache: AuthCache) -> Self {
        Self { auths: HashMap::new(), cache }
    }

    /// The account behind an auth file in `dir`, and whether it came from
    /// the cache (the file is gone or unreadable).
    pub fn resolve(&mut self, dir: &Path, auth: &str) -> Option<(IngestAccount, bool)> {
        let read = self
            .auths
            .entry((dir.to_path_buf(), auth.to_string()))
            .or_insert_with(|| auth_path(dir, auth).and_then(|p| read_auth_file(&p)).and_then(|b| parse_auth(&b)))
            .clone();
        match read {
            Some(a) => {
                self.cache.remember(auth, &a);
                Some((a, false))
            }
            None => self.cache.get(auth).map(|a| (a, true)),
        }
    }

    /// The account behind an auth file in `dir` (parsed once per run).
    pub fn account(&mut self, dir: &Path, auth: &str) -> Option<IngestAccount> {
        self.resolve(dir, auth).map(|(a, _)| a)
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
        let m = |ts: &str, model: &str| match_entry(&sels, "8dfd5956-x", model, utc(ts), None).map(|s| s.auth.as_str());
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
        assert_eq!(match_entry(&sels, "", "claude-opus-5-5", utc("2026-10-15T10:02:10Z"), None), None);
    }

    #[test]
    fn matching_prefers_requests_still_running() {
        // A long request (a.json) overlaps a short later one (b.json) that already ended.
        let sels = vec![
            s("2026-10-15T10:00:00Z", Some("2026-10-15T10:05:00Z"), '8', "a.json", "m"),
            s("2026-10-15T10:01:00Z", Some("2026-10-15T10:01:10Z"), '8', "b.json", "m"),
        ];
        let m = |ts: &str| match_entry(&sels, "8", "m", utc(ts), None).map(|s| s.auth.as_str());
        assert_eq!(m("2026-10-15T10:01:05Z"), Some("b.json"), "both running: the end closest after the entry");
        assert_eq!(m("2026-10-15T10:04:00Z"), Some("a.json"), "b ended long before");
        assert_eq!(m("2026-10-15T10:01:14Z"), Some("a.json"), "an end not before the entry beats one within the end slack");
        // Nothing running: the latest earlier request is the best guess.
        assert_eq!(m("2026-10-15T10:06:00Z"), Some("b.json"));
        // Unknown end (its end line not read): still a candidate.
        let open = vec![s("2026-10-15T10:00:00Z", None, '8', "c.json", "m")];
        assert_eq!(match_entry(&open, "8", "m", utc("2026-10-15T10:03:00Z"), None).map(|s| s.auth.as_str()), Some("c.json"));
        let only_slack = vec![s("2026-10-15T10:01:00Z", Some("2026-10-15T10:01:10Z"), '8', "b.json", "m")];
        assert_eq!(match_entry(&only_slack, "8", "m", utc("2026-10-15T10:01:14Z"), None).map(|s| s.auth.as_str()), Some("b.json"), "within the end slack");
    }

    /// QA repro: two sessions sharing a first character, on different pool
    /// accounts, with overlapping requests. Latest start credited both to pool-b.
    #[test]
    fn matching_prefers_the_end_closest_after_the_entry() {
        let sels = vec![
            s("2026-10-15T10:30:00Z", Some("2026-10-15T10:30:30Z"), 'c', "pool-a.json", "m"),
            s("2026-10-15T10:30:20Z", Some("2026-10-15T10:30:40Z"), 'c', "pool-b.json", "m"),
        ];
        let m = |sid: &str, ts: &str| match_entry(&sels, sid, "m", utc(ts), None).map(|s| s.auth.as_str());
        assert_eq!(m("cccc0005-x", "2026-10-15T10:30:29.500Z"), Some("pool-a.json"));
        assert_eq!(m("cccc0006-x", "2026-10-15T10:30:39.500Z"), Some("pool-b.json"));
        // A logged end truncated to the second still counts as not before the entry.
        assert_eq!(m("cccc0005-x", "2026-10-15T10:30:30.400Z"), Some("pool-a.json"));
        // Equal fit: the latest start.
        let tie = vec![
            s("2026-10-15T10:30:00Z", Some("2026-10-15T10:30:30Z"), 'c', "early.json", "m"),
            s("2026-10-15T10:30:10Z", Some("2026-10-15T10:30:30Z"), 'c', "late.json", "m"),
        ];
        assert_eq!(match_entry(&tie, "c", "m", utc("2026-10-15T10:30:29Z"), None).map(|s| s.auth.as_str()), Some("late.json"));
    }

    #[test]
    fn matching_prefers_the_sessions_affinity() {
        let sels = vec![
            s("2026-10-15T10:30:00Z", Some("2026-10-15T10:30:30Z"), 'c', "pool-a.json", "m"),
            s("2026-10-15T10:30:20Z", Some("2026-10-15T10:30:40Z"), 'c', "pool-b.json", "m"),
        ];
        let m = |aff: Option<&str>, ts: &str| match_entry(&sels, "c", "m", utc(ts), aff).map(|s| s.auth.as_str());
        // Session affinity beats a better-fitting end of another account.
        assert_eq!(m(Some("pool-b.json"), "2026-10-15T10:30:29.500Z"), Some("pool-b.json"));
        assert_eq!(m(Some("pool-a.json"), "2026-10-15T10:30:29.500Z"), Some("pool-a.json"));
        // An affine request that is not running does not beat a running one.
        assert_eq!(m(Some("pool-a.json"), "2026-10-15T10:30:39.500Z"), Some("pool-b.json"));
        // Nothing running: the affine account, else the latest start.
        assert_eq!(m(Some("pool-a.json"), "2026-10-15T10:31:00Z"), Some("pool-a.json"));
        assert_eq!(m(None, "2026-10-15T10:31:00Z"), Some("pool-b.json"));
        assert_eq!(m(Some("other.json"), "2026-10-15T10:31:00Z"), Some("pool-b.json"));
    }

    #[test]
    fn running_candidates_for_inference() {
        let sels = vec![
            s("2026-10-15T10:00:00Z", Some("2026-10-15T10:00:30Z"), '9', "a.json", "claude-opus-5-5"),
            s("2026-10-15T10:05:00Z", None, '9', "a.json", "claude-opus-5-5"),
        ];
        let has = |sid: &str, model: &str, ts: &str| has_running_candidate(&sels, sid, model, utc(ts));
        assert!(has("9c01", "claude-opus-5-5", "2026-10-15T10:00:29Z"));
        assert!(has("9c01", "claude-opus-5-5-20261001", "2026-10-15T10:00:34Z"), "end slack, dated model");
        assert!(!has("9c01", "claude-opus-5-5", "2026-10-15T10:01:00Z"), "ended before");
        assert!(!has("1abc", "claude-opus-5-5", "2026-10-15T10:00:29Z"), "other first character");
        assert!(!has("9c01", "claude-haiku-5-5", "2026-10-15T10:00:29Z"), "other model");
        assert!(has("9c01", "claude-opus-5-5", "2026-10-15T10:06:00Z"), "unknown end: still running");
        assert!(!has("", "claude-opus-5-5", "2026-10-15T10:00:29Z"));
        assert_eq!(infer(2, 2), None, "too few entries");
        assert_eq!(infer(3, 3), Some(true));
        assert_eq!(infer(4, 5), Some(true), "80%");
        assert_eq!(infer(3, 4), Some(false), "75%");
        assert_eq!(infer(0, 1200), Some(false));
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
        for bad in ["../x.json", "a/b.json", ".hidden.json", "config.yaml", "x\\y.json", "C:x.json", "c:..\\x.json"] {
            assert_eq!(auth_path(Path::new("/p"), bad), None, "{bad}");
        }
    }

    #[test]
    fn oversized_auth_files_are_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let ok = r#"{"type":"claude","account_uuid":"acc-ok","email":"ok@x"}"#;
        fs::write(dir.path().join("ok.json"), ok).unwrap();
        // Valid JSON identity, padded past the 1 MiB cap.
        let big = format!(r#"{{"type":"claude","account_uuid":"acc-big","email":"big@x","pad":"{}"}}"#, "x".repeat(MAX_AUTH_BYTES as usize));
        fs::write(dir.path().join("big.json"), big).unwrap();
        assert_eq!(pool_accounts(dir.path()), vec!["ok@x".to_string()]);
        let mut r = PoolResolver::default();
        assert_eq!(r.account(dir.path(), "ok.json").unwrap().account_uuid, "acc-ok");
        assert_eq!(r.account(dir.path(), "big.json"), None);
    }

    #[test]
    fn stray_temp_files_in_the_markers_dir_are_not_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let now = utc("2026-10-15T10:00:00Z");
        with_log(&paths);
        record_session(&paths, "sess-a", None, Some("http://127.0.0.1:8317"), HookPoint::SessionStart, ProxyMode::Auto, now);
        let markers = paths.proxy_sessions_dir();
        // Left by an interrupted atomic write.
        let tmp = markers.join("sess-b.tmp.4242");
        fs::copy(markers.join("sess-a.json"), &tmp).unwrap();
        let out = status_report(&paths, None, None, None, now);
        assert!(out.contains("recent sessions (1 recorded") && !out.contains("sess-b"), "{out}");
        // Pruning only removes session records.
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        for p in [&tmp, &markers.join("sess-a.json")] {
            File::options().write(true).open(p).unwrap().set_modified(hour_ago).unwrap();
        }
        prune_markers(&paths, std::time::Duration::from_secs(60));
        assert!(tmp.exists() && !markers.join("sess-a.json").exists());
    }

    #[test]
    fn status_labels_inferred_sessions_by_what_a_sync_credits() {
        use crate::sessions::{Inferred, SessionStates};
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let now = utc("2026-10-15T10:00:00Z");
        with_log(&paths);
        let mut st = SessionStates::default();
        for id in ["inf-only", "inf-hooked"] {
            st.touch(id, now).inferred = Some(Inferred { proxied: true, matched: 5, total: 5, decided_at: now });
        }
        st.save(&paths.sessions(), now).unwrap();
        // A later Stop hook added a record without ANTHROPIC_BASE_URL: it wins.
        record_session(&paths, "inf-hooked", None, None, HookPoint::Later, ProxyMode::Auto, now);
        let out = status_report(&paths, None, None, None, now);
        assert!(out.contains("inf-hooked [Claude account (hook record overrides the log inference)]"), "{out}");
        assert!(out.contains("  inf-hooked [superseded by a hook record] decided"), "{out}");
        assert!(out.contains("  inf-only [pool] decided"), "{out}");
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
        // The port alone is not enough for a public host.
        assert!(!url_is_proxy("https://proxy.example", Some(443)));
        assert!(!url_is_proxy("http://localhost.evil.com:8317", Some(8317)));
        assert!(!url_is_proxy("http://8.8.8.8:8317", Some(8317)));
        assert!(!url_is_proxy("http://[2001:db8::1]:8317", Some(8317)));
        for ok in ["http://10.1.2.3:8317", "http://172.16.0.9:8317", "http://169.254.1.1:8317", "http://[fd00::1]:8317",
            "http://[fe80::1]:8317", "http://0.0.0.0:8317", "http://[::ffff:192.168.0.2]:8317"] {
            assert!(url_is_proxy(ok, Some(8317)), "{ok}");
        }
        assert!(!url_is_proxy("http://172.32.0.9:8317", Some(8317)), "outside 172.16/12");
        // This machine's own name, with or without .local.
        assert!(url_is_proxy_on("http://my-mac.local:8317", Some(8317), "My-Mac"));
        assert!(url_is_proxy_on("http://my-mac:8317", Some(8317), "my-mac.local"));
        assert!(!url_is_proxy_on("http://my-mac.evil.com:8317", Some(8317), "my-mac"));
        assert!(!url_is_proxy_on("http://other:8317", Some(8317), ""));
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
        assert!(out.contains("this shell: ANTHROPIC_BASE_URL=http://127.0.0.1:8317 from this shell's environment points at a CLIProxyAPI (log found)"), "{out}");
        // Hook records keep naming Claude Code's environment.
        assert!(out.contains("sess-1 [pool (mode on)] 2026-10-15 10:00 UTC: ANTHROPIC_BASE_URL=http://127.0.0.1:8317 from Claude Code's environment"), "{out}");
        assert!(out.contains("pool accounts (1):\n  pool-a@example.org"), "{out}");
        assert!(out.contains("sess-1 [pool (mode on)] 2026-10-15 10:00 UTC: "), "{out}");
        assert!(!out.contains("SECRET") && !out.contains("codex@"), "{out}");

        let out = status_report(&paths, Some("off"), None, None, now);
        assert!(out.contains("proxy mode: off (from CCTRACK_PROXY_MODE)") && out.contains("sess-1 [Claude account (mode off)]"), "{out}");
        let out = status_report(&paths, Some("maybe"), None, None, now);
        assert!(out.contains("proxy mode: on (saved)") && out.contains("ignoring CCTRACK_PROXY_MODE"), "{out}");
    }

    #[test]
    fn empty_base_url_in_a_higher_priority_file_means_not_proxied() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let cwd = dir.path().join("proj");
        let now = utc("2026-10-15T10:00:00Z");
        with_log(&paths);
        settings(&paths.claude_dir.join("settings.json"), "http://127.0.0.1:8317");
        assert!(detect(&paths, Some(&cwd), None, now).proxy_url);
        // The project overrides the user's proxy URL with "": Claude Code uses the default API.
        settings(&cwd.join(".claude/settings.json"), "");
        assert_eq!(find_base_url(&paths, Some(&cwd), None), Some((UrlSource::ProjectSettings, String::new())));
        let d = detect(&paths, Some(&cwd), None, now);
        assert!(!d.proxy_url && d.source == Some(UrlSource::ProjectSettings), "{d:?}");
        assert!(d.describe().contains("set to \"\" in project .claude/settings.json"), "{}", d.describe());
        // Explicitly not proxied: no inference from the log either.
        let v = session_verdict(&paths, ProxyMode::Auto, "s9", Some(cwd.to_str().unwrap()), now);
        assert!(matches!(&v, Verdict::NotProxied { why, .. } if why.contains("set to \"\"")), "{v:?}");
        // No URL anywhere and no hook record, with a log: decided from the log.
        fs::remove_file(cwd.join(".claude/settings.json")).unwrap();
        fs::remove_file(paths.claude_dir.join("settings.json")).unwrap();
        assert_eq!(session_verdict(&paths, ProxyMode::Auto, "s9", Some(cwd.to_str().unwrap()), now), Verdict::Infer { dir: paths.cliproxy_dir.clone() });
        // A hook record saying "not proxied" is not second-guessed.
        record_session(&paths, "s9", None, None, HookPoint::SessionStart, ProxyMode::Auto, now);
        assert!(matches!(session_verdict(&paths, ProxyMode::Auto, "s9", None, now), Verdict::NotProxied { .. }));
        // ...except in mode on, which forces log attribution.
        assert!(matches!(session_verdict(&paths, ProxyMode::On, "s9", None, now), Verdict::Proxied(_)));
    }

    #[test]
    fn status_session_lines_follow_whether_a_log_exists_now() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let now = utc("2026-10-15T10:00:00Z");
        with_log(&paths);
        record_session(&paths, "sess-p", None, Some("http://127.0.0.1:8317"), HookPoint::SessionStart, ProxyMode::Auto, now);
        record_session(&paths, "sess-n", None, None, HookPoint::SessionStart, ProxyMode::Auto, now);
        fs::remove_file(paths.cliproxy_dir.join(MAIN_LOG)).unwrap();
        for mode in [None, Some("on")] {
            let out = status_report(&paths, mode, None, None, now);
            for l in out.lines().filter(|l| l.contains("sess-")) {
                assert!(!l.contains("[pool"), "{l}");
            }
            assert!(out.contains("sess-p [Claude account"), "{out}");
            assert!(out.contains("but there is no log at"), "{out}");
        }
        // Recorded without a log, and the log appeared later.
        with_log(&paths);
        let out = status_report(&paths, None, None, None, now);
        let line = out.lines().find(|l| l.contains("sess-p")).unwrap();
        assert!(line.contains("[pool]") && line.contains("(log found)") && line.contains("UTC"), "{line}");
        let out = status_report(&paths, Some("on"), None, None, now);
        assert!(out.contains("sess-n [pool (mode on)]"), "{out}");
        assert!(out.contains("even sessions a hook saw as not proxied"), "{out}");
    }

    #[test]
    fn status_shows_inferred_and_suspicious_sessions_and_a_calibrated_clock() {
        use crate::sessions::{Inferred, LogEvidence, SessionStates};
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let now = utc("2026-10-15T10:00:00Z");
        with_log(&paths);
        let mut st = SessionStates::default();
        st.touch("9c013753", now).inferred = Some(Inferred { proxied: true, matched: 1200, total: 1218, decided_at: now });
        let s = st.touch("d215d4b4", now);
        s.not_proxied = Some("ANTHROPIC_BASE_URL is not set".into());
        s.log_evidence = Some(LogEvidence { matched: 9, total: 10 });
        let s = st.touch("plain", now);
        s.not_proxied = Some("ANTHROPIC_BASE_URL is not set".into());
        s.log_evidence = Some(LogEvidence { matched: 0, total: 10 });
        st.save(&paths.sessions(), now).unwrap();
        let mut clock = LogClock::default();
        clock.record(&paths.cliproxy_dir, Some((180, 12)), now);
        clock.save(&paths).unwrap();
        let out = status_report(&paths, None, None, None, now);
        assert!(out.contains("sessions inferred from the proxy log (1):\n  9c013753 [pool] decided 2026-10-15 10:00 UTC: 1200 of 1218"), "{out}");
        assert!(out.contains("look proxied but were credited to the Claude account (1):\n  d215d4b4: 9 of 10"), "{out}");
        assert!(!out.contains("  plain:"), "{out}");
        assert!(out.contains("read as UTC+03:00 instead of this machine's local time"), "{out}");
    }

    #[test]
    fn requests_spanning_a_log_rotation_get_their_end() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("logs")).unwrap();
        let base = utc("2026-10-12T12:44:40Z");
        fs::write(dir.path().join("logs/main-2026-10-12T12-44-49.634.log"),
            sel(&local(base), "rot1", "cache hit", "claude:8...", "a.json", "m") + "\n").unwrap();
        fs::write(dir.path().join("logs/main.log"), format!(
            "[{}] [rot1] [info ] [gin_logger.go:108] 200 |  12s | 127.0.0.1 | POST \"/v1/messages\"\n",
            local(base + Duration::seconds(12)))).unwrap();
        let chars: HashSet<char> = ['8'].into();
        let log = PoolLog::load(dir.path(), base - Duration::minutes(15), base + Duration::minutes(1), &chars, &ist());
        assert_eq!(log.selections.len(), 1);
        assert_eq!(log.selections[0].end, Some(base + Duration::seconds(12)));
        // So an entry stamped long after the request is not matched as still running.
        assert!(!has_running_candidate(&log.selections, "8", "m", base + Duration::minutes(2)));
    }

    #[test]
    fn each_group_of_entries_is_sought_and_read_within_its_own_cap() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("logs")).unwrap();
        let day1 = utc("2026-10-13T10:00:00Z");
        let day3 = utc("2026-10-15T10:00:00Z");
        let mut f = File::create(dir.path().join("logs/main.log")).unwrap();
        // Two busy days of noise between the two groups of interest.
        for i in 0..40_000i64 {
            let t = day1 - Duration::hours(1) + Duration::seconds(i * 4);
            writeln!(f, "{}", sel(&local(t), &format!("n{i}"), "cache hit", "claude:3...", "noise.json", "m")).unwrap();
            if t > day1 && t < day1 + Duration::seconds(5) {
                writeln!(f, "{}", sel(&local(day1 + Duration::seconds(5)), "g1", "cache hit", "claude:8...", "one.json", "m")).unwrap();
            }
        }
        writeln!(f, "{}", sel(&local(day3), "g2", "cache hit", "claude:8...", "two.json", "m")).unwrap();
        drop(f);
        let windows = read_windows(&[day1 + Duration::seconds(10), day3 + Duration::seconds(10)]);
        assert_eq!(windows.len(), 2, "{windows:?}");
        let chars: HashSet<char> = ['8'].into();
        // A cap far too small for one window over both days still finds both.
        let log = PoolLog::load_capped(dir.path(), &windows, &chars, &ist(), 512 * 1024);
        let auths: Vec<&str> = log.selections.iter().map(|s| s.auth.as_str()).collect();
        assert_eq!(auths, ["one.json", "two.json"]);
        let one = PoolLog::load_capped(dir.path(), &[(windows[0].0, windows[1].1)], &chars, &ist(), 512 * 1024);
        assert_eq!(one.selections.iter().map(|s| s.auth.as_str()).collect::<Vec<_>>(), ["one.json"], "one window: the cap stops before day 3");
    }

    #[test]
    fn read_windows_group_by_gap_and_span() {
        let t = utc("2026-10-15T00:00:00Z");
        let at = |m: i64| t + Duration::minutes(m);
        let w = read_windows(&[at(30), at(0), at(50), at(200), at(201)]);
        assert_eq!(w, vec![(at(0) - REQUEST_WINDOW, at(50) + Duration::seconds(1)), (at(200) - REQUEST_WINDOW, at(201) + Duration::seconds(1))]);
        // Continuous activity is split every 6 hours.
        let busy: Vec<DateTime<Utc>> = (0..=24 * 6).map(|i| at(i * 10)).collect();
        assert_eq!(read_windows(&busy).len(), 4);
        assert!(read_windows(&[]).is_empty());
    }

    /// A zone with a DST change like CET/CEST: +1 h, +2 h from 2025-03-30
    /// 01:00 UTC, +1 h again from 2025-10-26 01:00 UTC.
    #[derive(Clone, Debug)]
    struct Cet;
    #[derive(Clone, Copy, Debug)]
    struct CetOffset(FixedOffset);
    impl Offset for CetOffset {
        fn fix(&self) -> FixedOffset {
            self.0
        }
    }
    impl std::fmt::Display for CetOffset {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }
    impl Cet {
        fn at_utc(t: &NaiveDateTime) -> FixedOffset {
            let summer = *t >= utc("2025-03-30T01:00:00Z").naive_utc() && *t < utc("2025-10-26T01:00:00Z").naive_utc();
            FixedOffset::east_opt(if summer { 7200 } else { 3600 }).unwrap()
        }
    }
    impl TimeZone for Cet {
        type Offset = CetOffset;
        fn from_offset(_: &CetOffset) -> Self {
            Cet
        }
        fn offset_from_local_date(&self, _: &chrono::NaiveDate) -> LocalResult<CetOffset> {
            unimplemented!()
        }
        fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> LocalResult<CetOffset> {
            let fits: Vec<FixedOffset> = [3600, 7200]
                .into_iter()
                .map(|s| FixedOffset::east_opt(s).unwrap())
                .filter(|o| Cet::at_utc(&(*local - Duration::seconds(o.local_minus_utc().into()))) == *o)
                .collect();
            match fits.as_slice() {
                [] => LocalResult::None,
                [o] => LocalResult::Single(CetOffset(*o)),
                [a, b] => LocalResult::Ambiguous(CetOffset(*b), CetOffset(*a)),
                _ => unreachable!(),
            }
        }
        fn offset_from_utc_date(&self, _: &chrono::NaiveDate) -> CetOffset {
            unimplemented!()
        }
        fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> CetOffset {
            CetOffset(Cet::at_utc(utc))
        }
    }

    #[test]
    fn dst_ambiguous_and_nonexistent_local_times_map_deterministically() {
        let line = |ts: &str| format!("[{ts}] [r] [info ] [gin_logger.go:108] 200 | 1s | 127.0.0.1 | POST \"/v1/messages\"");
        let at = |ts: &str| parse_noted(&line(ts), &Cet).map(|(l, n)| (l.at(), n));
        assert_eq!(at("2025-07-01 12:00:00"), Some((utc("2025-07-01T10:00:00Z"), DstNote::Normal)));
        // 02:30 happens twice on 2025-10-26: the earlier instant (still +2 h).
        assert_eq!(at("2025-10-26 02:30:00"), Some((utc("2025-10-26T00:30:00Z"), DstNote::Ambiguous)));
        // 02:30 never happens on 2025-03-30: the earliest reading (+2 h).
        assert_eq!(at("2025-03-30 02:30:00"), Some((utc("2025-03-30T00:30:00Z"), DstNote::Nonexistent)));
        // Counted while reading.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("logs")).unwrap();
        fs::write(dir.path().join("logs/main.log"), [
            sel("2025-10-26 02:30:00", "a", "cache hit", "claude:8...", "a.json", "m"),
            sel("2025-10-26 02:40:00", "b", "cache hit", "claude:8...", "b.json", "m"),
        ].join("\n") + "\n").unwrap();
        let chars: HashSet<char> = ['8'].into();
        let log = PoolLog::load(dir.path(), utc("2025-10-26T00:00:00Z"), utc("2025-10-26T01:00:00Z"), &chars, &Cet);
        assert_eq!((log.selections.len(), log.dst_ambiguous, log.dst_nonexistent), (2, 2, 0));
    }

    #[test]
    fn calibration_finds_the_logs_utc_offset() {
        // Requests logged in IST but read as UTC.
        let base = utc("2026-10-15T10:00:00Z");
        let naive_sel = |t: DateTime<Utc>, c: char, end: i64| {
            let n = t.with_timezone(&ist()).naive_local().and_utc();
            Selection { at: n, end: Some(n + Duration::seconds(end)), request_id: n.to_string(), session_char: Some(c), auth: "a.json".into(), model: "m".into() }
        };
        let mut naive: Vec<Selection> = (0..6).map(|i| naive_sel(base + Duration::minutes(i * 7), '9', 20)).collect();
        naive.push(naive_sel(base + Duration::hours(2), '3', 20));
        naive.sort_by_key(|s| s.at);
        // Two hook-recorded proxied sessions.
        let session = |i: i64| if i % 2 == 0 { "9c01" } else { "9d02" };
        let probes: Vec<(&str, &str, DateTime<Utc>)> =
            (0..6).map(|i| (session(i), "m", base + Duration::minutes(i * 7) + Duration::seconds(18))).collect();
        let none = |n: usize| vec![false; n];
        assert_eq!(calibrate(&naive, &probes, &none(6)), Some((330, 6)));
        // Not a clear winner: too few matches, or no better than the current offset.
        assert_eq!(calibrate(&naive, &probes[..2], &none(2)), None);
        assert_eq!(calibrate(&naive, &probes, &[true, true, true, true, false, false]), None);
        // Most probes match nothing at any offset (e.g. their log lines are gone).
        let mut sparse = probes.clone();
        sparse.extend((0..7).map(|i| ("9c01", "m", base + Duration::hours(5) + Duration::minutes(i * 3))));
        assert_eq!(calibrate(&naive, &sparse, &none(13)), None);
        assert_eq!(calibrate(&naive, &sparse[..12], &none(12)), Some((330, 6)));
        // Nothing to find.
        assert_eq!(calibrate(&naive, &[("1abc", "m", base)], &none(1)), None);
        // One session alone never decides (e.g. a per-turn sync of one session).
        let one: Vec<_> = probes.iter().map(|&(_, m, t)| ("9c01", m, t)).collect();
        assert_eq!(calibrate(&naive, &one, &none(6)), None);
        // A session that matches with the offset in use must not lose matches.
        let three = base + Duration::hours(2);
        let three_naive = three.with_timezone(&ist()).naive_local().and_utc() + Duration::seconds(10);
        let mut with_matching = probes.clone();
        with_matching.push(("3e03", "m", three_naive));
        let mut current = none(6);
        current.push(true);
        assert_eq!(calibrate(&naive, &with_matching, &current), None, "3e03 matches now, not at +05:30");
        assert_eq!(calibrate(&naive, &with_matching, &none(7)), Some((330, 6)), "the same probes without a current match");
        assert_eq!(fmt_offset(330), "UTC+05:30");
        assert_eq!(fmt_offset(-210), "UTC-03:30");
        // The clock is retried only after a day when nothing was found.
        let dir = Path::new("/p");
        let mut clock = LogClock::default();
        assert!(clock.may_calibrate(dir, base) && clock.offset(dir).is_none());
        clock.record(dir, None, base);
        assert!(!clock.may_calibrate(dir, base + Duration::hours(1)) && clock.may_calibrate(dir, base + Duration::hours(25)));
        assert!(clock.offset(dir).is_none() && clock.warning(dir).is_none());
        clock.record(dir, Some((330, 6)), base);
        assert_eq!(clock.offset(dir), FixedOffset::east_opt(330 * 60));
        assert!(clock.warning(dir).unwrap().contains("UTC+05:30"));
    }

    #[test]
    fn auth_identities_are_cached_for_deleted_files() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let home = &paths.cliproxy_dir;
        fs::create_dir_all(home).unwrap();
        fs::write(home.join("p.json"), r#"{"type":"claude","account_uuid":"acc-p","email":"p@x","organization_uuid":"org-p","access_token":"sk-ant-SECRET"}"#).unwrap();
        let mut r = PoolResolver::with_cache(AuthCache::load(&paths));
        assert_eq!(r.resolve(home, "p.json").map(|(a, c)| (a.account_uuid, c)), Some(("acc-p".into(), false)));
        r.cache.save(&paths).unwrap();
        let raw = fs::read_to_string(paths.proxy_auths()).unwrap();
        assert!(raw.contains("acc-p") && raw.contains("p@x") && raw.contains("org-p") && !raw.contains("SECRET"), "{raw}");
        // The next run: the file is gone (deleted or renamed).
        fs::remove_file(home.join("p.json")).unwrap();
        let mut r = PoolResolver::with_cache(AuthCache::load(&paths));
        let (a, cached) = r.resolve(home, "p.json").unwrap();
        assert!(cached && a.account_uuid == "acc-p" && a.email.as_deref() == Some("p@x") && a.org_uuid.as_deref() == Some("org-p"));
        assert_eq!(r.resolve(home, "never-seen.json"), None);
        assert!(!r.cache.dirty, "nothing new to save");
    }
}

# cctrack CLI + Plugin Implementation Plan (Plan 2 of 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Rust `cctrack` CLI (Google login, transcript parsing, idempotent upload, hook gate) and the Claude Code plugin that blocks work until the user is logged in and uploads usage plus Haiku-written summaries.

**Architecture:** A library crate (`cctrack`) with one module per concern plus a thin clap binary. Everything is synchronous (`reqwest::blocking`); a std `TcpListener` handles the OAuth loopback. Local state is JSON files written atomically. Hooks run `cctrack hook <event>` through a POSIX wrapper that fails closed for gate events.

**Tech Stack:** Rust 1.93 (edition 2024, MSRV 1.89 for `File::try_lock`), clap 4, serde/serde_json, chrono, reqwest (blocking, json, rustls), directories, open, sha2, base64, rand, gethostname, anyhow; dev: tempfile, mockito.

**Spec:** `docs/superpowers/specs/2026-10-13-cctrack-design.md`

**Depends on:** Plan 1 Task 1 (`api/openapi.yaml`) for the wire format. Server need not be running — tests use a mock HTTP server.

## Global Constraints

- **Never install, upgrade, or uninstall software without the user's explicit permission.** Adding a crate to `Cargo.toml` counts; only crates on the user-approved list may be added. `cargo install` is never run by the agent. Pass this rule to every subagent.
- Binary and plugin are both named `cctrack`. Plugin names must not start with `claude-`.
- Files, verbatim: credentials `~/.config/cctrack/credentials.json` (mode 0600, honours `XDG_CONFIG_HOME`, override `CCTRACK_CONFIG_DIR`); state `~/.local/state/cctrack/` (honours `XDG_STATE_HOME`, override `CCTRACK_STATE_DIR`); Claude dir `$CLAUDE_CONFIG_DIR` or `~/.claude`; Claude config `$CLAUDE_CONFIG_DIR/.claude.json` or `~/.claude.json`.
- Only token counts and metadata (session id, message id, model, timestamp, cwd, git branch) leave the machine. Never upload message content.
- Dedupe key is `message.id` (requestId is usually absent; duplicated lines carry identical usage).
- Skip lines with `model == "<synthetic>"` or `isApiErrorMessage == true`. Skip entries older than `logged_in_at`.
- Offsets advance only after the server accepts the batch containing a file's last entry. Batches ≤1500 entries (≈ <1 MB).
- `account_uuid` `"api_key"` means API-key auth (no `oauthAccount`).
- Gate events (`user-prompt-submit`, `pre-tool-use`) fail closed: missing binary, crash, or non-zero exit ⇒ wrapper exits 2.
- The gate reads only local files — never the network.
- Hooks never print anything to stdout except the documented Claude Code hook JSON.
- Library code takes `&Paths` explicitly; only `main.rs` reads env vars (edition 2024 makes `set_var` unsafe, so tests construct `Paths` directly).
- POSIX only (macOS/Linux) for the plugin wrapper.
- Commit after each task with messages ending in `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Shell chaining past the gate** — `cctrack status && rm -rf x`, `cctrack login; curl …`, `` cctrack whoami `id` `` must be denied while logged out. Test in Task 5.
2. **Account switched mid-day before a sync** — entries must be attributed to the account active at their timestamp (timeline), not the account active at sync time. Test in Task 3.
3. **Partial trailing line / truncated or rewritten transcript** — an incomplete last line is not consumed; a file shorter than its stored offset restarts at 0. Test in Task 2.
4. **Concurrent syncs** (Stop hook fires while a SessionStart sweep runs) — second sync exits quietly without uploading or corrupting offsets. Test in Task 3.
5. **Malformed or missing `~/.claude.json` / hook stdin** — never panics; gate still blocks when logged out and allows when logged in. Tests in Tasks 2 and 5.

---

## File Structure

```
apps/cli/
  Cargo.toml
  src/
    main.rs          # clap commands, wiring, exit codes          (Tasks 1,3,4,5)
    lib.rs           # module list                                 (Task 1)
    paths.rs         # Paths (all file locations)                  (Task 1)
    state.rs         # atomic JSON read/write, LastSession         (Task 1)
    credentials.rs   # Credentials, AuthState, load/save/invalid   (Task 1)
    claude_json.rs   # oauthAccount + cachedUsageUtilization       (Task 2)
    transcript.rs    # JSONL line → UsageEntry                     (Task 2)
    scan.rs          # discover files, offsets, read new lines     (Task 2)
    timeline.rs      # account-at-time history                     (Task 3)
    wire.rs          # serde types for the API                     (Task 3)
    client.rs        # blocking HTTP client                        (Task 3)
    sync.rs          # lock → scan → batch → upload → offsets      (Task 3)
    login.rs         # PKCE, loopback, paste                       (Task 4)
    hook.rs          # gate decisions + side effects               (Task 5)
    detach.rs        # spawn detached child                        (Task 5)
  tests/
    sync.rs          # end-to-end sync against mockito             (Task 3)
plugin/cctrack/
  .claude-plugin/plugin.json
  hooks/hooks.json
  scripts/run, scripts/test-run.sh
  skills/cctrack/SKILL.md
.claude-plugin/marketplace.json
```

---

### Task 1: Crate scaffold, paths, state, credentials, `status`

**Files:**
- Create: `apps/cli/Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/paths.rs`, `src/state.rs`, `src/credentials.rs`

**Interfaces:**
- Produces:
  - `paths::Paths { config_dir, state_dir, claude_dir, claude_json: PathBuf }`, `Paths::from_env() -> anyhow::Result<Paths>`, `Paths::under(root: &Path) -> Paths` (test helper: everything inside `root`), methods `credentials()`, `offsets()`, `sync_lock()`, `timeline()`, `last_session()`, `last_snapshot()`, `sync_log()`, `projects_dir()` → `PathBuf`
  - `state::read_json<T: DeserializeOwned>(&Path) -> Option<T>`, `state::write_json_atomic<T: Serialize>(&Path, &T, mode: Option<u32>) -> anyhow::Result<()>`, `state::LastSession { session_id: String, transcript_path: Option<String>, cwd: Option<String> }`
  - `credentials::UserInfo { id: i64, email: String, name: String }`, `credentials::Credentials { server, token: String, user: UserInfo, logged_in_at: DateTime<Utc>, invalid: bool }`, `credentials::AuthState { LoggedIn(Credentials), Invalid(Credentials), LoggedOut }`, `load(&Paths) -> AuthState`, `save(&Paths, &Credentials) -> Result<()>`, `delete(&Paths) -> Result<()>`, `mark_invalid(&Paths) -> Result<()>`

- [ ] **Step 1: Create the crate and add dependencies**

```bash
mkdir -p apps/cli && cd apps/cli
cargo init --name cctrack --edition 2024
cargo add clap --features derive
cargo add serde --features derive
cargo add serde_json anyhow directories open sha2 base64 rand gethostname
cargo add chrono --features serde
cargo add reqwest --no-default-features --features blocking,json,rustls-tls
cargo add --dev tempfile mockito
```

If `cargo add reqwest` rejects `rustls-tls` (feature names changed in newer reqwest), run `cargo info reqwest`, pick the rustls TLS feature it lists, and use that instead. Then set in `Cargo.toml`:

```toml
[package]
name = "cctrack"
version = "0.1.0"
edition = "2024"
rust-version = "1.89"
description = "Track Claude Code usage across a team"

[lib]
name = "cctrack"
path = "src/lib.rs"

[[bin]]
name = "cctrack"
path = "src/main.rs"
```

- [ ] **Step 2: Write failing tests (in-module)**

`apps/cli/src/credentials.rs` (tests at the bottom; implementation in Step 4):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use chrono::TimeZone;

    fn creds() -> Credentials {
        Credentials {
            server: "https://cctrack.test".into(),
            token: "cct_abc".into(),
            user: UserInfo { id: 1, email: "dev@example.com".into(), name: "Dev".into() },
            logged_in_at: Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
            invalid: false,
        }
    }

    #[test]
    fn missing_file_is_logged_out() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(load(&Paths::under(dir.path())), AuthState::LoggedOut));
    }

    #[test]
    fn save_load_roundtrip_with_0600() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        save(&paths, &creds()).unwrap();
        match load(&paths) {
            AuthState::LoggedIn(c) => assert_eq!(c, creds()),
            _ => panic!("expected LoggedIn"),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(paths.credentials()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn mark_invalid_then_delete() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        save(&paths, &creds()).unwrap();
        mark_invalid(&paths).unwrap();
        assert!(matches!(load(&paths), AuthState::Invalid(_)));
        delete(&paths).unwrap();
        delete(&paths).unwrap(); // idempotent
        assert!(matches!(load(&paths), AuthState::LoggedOut));
    }

    #[test]
    fn corrupt_file_is_logged_out() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::write(paths.credentials(), b"{not json").unwrap();
        assert!(matches!(load(&paths), AuthState::LoggedOut));
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cd apps/cli && cargo test`
Expected: FAIL to compile — missing modules/types.

- [ ] **Step 4: Implement**

`apps/cli/src/lib.rs`:
```rust
//! cctrack: track Claude Code usage across a team that shares subscriptions.

pub mod claude_json;
pub mod client;
pub mod credentials;
pub mod detach;
pub mod hook;
pub mod login;
pub mod paths;
pub mod scan;
pub mod state;
pub mod sync;
pub mod timeline;
pub mod transcript;
pub mod wire;
```

For this task only, create the not-yet-written modules as empty files so the crate compiles: `claude_json.rs`, `client.rs`, `detach.rs`, `hook.rs`, `login.rs`, `scan.rs`, `sync.rs`, `timeline.rs`, `transcript.rs`, `wire.rs` each containing a single doc comment line, e.g. `//! Parses ~/.claude.json.` Later tasks fill them in.

`apps/cli/src/paths.rs`:
```rust
//! All file locations used by cctrack.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub claude_dir: PathBuf,
    pub claude_json: PathBuf,
}

impl Paths {
    pub fn from_env() -> Result<Self> {
        let home = directories::BaseDirs::new()
            .context("cannot determine home directory")?
            .home_dir()
            .to_path_buf();
        let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let config_dir = env("CCTRACK_CONFIG_DIR").unwrap_or_else(|| {
            env("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")).join("cctrack")
        });
        let state_dir = env("CCTRACK_STATE_DIR").unwrap_or_else(|| {
            env("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state")).join("cctrack")
        });
        let (claude_dir, claude_json) = match env("CLAUDE_CONFIG_DIR") {
            Some(d) => (d.clone(), d.join(".claude.json")),
            None => (home.join(".claude"), home.join(".claude.json")),
        };
        Ok(Self { config_dir, state_dir, claude_dir, claude_json })
    }

    /// Every location inside `root`; used by tests.
    pub fn under(root: &Path) -> Self {
        Self {
            config_dir: root.join("config"),
            state_dir: root.join("state"),
            claude_dir: root.join("claude"),
            claude_json: root.join("claude.json"),
        }
    }

    pub fn credentials(&self) -> PathBuf { self.config_dir.join("credentials.json") }
    pub fn offsets(&self) -> PathBuf { self.state_dir.join("offsets.json") }
    pub fn sync_lock(&self) -> PathBuf { self.state_dir.join("sync.lock") }
    pub fn timeline(&self) -> PathBuf { self.state_dir.join("accounts.json") }
    pub fn last_session(&self) -> PathBuf { self.state_dir.join("last_session.json") }
    pub fn last_snapshot(&self) -> PathBuf { self.state_dir.join("last_snapshot.json") }
    pub fn sync_log(&self) -> PathBuf { self.state_dir.join("sync.log") }
    pub fn projects_dir(&self) -> PathBuf { self.claude_dir.join("projects") }
}
```

`apps/cli/src/state.rs`:
```rust
//! Small JSON state files, written atomically.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Writes to a temp file and renames over `path`. `mode` sets unix permissions
/// on creation (e.g. 0o600 for credentials).
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T, mode: Option<u32>) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    {
        let mut opts = OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        if let Some(m) = mode {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(m);
        }
        #[cfg(not(unix))]
        let _ = mode;
        let mut f = opts.open(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(value)?)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LastSession {
    pub session_id: String,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
}
```

`apps/cli/src/credentials.rs` (above the tests from Step 2):
```rust
//! The CLI's API token and identity.

use std::fs;
use std::io::ErrorKind;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;
use crate::state::{read_json, write_json_atomic};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct UserInfo {
    pub id: i64,
    pub email: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Credentials {
    pub server: String,
    pub token: String,
    pub user: UserInfo,
    pub logged_in_at: DateTime<Utc>,
    #[serde(default)]
    pub invalid: bool,
}

#[derive(Debug)]
pub enum AuthState {
    LoggedIn(Credentials),
    /// The server rejected the token (revoked/expired); the user must log in again.
    Invalid(Credentials),
    LoggedOut,
}

pub fn load(paths: &Paths) -> AuthState {
    match read_json::<Credentials>(&paths.credentials()) {
        Some(c) if c.invalid => AuthState::Invalid(c),
        Some(c) if !c.token.is_empty() => AuthState::LoggedIn(c),
        _ => AuthState::LoggedOut,
    }
}

pub fn save(paths: &Paths, creds: &Credentials) -> Result<()> {
    write_json_atomic(&paths.credentials(), creds, Some(0o600))
}

pub fn delete(paths: &Paths) -> Result<()> {
    match fs::remove_file(paths.credentials()) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

pub fn mark_invalid(paths: &Paths) -> Result<()> {
    if let AuthState::LoggedIn(mut c) = load(paths) {
        c.invalid = true;
        save(paths, &c)?;
    }
    Ok(())
}
```

`apps/cli/src/main.rs` (first version — `status` only; later tasks add commands):
```rust
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

use cctrack::claude_json;
use cctrack::credentials::{self, AuthState};
use cctrack::paths::Paths;

#[derive(Parser)]
#[command(name = "cctrack", version, about = "Track Claude Code usage for your team")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show local login state (no network). Exit code 0 when logged in.
    Status {
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("cctrack: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let paths = Paths::from_env()?;
    match cli.cmd {
        Cmd::Status { json } => status(&paths, json),
    }
}

fn status(paths: &Paths, json: bool) -> Result<ExitCode> {
    let auth = credentials::load(paths);
    let claude = claude_json::read(&paths.claude_json);
    let (logged_in, revoked, creds) = match &auth {
        AuthState::LoggedIn(c) => (true, false, Some(c)),
        AuthState::Invalid(c) => (false, true, Some(c)),
        AuthState::LoggedOut => (false, false, None),
    };
    if json {
        let v = serde_json::json!({
            "logged_in": logged_in,
            "revoked": revoked,
            "email": creds.map(|c| &c.user.email),
            "server": creds.map(|c| &c.server),
            "claude_account": claude.account.as_ref().map(|a| serde_json::json!({
                "account_uuid": a.account_uuid,
                "email": a.email_address,
                "org_type": a.organization_type,
                "rate_limit_tier": a.organization_rate_limit_tier,
            })),
        });
        println!("{v}");
    } else {
        match &auth {
            AuthState::LoggedIn(c) => println!("cctrack: logged in as {} ({})", c.user.email, c.server),
            AuthState::Invalid(c) => println!("cctrack: login for {} was revoked or expired — run `cctrack login`", c.user.email),
            AuthState::LoggedOut => println!("cctrack: not logged in — run `cctrack login --server <url>`"),
        }
        match &claude.account {
            Some(a) => println!(
                "Claude account: {} ({} / {})",
                a.email_address.as_deref().unwrap_or(&a.account_uuid),
                a.organization_type.as_deref().unwrap_or("?"),
                a.organization_rate_limit_tier.as_deref().unwrap_or("?"),
            ),
            None => println!("Claude account: none detected (API key or not signed in)"),
        }
    }
    Ok(if logged_in { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
```

`main.rs` uses `claude_json::read`, which Task 2 implements. For Task 1, add a temporary minimal `claude_json.rs`:
```rust
//! Parses ~/.claude.json. (Full implementation in Task 2.)

use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct OAuthAccount {
    pub account_uuid: String,
    pub organization_uuid: Option<String>,
    pub email_address: Option<String>,
    pub organization_type: Option<String>,
    pub organization_rate_limit_tier: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClaudeState {
    pub account: Option<OAuthAccount>,
}

pub fn read(_path: &Path) -> ClaudeState {
    ClaudeState::default()
}
```

- [ ] **Step 5: Run tests and smoke-test**

Run: `cd apps/cli && cargo test && CCTRACK_CONFIG_DIR=$(mktemp -d) cargo run -q -- status; echo "exit=$?"`
Expected: tests PASS; status prints `cctrack: not logged in…` and `exit=1`.

- [ ] **Step 6: Commit**

```bash
git add apps/cli
git commit -m "feat(cli): crate scaffold, paths, credentials, status command

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `~/.claude.json`, transcript parsing, file scanning

**Files:**
- Modify (replace): `apps/cli/src/claude_json.rs`
- Create content: `apps/cli/src/transcript.rs`, `apps/cli/src/scan.rs`

**Interfaces:**
- Produces:
  - `claude_json::OAuthAccount` (now also `Serialize, Deserialize`), `claude_json::UsageSnapshot { account_uuid: String, fetched_at: DateTime<Utc>, five_hour_pct: Option<f64>, five_hour_resets_at: Option<DateTime<Utc>>, seven_day_pct: Option<f64>, seven_day_resets_at: Option<DateTime<Utc>> }`, `ClaudeState { account: Option<OAuthAccount>, snapshot: Option<UsageSnapshot> }`, `read(&Path) -> ClaudeState`, `parse(&[u8]) -> ClaudeState`
  - `transcript::UsageEntry { session_id, message_id: String, request_id: Option<String>, model: String, ts: DateTime<Utc>, input, output, cache_write_5m, cache_write_1h, cache_read: i64, speed: String, is_subagent: bool, cwd: Option<String>, git_branch: Option<String> }`, `parse_line(line: &str, subagent_file: bool, session_hint: &str) -> Option<UsageEntry>`
  - `scan::TranscriptFile { path: PathBuf, subagent: bool, session_hint: String }`, `discover(projects_dir: &Path, modified_since: Option<SystemTime>) -> Vec<TranscriptFile>`, `session_files(transcript: &Path) -> Vec<TranscriptFile>`, `find_transcript(projects_dir: &Path, session_id: &str) -> Option<PathBuf>`, `scan::Offsets` with `load(&Path) -> Offsets`, `save(&self, &Path) -> Result<()>`, `get(&self, &Path) -> u64`, `set(&mut self, &Path, u64)`, `scan::NewLines { lines: Vec<String>, end: u64 }`, `read_new_lines(&Path, offset: u64) -> io::Result<NewLines>`

- [ ] **Step 1: Write failing tests**

Append to `apps/cli/src/claude_json.rs` (after replacing it in Step 3 — write the test module first in a scratch buffer, then paste below the implementation):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "oauthAccount": {
        "accountUuid": "acc-1", "emailAddress": "shared@example.com", "organizationUuid": "org-1",
        "organizationType": "claude_max", "organizationRateLimitTier": "default_claude_max_20x",
        "displayName": "ignored"
      },
      "cachedUsageUtilization": {
        "fetchedAtMs": 1791481909879, "accountUuid": "acc-1",
        "utilization": {
          "five_hour": {"utilization": 22, "resets_at": "2026-10-13T05:00:00+00:00"},
          "seven_day": {"utilization": 10.5, "resets_at": "2026-10-17T00:00:00Z"},
          "iguana_necktie": {"utilization": 80.6}
        }
      }
    }"#;

    #[test]
    fn parses_account_and_snapshot() {
        let st = parse(SAMPLE.as_bytes());
        let a = st.account.unwrap();
        assert_eq!(a.account_uuid, "acc-1");
        assert_eq!(a.organization_rate_limit_tier.as_deref(), Some("default_claude_max_20x"));
        let s = st.snapshot.unwrap();
        assert_eq!(s.account_uuid, "acc-1");
        assert_eq!(s.five_hour_pct, Some(22.0));
        assert_eq!(s.seven_day_pct, Some(10.5));
        assert_eq!(s.fetched_at.timestamp_millis(), 1791481909879);
        assert_eq!(s.five_hour_resets_at.unwrap().to_rfc3339(), "2026-10-13T05:00:00+00:00");
    }

    #[test]
    fn tolerates_missing_and_malformed() {
        assert_eq!(parse(b"not json"), ClaudeState::default());
        assert_eq!(parse(b"{}"), ClaudeState::default());
        let st = parse(br#"{"oauthAccount": {"emailAddress": "x"}, "cachedUsageUtilization": {"accountUuid": "a"}}"#);
        assert!(st.account.is_none(), "account without uuid is ignored");
        assert!(st.snapshot.is_none(), "snapshot without fetchedAtMs/percentages is ignored");
        let st = parse(br#"{"cachedUsageUtilization": {"accountUuid": "a", "fetchedAtMs": 1, "utilization": {"five_hour": {"utilization": "high"}}}}"#);
        assert!(st.snapshot.is_none(), "non-numeric utilization is ignored");
        assert_eq!(read(std::path::Path::new("/definitely/missing.json")), ClaudeState::default());
    }
}
```

`apps/cli/src/transcript.rs` tests (paste below the implementation):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    const ASSISTANT: &str = r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-10-01T10:00:00.000Z","cwd":"/home/dev/proj","gitBranch":"main","isSidechain":false,"message":{"id":"msg_1","model":"claude-opus-5-5","usage":{"input_tokens":2,"output_tokens":325,"cache_creation_input_tokens":85294,"cache_read_input_tokens":7,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":85294},"speed":"standard"}}}"#;

    #[test]
    fn parses_assistant_usage() {
        let e = parse_line(ASSISTANT, false, "hint").unwrap();
        assert_eq!(e.session_id, "s1");
        assert_eq!(e.message_id, "msg_1");
        assert_eq!(e.request_id, None);
        assert_eq!(e.model, "claude-opus-5-5");
        assert_eq!((e.input, e.output, e.cache_read), (2, 325, 7));
        assert_eq!((e.cache_write_5m, e.cache_write_1h), (0, 85294));
        assert_eq!(e.speed, "standard");
        assert_eq!(e.cwd.as_deref(), Some("/home/dev/proj"));
        assert_eq!(e.git_branch.as_deref(), Some("main"));
        assert!(!e.is_subagent);
    }

    #[test]
    fn legacy_cache_creation_goes_to_5m() {
        let line = r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-10-01T10:00:00Z","requestId":"req_9","message":{"id":"msg_2","model":"claude-sonnet-4-5","usage":{"input_tokens":1,"output_tokens":1,"cache_creation_input_tokens":500}}}"#;
        let e = parse_line(line, false, "hint").unwrap();
        assert_eq!((e.cache_write_5m, e.cache_write_1h), (500, 0));
        assert_eq!(e.request_id.as_deref(), Some("req_9"));
    }

    #[test]
    fn subagent_flag_and_session_hint() {
        let line = r#"{"type":"assistant","timestamp":"2026-10-01T10:00:00Z","isSidechain":true,"message":{"id":"msg_3","model":"claude-haiku-4-5","usage":{"input_tokens":1,"output_tokens":1}}}"#;
        let e = parse_line(line, false, "parent-session").unwrap();
        assert_eq!(e.session_id, "parent-session");
        assert!(e.is_subagent);
        let e = parse_line(ASSISTANT, true, "x").unwrap();
        assert!(e.is_subagent, "files under subagents/ are subagent usage");
    }

    #[test]
    fn skips_non_usage_lines() {
        let skip = [
            r#"{"type":"user","message":{"content":"hi"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-01T10:00:00Z","message":{"id":"m","model":"<synthetic>","usage":{"input_tokens":0}}}"#,
            r#"{"type":"assistant","isApiErrorMessage":true,"timestamp":"2026-10-01T10:00:00Z","message":{"id":"m","model":"claude-opus-5-5","usage":{"input_tokens":0}}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-01T10:00:00Z","message":{"model":"claude-opus-5-5","usage":{"input_tokens":1}}}"#,
            r#"{"type":"assistant","timestamp":"not-a-date","message":{"id":"m","model":"claude-opus-5-5","usage":{"input_tokens":1}}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-01T10:00:00Z","message":{"id":"m","model":"claude-opus-5-5"}}"#,
            "{truncated",
            "",
        ];
        for line in skip {
            assert!(parse_line(line, false, "h").is_none(), "should skip: {line}");
        }
    }

    #[test]
    fn negative_counts_clamp_to_zero() {
        let line = r#"{"type":"assistant","timestamp":"2026-10-01T10:00:00Z","message":{"id":"m","model":"x","usage":{"input_tokens":-5,"output_tokens":3}}}"#;
        let e = parse_line(line, false, "h").unwrap();
        assert_eq!((e.input, e.output), (0, 3));
    }
}
```

`apps/cli/src/scan.rs` tests (paste below the implementation):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discovers_main_and_subagent_files() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("projects/-home-dev-proj");
        fs::create_dir_all(proj.join("sess-1/subagents/nested")).unwrap();
        fs::write(proj.join("sess-1.jsonl"), "").unwrap();
        fs::write(proj.join("sess-1/subagents/agent-a.jsonl"), "").unwrap();
        fs::write(proj.join("sess-1/subagents/nested/agent-b.jsonl"), "").unwrap();
        fs::write(proj.join("notes.txt"), "").unwrap();

        let files = discover(&dir.path().join("projects"), None);
        assert_eq!(files.len(), 3);
        let main: Vec<_> = files.iter().filter(|f| !f.subagent).collect();
        assert_eq!(main.len(), 1);
        assert_eq!(main[0].session_hint, "sess-1");
        assert!(files.iter().filter(|f| f.subagent).all(|f| f.session_hint == "sess-1"));

        let session = session_files(&proj.join("sess-1.jsonl"));
        assert_eq!(session.len(), 3);
        assert_eq!(find_transcript(&dir.path().join("projects"), "sess-1"), Some(proj.join("sess-1.jsonl")));
        assert_eq!(find_transcript(&dir.path().join("projects"), "nope"), None);
        assert!(discover(&dir.path().join("missing"), None).is_empty());
    }

    #[test]
    fn modified_since_filters_old_files() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("projects/p");
        fs::create_dir_all(&proj).unwrap();
        fs::write(proj.join("s.jsonl"), "").unwrap();
        let future = SystemTime::now() + std::time::Duration::from_secs(3600);
        assert!(discover(&dir.path().join("projects"), Some(future)).is_empty());
    }

    #[test]
    fn reads_only_complete_lines_and_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("t.jsonl");
        fs::write(&f, "one\ntwo\npart").unwrap();
        let r = read_new_lines(&f, 0).unwrap();
        assert_eq!(r.lines, vec!["one", "two"]);
        assert_eq!(r.end, 8);
        let r2 = read_new_lines(&f, r.end).unwrap();
        assert!(r2.lines.is_empty());
        assert_eq!(r2.end, 8, "partial line is not consumed");
        fs::write(&f, "one\ntwo\npartial\n").unwrap();
        let r3 = read_new_lines(&f, 8).unwrap();
        assert_eq!(r3.lines, vec!["partial"]);
    }

    #[test]
    fn shrunk_file_restarts_from_zero() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("t.jsonl");
        fs::write(&f, "a\n").unwrap();
        let r = read_new_lines(&f, 999).unwrap();
        assert_eq!(r.lines, vec!["a"]);
        assert_eq!(r.end, 2);
    }

    #[test]
    fn offsets_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("state/offsets.json");
        let mut o = Offsets::load(&store);
        assert_eq!(o.get(Path::new("/x")), 0);
        o.set(Path::new("/x"), 42);
        o.save(&store).unwrap();
        assert_eq!(Offsets::load(&store).get(Path::new("/x")), 42);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd apps/cli && cargo test`
Expected: FAIL to compile (missing `parse`, `parse_line`, `discover`, …).

- [ ] **Step 3: Implement**

`apps/cli/src/claude_json.rs` (replace the Task 1 stub; keep the test module from Step 1 at the bottom):
```rust
//! Reads the signed-in Claude account and the cached plan-limit utilisation
//! from Claude Code's config file. Both are undocumented, so parsing is
//! defensive: any unexpected shape yields "unknown", never an error.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OAuthAccount {
    pub account_uuid: String,
    pub organization_uuid: Option<String>,
    pub email_address: Option<String>,
    pub organization_type: Option<String>,
    pub organization_rate_limit_tier: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageSnapshot {
    pub account_uuid: String,
    pub fetched_at: DateTime<Utc>,
    pub five_hour_pct: Option<f64>,
    pub five_hour_resets_at: Option<DateTime<Utc>>,
    pub seven_day_pct: Option<f64>,
    pub seven_day_resets_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClaudeState {
    pub account: Option<OAuthAccount>,
    pub snapshot: Option<UsageSnapshot>,
}

pub fn read(path: &Path) -> ClaudeState {
    std::fs::read(path).map(|b| parse(&b)).unwrap_or_default()
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from)
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

pub fn parse(bytes: &[u8]) -> ClaudeState {
    let Ok(v) = serde_json::from_slice::<Value>(bytes) else {
        return ClaudeState::default();
    };
    let account = v.get("oauthAccount").and_then(|oa| {
        Some(OAuthAccount {
            account_uuid: str_field(oa, "accountUuid")?,
            organization_uuid: str_field(oa, "organizationUuid"),
            email_address: str_field(oa, "emailAddress"),
            organization_type: str_field(oa, "organizationType"),
            organization_rate_limit_tier: str_field(oa, "organizationRateLimitTier"),
        })
    });
    let snapshot = v.get("cachedUsageUtilization").and_then(|cu| {
        let window = |name: &str| cu.get("utilization").and_then(|u| u.get(name));
        let pct = |name: &str| window(name).and_then(|w| w.get("utilization")).and_then(Value::as_f64);
        let reset = |name: &str| {
            window(name).and_then(|w| w.get("resets_at")).and_then(Value::as_str).and_then(parse_ts)
        };
        let fetched_ms = cu.get("fetchedAtMs").and_then(|f| f.as_i64().or_else(|| f.as_f64().map(|x| x as i64)))?;
        let snap = UsageSnapshot {
            account_uuid: str_field(cu, "accountUuid")?,
            fetched_at: DateTime::from_timestamp_millis(fetched_ms)?,
            five_hour_pct: pct("five_hour"),
            five_hour_resets_at: reset("five_hour"),
            seven_day_pct: pct("seven_day"),
            seven_day_resets_at: reset("seven_day"),
        };
        (snap.five_hour_pct.is_some() || snap.seven_day_pct.is_some()).then_some(snap)
    });
    ClaudeState { account, snapshot }
}
```

`apps/cli/src/transcript.rs`:
```rust
//! Extracts token usage from Claude Code transcript lines
//! (`~/.claude/projects/<project>/<session>.jsonl`).

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct UsageEntry {
    pub session_id: String,
    pub message_id: String,
    pub request_id: Option<String>,
    pub model: String,
    pub ts: DateTime<Utc>,
    pub input: i64,
    pub output: i64,
    pub cache_write_5m: i64,
    pub cache_write_1h: i64,
    pub cache_read: i64,
    pub speed: String,
    pub is_subagent: bool,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
}

fn count(o: &Map<String, Value>, key: &str) -> i64 {
    o.get(key).and_then(Value::as_i64).unwrap_or(0).max(0)
}

/// Returns the usage carried by an assistant line, or None for any other
/// line (user/system lines, synthetic messages, API errors, malformed JSON).
pub fn parse_line(line: &str, subagent_file: bool, session_hint: &str) -> Option<UsageEntry> {
    if !line.contains("\"assistant\"") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "assistant" {
        return None;
    }
    if v.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let msg = v.get("message")?;
    let model = msg.get("model")?.as_str()?;
    if model.is_empty() || model == "<synthetic>" {
        return None;
    }
    let usage = msg.get("usage")?.as_object()?;
    let message_id = msg.get("id")?.as_str().filter(|s| !s.is_empty())?;
    let ts = DateTime::parse_from_rfc3339(v.get("timestamp")?.as_str()?).ok()?.with_timezone(&Utc);

    let creation_total = count(usage, "cache_creation_input_tokens");
    let (mut w5, mut w1) = match usage.get("cache_creation").and_then(Value::as_object) {
        Some(cc) => (count(cc, "ephemeral_5m_input_tokens"), count(cc, "ephemeral_1h_input_tokens")),
        None => (0, 0),
    };
    if w5 + w1 == 0 && creation_total > 0 {
        w5 = creation_total;
        w1 = 0;
    }
    let field = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);

    Some(UsageEntry {
        session_id: field("sessionId").unwrap_or_else(|| session_hint.to_string()),
        message_id: message_id.to_string(),
        request_id: field("requestId"),
        model: model.to_string(),
        ts,
        input: count(usage, "input_tokens"),
        output: count(usage, "output_tokens"),
        cache_write_5m: w5,
        cache_write_1h: w1,
        cache_read: count(usage, "cache_read_input_tokens"),
        speed: usage.get("speed").and_then(Value::as_str).unwrap_or("standard").to_string(),
        is_subagent: subagent_file || v.get("isSidechain").and_then(Value::as_bool) == Some(true),
        cwd: field("cwd"),
        git_branch: field("gitBranch"),
    })
}
```

`apps/cli/src/scan.rs`:
```rust
//! Finds transcript files and reads only the bytes appended since last sync.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::state::{read_json, write_json_atomic};

#[derive(Clone, Debug, PartialEq)]
pub struct TranscriptFile {
    pub path: PathBuf,
    pub subagent: bool,
    /// Session id implied by the path, used when a line lacks `sessionId`.
    pub session_hint: String,
}

fn name_of(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn is_jsonl(p: &Path) -> bool {
    p.extension().is_some_and(|e| e == "jsonl")
}

fn fresh(p: &Path, since: Option<SystemTime>) -> bool {
    match since {
        None => true,
        Some(t) => fs::metadata(p).and_then(|m| m.modified()).map(|m| m >= t).unwrap_or(true),
    }
}

fn collect_subagents(dir: &Path, session: &str, since: Option<SystemTime>, out: &mut Vec<TranscriptFile>) {
    let Ok(items) = fs::read_dir(dir) else { return };
    for item in items.flatten() {
        let p = item.path();
        if p.is_dir() {
            collect_subagents(&p, session, since, out);
        } else if is_jsonl(&p) && fresh(&p, since) {
            out.push(TranscriptFile { path: p, subagent: true, session_hint: session.to_string() });
        }
    }
}

/// Lists `projects/*/*.jsonl` and `projects/*/<session>/subagents/**/*.jsonl`
/// modified at or after `modified_since`.
pub fn discover(projects_dir: &Path, modified_since: Option<SystemTime>) -> Vec<TranscriptFile> {
    let mut out = Vec::new();
    let Ok(projects) = fs::read_dir(projects_dir) else { return out };
    for proj in projects.flatten() {
        let Ok(items) = fs::read_dir(proj.path()) else { continue };
        for item in items.flatten() {
            let p = item.path();
            if p.is_dir() {
                let session = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                collect_subagents(&p.join("subagents"), &session, modified_since, &mut out);
            } else if is_jsonl(&p) && fresh(&p, modified_since) {
                out.push(TranscriptFile { session_hint: name_of(&p), path: p, subagent: false });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// The given main transcript plus its subagent transcripts.
pub fn session_files(transcript: &Path) -> Vec<TranscriptFile> {
    let session = name_of(transcript);
    let mut out = vec![TranscriptFile { path: transcript.to_path_buf(), subagent: false, session_hint: session.clone() }];
    collect_subagents(&transcript.with_extension("").join("subagents"), &session, None, &mut out);
    out
}

pub fn find_transcript(projects_dir: &Path, session_id: &str) -> Option<PathBuf> {
    fs::read_dir(projects_dir).ok()?.flatten().map(|p| p.path().join(format!("{session_id}.jsonl"))).find(|p| p.is_file())
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Offsets {
    files: BTreeMap<String, u64>,
}

impl Offsets {
    pub fn load(path: &Path) -> Self {
        read_json(path).unwrap_or_default()
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        write_json_atomic(path, self, None)
    }
    pub fn get(&self, file: &Path) -> u64 {
        self.files.get(&file.to_string_lossy().into_owned()).copied().unwrap_or(0)
    }
    pub fn set(&mut self, file: &Path, offset: u64) {
        self.files.insert(file.to_string_lossy().into_owned(), offset);
    }
}

#[derive(Debug, PartialEq)]
pub struct NewLines {
    pub lines: Vec<String>,
    /// Byte offset just past the last complete line read.
    pub end: u64,
}

/// Reads complete lines after `offset`. A file shorter than `offset` was
/// rewritten, so reading restarts at 0 (the server dedupes re-sent entries).
pub fn read_new_lines(path: &Path, offset: u64) -> io::Result<NewLines> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let start = if offset > len { 0 } else { offset };
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') else {
        return Ok(NewLines { lines: vec![], end: start });
    };
    let text = String::from_utf8_lossy(&buf[..=last_nl]);
    Ok(NewLines {
        lines: text.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect(),
        end: start + last_nl as u64 + 1,
    })
}
```

- [ ] **Step 4: Run tests**

Run: `cd apps/cli && cargo test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/cli
git commit -m "feat(cli): parse claude.json, transcript usage lines, incremental file scanning

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Account timeline, wire types, HTTP client, sync, `sync` command

**Files:**
- Create content: `apps/cli/src/timeline.rs`, `src/wire.rs`, `src/client.rs`, `src/sync.rs`
- Create: `apps/cli/tests/sync.rs`
- Modify: `apps/cli/src/main.rs` (add `Sync` command)

**Interfaces:**
- Consumes: Tasks 1–2.
- Produces:
  - `timeline::Observation { since: DateTime<Utc>, account: Option<OAuthAccount> }`, `timeline::Timeline` with `load(&Path)`, `save(&self, &Path) -> Result<()>`, `observe(&mut self, now, Option<OAuthAccount>) -> bool`, `account_at(&self, ts) -> Option<Option<&OAuthAccount>>`; `timeline::observe_current(&Paths, now) -> ClaudeState`
  - `wire::{Device, IngestAccount, IngestSession, IngestEntry, LimitSnapshotIn, IngestRequest, IngestResponse, Me, ExchangeRequest, ExchangeResponse}`; `wire::API_KEY_ACCOUNT = "api_key"`
  - `client::ApiError { Unauthorized(String), NotFound(String), Http(u16, String), Network(String) }`, `client::ApiClient::new(server: &str, token: Option<&str>) -> ApiClient`, methods `ingest(&IngestRequest) -> Result<IngestResponse, ApiError>`, `set_summary(&str, &str) -> Result<(), ApiError>`, `me() -> Result<Me, ApiError>`, `exchange(code, verifier, device: &str) -> Result<ExchangeResponse, ApiError>`, `revoke_own_token() -> Result<(), ApiError>`
  - `sync::SyncOptions { transcript: Option<PathBuf>, wait_for_lock: Duration }`, `sync::SyncReport { files_read, entries_sent: usize, accepted, duplicates: i64, batches: usize, skipped_locked: bool }`, `sync::run(&Paths, &Credentials, &ApiClient, &SyncOptions, now: DateTime<Utc>) -> anyhow::Result<SyncReport>`, `sync::build_request(&Device, &[UsageEntry], &Timeline, &ClaudeState, Option<&UsageSnapshot>) -> IngestRequest`, `sync::hostname() -> String`, `sync::BATCH_ENTRIES = 1500`

- [ ] **Step 1: Timeline with tests**

`apps/cli/src/timeline.rs`:
```rust
//! Which Claude account was active when. Hooks record an observation whenever
//! the signed-in account changes, so a later sync attributes each entry to the
//! account active at its timestamp rather than the account active at sync time.

use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::claude_json::{self, ClaudeState, OAuthAccount};
use crate::paths::Paths;
use crate::state::{read_json, write_json_atomic};

const MAX_OBSERVATIONS: usize = 500;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Observation {
    pub since: DateTime<Utc>,
    /// None means API-key authentication.
    pub account: Option<OAuthAccount>,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Timeline {
    pub observations: Vec<Observation>,
}

fn uuid(a: &Option<OAuthAccount>) -> Option<&str> {
    a.as_ref().map(|a| a.account_uuid.as_str())
}

impl Timeline {
    pub fn load(path: &Path) -> Self {
        read_json(path).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_json_atomic(path, self, None)
    }

    /// Records `account` if it differs from the latest observation.
    pub fn observe(&mut self, now: DateTime<Utc>, account: Option<OAuthAccount>) -> bool {
        if let Some(last) = self.observations.last() {
            if uuid(&last.account) == uuid(&account) {
                return false;
            }
        }
        self.observations.push(Observation { since: now, account });
        if self.observations.len() > MAX_OBSERVATIONS {
            let excess = self.observations.len() - MAX_OBSERVATIONS;
            self.observations.drain(..excess);
        }
        true
    }

    /// The account active at `ts`: the latest observation at or before it, or
    /// the earliest observation for older entries. None if nothing was observed.
    pub fn account_at(&self, ts: DateTime<Utc>) -> Option<Option<&OAuthAccount>> {
        let obs = self.observations.iter().rev().find(|o| o.since <= ts).or(self.observations.first())?;
        Some(obs.account.as_ref())
    }
}

/// Reads the current account from ~/.claude.json and records it.
pub fn observe_current(paths: &Paths, now: DateTime<Utc>) -> ClaudeState {
    let state = claude_json::read(&paths.claude_json);
    let mut t = Timeline::load(&paths.timeline());
    if t.observe(now, state.account.clone()) {
        let _ = t.save(&paths.timeline());
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn acct(id: &str) -> Option<OAuthAccount> {
        Some(OAuthAccount {
            account_uuid: id.into(),
            organization_uuid: None,
            email_address: None,
            organization_type: None,
            organization_rate_limit_tier: None,
        })
    }

    fn at(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 1, h, 0, 0).unwrap()
    }

    #[test]
    fn observe_dedupes_and_account_at_picks_active() {
        let mut t = Timeline::default();
        assert!(t.account_at(at(5)).is_none());
        assert!(t.observe(at(9), acct("A")));
        assert!(!t.observe(at(10), acct("A")));
        assert!(t.observe(at(12), acct("B")));
        assert!(t.observe(at(14), None));

        assert_eq!(t.account_at(at(8)).unwrap().unwrap().account_uuid, "A"); // before first: earliest
        assert_eq!(t.account_at(at(11)).unwrap().unwrap().account_uuid, "A");
        assert_eq!(t.account_at(at(12)).unwrap().unwrap().account_uuid, "B");
        assert!(t.account_at(at(15)).unwrap().is_none()); // API key period
    }

    #[test]
    fn caps_history() {
        let mut t = Timeline::default();
        for i in 0..(MAX_OBSERVATIONS + 10) {
            t.observe(at(0) + chrono::Duration::minutes(i as i64), acct(&i.to_string()));
        }
        assert_eq!(t.observations.len(), MAX_OBSERVATIONS);
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("accounts.json");
        let mut t = Timeline::default();
        t.observe(at(9), acct("A"));
        t.save(&p).unwrap();
        assert_eq!(Timeline::load(&p).observations, t.observations);
    }
}
```

- [ ] **Step 2: Wire types and client**

`apps/cli/src/wire.rs`:
```rust
//! Request/response types for the cctrack API (api/openapi.yaml).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::credentials::UserInfo;

pub const API_KEY_ACCOUNT: &str = "api_key";

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Device {
    pub hostname: String,
    pub os: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct IngestAccount {
    pub account_uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_uuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_tier: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct IngestSession {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct IngestEntry {
    pub session_id: String,
    pub account_uuid: String,
    pub message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub model: String,
    pub ts: DateTime<Utc>,
    pub input: i64,
    pub output: i64,
    pub cache_write_5m: i64,
    pub cache_write_1h: i64,
    pub cache_read: i64,
    pub speed: String,
    pub is_subagent: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LimitSnapshotIn {
    pub account_uuid: String,
    pub captured_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour_resets_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seven_day_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seven_day_resets_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct IngestRequest {
    pub device: Device,
    pub accounts: Vec<IngestAccount>,
    pub sessions: Vec<IngestSession>,
    pub entries: Vec<IngestEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_snapshot: Option<LimitSnapshotIn>,
}

#[derive(Deserialize, Debug)]
pub struct IngestResponse {
    pub accepted: i64,
    pub duplicates: i64,
}

#[derive(Deserialize, Debug)]
pub struct Me {
    pub id: i64,
    pub email: String,
    pub name: String,
    pub is_admin: bool,
}

#[derive(Serialize, Debug)]
pub struct ExchangeRequest<'a> {
    pub code: &'a str,
    pub code_verifier: &'a str,
    pub device_name: &'a str,
}

#[derive(Deserialize, Debug)]
pub struct ExchangeResponse {
    pub token: String,
    pub user: UserInfo,
}
```

`apps/cli/src/client.rs`:
```rust
//! Blocking HTTP client for the cctrack API.

use std::fmt;
use std::time::Duration;

use reqwest::Url;
use reqwest::blocking::{Client, RequestBuilder};
use serde::de::DeserializeOwned;

use crate::wire::{ExchangeRequest, ExchangeResponse, IngestRequest, IngestResponse, Me};

#[derive(Debug)]
pub enum ApiError {
    Unauthorized(String),
    NotFound(String),
    Http(u16, String),
    Network(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Unauthorized(m) => write!(f, "unauthorized: {m}"),
            ApiError::NotFound(m) => write!(f, "not found: {m}"),
            ApiError::Http(code, m) => write!(f, "server returned {code}: {m}"),
            ApiError::Network(m) => write!(f, "network error: {m}"),
        }
    }
}

impl std::error::Error for ApiError {}

pub struct ApiClient {
    base: String,
    token: Option<String>,
    http: Client,
}

impl ApiClient {
    pub fn new(server: &str, token: Option<&str>) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("cctrack/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("build HTTP client");
        Self { base: server.trim_end_matches('/').to_string(), token: token.map(String::from), http }
    }

    fn url(&self, segments: &[&str]) -> Result<Url, ApiError> {
        let mut url = Url::parse(&self.base).map_err(|e| ApiError::Network(format!("bad server URL: {e}")))?;
        url.path_segments_mut()
            .map_err(|_| ApiError::Network("bad server URL".into()))?
            .pop_if_empty()
            .push("api")
            .extend(segments);
        Ok(url)
    }

    fn authed(&self, rb: RequestBuilder) -> RequestBuilder {
        match &self.token {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    /// Sends a request; returns the parsed body, or None for 204.
    fn send<T: DeserializeOwned>(&self, rb: RequestBuilder) -> Result<Option<T>, ApiError> {
        let resp = self.authed(rb).send().map_err(|e| ApiError::Network(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }
        let text = resp.text().unwrap_or_default();
        if status.is_success() {
            return serde_json::from_str(&text).map(Some).map_err(|e| ApiError::Http(status.as_u16(), e.to_string()));
        }
        let msg = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
            .unwrap_or(text);
        Err(match status.as_u16() {
            401 => ApiError::Unauthorized(msg),
            404 => ApiError::NotFound(msg),
            code => ApiError::Http(code, msg),
        })
    }

    pub fn ingest(&self, body: &IngestRequest) -> Result<IngestResponse, ApiError> {
        let url = self.url(&["ingest"])?;
        self.send(self.http.post(url).json(body))?.ok_or_else(|| ApiError::Http(204, "empty response".into()))
    }

    pub fn set_summary(&self, session_id: &str, summary: &str) -> Result<(), ApiError> {
        let url = self.url(&["sessions", session_id, "summary"])?;
        self.send::<serde_json::Value>(self.http.post(url).json(&serde_json::json!({ "summary": summary })))?;
        Ok(())
    }

    pub fn me(&self) -> Result<Me, ApiError> {
        let url = self.url(&["me"])?;
        self.send(self.http.get(url))?.ok_or_else(|| ApiError::Http(204, "empty response".into()))
    }

    pub fn exchange(&self, code: &str, verifier: &str, device: &str) -> Result<ExchangeResponse, ApiError> {
        let url = self.url(&["auth", "cli", "exchange"])?;
        let body = ExchangeRequest { code, code_verifier: verifier, device_name: device };
        self.send(self.http.post(url).json(&body))?.ok_or_else(|| ApiError::Http(204, "empty response".into()))
    }

    pub fn revoke_own_token(&self) -> Result<(), ApiError> {
        let url = self.url(&["auth", "token"])?;
        self.send::<serde_json::Value>(self.http.delete(url))?;
        Ok(())
    }
}
```

- [ ] **Step 3: Write the failing end-to-end sync test**

`apps/cli/tests/sync.rs`:
```rust
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use serde_json::Value;

use cctrack::client::ApiClient;
use cctrack::credentials::{self, AuthState, Credentials, UserInfo};
use cctrack::paths::Paths;
use cctrack::sync::{self, SyncOptions};

const CLAUDE_JSON: &str = r#"{
  "oauthAccount": {"accountUuid": "acc-1", "emailAddress": "shared@example.com",
                   "organizationType": "claude_max", "organizationRateLimitTier": "default_claude_max_20x"},
  "cachedUsageUtilization": {"fetchedAtMs": 1791481909879, "accountUuid": "acc-1",
    "utilization": {"five_hour": {"utilization": 22, "resets_at": "2026-10-13T05:00:00Z"},
                    "seven_day": {"utilization": 10, "resets_at": "2026-10-17T00:00:00Z"}}}
}"#;

fn assistant(id: &str, ts: &str, input: i64) -> String {
    format!(
        r#"{{"type":"assistant","sessionId":"sess-1","timestamp":"{ts}","cwd":"/home/dev/proj","gitBranch":"main","message":{{"id":"{id}","model":"claude-opus-5-5","usage":{{"input_tokens":{input},"output_tokens":5}}}}}}"#
    )
}

struct Fixture {
    _dir: tempfile::TempDir,
    paths: Paths,
    transcript: std::path::PathBuf,
    creds: Credentials,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    fs::write(&paths.claude_json, CLAUDE_JSON).unwrap();
    let proj = paths.projects_dir().join("-home-dev-proj");
    fs::create_dir_all(proj.join("sess-1/subagents")).unwrap();
    let transcript = proj.join("sess-1.jsonl");
    let lines = [
        r#"{"type":"user","sessionId":"sess-1","message":{"content":"secret prompt"}}"#.to_string(),
        assistant("msg_1", "2026-10-01T10:00:00Z", 100),
        assistant("msg_1", "2026-10-01T10:00:00Z", 100), // same response, second content block
        assistant("msg_old", "2026-09-30T10:00:00Z", 999), // before login
        r#"{"type":"assistant","sessionId":"sess-1","timestamp":"2026-10-01T10:01:00Z","message":{"id":"x","model":"<synthetic>","usage":{"input_tokens":0}}}"#.to_string(),
    ];
    fs::write(&transcript, lines.join("\n") + "\n" + &assistant("msg_4", "2026-10-01T10:05:00Z", 4)).unwrap(); // msg_4 has no trailing newline yet
    fs::write(proj.join("sess-1/subagents/agent-a.jsonl"), assistant("msg_3", "2026-10-01T10:02:00Z", 3) + "\n").unwrap();

    let creds = Credentials {
        server: "http://unused".into(),
        token: "cct_test".into(),
        user: UserInfo { id: 1, email: "dev@example.com".into(), name: "Dev".into() },
        logged_in_at: Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
        invalid: false,
    };
    credentials::save(&paths, &creds).unwrap();
    Fixture { _dir: dir, paths, transcript, creds }
}

fn opts() -> SyncOptions {
    SyncOptions { transcript: None, wait_for_lock: Duration::from_secs(0) }
}

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap()
}

#[test]
fn sync_uploads_new_entries_once_and_resumes() {
    let fx = fixture();
    let mut server = mockito::Server::new();
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sink = bodies.clone();
    let mock = server
        .mock("POST", "/api/ingest")
        .match_header("authorization", "Bearer cct_test")
        .with_status(200)
        .with_body_from_request(move |req| {
            sink.lock().unwrap().push(serde_json::from_slice(req.body().unwrap()).unwrap());
            br#"{"accepted":2,"duplicates":0}"#.to_vec()
        })
        .expect(2)
        .create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));

    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 2);

    {
        let bodies = bodies.lock().unwrap();
        let body = &bodies[0];
        let ids: Vec<&str> = body["entries"].as_array().unwrap().iter().map(|e| e["message_id"].as_str().unwrap()).collect();
        assert_eq!(ids.len(), 2, "msg_1 once + msg_3; got {ids:?}");
        assert!(ids.contains(&"msg_1") && ids.contains(&"msg_3"));
        let sub = body["entries"].as_array().unwrap().iter().find(|e| e["message_id"] == "msg_3").unwrap();
        assert_eq!(sub["is_subagent"], true);
        assert_eq!(sub["session_id"], "sess-1");
        assert!(body["entries"].as_array().unwrap().iter().all(|e| e["account_uuid"] == "acc-1"));
        assert_eq!(body["accounts"][0]["account_uuid"], "acc-1");
        assert_eq!(body["accounts"][0]["rate_limit_tier"], "default_claude_max_20x");
        assert_eq!(body["sessions"][0]["session_id"], "sess-1");
        assert_eq!(body["sessions"][0]["cwd"], "/home/dev/proj");
        assert_eq!(body["limit_snapshot"]["five_hour_pct"], 22.0);
        assert!(!body.to_string().contains("secret prompt"), "no transcript content may be uploaded");
    }

    // Nothing new: no request.
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 0);

    // Completing the partial line makes msg_4 available; snapshot is not re-sent.
    let mut content = fs::read_to_string(&fx.transcript).unwrap();
    content.push('\n');
    fs::write(&fx.transcript, content).unwrap();
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 1);
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies[1]["entries"][0]["message_id"], "msg_4");
    assert!(bodies[1].get("limit_snapshot").is_none());
    mock.assert();
}

#[test]
fn unauthorized_marks_credentials_invalid_and_keeps_offsets() {
    let fx = fixture();
    let mut server = mockito::Server::new();
    let mock = server.mock("POST", "/api/ingest").with_status(401).with_body(r#"{"error":"invalid or revoked token"}"#).expect(2).create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));

    assert!(sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).is_err());
    assert!(matches!(credentials::load(&fx.paths), AuthState::Invalid(_)));
    // Offsets did not advance: the same entries are attempted again.
    let err = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap_err();
    assert!(err.to_string().contains("cctrack login"));
    mock.assert();
}

#[test]
fn concurrent_sync_skips_when_locked() {
    let fx = fixture();
    fs::create_dir_all(&fx.paths.state_dir).unwrap();
    let held = fs::OpenOptions::new().create(true).write(true).truncate(false).open(fx.paths.sync_lock()).unwrap();
    held.lock().unwrap();
    let client = ApiClient::new("http://127.0.0.1:9", Some("cct_test")); // never contacted
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert!(report.skipped_locked);
}

#[test]
fn entries_attributed_to_account_active_at_their_time() {
    use cctrack::claude_json::{ClaudeState, OAuthAccount};
    use cctrack::timeline::Timeline;
    use cctrack::transcript::parse_line;
    use cctrack::wire::Device;

    let acct = |id: &str| Some(OAuthAccount {
        account_uuid: id.into(), organization_uuid: None, email_address: None,
        organization_type: None, organization_rate_limit_tier: None,
    });
    let mut t = Timeline::default();
    t.observe(Utc.with_ymd_and_hms(2026, 10, 1, 9, 0, 0).unwrap(), acct("A"));
    t.observe(Utc.with_ymd_and_hms(2026, 10, 1, 11, 0, 0).unwrap(), acct("B"));
    let early = parse_line(&assistant("m1", "2026-10-01T10:00:00Z", 1), false, "h").unwrap();
    let late = parse_line(&assistant("m2", "2026-10-01T11:30:00Z", 1), false, "h").unwrap();
    let current = ClaudeState { account: acct("B"), snapshot: None };
    let device = Device { hostname: "h".into(), os: "macos".into() };

    let req = sync::build_request(&device, &[early, late], &t, &current, None);
    assert_eq!(req.entries[0].account_uuid, "A");
    assert_eq!(req.entries[1].account_uuid, "B");
    assert_eq!(req.accounts.len(), 2);

    // No timeline and no oauthAccount: API key.
    let req = sync::build_request(&device, &[parse_line(&assistant("m3", "2026-10-01T10:00:00Z", 1), false, "h").unwrap()],
        &Timeline::default(), &ClaudeState::default(), None);
    assert_eq!(req.entries[0].account_uuid, "api_key");
}
```

- [ ] **Step 4: Run to verify failure**

Run: `cd apps/cli && cargo test --test sync`
Expected: FAIL to compile — `sync::run`, `SyncOptions`, `build_request` missing.

- [ ] **Step 5: Implement sync**

`apps/cli/src/sync.rs`:
```rust
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
        let account = match timeline.account_at(e.ts) {
            Some(a) => a.cloned(),
            None => claude.account.clone(),
        };
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
}

pub fn run(paths: &Paths, creds: &Credentials, client: &ApiClient, opts: &SyncOptions, now: DateTime<Utc>) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    fs::create_dir_all(&paths.state_dir)?;
    let lock = OpenOptions::new().create(true).write(true).truncate(false).open(paths.sync_lock())?;
    if !acquire(&lock, opts.wait_for_lock)? {
        report.skipped_locked = true;
        return Ok(report);
    }

    let claude = claude_json::read(&paths.claude_json);
    let mut timeline = Timeline::load(&paths.timeline());
    if timeline.observe(now, claude.account.clone()) {
        timeline.save(&paths.timeline())?;
    }

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

    let mut batches = vec![Batch::default()];
    for (path, end, entries) in pending {
        for chunk in entries.chunks(BATCH_ENTRIES) {
            if batches.last().unwrap().entries.len() + chunk.len() > BATCH_ENTRIES {
                batches.push(Batch::default());
            }
            batches.last_mut().unwrap().entries.extend_from_slice(chunk);
        }
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
```

`Option::is_none_or` is stable since Rust 1.82. `File::lock`/`try_lock` and `TryLockError` are stable since 1.89.

- [ ] **Step 6: Add the `sync` command**

In `apps/cli/src/main.rs` add to `enum Cmd`:
```rust
    /// Upload new usage from Claude Code transcripts.
    Sync {
        /// Only sync this session transcript (used by hooks).
        #[arg(long)]
        transcript: Option<std::path::PathBuf>,
        /// Log errors to the state dir instead of stderr (used by hooks).
        #[arg(long)]
        quiet: bool,
    },
```
add to the `match` in `run`:
```rust
        Cmd::Sync { transcript, quiet } => sync_cmd(&paths, transcript, quiet),
```
and the function:
```rust
fn sync_cmd(paths: &Paths, transcript: Option<std::path::PathBuf>, quiet: bool) -> Result<ExitCode> {
    let AuthState::LoggedIn(creds) = credentials::load(paths) else {
        if !quiet {
            eprintln!("cctrack: not logged in — run `cctrack login`");
        }
        return Ok(ExitCode::FAILURE);
    };
    let client = cctrack::client::ApiClient::new(&creds.server, Some(&creds.token));
    let opts = cctrack::sync::SyncOptions {
        transcript,
        wait_for_lock: if quiet { std::time::Duration::ZERO } else { std::time::Duration::from_secs(30) },
    };
    match cctrack::sync::run(paths, &creds, &client, &opts, chrono::Utc::now()) {
        Ok(r) if !quiet => {
            if r.skipped_locked {
                println!("cctrack: another sync is running");
            } else {
                println!("cctrack: sent {} entries ({} new, {} already recorded)", r.entries_sent, r.accepted, r.duplicates);
            }
            Ok(ExitCode::SUCCESS)
        }
        Ok(_) => Ok(ExitCode::SUCCESS),
        Err(e) if quiet => {
            log_error(paths, &format!("{e:#}"));
            Ok(ExitCode::FAILURE)
        }
        Err(e) => Err(e),
    }
}

fn log_error(paths: &Paths, msg: &str) {
    use std::io::Write;
    let _ = std::fs::create_dir_all(&paths.state_dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(paths.sync_log()) {
        let _ = writeln!(f, "{} {msg}", chrono::Utc::now().to_rfc3339());
    }
}
```

- [ ] **Step 7: Run tests**

Run: `cd apps/cli && cargo test`
Expected: PASS (all unit tests + `tests/sync.rs`).

- [ ] **Step 8: Commit**

```bash
git add apps/cli
git commit -m "feat(cli): idempotent batched sync with per-entry account attribution

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Login (loopback + paste), logout, whoami

**Files:**
- Create content: `apps/cli/src/login.rs`
- Modify: `apps/cli/src/main.rs` (add `Login`, `Logout`, `Whoami`)

**Interfaces:**
- Consumes: `ApiClient`, `credentials`, `timeline::observe_current`, `sync::hostname`.
- Produces: `login::Pkce { verifier, challenge: String }`, `login::pkce() -> Pkce`, `login::random_state() -> String`, `login::start_url(server: &str, port: Option<u16>, state: &str, challenge: &str) -> String`, `login::wait_for_callback(&TcpListener, expected_state: &str, timeout: Duration) -> anyhow::Result<String>`, `login::login(&Paths, server: &str, paste: bool, now: DateTime<Utc>) -> anyhow::Result<Credentials>`

- [ ] **Step 1: Write failing tests**

Bottom of `apps/cli/src/login.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpStream;

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let p = pkce();
        assert_eq!(p.verifier.len(), 43);
        let expect = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(p.verifier.as_bytes()));
        assert_eq!(p.challenge, expect);
        assert_ne!(pkce().verifier, p.verifier);
        assert!(random_state().len() >= 16);
    }

    #[test]
    fn start_url_modes() {
        let u = start_url("https://cctrack.test/", Some(5555), "st", "ch");
        assert_eq!(u, "https://cctrack.test/api/auth/cli/start?port=5555&state=st&code_challenge=ch");
        let u = start_url("https://cctrack.test", None, "st", "ch");
        assert_eq!(u, "https://cctrack.test/api/auth/cli/start?mode=paste&state=st&code_challenge=ch");
    }

    fn get(addr: std::net::SocketAddr, target: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        write!(s, "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    #[test]
    fn callback_returns_code_and_ignores_other_paths() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            let favicon = get(addr, "/favicon.ico");
            let ok = get(addr, "/callback?code=abc%2D1&state=xyz");
            (favicon, ok)
        });
        let code = wait_for_callback(&listener, "xyz", Duration::from_secs(5)).unwrap();
        assert_eq!(code, "abc-1");
        let (favicon, ok) = client.join().unwrap();
        assert!(favicon.starts_with("HTTP/1.1 404"));
        assert!(ok.starts_with("HTTP/1.1 200"));
    }

    #[test]
    fn callback_rejects_state_mismatch() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = std::thread::spawn(move || get(addr, "/callback?code=abc&state=evil"));
        let err = wait_for_callback(&listener, "xyz", Duration::from_secs(5)).unwrap_err();
        assert!(err.to_string().contains("state"));
        assert!(client.join().unwrap().starts_with("HTTP/1.1 400"));
    }

    #[test]
    fn callback_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(wait_for_callback(&listener, "xyz", Duration::from_millis(300)).is_err());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd apps/cli && cargo test login`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

`apps/cli/src/login.rs` (above the tests):
```rust
//! Google sign-in via the cctrack server: PKCE + a loopback redirect, or a
//! code the user pastes when no browser can reach this machine.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use base64::Engine;
use chrono::{DateTime, Utc};
use reqwest::Url;
use sha2::{Digest, Sha256};

use crate::client::ApiClient;
use crate::credentials::{self, AuthState, Credentials};
use crate::paths::Paths;
use crate::sync::hostname;
use crate::timeline;

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn pkce() -> Pkce {
    let verifier = b64(&rand::random::<[u8; 32]>());
    let challenge = b64(&Sha256::digest(verifier.as_bytes()));
    Pkce { verifier, challenge }
}

pub fn random_state() -> String {
    b64(&rand::random::<[u8; 24]>())
}

pub fn start_url(server: &str, port: Option<u16>, state: &str, challenge: &str) -> String {
    let mut url = Url::parse(&format!("{}/api/auth/cli/start", server.trim_end_matches('/'))).expect("valid server URL");
    {
        let mut q = url.query_pairs_mut();
        match port {
            Some(p) => q.append_pair("port", &p.to_string()),
            None => q.append_pair("mode", "paste"),
        };
        q.append_pair("state", state);
        q.append_pair("code_challenge", challenge);
    }
    url.into()
}

fn respond(stream: &mut TcpStream, status: u16, message: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Not Found",
    };
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>cctrack</title>\
         <p style=\"font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto\">{message}</p>"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Serves the loopback redirect until `/callback` arrives with the right state.
pub fn wait_for_callback(listener: &TcpListener, expected_state: &str, timeout: Duration) -> Result<String> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let target = request.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/");
                let Ok(url) = Url::parse(&format!("http://127.0.0.1{target}")) else {
                    respond(&mut stream, 400, "Bad request.");
                    continue;
                };
                if url.path() != "/callback" {
                    respond(&mut stream, 404, "Not found.");
                    continue;
                }
                let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
                if query.get("state").map(String::as_str) != Some(expected_state) {
                    respond(&mut stream, 400, "Sign-in state mismatch. Run <code>cctrack login</code> again.");
                    bail!("login failed: state mismatch (possible CSRF); try again");
                }
                let Some(code) = query.get("code").cloned() else {
                    respond(&mut stream, 400, "Missing code. Run <code>cctrack login</code> again.");
                    bail!("login failed: no code in callback");
                };
                respond(&mut stream, 200, "Signed in to cctrack. You can close this tab and return to the terminal.");
                return Ok(code);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    bail!("timed out waiting for the browser sign-in");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

pub fn login(paths: &Paths, server: &str, paste: bool, now: DateTime<Utc>) -> Result<Credentials> {
    let server = server.trim_end_matches('/');
    let pk = pkce();
    let state = random_state();
    let code = if paste {
        let url = start_url(server, None, &state, &pk.challenge);
        eprintln!("Open this URL in a browser and sign in with Google:\n\n  {url}\n");
        eprint!("Paste the code shown after sign-in: ");
        io::stderr().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        line.trim().to_string()
    } else {
        let listener = TcpListener::bind("127.0.0.1:0").context("bind loopback listener")?;
        let port = listener.local_addr()?.port();
        let url = start_url(server, Some(port), &state, &pk.challenge);
        eprintln!("Opening your browser to sign in with Google…\nIf it does not open, visit:\n\n  {url}\n");
        let _ = open::that(&url);
        wait_for_callback(&listener, &state, Duration::from_secs(300))?
    };
    if code.is_empty() {
        bail!("no code entered");
    }

    let resp = ApiClient::new(server, None).exchange(&code, &pk.verifier, &hostname())?;
    // Keep the earlier start time when the same user logs in again, so usage
    // between the two logins that was never synced is not dropped.
    let logged_in_at = match credentials::load(paths) {
        AuthState::LoggedIn(c) | AuthState::Invalid(c) if c.user.id == resp.user.id && c.server == server => {
            c.logged_in_at.min(now)
        }
        _ => now,
    };
    let creds = Credentials { server: server.to_string(), token: resp.token, user: resp.user, logged_in_at, invalid: false };
    credentials::save(paths, &creds)?;
    timeline::observe_current(paths, now);
    Ok(creds)
}
```

- [ ] **Step 4: Add the commands**

In `apps/cli/src/main.rs` add to `enum Cmd`:
```rust
    /// Sign in with Google. Required before Claude Code will start work.
    Login {
        /// cctrack server URL, e.g. https://cctrack.example.com (remembered after first login).
        #[arg(long, env = "CCTRACK_SERVER")]
        server: Option<String>,
        /// Print a URL and paste the code instead of opening a browser (SSH/headless).
        #[arg(long)]
        paste: bool,
    },
    /// Revoke this device's token and delete local credentials.
    Logout,
    /// Show who the server thinks you are.
    Whoami,
```
Enable clap's env feature: `cargo add clap --features derive,env`.

Add to the `match`:
```rust
        Cmd::Login { server, paste } => {
            let server = server
                .or_else(|| match credentials::load(&paths) {
                    AuthState::LoggedIn(c) | AuthState::Invalid(c) => Some(c.server),
                    AuthState::LoggedOut => None,
                })
                .ok_or_else(|| anyhow::anyhow!("pass --server https://<your-cctrack-server> (or set CCTRACK_SERVER)"))?;
            let creds = cctrack::login::login(&paths, &server, paste, chrono::Utc::now())?;
            println!("cctrack: logged in as {} ({})", creds.user.email, creds.server);
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Logout => {
            if let AuthState::LoggedIn(c) | AuthState::Invalid(c) = credentials::load(&paths) {
                let _ = cctrack::client::ApiClient::new(&c.server, Some(&c.token)).revoke_own_token();
            }
            credentials::delete(&paths)?;
            println!("cctrack: logged out");
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Whoami => {
            let AuthState::LoggedIn(c) = credentials::load(&paths) else {
                anyhow::bail!("not logged in — run `cctrack login`");
            };
            match cctrack::client::ApiClient::new(&c.server, Some(&c.token)).me() {
                Ok(me) => {
                    println!("{} <{}>{} on {}", me.name, me.email, if me.is_admin { " (admin)" } else { "" }, c.server);
                    Ok(ExitCode::SUCCESS)
                }
                Err(cctrack::client::ApiError::Unauthorized(_)) => {
                    credentials::mark_invalid(&paths)?;
                    anyhow::bail!("your token was revoked or expired — run `cctrack login`")
                }
                Err(e) => Err(e.into()),
            }
        }
```

- [ ] **Step 5: Run tests**

Run: `cd apps/cli && cargo test && cargo run -q -- login --help`
Expected: PASS; help shows `--server` and `--paste`.

- [ ] **Step 6: Commit**

```bash
git add apps/cli
git commit -m "feat(cli): Google login via loopback or paste, logout, whoami

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Hook gate, detached sync, `upload --summary`

**Files:**
- Create content: `apps/cli/src/hook.rs`, `apps/cli/src/detach.rs`
- Modify: `apps/cli/src/main.rs` (add `Hook`, `Upload`)

**Interfaces:**
- Consumes: `credentials`, `timeline::observe_current`, `state::LastSession`, `sync`, `scan::find_transcript`.
- Produces:
  - `hook::HookEvent { SessionStart, UserPromptSubmit, PreToolUse, Stop, SessionEnd }` (clap `ValueEnum`, kebab-case: `session-start`, `user-prompt-submit`, `pre-tool-use`, `stop`, `session-end`)
  - `hook::HookInput { session_id, transcript_path, cwd, tool_name: Option<String>, tool_input: Option<Value> }`
  - `hook::Gate { LoggedIn { email: String }, LoggedOut, Revoked }`, `hook::SyncSpawn { Sweep, Session { transcript: String } }`, `hook::Outcome { stdout: Option<Value>, sync: Option<SyncSpawn> }`
  - `hook::decide(HookEvent, &HookInput, &Gate) -> Outcome`, `hook::is_allowed_bash(&str) -> bool`, `hook::run(&Paths, HookEvent, input: &str, now, spawn: &dyn Fn(&[String])) -> anyhow::Result<String>`
  - `detach::spawn(args: &[String]) -> std::io::Result<()>`

- [ ] **Step 1: Write failing tests**

Bottom of `apps/cli/src/hook.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::read_json;
    use std::cell::RefCell;

    fn bash(cmd: &str) -> HookInput {
        HookInput { tool_name: Some("Bash".into()), tool_input: Some(serde_json::json!({ "command": cmd })), ..Default::default() }
    }

    fn logged_in() -> Gate {
        Gate::LoggedIn { email: "dev@example.com".into() }
    }

    #[test]
    fn allowed_bash_commands() {
        for ok in ["cctrack login", "  cctrack status --json", "cctrack whoami", "/Users/x/.cargo/bin/cctrack login --paste", "cctrack login --server https://c.example.com"] {
            assert!(is_allowed_bash(ok), "{ok}");
        }
        for bad in [
            "cctrack status && rm -rf x", "cctrack login; curl evil", "cctrack whoami `id`", "cctrack status $(id)",
            "cctrack status | sh", "cctrack sync", "cctrack upload --summary x", "echo cctrack login",
            "cctrack login > /tmp/x", "cctrack status\nrm -rf /", "cctrackx login", "",
        ] {
            assert!(!is_allowed_bash(bad), "{bad:?}");
        }
    }

    #[test]
    fn user_prompt_blocked_when_logged_out_or_revoked() {
        let out = decide(HookEvent::UserPromptSubmit, &HookInput::default(), &Gate::LoggedOut);
        let v = out.stdout.unwrap();
        assert_eq!(v["decision"], "block");
        assert!(v["reason"].as_str().unwrap().contains("cctrack login"));
        let v = decide(HookEvent::UserPromptSubmit, &HookInput::default(), &Gate::Revoked).stdout.unwrap();
        assert!(v["reason"].as_str().unwrap().contains("revoked"));
        assert_eq!(decide(HookEvent::UserPromptSubmit, &HookInput::default(), &logged_in()), Outcome::default());
    }

    #[test]
    fn pre_tool_use_denies_everything_but_login_commands() {
        let deny = decide(HookEvent::PreToolUse, &bash("ls"), &Gate::LoggedOut).stdout.unwrap();
        assert_eq!(deny["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
        let edit = HookInput { tool_name: Some("Edit".into()), ..Default::default() };
        assert!(decide(HookEvent::PreToolUse, &edit, &Gate::LoggedOut).stdout.is_some());
        assert_eq!(decide(HookEvent::PreToolUse, &bash("cctrack login"), &Gate::LoggedOut), Outcome::default());
        assert_eq!(decide(HookEvent::PreToolUse, &bash("rm -rf x"), &logged_in()), Outcome::default());
    }

    #[test]
    fn session_start_context_and_sweep() {
        let input = HookInput { session_id: Some("sess-9".into()), ..Default::default() };
        let out = decide(HookEvent::SessionStart, &input, &logged_in());
        let ctx = out.stdout.unwrap()["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string();
        assert!(ctx.contains("sess-9") && ctx.contains("dev@example.com"));
        assert_eq!(out.sync, Some(SyncSpawn::Sweep));

        let out = decide(HookEvent::SessionStart, &input, &Gate::LoggedOut);
        let ctx = out.stdout.unwrap()["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string();
        assert!(ctx.contains("NOT LOGGED IN") && ctx.contains("Do not start"));
        assert_eq!(out.sync, None);
    }

    #[test]
    fn stop_and_session_end_spawn_session_sync() {
        let input = HookInput { session_id: Some("s".into()), transcript_path: Some("/t/s.jsonl".into()), ..Default::default() };
        for ev in [HookEvent::Stop, HookEvent::SessionEnd] {
            let out = decide(ev, &input, &logged_in());
            assert_eq!(out.stdout, None);
            assert_eq!(out.sync, Some(SyncSpawn::Session { transcript: "/t/s.jsonl".into() }));
            assert_eq!(decide(ev, &input, &Gate::LoggedOut), Outcome::default());
        }
    }

    #[test]
    fn run_records_last_session_and_handles_bad_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let spawned = RefCell::new(Vec::<Vec<String>>::new());
        let spawn = |args: &[String]| spawned.borrow_mut().push(args.to_vec());

        let out = run(&paths, HookEvent::SessionStart,
            r#"{"session_id":"sess-1","transcript_path":"/t/sess-1.jsonl","cwd":"/p"}"#, Utc::now(), &spawn).unwrap();
        assert!(out.contains("NOT LOGGED IN"));
        let last: LastSession = read_json(&paths.last_session()).unwrap();
        assert_eq!(last.session_id, "sess-1");
        assert!(spawned.borrow().is_empty(), "no sync while logged out");

        let out = run(&paths, HookEvent::UserPromptSubmit, "garbage", Utc::now(), &spawn).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["decision"], "block");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd apps/cli && cargo test hook`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

`apps/cli/src/detach.rs`:
```rust
//! Spawn `cctrack <args>` fully detached so hooks return immediately.

use std::process::{Command, Stdio};

pub fn spawn(args: &[String]) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // survive the hook's process group being torn down
    }
    cmd.spawn().map(|_| ())
}
```

`apps/cli/src/hook.rs` (above the tests):
```rust
//! Claude Code hook handlers. The gate (UserPromptSubmit, PreToolUse) only
//! reads local credentials so it is fast and works offline.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::credentials::{self, AuthState};
use crate::paths::Paths;
use crate::state::{LastSession, write_json_atomic};
use crate::timeline;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum HookEvent {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    Stop,
    SessionEnd,
}

#[derive(Deserialize, Default, Debug)]
pub struct HookInput {
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<Value>,
}

#[derive(Debug, PartialEq)]
pub enum Gate {
    LoggedIn { email: String },
    LoggedOut,
    Revoked,
}

#[derive(Debug, PartialEq)]
pub enum SyncSpawn {
    Sweep,
    Session { transcript: String },
}

#[derive(Debug, PartialEq, Default)]
pub struct Outcome {
    pub stdout: Option<Value>,
    pub sync: Option<SyncSpawn>,
}

const LOGIN_HELP: &str = "Run `cctrack login` in a terminal (or type `! cctrack login` in Claude Code)";
const ALLOWED_SUBCOMMANDS: [&str; 3] = ["login", "status", "whoami"];

fn blocked_reason(gate: &Gate) -> String {
    match gate {
        Gate::Revoked => format!("cctrack: your cctrack login was revoked or has expired. {LOGIN_HELP}, then try again."),
        _ => format!("cctrack: you are not logged in to cctrack. {LOGIN_HELP}, then try again."),
    }
}

/// True for a single, unchained `cctrack login|status|whoami …` command.
pub fn is_allowed_bash(command: &str) -> bool {
    const FORBIDDEN: [&str; 8] = [";", "&", "|", "`", "$(", ">", "<", "\n"];
    if FORBIDDEN.iter().any(|f| command.contains(f)) {
        return false;
    }
    let mut words = command.split_whitespace();
    let (Some(bin), Some(sub)) = (words.next(), words.next()) else { return false };
    (bin == "cctrack" || bin.ends_with("/cctrack")) && ALLOWED_SUBCOMMANDS.contains(&sub)
}

pub fn decide(event: HookEvent, input: &HookInput, gate: &Gate) -> Outcome {
    let logged_in = matches!(gate, Gate::LoggedIn { .. });
    match event {
        HookEvent::SessionStart => {
            let session = input.session_id.as_deref().unwrap_or("unknown");
            let context = match gate {
                Gate::LoggedIn { email } => format!(
                    "cctrack: logged in as {email}. Claude Code session id: {session}. \
                     When you finish a meaningful piece of work, follow the cctrack skill to upload a 1-2 line summary \
                     with `cctrack upload --session {session} --summary \"...\"`."
                ),
                _ => format!(
                    "cctrack: NOT LOGGED IN. Do not start any work in this session. {}. \
                     Until then every tool except `cctrack login`, `cctrack status` and `cctrack whoami` is blocked.",
                    blocked_reason(gate)
                ),
            };
            Outcome {
                stdout: Some(json!({ "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": context } })),
                sync: logged_in.then_some(SyncSpawn::Sweep),
            }
        }
        HookEvent::UserPromptSubmit if !logged_in => Outcome {
            stdout: Some(json!({ "decision": "block", "reason": blocked_reason(gate) })),
            sync: None,
        },
        HookEvent::PreToolUse if !logged_in => {
            let command = input.tool_input.as_ref().and_then(|t| t.get("command")).and_then(Value::as_str).unwrap_or("");
            if input.tool_name.as_deref() == Some("Bash") && is_allowed_bash(command) {
                return Outcome::default();
            }
            Outcome {
                stdout: Some(json!({ "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": format!("{} Stop working and ask the user to log in.", blocked_reason(gate)),
                } })),
                sync: None,
            }
        }
        HookEvent::Stop | HookEvent::SessionEnd if logged_in => Outcome {
            stdout: None,
            sync: input.transcript_path.clone().map(|transcript| SyncSpawn::Session { transcript }),
        },
        _ => Outcome::default(),
    }
}

/// Runs a hook: records local state, decides, spawns any sync, and returns
/// the JSON to print on stdout (possibly empty).
pub fn run(paths: &Paths, event: HookEvent, input: &str, now: DateTime<Utc>, spawn: &dyn Fn(&[String])) -> Result<String> {
    let input: HookInput = serde_json::from_str(input).unwrap_or_default();
    let gate = match credentials::load(paths) {
        AuthState::LoggedIn(c) => Gate::LoggedIn { email: c.user.email },
        AuthState::Invalid(_) => Gate::Revoked,
        AuthState::LoggedOut => Gate::LoggedOut,
    };
    match event {
        HookEvent::SessionStart => {
            if let Some(session_id) = &input.session_id {
                let last = LastSession {
                    session_id: session_id.clone(),
                    transcript_path: input.transcript_path.clone(),
                    cwd: input.cwd.clone(),
                };
                let _ = write_json_atomic(&paths.last_session(), &last, None);
            }
            timeline::observe_current(paths, now);
        }
        HookEvent::UserPromptSubmit | HookEvent::Stop => {
            timeline::observe_current(paths, now);
        }
        HookEvent::PreToolUse | HookEvent::SessionEnd => {}
    }
    let outcome = decide(event, &input, &gate);
    if let Some(s) = &outcome.sync {
        let args: Vec<String> = match s {
            SyncSpawn::Sweep => vec!["sync".into(), "--quiet".into()],
            SyncSpawn::Session { transcript } => vec!["sync".into(), "--quiet".into(), "--transcript".into(), transcript.clone()],
        };
        spawn(&args);
    }
    Ok(outcome.stdout.map(|v| v.to_string()).unwrap_or_default())
}
```

- [ ] **Step 4: Add `hook` and `upload` commands**

In `apps/cli/src/main.rs` add to `enum Cmd`:
```rust
    /// Handle a Claude Code hook event (reads hook JSON on stdin).
    Hook { event: cctrack::hook::HookEvent },
    /// Sync this session and attach a short summary of the work done.
    Upload {
        #[arg(long)]
        summary: String,
        /// Claude Code session id (defaults to the most recently started session).
        #[arg(long)]
        session: Option<String>,
    },
```
Add to the `match`:
```rust
        Cmd::Hook { event } => {
            let mut input = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)?;
            let spawn = |args: &[String]| {
                let _ = cctrack::detach::spawn(args);
            };
            let out = cctrack::hook::run(&paths, event, &input, chrono::Utc::now(), &spawn)?;
            if !out.is_empty() {
                println!("{out}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Upload { summary, session } => upload(&paths, &summary, session),
```
And the function:
```rust
fn upload(paths: &Paths, summary: &str, session: Option<String>) -> Result<ExitCode> {
    use cctrack::client::{ApiClient, ApiError};
    use cctrack::state::{LastSession, read_json};

    let AuthState::LoggedIn(creds) = credentials::load(paths) else {
        anyhow::bail!("not logged in — run `cctrack login`");
    };
    let summary = summary.trim();
    if summary.is_empty() || summary.chars().count() > 500 {
        anyhow::bail!("summary must be 1-500 characters");
    }
    let last: Option<LastSession> = read_json(&paths.last_session());
    let session_id = session
        .or_else(|| last.as_ref().map(|l| l.session_id.clone()))
        .ok_or_else(|| anyhow::anyhow!("no session id known; pass --session <id>"))?;
    let transcript = last
        .filter(|l| l.session_id == session_id)
        .and_then(|l| l.transcript_path)
        .map(std::path::PathBuf::from)
        .or_else(|| cctrack::scan::find_transcript(&paths.projects_dir(), &session_id));

    let client = ApiClient::new(&creds.server, Some(&creds.token));
    let opts = cctrack::sync::SyncOptions { transcript, wait_for_lock: std::time::Duration::from_secs(30) };
    cctrack::sync::run(paths, &creds, &client, &opts, chrono::Utc::now())?;
    match client.set_summary(&session_id, summary) {
        Ok(()) => {
            println!("cctrack: summary uploaded for session {session_id}");
            Ok(ExitCode::SUCCESS)
        }
        Err(ApiError::NotFound(_)) => {
            anyhow::bail!("the server has no usage recorded for session {session_id} yet (no model calls since login?)")
        }
        Err(ApiError::Unauthorized(_)) => {
            credentials::mark_invalid(paths)?;
            anyhow::bail!("your token was revoked or expired — run `cctrack login`")
        }
        Err(e) => Err(e.into()),
    }
}
```

- [ ] **Step 5: Run tests and a manual gate check**

Run:
```bash
cd apps/cli && cargo test
tmp=$(mktemp -d)
echo '{"tool_name":"Bash","tool_input":{"command":"ls"}}' | CCTRACK_CONFIG_DIR=$tmp CCTRACK_STATE_DIR=$tmp cargo run -q -- hook pre-tool-use
```
Expected: tests PASS; the command prints a JSON object with `"permissionDecision":"deny"`.

- [ ] **Step 6: Commit**

```bash
git add apps/cli
git commit -m "feat(cli): hook gate, detached syncs, upload --summary

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Claude Code plugin and marketplace

**Files:**
- Create: `plugin/cctrack/.claude-plugin/plugin.json`, `plugin/cctrack/hooks/hooks.json`, `plugin/cctrack/scripts/run`, `plugin/cctrack/scripts/test-run.sh`, `plugin/cctrack/skills/cctrack/SKILL.md`, `.claude-plugin/marketplace.json`

**Interfaces:**
- Consumes: `cctrack hook <event>` (Task 5), `cctrack upload` (Task 5).
- Produces: an installable plugin (`/plugin marketplace add <repo>` → `/plugin install cctrack@cctrack`).

- [ ] **Step 1: Write the wrapper test first**

`plugin/cctrack/scripts/test-run.sh`:
```sh
#!/bin/sh
# Tests for scripts/run: gate events must fail closed.
set -u
here=$(cd "$(dirname "$0")" && pwd)
run="$here/run"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fails=0

fake() { # fake <name> <exit-code> <stdout>
  { echo '#!/bin/sh'; printf "printf '%%s' '%s'\n" "$3"; echo "exit $2"; } > "$tmp/$1"
  chmod +x "$tmp/$1"
}

check() { # check <description> <expected-exit> <actual-exit>
  if [ "$2" -eq "$3" ]; then echo "ok   - $1"; else echo "FAIL - $1 (expected $2, got $3)"; fails=$((fails + 1)); fi
}

# Missing binary: gate events block, others pass.
for ev in user-prompt-submit pre-tool-use; do
  HOME="$tmp" PATH=/usr/bin:/bin CCTRACK_BIN="$tmp/missing" sh "$run" hook "$ev" </dev/null >/dev/null 2>&1
  check "missing binary blocks $ev" 2 $?
done
HOME="$tmp" PATH=/usr/bin:/bin CCTRACK_BIN="$tmp/missing" sh "$run" hook session-start </dev/null >/dev/null 2>&1
check "missing binary does not fail session-start" 0 $?

# Success passes stdout through.
fake ok 0 '{"decision":"block"}'
out=$(CCTRACK_BIN="$tmp/ok" sh "$run" hook user-prompt-submit </dev/null)
check "exit 0 passes" 0 $?
[ "$out" = '{"decision":"block"}' ] && echo "ok   - stdout passthrough" || { echo "FAIL - stdout passthrough: $out"; fails=$((fails + 1)); }

# Crash (e.g. panic = 101) blocks gate events, not others.
fake crash 101 ''
CCTRACK_BIN="$tmp/crash" sh "$run" hook pre-tool-use </dev/null >/dev/null 2>&1
check "crash blocks pre-tool-use" 2 $?
CCTRACK_BIN="$tmp/crash" sh "$run" hook stop </dev/null >/dev/null 2>&1
check "crash does not fail stop" 0 $?

# Explicit exit 2 stays 2.
fake two 2 ''
CCTRACK_BIN="$tmp/two" sh "$run" hook user-prompt-submit </dev/null >/dev/null 2>&1
check "exit 2 stays 2" 2 $?

[ "$fails" -eq 0 ] || { echo "$fails failure(s)"; exit 1; }
echo "all wrapper tests passed"
```

- [ ] **Step 2: Run to verify failure**

Run: `sh plugin/cctrack/scripts/test-run.sh`
Expected: FAIL (`run` does not exist).

- [ ] **Step 3: Write the wrapper**

`plugin/cctrack/scripts/run` (then `chmod +x plugin/cctrack/scripts/run plugin/cctrack/scripts/test-run.sh`):
```sh
#!/bin/sh
# Runs the cctrack CLI for a Claude Code hook. Gate events fail closed: if the
# binary is missing or exits non-zero, exit 2 so Claude Code blocks.
# Usage: run hook <event>
event="${2:-}"
gate=0
case "$event" in
  user-prompt-submit|pre-tool-use) gate=1 ;;
esac

bin="${CCTRACK_BIN:-}"
if [ -z "$bin" ]; then
  if command -v cctrack >/dev/null 2>&1; then
    bin=$(command -v cctrack)
  elif [ -x "$HOME/.cargo/bin/cctrack" ]; then
    bin="$HOME/.cargo/bin/cctrack"
  fi
fi

if [ -z "$bin" ] || [ ! -x "$bin" ]; then
  if [ "$gate" -eq 1 ]; then
    echo "cctrack is not installed. Install it (cargo install --path apps/cli from the cctrack repo), then run 'cctrack login'." >&2
    exit 2
  fi
  exit 0
fi

"$bin" "$@"
rc=$?
if [ "$rc" -ne 0 ] && [ "$gate" -eq 1 ]; then
  [ "$rc" -eq 2 ] || echo "cctrack hook failed (exit $rc); blocking until it works. Run 'cctrack status' in a terminal." >&2
  exit 2
fi
exit 0
```

- [ ] **Step 4: Run the wrapper tests**

Run: `sh plugin/cctrack/scripts/test-run.sh`
Expected: `all wrapper tests passed`.

- [ ] **Step 5: Plugin manifest, hooks, skill, marketplace**

`plugin/cctrack/.claude-plugin/plugin.json`:
```json
{
  "name": "cctrack",
  "version": "0.1.0",
  "description": "Requires cctrack login before Claude Code works, and uploads token usage and short work summaries to your team's cctrack dashboard.",
  "author": { "name": "cctrack maintainers" }
}
```

`plugin/cctrack/hooks/hooks.json`:
```json
{
  "hooks": {
    "SessionStart": [
      { "hooks": [{ "type": "command", "command": "\"${CLAUDE_PLUGIN_ROOT}/scripts/run\" hook session-start", "timeout": 10 }] }
    ],
    "UserPromptSubmit": [
      { "hooks": [{ "type": "command", "command": "\"${CLAUDE_PLUGIN_ROOT}/scripts/run\" hook user-prompt-submit", "timeout": 10 }] }
    ],
    "PreToolUse": [
      { "matcher": "*", "hooks": [{ "type": "command", "command": "\"${CLAUDE_PLUGIN_ROOT}/scripts/run\" hook pre-tool-use", "timeout": 10 }] }
    ],
    "Stop": [
      { "hooks": [{ "type": "command", "command": "\"${CLAUDE_PLUGIN_ROOT}/scripts/run\" hook stop", "timeout": 10 }] }
    ],
    "SessionEnd": [
      { "hooks": [{ "type": "command", "command": "\"${CLAUDE_PLUGIN_ROOT}/scripts/run\" hook session-end", "timeout": 5 }] }
    ]
  }
}
```

`plugin/cctrack/skills/cctrack/SKILL.md`:
```markdown
---
name: cctrack
description: Use at the start of every session, whenever a message mentions cctrack (login required, blocked prompt, denied tool), and after finishing a meaningful piece of work to upload a short usage summary with the cctrack CLI.
---

# cctrack — usage tracking

This team tracks Claude Code plan usage with the `cctrack` CLI. Hooks upload token
counts automatically; your jobs are to respect the login gate and to attach a
short summary of the work you did.

## 1. The login gate (mandatory)

If the session context, a blocked prompt, or a denied tool says cctrack is
**not logged in**, or that the login was **revoked/expired**:

- Do **not** start or continue the user's task. Do not try to work around the block.
- Tell the user: "cctrack needs you to sign in first. Run `cctrack login` in a
  terminal (or type `! cctrack login` here), then ask me again."
- You may run only `cctrack status`, `cctrack whoami`, or `cctrack login` to help.
  `cctrack login` opens a browser; on a headless machine use `cctrack login --paste`.
- If `cctrack` is not installed, tell the user to install it
  (`cargo install --path apps/cli` from the cctrack repo) and stop.

## 2. Upload a summary after meaningful work

When you have finished a meaningful unit of work (a feature, fix, investigation,
or review — not trivial Q&A), and before you end a session in which work was done:

1. Write yourself a few bullet notes of what was accomplished.
2. Dispatch a subagent with **model `haiku`** and this prompt:
   "Summarise the following work in at most 2 short lines for a team usage
   dashboard. Plain text, no code, no file contents, no secrets or credentials,
   no customer data. Notes: <your bullet notes>"
3. Run, using the session id from the cctrack SessionStart context:
   `cctrack upload --session <session-id> --summary "<the haiku summary>"`
4. If the command fails, mention it briefly to the user and carry on — never
   retry in a loop. Do not upload more than once per unit of work.

## 3. Status

- `cctrack status` — local, instant: login state and detected Claude account.
- `cctrack whoami` — checks the token with the server.
- `cctrack sync` — manual upload of any pending usage (hooks normally do this).
```

`.claude-plugin/marketplace.json`:
```json
{
  "name": "cctrack",
  "owner": { "name": "cctrack maintainers" },
  "plugins": [
    {
      "name": "cctrack",
      "source": "./plugin/cctrack",
      "description": "Requires cctrack login before Claude Code works and uploads usage plus short summaries.",
      "version": "0.1.0"
    }
  ]
}
```

- [ ] **Step 6: Validate**

Run: `python3 -I -m json.tool plugin/cctrack/hooks/hooks.json >/dev/null && python3 -I -m json.tool .claude-plugin/marketplace.json >/dev/null && sh plugin/cctrack/scripts/test-run.sh`
Expected: no JSON errors; wrapper tests pass. If the `claude` CLI is available, also run `claude plugin validate .` and fix anything it reports.

- [ ] **Step 7: Commit**

```bash
git add plugin .claude-plugin
git commit -m "feat(plugin): cctrack Claude Code plugin with fail-closed login gate and summary skill

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

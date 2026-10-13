//! Claude Code hook handlers. The gate (UserPromptSubmit, PreToolUse) only
//! reads local credentials so it is fast and works offline. SessionStart also
//! checks the token online (with a short time limit) so a revoked login blocks
//! the very first prompt.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::{ApiClient, ApiError};
use crate::credentials::{self, AuthState, Credentials};
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

const ALLOWED_SUBCOMMANDS: [&str; 3] = ["login", "status", "whoami"];

/// The cctrack binary the gate trusts: the one running this hook, plus the
/// hook process's PATH used to check what a bare `cctrack` resolves to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrustedBinary {
    /// Canonical path of the running binary; None if it cannot be determined
    /// (then every Bash command is denied while logged out).
    pub exe: Option<PathBuf>,
    pub path_env: Option<OsString>,
}

impl TrustedBinary {
    pub fn current() -> Self {
        Self {
            exe: std::env::current_exe().and_then(fs::canonicalize).ok(),
            path_env: std::env::var_os("PATH"),
        }
    }

    /// How to invoke the trusted binary in messages: its canonical path.
    fn command(&self) -> String {
        self.exe.as_ref().map_or_else(|| "cctrack".to_string(), |p| p.display().to_string())
    }

    /// Whether `word`, the first word of a Bash command, runs exactly this binary.
    fn runs(&self, word: &str) -> bool {
        let Some(exe) = &self.exe else { return false };
        // Exact text: no lexical normalisation (`//`, `/./`) of the canonical path.
        if exe.to_str() == Some(word) {
            return true;
        }
        word == "cctrack"
            && resolve_on_path("cctrack", self.path_env.as_deref()).and_then(|p| fs::canonicalize(p).ok()).as_ref() == Some(exe)
    }
}

/// What a shell would run for the bare command `name`: the first executable
/// `<dir>/<name>` in `path_env`. A relative or empty PATH entry depends on the
/// shell's working directory, so reaching one means we cannot tell: None.
pub fn resolve_on_path(name: &str, path_env: Option<&OsStr>) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_env?) {
        if !dir.is_absolute() {
            return None;
        }
        let candidate = dir.join(name);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(p: &Path) -> bool {
    let Ok(meta) = fs::metadata(p) else { return false };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// How to log in. With no stored credentials the server is unknown, so the
/// user must name it (the agent itself may never pass `--server`).
fn login_help(gate: &Gate, bin: &TrustedBinary) -> String {
    let cmd = match gate {
        Gate::LoggedOut => format!("{} login --server <your-cctrack-url>", bin.command()),
        _ => format!("{} login", bin.command()),
    };
    format!("Run `{cmd}` in a terminal (or type `! {cmd}` in Claude Code)")
}

fn login_problem(gate: &Gate) -> &'static str {
    match gate {
        Gate::Revoked => "your cctrack login was revoked or has expired",
        _ => "you are not logged in to cctrack",
    }
}

fn blocked_reason(gate: &Gate, bin: &TrustedBinary) -> String {
    format!("cctrack: {}. {}, then try again.", login_problem(gate), login_help(gate, bin))
}

/// The only Bash commands allowed while logged out.
fn allowed_commands(bin: &TrustedBinary) -> String {
    let cmd = bin.command();
    format!("`{cmd} login`, `{cmd} status` and `{cmd} whoami`")
}

/// True for a single, unchained `cctrack login|status|whoami …` command that
/// runs the trusted cctrack binary (by its canonical path, or as bare
/// `cctrack` when PATH resolves to it) and cannot pick the server.
pub fn is_allowed_bash(command: &str, bin: &TrustedBinary) -> bool {
    // Chaining, substitution, redirection, expansion, globbing, quoting and escapes.
    const FORBIDDEN: [char; 17] = [';', '&', '|', '`', '>', '<', '(', ')', '$', '~', '*', '?', '[', '{', '\\', '\'', '"'];
    // Bash only splits words on space and tab; any other whitespace (newline,
    // CR, NBSP, …) would make our word split differ from the shell's.
    if command.chars().any(|c| FORBIDDEN.contains(&c) || (c.is_whitespace() && c != ' ' && c != '\t')) {
        return false;
    }
    let mut words = command.split([' ', '\t']).filter(|w| !w.is_empty());
    let (Some(first), Some(sub)) = (words.next(), words.next()) else { return false };
    if !ALLOWED_SUBCOMMANDS.contains(&sub) || !bin.runs(first) {
        return false;
    }
    // While logged out the agent must not choose (or redirect) the server.
    !(sub == "login" && words.any(|w| w == "--server" || w.starts_with("--server=")))
}

pub fn decide(event: HookEvent, input: &HookInput, gate: &Gate, bin: &TrustedBinary) -> Outcome {
    let logged_in = matches!(gate, Gate::LoggedIn { .. });
    match event {
        HookEvent::SessionStart => {
            let session = input.session_id.as_deref().unwrap_or("unknown");
            let context = match gate {
                Gate::LoggedIn { email } => format!(
                    "cctrack: logged in as {email}. Claude Code session id: {session}. \
                     Token usage uploads automatically. When you finish a meaningful piece of work, follow the cctrack skill \
                     to attach a 1-2 line summary with `cctrack upload --session {session} --summary '...'` \
                     (single quotes; remove any ' from the summary)."
                ),
                _ => format!(
                    "cctrack: NOT LOGGED IN ({}). Do not start any work in this session. {}. \
                     Until then every tool except {} is blocked. \
                     Claude Code session id: {session}.",
                    login_problem(gate),
                    login_help(gate, bin),
                    allowed_commands(bin)
                ),
            };
            Outcome {
                stdout: Some(json!({ "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": context } })),
                sync: logged_in.then_some(SyncSpawn::Sweep),
            }
        }
        HookEvent::UserPromptSubmit if !logged_in => Outcome {
            stdout: Some(json!({ "decision": "block", "reason": blocked_reason(gate, bin) })),
            sync: None,
        },
        HookEvent::PreToolUse if !logged_in => {
            let command = input.tool_input.as_ref().and_then(|t| t.get("command")).and_then(Value::as_str).unwrap_or("");
            if input.tool_name.as_deref() == Some("Bash") && is_allowed_bash(command, bin) {
                return Outcome::default();
            }
            Outcome {
                stdout: Some(json!({ "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": format!(
                        "{} Only {} may run until then. Stop working and ask the user to log in.",
                        blocked_reason(gate, bin),
                        allowed_commands(bin)
                    ),
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

/// How long SessionStart waits for the server to confirm the token.
pub const SESSION_START_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

/// The server's verdict on stored credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenCheck {
    Valid,
    /// HTTP 401: revoked or expired.
    Rejected,
    /// No answer in time, network error, or any other failure: offline must
    /// not block work, so the local credentials stand.
    Unknown,
}

/// Checks `creds` against the server (`GET /api/me`) within `timeout`.
pub fn check_token_online(creds: &Credentials, timeout: Duration) -> TokenCheck {
    match ApiClient::with_timeout(&creds.server, Some(&creds.token), timeout).me() {
        Ok(_) => TokenCheck::Valid,
        Err(ApiError::Unauthorized(_)) => TokenCheck::Rejected,
        Err(_) => TokenCheck::Unknown,
    }
}

/// Runs a hook: records local state, decides, spawns any sync, and returns
/// the JSON to print on stdout (possibly empty). On SessionStart with stored
/// credentials, `check_token` asks the server whether the token still works;
/// the other events never touch the network.
pub fn run(
    paths: &Paths,
    event: HookEvent,
    input: &str,
    now: DateTime<Utc>,
    bin: &TrustedBinary,
    spawn: &dyn Fn(&[String]),
    check_token: &dyn Fn(&Credentials) -> TokenCheck,
) -> Result<String> {
    let input: HookInput = serde_json::from_str(input).unwrap_or_default();
    let gate = match credentials::load(paths) {
        AuthState::LoggedIn(c) if event == HookEvent::SessionStart && check_token(&c) == TokenCheck::Rejected => {
            // Record it so UserPromptSubmit blocks the first prompt. Only this
            // token is marked: a newer login saved meanwhile stays valid.
            let marked = credentials::mark_invalid(paths, &c.token);
            match credentials::load(paths) {
                AuthState::LoggedIn(n) if n.token != c.token => Gate::LoggedIn { email: n.user.email },
                AuthState::LoggedOut if marked.is_ok() => Gate::LoggedOut,
                _ => Gate::Revoked,
            }
        }
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
    let outcome = decide(event, &input, &gate, bin);
    if let Some(s) = &outcome.sync {
        let args: Vec<String> = match s {
            SyncSpawn::Sweep => vec!["sync".into(), "--quiet".into()],
            SyncSpawn::Session { transcript } => vec!["sync".into(), "--quiet".into(), "--transcript".into(), transcript.clone()],
        };
        spawn(&args);
    }
    Ok(outcome.stdout.map(|v| v.to_string()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::read_json;
    use std::path::Path;
    use std::cell::RefCell;

    fn valid(_: &Credentials) -> TokenCheck {
        TokenCheck::Valid
    }

    fn bash(cmd: &str) -> HookInput {
        HookInput { tool_name: Some("Bash".into()), tool_input: Some(serde_json::json!({ "command": cmd })), ..Default::default() }
    }

    fn logged_in() -> Gate {
        Gate::LoggedIn { email: "dev@example.com".into() }
    }

    fn write_exe(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A trusted binary at `<tmp>/install/cctrack` with PATH = `<tmp>/install`.
    fn trusted() -> (tempfile::TempDir, TrustedBinary) {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        write_exe(&root.join("install/cctrack"));
        let bin = TrustedBinary { exe: Some(root.join("install/cctrack")), path_env: Some(root.join("install").into_os_string()) };
        (dir, bin)
    }

    fn with_path(bin: &TrustedBinary, dirs: &[&Path]) -> TrustedBinary {
        TrustedBinary { exe: bin.exe.clone(), path_env: Some(std::env::join_paths(dirs).unwrap()) }
    }

    #[test]
    fn allowed_bash_commands() {
        let (_dir, bin) = trusted();
        let exe = bin.command();
        for ok in [
            "cctrack login".to_string(), "  cctrack status --json".into(), "cctrack whoami".into(), "cctrack login --paste".into(),
            "cctrack\tstatus".into(), format!("{exe} login --paste"), format!("{exe} whoami"), format!(" {exe}\tstatus"),
        ] {
            assert!(is_allowed_bash(&ok, &bin), "{ok}");
        }
        for bad in [
            "cctrack status && rm -rf x", "cctrack login; curl evil", "cctrack whoami `id`", "cctrack status $(id)",
            "cctrack status | sh", "cctrack sync", "cctrack upload --summary x", "echo cctrack login",
            "cctrack login > /tmp/x", "cctrack status\nrm -rf /", "cctrackx login", "",
            // look-alike binaries: only the trusted binary's canonical path or bare `cctrack`
            "./cctrack login", "tools/cctrack login", "/*/cctrack login", "~/x/cctrack login", "$HOME/x/cctrack login",
            "/repo/../tmp/cctrack login", "/../cctrack status", "/a/./../cctrack status", "/x/cctrack/.. login", "cctrack/cctrack login",
            "/opt/cc?rack/cctrack login", "/opt/[c]/cctrack login", "/opt/{a,b}/cctrack login",
            "/tmp/x/cctrack status", "/Users/x/.cargo/bin/cctrack login", "/opt/homebrew/bin/cctrack whoami",
            // quoting, escaping and expansion
            "cctrack login \"x\"", "cctrack login 'x'", "\\cctrack login", "cctrack status ${IFS}x", "cctrack status ~",
            "cctrack status *", "cctrack status\rrm", "cctrack\u{a0}login", "/x/cctrack\u{a0}login status",
            // the agent must not pick the server while logged out
            "cctrack login --server https://evil.example.com", "cctrack login --server=https://evil.example.com",
            "cctrack login --paste --server https://evil.example.com",
        ] {
            assert!(!is_allowed_bash(bad, &bin), "{bad:?}");
        }
        assert!(!is_allowed_bash(&format!("{exe} login --server=https://evil.example.com"), &bin));
        // Non-canonical spellings of the trusted path are not accepted.
        let parent = bin.exe.as_ref().unwrap().parent().unwrap();
        assert!(!is_allowed_bash(&format!("{}/../install/cctrack status", parent.display()), &bin));
        assert!(!is_allowed_bash(&format!("{}//cctrack status", parent.display()), &bin));
        assert!(!is_allowed_bash(&format!("{}/./cctrack status", parent.display()), &bin));
    }

    #[test]
    fn bare_cctrack_must_resolve_to_the_running_binary() {
        let (dir, bin) = trusted();
        let root = fs::canonicalize(dir.path()).unwrap();
        let install = root.join("install");
        // A different executable named cctrack earlier on PATH: bare `cctrack` would run it.
        write_exe(&root.join("hostile/cctrack"));
        assert!(!is_allowed_bash("cctrack status", &with_path(&bin, &[&root.join("hostile"), &install])));
        assert!(!is_allowed_bash(&format!("{}/hostile/cctrack status", root.display()), &bin));
        // A non-executable or directory named cctrack earlier on PATH is skipped, as the shell does.
        fs::create_dir_all(root.join("noexec")).unwrap();
        fs::write(root.join("noexec/cctrack"), "x").unwrap();
        fs::create_dir_all(root.join("dir/cctrack")).unwrap();
        assert!(is_allowed_bash("cctrack status", &with_path(&bin, &[&root.join("noexec"), &root.join("dir"), &install])));
        // A symlink on PATH to the running binary resolves to it.
        fs::create_dir_all(root.join("links")).unwrap();
        std::os::unix::fs::symlink(install.join("cctrack"), root.join("links/cctrack")).unwrap();
        assert!(is_allowed_bash("cctrack whoami", &with_path(&bin, &[&root.join("links")])));
        // ...but the symlink's own path is not the canonical path.
        assert!(!is_allowed_bash(&format!("{}/links/cctrack whoami", root.display()), &bin));
        // Relative or empty PATH entries before the match make resolution cwd-dependent.
        assert!(!is_allowed_bash("cctrack status", &with_path(&bin, &[Path::new("."), &install])));
        assert!(!is_allowed_bash("cctrack status", &with_path(&bin, &[Path::new(""), &install])));
        // Not on PATH at all, or no PATH: only the canonical path works.
        assert!(!is_allowed_bash("cctrack status", &with_path(&bin, &[&root.join("noexec")])));
        let no_path = TrustedBinary { exe: bin.exe.clone(), path_env: None };
        assert!(!is_allowed_bash("cctrack status", &no_path));
        assert!(is_allowed_bash(&format!("{} status", bin.command()), &no_path));
        // Unknown running binary: nothing is allowed.
        let unknown = TrustedBinary { exe: None, path_env: bin.path_env.clone() };
        assert!(!is_allowed_bash("cctrack status", &unknown));
    }

    /// A trusted binary with a fixed path; no file needs to exist for `decide`'s messages.
    fn fixed_bin() -> TrustedBinary {
        TrustedBinary { exe: Some(PathBuf::from("/opt/cct/bin/cctrack")), path_env: None }
    }

    #[test]
    fn user_prompt_blocked_when_logged_out_or_revoked() {
        let bin = fixed_bin();
        let out = decide(HookEvent::UserPromptSubmit, &HookInput::default(), &Gate::LoggedOut, &bin);
        let v = out.stdout.unwrap();
        assert_eq!(v["decision"], "block");
        // No stored credentials: the server is unknown, so first-login help names --server,
        // and uses the exact (canonical) binary path the gate allows.
        assert!(v["reason"].as_str().unwrap().contains("`/opt/cct/bin/cctrack login --server <your-cctrack-url>`"), "{v}");
        let v = decide(HookEvent::UserPromptSubmit, &HookInput::default(), &Gate::Revoked, &bin).stdout.unwrap();
        let reason = v["reason"].as_str().unwrap();
        assert!(reason.contains("revoked") && reason.contains("`/opt/cct/bin/cctrack login`") && !reason.contains("--server"), "{reason}");
        assert_eq!(decide(HookEvent::UserPromptSubmit, &HookInput::default(), &logged_in(), &bin), Outcome::default());
    }

    #[test]
    fn pre_tool_use_denies_everything_but_login_commands() {
        let (_dir, bin) = trusted();
        let exe = bin.command();
        let deny = decide(HookEvent::PreToolUse, &bash("ls"), &Gate::LoggedOut, &bin).stdout.unwrap();
        assert_eq!(deny["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
        let reason = deny["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains(&format!("`{exe} login --server <your-cctrack-url>`")), "{reason}");
        assert!(reason.contains(&format!("`{exe} status`")) && reason.contains(&format!("`{exe} whoami`")), "{reason}");
        let deny = decide(HookEvent::PreToolUse, &bash("ls"), &Gate::Revoked, &bin).stdout.unwrap();
        let reason = deny["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains(&format!("`{exe} login`")) && !reason.contains("--server"), "{reason}");
        let edit = HookInput { tool_name: Some("Edit".into()), ..Default::default() };
        assert!(decide(HookEvent::PreToolUse, &edit, &Gate::LoggedOut, &bin).stdout.is_some());
        assert_eq!(decide(HookEvent::PreToolUse, &bash("cctrack login"), &Gate::LoggedOut, &bin), Outcome::default());
        assert_eq!(decide(HookEvent::PreToolUse, &bash(&format!("{exe} status")), &Gate::LoggedOut, &bin), Outcome::default());
        assert!(decide(HookEvent::PreToolUse, &bash("/tmp/x/cctrack status"), &Gate::LoggedOut, &bin).stdout.is_some());
        assert_eq!(decide(HookEvent::PreToolUse, &bash("rm -rf x"), &logged_in(), &bin), Outcome::default());
    }

    #[test]
    fn session_start_context_and_sweep() {
        let bin = fixed_bin();
        let input = HookInput { session_id: Some("sess-9".into()), ..Default::default() };
        let out = decide(HookEvent::SessionStart, &input, &logged_in(), &bin);
        let ctx = out.stdout.unwrap()["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string();
        assert!(ctx.contains("sess-9") && ctx.contains("dev@example.com") && ctx.contains("automatically"));
        // Summaries go in single quotes so backticks/$ in them are never executed.
        assert!(ctx.contains("--summary '") && !ctx.contains("--summary \""), "{ctx}");
        assert_eq!(out.sync, Some(SyncSpawn::Sweep));

        let out = decide(HookEvent::SessionStart, &input, &Gate::LoggedOut, &bin);
        let ctx = out.stdout.unwrap()["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string();
        assert!(ctx.contains("NOT LOGGED IN") && ctx.contains("Do not start"));
        // The session id is needed later if the user logs in mid-session.
        assert!(ctx.contains("Claude Code session id: sess-9"), "{ctx}");
        assert!(ctx.contains("`/opt/cct/bin/cctrack login --server <your-cctrack-url>`"), "{ctx}");
        assert!(ctx.contains("`/opt/cct/bin/cctrack status`") && ctx.contains("`/opt/cct/bin/cctrack whoami`"), "{ctx}");
        assert!(!ctx.contains(".."), "{ctx}");
        assert_eq!(ctx.matches("cctrack:").count(), 1, "{ctx}");
        assert_eq!(out.sync, None);

        let out = decide(HookEvent::SessionStart, &input, &Gate::Revoked, &bin);
        let ctx = out.stdout.unwrap()["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string();
        assert!(ctx.contains("NOT LOGGED IN") && ctx.contains("revoked") && !ctx.contains(".."), "{ctx}");
        assert!(ctx.contains("Claude Code session id: sess-9") && !ctx.contains("--server"), "{ctx}");
        assert_eq!(ctx.matches("cctrack:").count(), 1, "{ctx}");
        assert_eq!(out.sync, None);

        // Unknown running binary: messages fall back to plain `cctrack`.
        let none = TrustedBinary::default();
        let out = decide(HookEvent::SessionStart, &input, &Gate::LoggedOut, &none);
        let ctx = out.stdout.unwrap()["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string();
        assert!(ctx.contains("`cctrack login --server <your-cctrack-url>`"), "{ctx}");
    }

    #[test]
    fn stop_and_session_end_spawn_session_sync() {
        let bin = fixed_bin();
        let input = HookInput { session_id: Some("s".into()), transcript_path: Some("/t/s.jsonl".into()), ..Default::default() };
        for ev in [HookEvent::Stop, HookEvent::SessionEnd] {
            let out = decide(ev, &input, &logged_in(), &bin);
            assert_eq!(out.stdout, None);
            assert_eq!(out.sync, Some(SyncSpawn::Session { transcript: "/t/s.jsonl".into() }));
            assert_eq!(decide(ev, &input, &Gate::LoggedOut, &bin), Outcome::default());
        }
    }

    #[test]
    fn run_records_last_session_and_handles_bad_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let spawned = RefCell::new(Vec::<Vec<String>>::new());
        let spawn = |args: &[String]| spawned.borrow_mut().push(args.to_vec());

        let out = run(&paths, HookEvent::SessionStart,
            r#"{"session_id":"sess-1","transcript_path":"/t/sess-1.jsonl","cwd":"/p"}"#, Utc::now(), &fixed_bin(), &spawn, &valid).unwrap();
        assert!(out.contains("NOT LOGGED IN"));
        let last: LastSession = read_json(&paths.last_session()).unwrap();
        assert_eq!(last.session_id, "sess-1");
        assert!(spawned.borrow().is_empty(), "no sync while logged out");

        let out = run(&paths, HookEvent::UserPromptSubmit, "garbage", Utc::now(), &fixed_bin(), &spawn, &valid).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["decision"], "block");
    }

    fn save_creds(paths: &Paths, invalid: bool) {
        let creds = credentials::Credentials {
            server: "https://c.example.com".into(),
            token: "tok".into(),
            user: credentials::UserInfo { id: 1, email: "dev@example.com".into(), name: "Dev".into() },
            logged_in_at: Utc::now(),
            invalid,
        };
        credentials::save(paths, &creds).unwrap();
    }

    #[test]
    fn run_spawns_syncs_when_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        save_creds(&paths, false);
        let spawned = RefCell::new(Vec::<Vec<String>>::new());
        let spawn = |args: &[String]| spawned.borrow_mut().push(args.to_vec());

        let out = run(&paths, HookEvent::SessionStart, r#"{"session_id":"sess-2","transcript_path":"/t/sess-2.jsonl"}"#, Utc::now(), &fixed_bin(), &spawn, &valid).unwrap();
        assert!(out.contains("dev@example.com") && out.contains("sess-2"));
        assert_eq!(*spawned.borrow(), vec![vec!["sync".to_string(), "--quiet".to_string()]]);

        spawned.borrow_mut().clear();
        let out = run(&paths, HookEvent::Stop, r#"{"session_id":"sess-2","transcript_path":"/t/sess-2.jsonl"}"#, Utc::now(), &fixed_bin(), &spawn, &valid).unwrap();
        assert_eq!(out, "");
        assert_eq!(*spawned.borrow(), vec![vec!["sync", "--quiet", "--transcript", "/t/sess-2.jsonl"].into_iter().map(String::from).collect::<Vec<_>>()]);

        spawned.borrow_mut().clear();
        assert_eq!(run(&paths, HookEvent::UserPromptSubmit, "garbage", Utc::now(), &fixed_bin(), &spawn, &valid).unwrap(), "");
        assert!(spawned.borrow().is_empty());
    }

    #[test]
    fn run_blocks_prompt_when_credentials_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        save_creds(&paths, true);
        let spawned = RefCell::new(Vec::<Vec<String>>::new());
        let spawn = |args: &[String]| spawned.borrow_mut().push(args.to_vec());

        let out = run(&paths, HookEvent::UserPromptSubmit, r#"{"session_id":"s"}"#, Utc::now(), &fixed_bin(), &spawn, &valid).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["decision"], "block");
        assert!(v["reason"].as_str().unwrap().contains("revoked"));
        let out = run(&paths, HookEvent::Stop, r#"{"transcript_path":"/t/s.jsonl"}"#, Utc::now(), &fixed_bin(), &spawn, &valid).unwrap();
        assert_eq!(out, "");
        assert!(spawned.borrow().is_empty(), "no sync while revoked");
    }

    #[test]
    fn session_start_401_revokes_and_blocks_the_first_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        save_creds(&paths, false);
        let spawned = RefCell::new(Vec::<Vec<String>>::new());
        let spawn = |args: &[String]| spawned.borrow_mut().push(args.to_vec());
        let checked = RefCell::new(Vec::<String>::new());
        let rejected = |c: &Credentials| {
            checked.borrow_mut().push(c.token.clone());
            TokenCheck::Rejected
        };

        let out = run(&paths, HookEvent::SessionStart, r#"{"session_id":"sess-3"}"#, Utc::now(), &fixed_bin(), &spawn, &rejected).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let ctx = v["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
        assert!(ctx.contains("NOT LOGGED IN") && ctx.contains("revoked") && ctx.contains("Do not start"), "{ctx}");
        assert!(!ctx.contains("dev@example.com"), "{ctx}");
        assert_eq!(*checked.borrow(), vec!["tok".to_string()]);
        assert!(spawned.borrow().is_empty(), "no sync for a revoked token");
        assert!(matches!(credentials::load(&paths), AuthState::Invalid(c) if c.token == "tok"));

        // The gate itself stays offline and now blocks the very first prompt.
        let panic_check = |_: &Credentials| -> TokenCheck { panic!("the gate must not check online") };
        let out = run(&paths, HookEvent::UserPromptSubmit, r#"{"session_id":"sess-3"}"#, Utc::now(), &fixed_bin(), &spawn, &panic_check).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["decision"], "block");
        assert!(v["reason"].as_str().unwrap().contains("revoked"), "{v}");
    }

    #[test]
    fn session_start_timeout_or_network_error_proceeds_as_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        save_creds(&paths, false);
        let spawned = RefCell::new(Vec::<Vec<String>>::new());
        let spawn = |args: &[String]| spawned.borrow_mut().push(args.to_vec());
        let unknown = |_: &Credentials| TokenCheck::Unknown;

        let out = run(&paths, HookEvent::SessionStart, r#"{"session_id":"sess-4"}"#, Utc::now(), &fixed_bin(), &spawn, &unknown).unwrap();
        assert!(out.contains("logged in as dev@example.com") && !out.contains("NOT LOGGED IN"), "{out}");
        assert_eq!(*spawned.borrow(), vec![vec!["sync".to_string(), "--quiet".to_string()]]);
        assert!(matches!(credentials::load(&paths), AuthState::LoggedIn(_)));
    }

    #[test]
    fn only_session_start_with_credentials_checks_online() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let spawn = |_: &[String]| {};
        let panic_check = |_: &Credentials| -> TokenCheck { panic!("no online check expected") };
        // Logged out: nothing to check.
        run(&paths, HookEvent::SessionStart, "{}", Utc::now(), &fixed_bin(), &spawn, &panic_check).unwrap();
        save_creds(&paths, false);
        for ev in [HookEvent::UserPromptSubmit, HookEvent::PreToolUse, HookEvent::Stop, HookEvent::SessionEnd] {
            run(&paths, ev, r#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#, Utc::now(), &fixed_bin(), &spawn, &panic_check).unwrap();
        }
        // Already marked invalid: no check either.
        save_creds(&paths, true);
        let out = run(&paths, HookEvent::SessionStart, "{}", Utc::now(), &fixed_bin(), &spawn, &panic_check).unwrap();
        assert!(out.contains("revoked"), "{out}");
    }

    fn creds_for(server: &str) -> Credentials {
        Credentials {
            server: server.into(),
            token: "tok".into(),
            user: credentials::UserInfo { id: 1, email: "dev@example.com".into(), name: "Dev".into() },
            logged_in_at: Utc::now(),
            invalid: false,
        }
    }

    #[test]
    fn online_check_maps_server_answers() {
        let mut server = mockito::Server::new();
        let ok = server.mock("GET", "/api/me").match_header("authorization", "Bearer tok").with_status(200)
            .with_body(r#"{"id":1,"email":"dev@example.com","name":"Dev","is_admin":false}"#).expect(1).create();
        assert_eq!(check_token_online(&creds_for(&server.url()), SESSION_START_CHECK_TIMEOUT), TokenCheck::Valid);
        ok.assert();
        ok.remove();
        let revoked = server.mock("GET", "/api/me").with_status(401).with_body(r#"{"error":"invalid or revoked token"}"#).expect(1).create();
        assert_eq!(check_token_online(&creds_for(&server.url()), SESSION_START_CHECK_TIMEOUT), TokenCheck::Rejected);
        revoked.assert();
        revoked.remove();
        server.mock("GET", "/api/me").with_status(500).create();
        assert_eq!(check_token_online(&creds_for(&server.url()), SESSION_START_CHECK_TIMEOUT), TokenCheck::Unknown);
        // Nothing listening.
        assert_eq!(check_token_online(&creds_for("http://127.0.0.1:9"), SESSION_START_CHECK_TIMEOUT), TokenCheck::Unknown);
    }

    #[test]
    fn online_check_gives_up_after_the_time_limit() {
        // Accepts connections (via the backlog) but never answers.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let start = std::time::Instant::now();
        assert_eq!(check_token_online(&creds_for(&url), SESSION_START_CHECK_TIMEOUT), TokenCheck::Unknown);
        let took = start.elapsed();
        assert!(took >= Duration::from_millis(1900) && took < Duration::from_secs(4), "{took:?}");
        drop(listener);
    }
}

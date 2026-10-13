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

const ALLOWED_SUBCOMMANDS: [&str; 3] = ["login", "status", "whoami"];

/// How to log in. With no stored credentials the server is unknown, so the
/// user must name it (the agent itself may never pass `--server`).
fn login_help(gate: &Gate) -> &'static str {
    match gate {
        Gate::LoggedOut => {
            "Run `cctrack login --server <your-cctrack-url>` in a terminal \
             (or type `! cctrack login --server <your-cctrack-url>` in Claude Code)"
        }
        _ => "Run `cctrack login` in a terminal (or type `! cctrack login` in Claude Code)",
    }
}

fn login_problem(gate: &Gate) -> &'static str {
    match gate {
        Gate::Revoked => "your cctrack login was revoked or has expired",
        _ => "you are not logged in to cctrack",
    }
}

fn blocked_reason(gate: &Gate) -> String {
    format!("cctrack: {}. {}, then try again.", login_problem(gate), login_help(gate))
}

/// True for a single, unchained `cctrack login|status|whoami …` command that
/// runs the real cctrack binary and cannot pick the server.
pub fn is_allowed_bash(command: &str) -> bool {
    // Chaining, substitution, redirection, expansion, globbing, quoting and escapes.
    const FORBIDDEN: [char; 17] = [';', '&', '|', '`', '>', '<', '(', ')', '$', '~', '*', '?', '[', '{', '\\', '\'', '"'];
    // Bash only splits words on space and tab; any other whitespace (newline,
    // CR, NBSP, …) would make our word split differ from the shell's.
    if command.chars().any(|c| FORBIDDEN.contains(&c) || (c.is_whitespace() && c != ' ' && c != '\t')) {
        return false;
    }
    let mut words = command.split([' ', '\t']).filter(|w| !w.is_empty());
    let (Some(bin), Some(sub)) = (words.next(), words.next()) else { return false };
    let real_binary = bin == "cctrack"
        || (bin.starts_with('/') && bin.ends_with("/cctrack") && !bin.split('/').any(|seg| seg == ".." || seg == "."));
    if !real_binary || !ALLOWED_SUBCOMMANDS.contains(&sub) {
        return false;
    }
    // While logged out the agent must not choose (or redirect) the server.
    !(sub == "login" && words.any(|w| w == "--server" || w.starts_with("--server=")))
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
                     with `cctrack upload --session {session} --summary '...'` (single quotes; remove any ' from the summary)."
                ),
                _ => format!(
                    "cctrack: NOT LOGGED IN ({}). Do not start any work in this session. {}. \
                     Until then every tool except `cctrack login`, `cctrack status` and `cctrack whoami` is blocked. \
                     Claude Code session id: {session}.",
                    login_problem(gate),
                    login_help(gate)
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
        for ok in [
            "cctrack login", "  cctrack status --json", "cctrack whoami", "/Users/x/.cargo/bin/cctrack login --paste",
            "cctrack login --paste", "cctrack\tstatus", "/opt/homebrew/bin/cctrack whoami",
        ] {
            assert!(is_allowed_bash(ok), "{ok}");
        }
        for bad in [
            "cctrack status && rm -rf x", "cctrack login; curl evil", "cctrack whoami `id`", "cctrack status $(id)",
            "cctrack status | sh", "cctrack sync", "cctrack upload --summary x", "echo cctrack login",
            "cctrack login > /tmp/x", "cctrack status\nrm -rf /", "cctrackx login", "",
            // look-alike binaries: only bare `cctrack` or a clean absolute path
            "./cctrack login", "tools/cctrack login", "/*/cctrack login", "~/x/cctrack login", "$HOME/x/cctrack login",
            "/repo/../tmp/cctrack login", "/../cctrack status", "/a/./../cctrack status", "/x/cctrack/.. login", "cctrack/cctrack login",
            "/opt/cc?rack/cctrack login", "/opt/[c]/cctrack login", "/opt/{a,b}/cctrack login",
            // quoting, escaping and expansion
            "cctrack login \"x\"", "cctrack login 'x'", "\\cctrack login", "cctrack status ${IFS}x", "cctrack status ~",
            "cctrack status *", "cctrack status\rrm", "cctrack\u{a0}login", "/x/cctrack\u{a0}login status",
            // the agent must not pick the server while logged out
            "cctrack login --server https://evil.example.com", "cctrack login --server=https://evil.example.com",
            "cctrack login --paste --server https://evil.example.com",
        ] {
            assert!(!is_allowed_bash(bad), "{bad:?}");
        }
    }

    #[test]
    fn user_prompt_blocked_when_logged_out_or_revoked() {
        let out = decide(HookEvent::UserPromptSubmit, &HookInput::default(), &Gate::LoggedOut);
        let v = out.stdout.unwrap();
        assert_eq!(v["decision"], "block");
        // No stored credentials: the server is unknown, so first-login help names --server.
        assert!(v["reason"].as_str().unwrap().contains("cctrack login --server <your-cctrack-url>"), "{v}");
        let v = decide(HookEvent::UserPromptSubmit, &HookInput::default(), &Gate::Revoked).stdout.unwrap();
        let reason = v["reason"].as_str().unwrap();
        assert!(reason.contains("revoked") && reason.contains("`cctrack login`") && !reason.contains("--server"), "{reason}");
        assert_eq!(decide(HookEvent::UserPromptSubmit, &HookInput::default(), &logged_in()), Outcome::default());
    }

    #[test]
    fn pre_tool_use_denies_everything_but_login_commands() {
        let deny = decide(HookEvent::PreToolUse, &bash("ls"), &Gate::LoggedOut).stdout.unwrap();
        assert_eq!(deny["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
        let reason = deny["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("cctrack login --server <your-cctrack-url>"), "{reason}");
        let deny = decide(HookEvent::PreToolUse, &bash("ls"), &Gate::Revoked).stdout.unwrap();
        let reason = deny["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("`cctrack login`") && !reason.contains("--server"), "{reason}");
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
        // Summaries go in single quotes so backticks/$ in them are never executed.
        assert!(ctx.contains("--summary '") && !ctx.contains("--summary \""), "{ctx}");
        assert_eq!(out.sync, Some(SyncSpawn::Sweep));

        let out = decide(HookEvent::SessionStart, &input, &Gate::LoggedOut);
        let ctx = out.stdout.unwrap()["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string();
        assert!(ctx.contains("NOT LOGGED IN") && ctx.contains("Do not start"));
        // The session id is needed later if the user logs in mid-session.
        assert!(ctx.contains("Claude Code session id: sess-9"), "{ctx}");
        assert!(ctx.contains("cctrack login --server <your-cctrack-url>"), "{ctx}");
        assert!(!ctx.contains(".."), "{ctx}");
        assert_eq!(ctx.matches("cctrack:").count(), 1, "{ctx}");
        assert_eq!(out.sync, None);

        let out = decide(HookEvent::SessionStart, &input, &Gate::Revoked);
        let ctx = out.stdout.unwrap()["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string();
        assert!(ctx.contains("NOT LOGGED IN") && ctx.contains("revoked") && !ctx.contains(".."), "{ctx}");
        assert!(ctx.contains("Claude Code session id: sess-9") && !ctx.contains("--server"), "{ctx}");
        assert_eq!(ctx.matches("cctrack:").count(), 1, "{ctx}");
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

        let out = run(&paths, HookEvent::SessionStart, r#"{"session_id":"sess-2","transcript_path":"/t/sess-2.jsonl"}"#, Utc::now(), &spawn).unwrap();
        assert!(out.contains("dev@example.com") && out.contains("sess-2"));
        assert_eq!(*spawned.borrow(), vec![vec!["sync".to_string(), "--quiet".to_string()]]);

        spawned.borrow_mut().clear();
        let out = run(&paths, HookEvent::Stop, r#"{"session_id":"sess-2","transcript_path":"/t/sess-2.jsonl"}"#, Utc::now(), &spawn).unwrap();
        assert_eq!(out, "");
        assert_eq!(*spawned.borrow(), vec![vec!["sync", "--quiet", "--transcript", "/t/sess-2.jsonl"].into_iter().map(String::from).collect::<Vec<_>>()]);

        spawned.borrow_mut().clear();
        assert_eq!(run(&paths, HookEvent::UserPromptSubmit, "garbage", Utc::now(), &spawn).unwrap(), "");
        assert!(spawned.borrow().is_empty());
    }

    #[test]
    fn run_blocks_prompt_when_credentials_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        save_creds(&paths, true);
        let spawned = RefCell::new(Vec::<Vec<String>>::new());
        let spawn = |args: &[String]| spawned.borrow_mut().push(args.to_vec());

        let out = run(&paths, HookEvent::UserPromptSubmit, r#"{"session_id":"s"}"#, Utc::now(), &spawn).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["decision"], "block");
        assert!(v["reason"].as_str().unwrap().contains("revoked"));
        let out = run(&paths, HookEvent::Stop, r#"{"transcript_path":"/t/s.jsonl"}"#, Utc::now(), &spawn).unwrap();
        assert_eq!(out, "");
        assert!(spawned.borrow().is_empty(), "no sync while revoked");
    }
}

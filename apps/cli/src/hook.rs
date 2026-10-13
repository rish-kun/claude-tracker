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

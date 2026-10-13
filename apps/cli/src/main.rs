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
    /// Upload new usage from Claude Code transcripts.
    Sync {
        /// Only sync this session transcript (used by hooks).
        #[arg(long)]
        transcript: Option<std::path::PathBuf>,
        /// Log errors to the state dir instead of stderr (used by hooks).
        #[arg(long)]
        quiet: bool,
    },
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
        Cmd::Sync { transcript, quiet } => sync_cmd(&paths, transcript, quiet),
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
            use cctrack::login::LogoutOutcome;
            let outcome = cctrack::login::logout(&paths)?;
            println!("cctrack: logged out");
            match outcome {
                LogoutOutcome::Revoked | LogoutOutcome::NotLoggedIn => Ok(ExitCode::SUCCESS),
                LogoutOutcome::RevokeFailed(e) => {
                    eprintln!(
                        "cctrack: could not revoke this device's token on the server ({e}); revoke it from the dashboard's Settings page"
                    );
                    Ok(ExitCode::FAILURE)
                }
            }
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

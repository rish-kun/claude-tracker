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

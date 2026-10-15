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
        /// Print what would be uploaded per session (account, how it was chosen,
        /// title) without uploading or changing any state.
        #[arg(long, conflicts_with = "quiet")]
        dry_run: bool,
        /// With --dry-run: read transcripts from the start (usage since login),
        /// not just what is new since the last sync.
        #[arg(long, requires = "dry_run")]
        rescan: bool,
    },
    /// Show or set how usage through a local CLIProxyAPI pool is credited:
    /// auto (detect per session, the default), on (every session), off (never).
    /// CCTRACK_PROXY_MODE overrides the saved mode.
    Proxy {
        #[arg(value_enum, default_value = "status")]
        action: ProxyAction,
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
        Cmd::Sync { transcript, dry_run: true, rescan, .. } => dry_run(&paths, transcript, rescan),
        Cmd::Sync { transcript, quiet, .. } => sync_cmd(&paths, transcript, quiet),
        Cmd::Proxy { action } => proxy_cmd(&paths, action),
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
                    credentials::mark_invalid(&paths, &c.token)?;
                    anyhow::bail!("your token was revoked or expired — run `cctrack login`")
                }
                Err(e) => Err(e.into()),
            }
        }
        Cmd::Hook { event } => {
            // Never fail on odd stdin: the decision must depend only on credentials.
            let mut raw = Vec::new();
            let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut raw);
            let input = String::from_utf8_lossy(&raw);
            let spawn = |args: &[String]| {
                let _ = cctrack::detach::spawn(args);
            };
            let bin = cctrack::hook::TrustedBinary::current();
            // Runs on a detached thread with a deadline (hook::check_with_deadline).
            let check: cctrack::hook::TokenChecker = std::sync::Arc::new(|c: &credentials::Credentials| {
                cctrack::hook::check_token_online(c, cctrack::hook::SESSION_START_CHECK_TIMEOUT)
            });
            let out = cctrack::hook::run(&paths, event, &input, chrono::Utc::now(), &bin, &spawn, &check)?;
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
                if r.titles_sent > 0 {
                    println!("cctrack: updated {} session titles", r.titles_sent);
                }
            }
            for id in &r.quarantined {
                eprintln!("cctrack: the server refused session {id}; its usage was dropped and it will not be retried (see {})", paths.sync_log().display());
            }
            Ok(ExitCode::SUCCESS)
        }
        Ok(_) => Ok(ExitCode::SUCCESS),
        Err(e) if quiet => {
            cctrack::sync::log(paths, &format!("{e:#}"));
            Ok(ExitCode::FAILURE)
        }
        Err(e) => Err(e),
    }
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ProxyAction {
    /// Detect per session whether it runs through a CLIProxyAPI pool.
    Auto,
    /// Treat every session as running through the pool.
    On,
    /// Never credit pool accounts (the behaviour before 0.2.0).
    Off,
    /// Show the mode, what is detected and why, and the pool accounts.
    Status,
}

fn proxy_cmd(paths: &Paths, action: ProxyAction) -> Result<ExitCode> {
    use cctrack::proxy::{self, ProxyMode};
    let mode = match action {
        ProxyAction::Auto => ProxyMode::Auto,
        ProxyAction::On => ProxyMode::On,
        ProxyAction::Off => ProxyMode::Off,
        ProxyAction::Status => {
            let env_url = std::env::var("ANTHROPIC_BASE_URL").ok();
            let cwd = std::env::current_dir().ok();
            let out = proxy::status_report(paths, proxy::env_mode().as_deref(), env_url.as_deref(), cwd.as_deref(), chrono::Utc::now());
            println!("{out}");
            return Ok(ExitCode::SUCCESS);
        }
    };
    proxy::save_mode(paths, mode)?;
    println!("cctrack: proxy mode set to {}", mode.as_str());
    if let Some(v) = proxy::env_mode().filter(|v| ProxyMode::parse(v).is_some_and(|m| m != mode)) {
        eprintln!("cctrack: note: {}={v} in this environment overrides the saved mode", proxy::MODE_ENV);
    }
    Ok(ExitCode::SUCCESS)
}

fn dry_run(paths: &Paths, transcript: Option<std::path::PathBuf>, rescan: bool) -> Result<ExitCode> {
    let AuthState::LoggedIn(creds) = credentials::load(paths) else {
        anyhow::bail!("not logged in — run `cctrack login`");
    };
    let opts = cctrack::sync::SyncOptions { transcript, wait_for_lock: std::time::Duration::ZERO };
    let r = cctrack::sync::dry_run(paths, &creds, &opts, rescan, chrono::Utc::now());
    if let Some((mode, source)) = r.mode {
        let source = match source {
            cctrack::proxy::ModeSource::Env => "from CCTRACK_PROXY_MODE",
            cctrack::proxy::ModeSource::Saved => "saved",
            cctrack::proxy::ModeSource::Default => "default",
        };
        println!("cctrack sync --dry-run: proxy mode {} ({source}); {} transcript files with new lines; nothing uploaded", mode.as_str(), r.files_read);
    }
    if r.sessions.is_empty() {
        println!("nothing to upload");
    }
    for s in &r.sessions {
        let title = match (&s.title, s.title_pending) {
            (Some(t), true) => format!(" title {t:?} (would be sent)"),
            (Some(t), false) => format!(" title {t:?}"),
            (None, _) => String::new(),
        };
        println!("session {}: {} entries{title}", s.session_id, s.entries);
        match &s.proxy {
            Some(Ok(why)) => println!("  proxied: {why}"),
            Some(Err(why)) => println!("  not proxied: {why}"),
            None => {}
        }
        for ((account, how), n) in &s.accounts {
            println!("  {n} entries -> {account} ({how})");
        }
    }
    Ok(ExitCode::SUCCESS)
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
            credentials::mark_invalid(paths, &creds.token)?;
            anyhow::bail!("your token was revoked or expired — run `cctrack login`")
        }
        Err(e) => Err(e.into()),
    }
}

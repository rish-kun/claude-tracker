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

use crate::client::{ApiClient, ApiError, is_secure_server};
use crate::credentials::{self, AuthState, Credentials};
use crate::paths::Paths;
use crate::sync::{self, SyncOptions, hostname};
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

/// Checks that `server` is an absolute https URL with a host (plain http
/// only for localhost, 127.0.0.1 and ::1) and no query or fragment, and
/// returns it without trailing slashes. Catches values like
/// `cctrack.example.com` (no scheme) and `localhost:3000` (parses as an
/// opaque URL with scheme "localhost"), which would otherwise panic or
/// produce an unusable link.
pub fn validate_server(server: &str) -> Result<String> {
    let Some(url) = Url::parse(server).ok().filter(|u| {
        matches!(u.scheme(), "http" | "https") && !u.cannot_be_a_base() && u.host_str().is_some_and(|h| !h.is_empty())
    }) else {
        bail!("invalid server URL '{server}': include the scheme, e.g. https://cctrack.example.com");
    };
    if url.query().is_some() || url.fragment().is_some() {
        bail!("invalid server URL '{server}': remove the query (?…) or fragment (#…)");
    }
    if !is_secure_server(&url) {
        bail!("invalid server URL '{server}': use https (plain http is only allowed for localhost, 127.0.0.1 and ::1)");
    }
    Ok(server.trim_end_matches('/').to_string())
}

/// `server` must already have passed [`validate_server`].
pub fn start_url(server: &str, port: Option<u16>, state: &str, challenge: &str) -> String {
    let mut url = Url::parse(&format!("{}/api/auth/cli/start", server.trim_end_matches('/')))
        .expect("server URL was checked by validate_server");
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
    login_with(paths, server, now, |server, state, challenge| {
        if paste {
            let url = start_url(server, None, state, challenge);
            eprintln!("Open this URL in a browser and sign in with Google:\n\n  {url}\n");
            eprint!("Paste the code shown after sign-in: ");
            io::stderr().flush()?;
            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            Ok(line.trim().to_string())
        } else {
            let listener = TcpListener::bind("127.0.0.1:0").context("bind loopback listener")?;
            let port = listener.local_addr()?.port();
            let url = start_url(server, Some(port), state, challenge);
            eprintln!("Opening your browser to sign in with Google…\nIf it does not open, visit:\n\n  {url}\n");
            let _ = open::that(&url);
            wait_for_callback(&listener, state, Duration::from_secs(300))
        }
    })
}

/// The login flow with the sign-in step (`obtain_code(server, state,
/// code_challenge)`) supplied by the caller.
pub fn login_with(
    paths: &Paths,
    server: &str,
    now: DateTime<Utc>,
    obtain_code: impl FnOnce(&str, &str, &str) -> Result<String>,
) -> Result<Credentials> {
    let server = validate_server(server)?;
    let server = server.as_str();
    // Usage recorded so far may belong to a different user than the one about
    // to sign in: upload it with the current token first. This runs before the
    // sign-in and code exchange, so interrupting it can never leave a freshly
    // minted token unsaved (and live) on the server.
    if let AuthState::LoggedIn(old) = credentials::load(paths) {
        best_effort_sync(paths, &old, "for the current login");
    }
    let pk = pkce();
    let state = random_state();
    let code = obtain_code(server, &state, &pk.challenge)?;
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

#[derive(Debug)]
pub enum LogoutOutcome {
    /// The server revoked the token, or already considered it invalid.
    Revoked,
    /// No local credentials existed.
    NotLoggedIn,
    /// Local credentials were deleted, but the server-side token may still be live.
    RevokeFailed(ApiError),
}

/// Uploads pending usage with `creds` before they stop being usable. Failures
/// are reported on stderr and never block the caller.
pub fn best_effort_sync(paths: &Paths, creds: &Credentials, context: &str) {
    let client = ApiClient::new(&creds.server, Some(&creds.token));
    let opts = SyncOptions { transcript: None, wait_for_lock: Duration::from_secs(10) };
    match sync::run(paths, creds, &client, &opts, Utc::now()) {
        Ok(r) if r.skipped_locked => {
            eprintln!("cctrack: another sync is still running; some usage {context} may not be uploaded")
        }
        Ok(_) => {}
        Err(e) => eprintln!("cctrack: could not upload pending usage {context}: {e:#}"),
    }
}

/// Uploads pending usage (best effort), revokes this device's token on the
/// server, then deletes local credentials regardless of whether the revoke
/// succeeded.
pub fn logout(paths: &Paths) -> Result<LogoutOutcome> {
    let auth = credentials::load(paths);
    let outcome = match auth {
        AuthState::LoggedIn(ref c) | AuthState::Invalid(ref c) => {
            // Invalid also covers a token with a revoked marker (c.invalid unset).
            if matches!(auth, AuthState::LoggedIn(_)) {
                best_effort_sync(paths, c, "before logging out");
            }
            match ApiClient::new(&c.server, Some(&c.token)).revoke_own_token() {
                Ok(()) | Err(ApiError::Unauthorized(_)) => LogoutOutcome::Revoked,
                Err(e) => LogoutOutcome::RevokeFailed(e),
            }
        }
        AuthState::LoggedOut => LogoutOutcome::NotLoggedIn,
    };
    credentials::delete(paths)?;
    Ok(outcome)
}


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
    fn validate_server_rejects_schemeless_and_opaque() {
        for bad in ["cctrack.example.com", "localhost:3000", "ftp://c.example.com", "https://", "mailto:a@b.c", ""] {
            let err = validate_server(bad).unwrap_err().to_string();
            assert!(err.contains("invalid server URL"), "{bad}: {err}");
            assert!(err.contains("include the scheme"), "{bad}: {err}");
        }
    }

    #[test]
    fn validate_server_accepts_https_and_loopback_http() {
        assert_eq!(validate_server("https://c.example.com").unwrap(), "https://c.example.com");
        assert_eq!(validate_server("https://c.example.com/").unwrap(), "https://c.example.com");
        assert_eq!(validate_server("https://c.example.com/cctrack/").unwrap(), "https://c.example.com/cctrack");
        assert_eq!(validate_server("http://localhost:8080").unwrap(), "http://localhost:8080");
        assert_eq!(validate_server("http://127.0.0.1:8080").unwrap(), "http://127.0.0.1:8080");
        assert_eq!(validate_server("http://[::1]:8080").unwrap(), "http://[::1]:8080");
    }

    #[test]
    fn validate_server_rejects_plain_http_to_remote_hosts() {
        for bad in ["http://c.example.com", "http://10.0.0.5:8080", "http://localhost.example.com", "http://127.0.0.2"] {
            let err = validate_server(bad).unwrap_err().to_string();
            assert!(err.contains("use https"), "{bad}: {err}");
        }
    }

    #[test]
    fn validate_server_rejects_query_and_fragment() {
        for bad in ["https://c.example.com?x=1", "https://c.example.com/?", "https://c.example.com#frag", "https://c.example.com/#"] {
            let err = validate_server(bad).unwrap_err().to_string();
            assert!(err.contains("query") && err.contains("fragment"), "{bad}: {err}");
        }
    }

    #[test]
    fn login_syncs_previous_user_before_sign_in_and_exchange() {
        use std::sync::{Arc, Mutex};
        let mut server = mockito::Server::new();
        let order: Arc<Mutex<Vec<&'static str>>> = Arc::default();
        let (o1, o2) = (order.clone(), order.clone());
        let ingest = server
            .mock("POST", "/api/ingest")
            .match_header("authorization", "Bearer cct_abc")
            .with_body_from_request(move |_| {
                o1.lock().unwrap().push("old-user-ingest");
                br#"{"accepted":1,"duplicates":0}"#.to_vec()
            })
            .create();
        let exchange = server
            .mock("POST", "/api/auth/cli/exchange")
            .with_body_from_request(move |_| {
                o2.lock().unwrap().push("exchange");
                br#"{"token":"cct_new","user":{"id":2,"email":"new@example.com","name":"New"}}"#.to_vec()
            })
            .create();

        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let old = Credentials {
            server: server.url(),
            token: "cct_abc".into(),
            user: crate::credentials::UserInfo { id: 1, email: "dev@example.com".into(), name: "Dev".into() },
            logged_in_at: "2026-10-01T00:00:00Z".parse().unwrap(),
            invalid: false,
        };
        credentials::save(&paths, &old).unwrap();
        let t = paths.projects_dir().join("-p/s1.jsonl");
        std::fs::create_dir_all(t.parent().unwrap()).unwrap();
        std::fs::write(&t, r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-10-01T10:00:00Z","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":1,"output_tokens":1}}}"#.to_string() + "\n").unwrap();

        let o3 = order.clone();
        let creds = login_with(&paths, &server.url(), Utc::now(), |_, _, _| {
            o3.lock().unwrap().push("sign-in");
            Ok("code-1".into())
        })
        .unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["old-user-ingest", "sign-in", "exchange"]);
        assert_eq!(creds.token, "cct_new");
        match credentials::load(&paths) {
            AuthState::LoggedIn(c) => assert_eq!((c.token.as_str(), c.user.id), ("cct_new", 2)),
            other => panic!("{other:?}"),
        }
        ingest.assert();
        exchange.assert();
    }

    #[test]
    fn login_interrupted_during_sign_in_keeps_old_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        saved_creds(&paths, "http://127.0.0.1:9");
        let err = login_with(&paths, "http://127.0.0.1:9", Utc::now(), |_, _, _| bail!("cancelled")).unwrap_err();
        assert!(err.to_string().contains("cancelled"));
        assert!(matches!(credentials::load(&paths), AuthState::LoggedIn(c) if c.token == "cct_abc"));
    }

    #[test]
    fn login_rejects_bad_server_before_any_io() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let err = login(&paths, "localhost:3000", false, Utc::now()).unwrap_err();
        assert!(err.to_string().contains("invalid server URL 'localhost:3000'"), "{err}");
    }

    fn saved_creds(paths: &Paths, server: &str) {
        let creds = Credentials {
            server: server.into(),
            token: "cct_abc".into(),
            user: crate::credentials::UserInfo { id: 1, email: "dev@example.com".into(), name: "Dev".into() },
            logged_in_at: Utc::now(),
            invalid: false,
        };
        credentials::save(paths, &creds).unwrap();
    }

    fn logout_with_status(status: usize) -> (LogoutOutcome, AuthState) {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("DELETE", "/api/auth/token")
            .match_header("authorization", "Bearer cct_abc")
            .with_status(status)
            .with_body(r#"{"error":"nope"}"#)
            .create();
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        saved_creds(&paths, &server.url());
        let out = logout(&paths).unwrap();
        mock.assert();
        (out, credentials::load(&paths))
    }

    #[test]
    fn logout_revoked_deletes_credentials() {
        let (out, auth) = logout_with_status(204);
        assert!(matches!(out, LogoutOutcome::Revoked));
        assert!(matches!(auth, AuthState::LoggedOut));
    }

    #[test]
    fn logout_unauthorized_counts_as_revoked() {
        let (out, auth) = logout_with_status(401);
        assert!(matches!(out, LogoutOutcome::Revoked));
        assert!(matches!(auth, AuthState::LoggedOut));
    }

    #[test]
    fn logout_server_error_reports_failure_but_deletes_credentials() {
        let (out, auth) = logout_with_status(500);
        assert!(matches!(out, LogoutOutcome::RevokeFailed(ApiError::Http(500, _))));
        assert!(matches!(auth, AuthState::LoggedOut));
    }

    #[test]
    fn logout_network_error_reports_failure_but_deletes_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        // Bind then drop to get a local port nothing listens on.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        saved_creds(&paths, &format!("http://127.0.0.1:{port}"));
        assert!(matches!(logout(&paths).unwrap(), LogoutOutcome::RevokeFailed(ApiError::Network(_))));
        assert!(matches!(credentials::load(&paths), AuthState::LoggedOut));
    }

    #[test]
    fn logout_syncs_pending_usage_before_revoking() {
        use std::sync::{Arc, Mutex};
        let mut server = mockito::Server::new();
        let order: Arc<Mutex<Vec<&'static str>>> = Arc::default();
        let (o1, o2) = (order.clone(), order.clone());
        let ingest = server
            .mock("POST", "/api/ingest")
            .match_header("authorization", "Bearer cct_abc")
            .with_body_from_request(move |_| {
                o1.lock().unwrap().push("ingest");
                br#"{"accepted":1,"duplicates":0}"#.to_vec()
            })
            .create();
        let revoke = server
            .mock("DELETE", "/api/auth/token")
            .match_header("authorization", "Bearer cct_abc")
            .with_body_from_request(move |_| {
                o2.lock().unwrap().push("revoke");
                b"{}".to_vec()
            })
            .create();

        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let creds = Credentials {
            server: server.url(),
            token: "cct_abc".into(),
            user: crate::credentials::UserInfo { id: 1, email: "dev@example.com".into(), name: "Dev".into() },
            logged_in_at: "2026-10-01T00:00:00Z".parse().unwrap(),
            invalid: false,
        };
        credentials::save(&paths, &creds).unwrap();
        let t = paths.projects_dir().join("-p/s1.jsonl");
        std::fs::create_dir_all(t.parent().unwrap()).unwrap();
        std::fs::write(&t, r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-10-01T10:00:00Z","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":1,"output_tokens":1}}}"#.to_string() + "\n").unwrap();

        assert!(matches!(logout(&paths).unwrap(), LogoutOutcome::Revoked));
        assert_eq!(*order.lock().unwrap(), vec!["ingest", "revoke"]);
        assert!(matches!(credentials::load(&paths), AuthState::LoggedOut));
        ingest.assert();
        revoke.assert();
    }

    #[test]
    fn logout_when_logged_out() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(logout(&Paths::under(dir.path())).unwrap(), LogoutOutcome::NotLoggedIn));
    }

    #[test]
    fn callback_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(wait_for_callback(&listener, "xyz", Duration::from_millis(300)).is_err());
    }
}

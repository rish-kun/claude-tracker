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

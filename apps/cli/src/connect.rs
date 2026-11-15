//! `cctrack proxy connect <url>`: get a key for the admin's CLIProxyAPI,
//! print how to point Claude Code and Codex at it over Tailscale, then check
//! whether the admin has approved it.
//!
//! The key is requested from the cctrack server with this device's CLI token
//! (so the admin sees a Google-verified email), saved in
//! `<config>/proxy-keys.json` (0600) by proxy URL, and reused on later runs.
//! No other file is written: the user pastes what they need.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use reqwest::Url;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::credentials::Credentials;
use crate::paths::Paths;
use crate::state::{lock_file, read_json, write_json_atomic};
use crate::wire::ProxyKeyRequested;

/// Keys the cctrack server hands out start with this.
pub const KEY_PREFIX: &str = "ccp_";
/// The env var the Codex snippet reads the key from.
pub const CODEX_ENV_KEY: &str = "CCTRACK_PROXY_KEY";

/// Validates and normalises the proxy URL: http(s), no credentials, query or
/// fragment, no trailing slash. Plain http is allowed only to loopback and
/// Tailscale (100.64.0.0/10) addresses, whose traffic is encrypted anyway.
pub fn normalize_url(raw: &str) -> Result<String, String> {
    let url = Url::parse(raw.trim()).map_err(|e| format!("{raw:?} is not a URL ({e}); use e.g. https://mac.tailnet.ts.net"))?;
    match url.scheme() {
        "https" => {}
        "http" if url.host_str().is_some_and(http_ok) => {}
        "http" => return Err(format!("refusing to send the key to {raw} over plain http; use the https://….ts.net URL")),
        _ => return Err(format!("{raw:?} must start with https://")),
    }
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err(format!("{raw:?} must be just the proxy address, without credentials, ?query or #fragment"));
    }
    let path = url.path().trim_end_matches('/');
    if !path.chars().all(|c| c.is_ascii_alphanumeric() || "/-._~".contains(c)) {
        return Err(format!("{raw:?} has unexpected characters in its path"));
    }
    let host = url.host_str().unwrap_or_default();
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    Ok(format!("{}://{host}{port}{path}", url.scheme()))
}

fn http_ok(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    match host.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_loopback() || (ip.octets()[0] == 100 && ip.octets()[1] & 0xC0 == 64),
        Ok(IpAddr::V6(ip)) => ip.is_loopback(),
        Err(_) => false,
    }
}

/// Checks the key's shape: `ccp_` plus base64url, so it is safe to paste into a shell.
pub fn validate_key(key: &str) -> Result<(), String> {
    let Some(rest) = key.strip_prefix(KEY_PREFIX) else {
        return Err(format!("the proxy key should start with {KEY_PREFIX}"));
    };
    if rest.len() < 16 || !rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("that doesn't look like a whole proxy key".into());
    }
    Ok(())
}

/// What to paste, for Claude Code and for Codex. `url` must be normalised.
pub fn snippet(url: &str, key: &str) -> String {
    format!(
        r#"Claude Code: add to your shell profile (or the "env" block of ~/.claude/settings.json):

  export ANTHROPIC_BASE_URL={url}
  export ANTHROPIC_AUTH_TOKEN={key}

  or in settings.json:
  "env": {{ "ANTHROPIC_BASE_URL": "{url}", "ANTHROPIC_AUTH_TOKEN": "{key}" }}

Codex: add to ~/.codex/config.toml, and `export {env}={key}` in your shell profile:

  model_provider = "cctrack-proxy"

  [model_providers.cctrack-proxy]
  name = "cctrack proxy"
  base_url = "{url}/v1"
  env_key = "{env}"
  wire_api = "responses"

The key is saved in cctrack's config dir; nothing else was written. Paste what you need."#,
        env = CODEX_ENV_KEY,
    )
}

/// One saved key: which login and device it was requested for, so a later
/// run reuses it only for the same server, account and device.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SavedKey {
    pub key: String,
    pub server: String,
    pub email: String,
    pub device: String,
    pub requested_at: DateTime<Utc>,
}

/// `<config>/proxy-keys.json`: saved keys by normalised proxy URL.
pub type SavedKeys = BTreeMap<String, SavedKey>;

pub fn load_keys(paths: &Paths) -> SavedKeys {
    read_json(&paths.proxy_keys()).unwrap_or_default()
}

/// How long `connect` waits for another `connect` on this machine to finish.
const LOCK_WAIT: Duration = Duration::from_secs(60);

/// Tightens an existing `proxy-keys.json` to 0600 (an older or hand-copied file
/// may be readable by others). A missing file is fine.
fn secure_keys_file(paths: &Paths) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = paths.proxy_keys();
        match std::fs::metadata(&path) {
            Ok(meta) if meta.permissions().mode() & 0o077 != 0 => {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| anyhow::anyhow!("couldn't make {} private: {e}", path.display()))?;
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    #[cfg(not(unix))]
    let _ = paths;
    Ok(())
}

/// Saves `key` for `url`, keeping other proxies' keys. The file is 0600.
pub fn save_key(paths: &Paths, url: &str, key: SavedKey) -> Result<()> {
    let mut keys = load_keys(paths);
    keys.insert(url.to_string(), key);
    write_json_atomic(&paths.proxy_keys(), &keys, Some(0o600))
}

/// The saved key for `url`, if it was requested by the same login for the
/// same device and still looks like a key.
pub fn saved_key_for(keys: &SavedKeys, url: &str, creds: &Credentials, device: &str) -> Option<SavedKey> {
    keys.get(url)
        .filter(|k| k.server == creds.server && k.email == creds.user.email && k.device == device && validate_key(&k.key).is_ok())
        .cloned()
}

/// The key to use and whether it was just requested.
#[derive(Debug, PartialEq)]
pub struct Obtained {
    pub key: String,
    pub fresh: bool,
}

/// Reuses the saved key for `url` unless `force_new`, otherwise asks the
/// server for one with `request(device)` and saves it. Runs under
/// `proxy-keys.lock`, so two `connect`s at once cannot save a key the server
/// has already replaced, or drop each other's entries.
pub fn obtain_key<E: Into<anyhow::Error>>(
    paths: &Paths,
    url: &str,
    creds: &Credentials,
    device: &str,
    force_new: bool,
    now: DateTime<Utc>,
    request: impl FnOnce(&str) -> Result<ProxyKeyRequested, E>,
) -> Result<Obtained> {
    let Some(_lock) = lock_file(&paths.proxy_keys_lock(), LOCK_WAIT)? else {
        anyhow::bail!("another `cctrack proxy connect` is still running; try again when it finishes");
    };
    secure_keys_file(paths)?;
    if !force_new && let Some(saved) = saved_key_for(&load_keys(paths), url, creds, device) {
        return Ok(Obtained { key: saved.key, fresh: false });
    }
    let got = request(device).map_err(Into::into)?;
    validate_key(&got.key).map_err(|e| anyhow::anyhow!("the server returned an unexpected key: {e}"))?;
    let saved = SavedKey {
        key: got.key.clone(),
        server: creds.server.clone(),
        email: creds.user.email.clone(),
        device: device.to_string(),
        requested_at: now,
    };
    save_key(paths, url, saved)?;
    Ok(Obtained { key: got.key, fresh: true })
}

/// What `GET <url>/v1/models` said about the key.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// 2xx; the number of models listed when the body has a `data` array.
    Approved(Option<usize>),
    /// 403: the proxy knows the key but the admin hasn't approved it.
    Pending,
    /// 401 for a key requested in this run: the proxy hasn't synced it yet,
    /// so it is as good as pending.
    NotSyncedYet,
    /// 401 for a saved key, with the proxy's message when it sent one.
    Unknown(Option<String>),
    Http(u16),
    Unreachable(String),
}

impl Outcome {
    /// Approved and pending both exit 0: the setup is done either way.
    pub fn is_success(&self) -> bool {
        matches!(self, Outcome::Approved(_) | Outcome::Pending | Outcome::NotSyncedYet)
    }

    /// `device_flag`: the `--device` value the user passed, repeated in the
    /// suggested commands so they act on the same saved key.
    pub fn report(&self, url: &str, email: &str, device: &str, device_flag: Option<&str>) -> String {
        let again = format!(
            "cctrack proxy connect {url}{}",
            device_flag.map(|d| format!(" --device {}", shell_quote(d))).unwrap_or_default()
        );
        match self {
            Outcome::Approved(Some(n)) => format!("approved, ready ({n} models)."),
            Outcome::Approved(None) => "approved, ready.".into(),
            Outcome::Pending => format!(
                "waiting for the admin to approve {email} on {device}. Run `{again}` again to check."
            ),
            Outcome::NotSyncedYet => format!(
                "requested: waiting for the admin to approve {email} on {device} (the proxy picks up new keys within a minute). Run `{again}` again to check."
            ),
            Outcome::Unknown(msg) => format!(
                "key unknown or revoked{}; run `{again} --new`",
                msg.as_deref().map(|m| format!(" ({m})")).unwrap_or_default()
            ),
            Outcome::Http(code) => format!("unreachable: {url}/v1/models returned HTTP {code}."),
            Outcome::Unreachable(e) => {
                format!("unreachable: couldn't reach {url} ({e}). Is Tailscale connected, and is the admin's Mac awake?")
            }
        }
    }
}

/// Quotes `s` for a POSIX shell when it has anything beyond `[A-Za-z0-9._-]`.
fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// `GET <url>/v1/models` with the key as a bearer token. `fresh` says the
/// key was requested in this run (see [`Outcome::NotSyncedYet`]).
pub fn check(url: &str, key: &str, fresh: bool, timeout: Duration) -> Outcome {
    let http = match Client::builder().timeout(timeout).user_agent(concat!("cctrack/", env!("CARGO_PKG_VERSION"))).build() {
        Ok(c) => c,
        Err(e) => return Outcome::Unreachable(e.to_string()),
    };
    let resp = match http.get(format!("{url}/v1/models")).bearer_auth(key).send() {
        Ok(r) => r,
        Err(e) => return Outcome::Unreachable(e.without_url().to_string()),
    };
    let status = resp.status().as_u16();
    let body = resp.text().ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    match status {
        200..=299 => Outcome::Approved(body.as_ref().and_then(|v| v.get("data")).and_then(|d| d.as_array()).map(Vec::len)),
        403 => Outcome::Pending,
        401 if fresh => Outcome::NotSyncedYet,
        401 => Outcome::Unknown(body.as_ref().and_then(error_message)),
        code => Outcome::Http(code),
    }
}

/// The message of an Anthropic-shaped (`{"error":{"message":…}}`) or flat (`{"error":"…"}`) error body.
fn error_message(v: &serde_json::Value) -> Option<String> {
    let e = v.get("error")?;
    e.get("message").and_then(|m| m.as_str()).or_else(|| e.as_str()).map(|m| m.chars().take(200).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "ccp_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_abcde";

    #[test]
    fn urls() {
        for (raw, want) in [
            ("https://mac.tail1.ts.net/", "https://mac.tail1.ts.net"),
            (" https://Mac.Tail1.ts.net ", "https://mac.tail1.ts.net"),
            ("https://mac.tail1.ts.net:8443/proxy/", "https://mac.tail1.ts.net:8443/proxy"),
            ("http://100.101.102.103:8318", "http://100.101.102.103:8318"),
            ("http://localhost:8318", "http://localhost:8318"),
            ("http://127.0.0.1:8318", "http://127.0.0.1:8318"),
        ] {
            assert_eq!(normalize_url(raw).as_deref(), Ok(want), "{raw}");
        }
        for bad in [
            "mac.tail1.ts.net",
            "http://mac.tail1.ts.net",
            "http://100.128.0.1",
            "http://10.0.0.5",
            "ftp://mac.tail1.ts.net",
            "https://u:p@mac.tail1.ts.net",
            "https://mac.tail1.ts.net/?x=1",
            "https://mac.tail1.ts.net/#f",
            "https://mac.tail1.ts.net/a'b",
        ] {
            assert!(normalize_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn keys() {
        assert!(validate_key(KEY).is_ok());
        assert!(validate_key("cct_AbCdEfGhIjKlMnOpQrStUv").is_err());
        assert!(validate_key("ccp_short").is_err());
        assert!(validate_key("ccp_AbCdEfGhIjKlMnOp; rm -rf").is_err());
    }

    #[test]
    fn snippet_has_claude_and_codex_settings() {
        let s = snippet("https://mac.tail1.ts.net", KEY);
        assert!(s.contains("export ANTHROPIC_BASE_URL=https://mac.tail1.ts.net\n"), "{s}");
        assert!(s.contains(&format!("export ANTHROPIC_AUTH_TOKEN={KEY}\n")), "{s}");
        assert!(s.contains(&format!(r#""env": {{ "ANTHROPIC_BASE_URL": "https://mac.tail1.ts.net", "ANTHROPIC_AUTH_TOKEN": "{KEY}" }}"#)), "{s}");
        assert!(s.contains("[model_providers.cctrack-proxy]\n"), "{s}");
        assert!(s.contains("base_url = \"https://mac.tail1.ts.net/v1\"\n"), "{s}");
        assert!(s.contains("env_key = \"CCTRACK_PROXY_KEY\"\n"), "{s}");
        assert!(s.contains(&format!("export CCTRACK_PROXY_KEY={KEY}")), "{s}");
    }

    fn creds() -> Credentials {
        use chrono::TimeZone;
        Credentials {
            server: "https://cctrack.test".into(),
            token: "cct_abc".into(),
            user: crate::credentials::UserInfo { id: 1, email: "dev@example.com".into(), name: "Dev".into() },
            logged_in_at: Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap(),
            invalid: false,
        }
    }

    fn requested(key: &str) -> ProxyKeyRequested {
        serde_json::from_value(serde_json::json!({
            "key": key, "status": "pending",
            "proxy_key": { "id": 7, "device_name": "mac", "user": {}, "status": "pending" }
        }))
        .unwrap()
    }

    const URL: &str = "https://mac.tail1.ts.net";
    const KEY2: &str = "ccp_ZyXwVuTsRqPoNmLkJiHgFeDcBa9876543210-_zyxwv";

    #[test]
    fn obtain_key_saves_then_reuses_and_new_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let c = creds();
        let now = Utc::now();
        let no_request = |_: &str| -> Result<ProxyKeyRequested> { panic!("should reuse the saved key") };

        // First run asks the server with the device name and saves the key, 0600.
        let mut asked = None;
        let got = obtain_key(&paths, URL, &c, "mac", false, now, |d: &str| {
            asked = Some(d.to_string());
            Ok::<_, anyhow::Error>(requested(KEY))
        })
        .unwrap();
        assert_eq!(got, Obtained { key: KEY.into(), fresh: true });
        assert_eq!(asked.as_deref(), Some("mac"));
        let saved = load_keys(&paths);
        assert_eq!(saved[URL], SavedKey { key: KEY.into(), server: c.server.clone(), email: c.user.email.clone(), device: "mac".into(), requested_at: now });
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(paths.proxy_keys()).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(paths.proxy_keys().parent(), Some(paths.config_dir.as_path()));
        }

        // Rerunning reuses it without asking.
        assert_eq!(obtain_key(&paths, URL, &c, "mac", false, now, no_request).unwrap(), Obtained { key: KEY.into(), fresh: false });

        // --new asks again and replaces the saved key.
        let got = obtain_key(&paths, URL, &c, "mac", true, now, |_: &str| Ok::<_, anyhow::Error>(requested(KEY2))).unwrap();
        assert_eq!(got, Obtained { key: KEY2.into(), fresh: true });
        assert_eq!(load_keys(&paths)[URL].key, KEY2);

        // Keys are per proxy URL: another proxy gets its own, the first is kept.
        let other = "http://100.101.102.103:8318";
        obtain_key(&paths, other, &c, "mac", false, now, |_: &str| Ok::<_, anyhow::Error>(requested(KEY))).unwrap();
        let saved = load_keys(&paths);
        assert_eq!((saved[URL].key.as_str(), saved[other].key.as_str()), (KEY2, KEY));
    }

    #[test]
    fn saved_key_is_only_reused_for_the_same_login_and_device() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let c = creds();
        save_key(&paths, URL, SavedKey { key: KEY.into(), server: c.server.clone(), email: c.user.email.clone(), device: "mac".into(), requested_at: Utc::now() }).unwrap();
        let keys = load_keys(&paths);
        assert!(saved_key_for(&keys, URL, &c, "mac").is_some());
        assert!(saved_key_for(&keys, "https://other.tail1.ts.net", &c, "mac").is_none());
        assert!(saved_key_for(&keys, URL, &c, "pc").is_none());
        let mut someone_else = c.clone();
        someone_else.user.email = "other@example.com".into();
        assert!(saved_key_for(&keys, URL, &someone_else, "mac").is_none());
        let mut other_server = c.clone();
        other_server.server = "https://cctrack2.test".into();
        assert!(saved_key_for(&keys, URL, &other_server, "mac").is_none());

        // A mangled saved key is not reused; a corrupt file reads as empty.
        save_key(&paths, URL, SavedKey { key: "ccp_bad key".into(), ..keys[URL].clone() }).unwrap();
        assert!(saved_key_for(&load_keys(&paths), URL, &c, "mac").is_none());
        std::fs::write(paths.proxy_keys(), b"{not json").unwrap();
        assert!(load_keys(&paths).is_empty());
    }

    #[test]
    fn obtain_key_propagates_server_errors_and_rejects_odd_keys() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let err = obtain_key(&paths, URL, &creds(), "mac", false, Utc::now(), |_: &str| {
            Err::<ProxyKeyRequested, _>(anyhow::anyhow!("server returned 401: invalid or revoked token"))
        })
        .unwrap_err();
        assert!(err.to_string().contains("401"), "{err}");
        let err = obtain_key(&paths, URL, &creds(), "mac", false, Utc::now(), |_: &str| Ok::<_, anyhow::Error>(requested("cct_nope"))).unwrap_err();
        assert!(err.to_string().contains("unexpected key"), "{err}");
        assert!(load_keys(&paths).is_empty(), "nothing saved on failure");
    }

    fn mock_models(status: usize, body: &str) -> (mockito::ServerGuard, mockito::Mock) {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/v1/models")
            .match_header("authorization", format!("Bearer {KEY}").as_str())
            .with_status(status)
            .with_body(body)
            .expect(1)
            .create();
        (server, m)
    }

    #[test]
    fn check_200_is_approved() {
        let (server, m) = mock_models(200, r#"{"object":"list","data":[{"id":"a"},{"id":"b"}]}"#);
        let got = check(&server.url(), KEY, false, Duration::from_secs(5));
        m.assert();
        assert_eq!(got, Outcome::Approved(Some(2)));
        assert!(got.is_success());
        assert_eq!(got.report(URL, "dev@example.com", "mac", None), "approved, ready (2 models).");
    }

    #[test]
    fn check_403_is_pending() {
        let (server, _m) = mock_models(403, r#"{"error":{"message":"key pending approval"}}"#);
        let got = check(&server.url(), KEY, false, Duration::from_secs(5));
        assert_eq!(got, Outcome::Pending);
        assert!(got.is_success());
        assert!(got.report(URL, "dev@example.com", "mac", None).starts_with("waiting for the admin to approve dev@example.com on mac"));
    }

    #[test]
    fn check_401_is_unknown_unless_just_requested() {
        let body = r#"{"type":"error","error":{"type":"authentication_error","message":"unknown key"}}"#;
        let (server, _m) = mock_models(401, body);
        let got = check(&server.url(), KEY, false, Duration::from_secs(5));
        assert_eq!(got, Outcome::Unknown(Some("unknown key".into())));
        assert!(!got.is_success());
        assert_eq!(
            got.report(URL, "dev@example.com", "mac", None),
            format!("key unknown or revoked (unknown key); run `cctrack proxy connect {URL} --new`")
        );
        assert_eq!(Outcome::Unknown(None).report(URL, "e", "d", None), format!("key unknown or revoked; run `cctrack proxy connect {URL} --new`"));

        // A key requested in this run that the proxy hasn't synced yet is pending.
        let (server, _m) = mock_models(401, body);
        let got = check(&server.url(), KEY, true, Duration::from_secs(5));
        assert_eq!(got, Outcome::NotSyncedYet);
        assert!(got.is_success());
        assert!(got.report(URL, "dev@example.com", "mac", None).contains("waiting for the admin to approve dev@example.com on mac"));
    }

    #[test]
    fn suggested_commands_keep_a_custom_device() {
        assert_eq!(
            Outcome::Unknown(None).report(URL, "e", "Work laptop", Some("Work laptop")),
            format!("key unknown or revoked; run `cctrack proxy connect {URL} --device 'Work laptop' --new`")
        );
        assert!(Outcome::Pending.report(URL, "e", "it's", Some("it's")).contains("--device 'it'\\''s'` again"));
        assert!(Outcome::NotSyncedYet.report(URL, "e", "mini", Some("mini")).contains(&format!("`cctrack proxy connect {URL} --device mini` again")));
    }

    #[cfg(unix)]
    #[test]
    fn reusing_a_saved_key_makes_the_file_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let c = creds();
        save_key(&paths, URL, SavedKey { key: KEY.into(), server: c.server.clone(), email: c.user.email.clone(), device: "mac".into(), requested_at: Utc::now() }).unwrap();
        std::fs::set_permissions(paths.proxy_keys(), std::fs::Permissions::from_mode(0o644)).unwrap();
        let got = obtain_key(&paths, URL, &c, "mac", false, Utc::now(), |_: &str| -> Result<ProxyKeyRequested> { panic!("should reuse") }).unwrap();
        assert!(!got.fresh);
        assert_eq!(std::fs::metadata(paths.proxy_keys()).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn connect_waits_for_another_connect() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let held = lock_file(&paths.proxy_keys_lock(), Duration::ZERO).unwrap().unwrap();
        let released = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = released.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            drop(held);
        });
        obtain_key(&paths, URL, &creds(), "mac", false, Utc::now(), |_: &str| {
            assert!(released.load(std::sync::atomic::Ordering::SeqCst), "requested while another connect held the lock");
            Ok::<_, anyhow::Error>(requested(KEY))
        })
        .unwrap();
        t.join().unwrap();
    }

    #[test]
    fn check_other_status_and_unreachable() {
        let mut server = mockito::Server::new();
        server.mock("GET", "/v1/models").with_status(502).create();
        let got = check(&server.url(), KEY, false, Duration::from_secs(5));
        assert_eq!(got, Outcome::Http(502));
        assert!(!got.is_success());
        assert!(got.report(URL, "e", "d", None).starts_with("unreachable:"));
        let got = check("http://127.0.0.1:1", KEY, false, Duration::from_secs(5));
        assert!(matches!(got, Outcome::Unreachable(_)), "{got:?}");
        assert!(!got.is_success());
        let report = got.report("http://127.0.0.1:1", "e", "d", None);
        assert!(report.starts_with("unreachable:") && !report.contains(KEY), "{report}");
    }
}

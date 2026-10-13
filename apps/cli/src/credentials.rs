//! The CLI's API token and identity.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::paths::Paths;
use crate::state::{lock_file, read_json, write_json_atomic};

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

/// Stored credentials are Invalid when flagged so, or when a revoked marker
/// exists for their token (see [`mark_invalid_bounded`]).
pub fn load(paths: &Paths) -> AuthState {
    match read_json::<Credentials>(&paths.credentials()) {
        Some(c) if c.invalid => AuthState::Invalid(c),
        Some(c) if c.token.is_empty() => AuthState::LoggedOut,
        Some(c) if revoked_marker(paths, &c.token).exists() => AuthState::Invalid(c),
        Some(c) => AuthState::LoggedIn(c),
        None => AuthState::LoggedOut,
    }
}

/// `<state>/revoked-<first 16 hex of sha256(token)>`: records that the server
/// rejected `token` when the credentials file itself could not be updated.
pub fn revoked_marker(paths: &Paths, token: &str) -> PathBuf {
    let hex: String = Sha256::digest(token.as_bytes())[..8].iter().map(|b| format!("{b:02x}")).collect();
    paths.state_dir.join(format!("revoked-{hex}"))
}

/// How long ordinary writers wait for `credentials.lock`.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// Serialises writers of the credentials file (`credentials.lock`), so a
/// stale rejection cannot interleave with saving a new login.
fn with_lock<T>(paths: &Paths, f: impl FnOnce() -> Result<T>) -> Result<T> {
    with_lock_within(paths, LOCK_WAIT, f)
}

fn with_lock_within<T>(paths: &Paths, wait: Duration, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let Some(_lock) = lock_file(&paths.credentials_lock(), wait)? else {
        bail!("timed out waiting for {}", paths.credentials_lock().display());
    };
    f()
}

pub fn save(paths: &Paths, creds: &Credentials) -> Result<()> {
    with_lock(paths, || write_json_atomic(&paths.credentials(), creds, Some(0o600)))
}

pub fn delete(paths: &Paths) -> Result<()> {
    with_lock(paths, || match fs::remove_file(paths.credentials()) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    })
}

/// Records that the server rejected `rejected_token`. Only marks the stored
/// credentials invalid if they still hold that token: a late 401 for a
/// token that has since been replaced (e.g. a background sync started
/// before a new login) must not invalidate the new login.
pub fn mark_invalid(paths: &Paths, rejected_token: &str) -> Result<()> {
    mark_invalid_within(paths, rejected_token, LOCK_WAIT)
}

fn mark_invalid_within(paths: &Paths, rejected_token: &str, wait: Duration) -> Result<()> {
    with_lock_within(paths, wait, || {
        if let Some(mut c) = read_json::<Credentials>(&paths.credentials())
            && !c.invalid
            && c.token == rejected_token
        {
            c.invalid = true;
            write_json_atomic(&paths.credentials(), &c, Some(0o600))?;
        }
        Ok(())
    })
}

/// [`mark_invalid`] for a hook, bounded to about `2 × attempt_wait`: two
/// attempts, each waiting up to `attempt_wait` for the lock. If both fail,
/// writes [`revoked_marker`] for the token instead, which [`load`] honours,
/// so the gate still blocks.
pub fn mark_invalid_bounded(paths: &Paths, rejected_token: &str, attempt_wait: Duration) -> Result<()> {
    if mark_invalid_within(paths, rejected_token, attempt_wait).is_ok() {
        return Ok(());
    }
    std::thread::sleep(Duration::from_millis(100));
    if mark_invalid_within(paths, rejected_token, attempt_wait).is_ok() {
        return Ok(());
    }
    fs::create_dir_all(&paths.state_dir)?;
    fs::write(revoked_marker(paths, rejected_token), b"")?;
    Ok(())
}

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
        mark_invalid(&paths, "cct_abc").unwrap();
        assert!(matches!(load(&paths), AuthState::Invalid(_)));
        delete(&paths).unwrap();
        delete(&paths).unwrap(); // idempotent
        assert!(matches!(load(&paths), AuthState::LoggedOut));
    }

    #[test]
    fn stale_rejection_does_not_invalidate_newer_token() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        save(&paths, &creds()).unwrap();
        // A new login replaced the token before a 401 for the old one arrived.
        let newer = Credentials { token: "cct_new".into(), ..creds() };
        save(&paths, &newer).unwrap();
        mark_invalid(&paths, "cct_abc").unwrap();
        match load(&paths) {
            AuthState::LoggedIn(c) => assert_eq!(c, newer),
            other => panic!("expected the new login to stay valid, got {other:?}"),
        }
        mark_invalid(&paths, "cct_new").unwrap();
        assert!(matches!(load(&paths), AuthState::Invalid(c) if c.token == "cct_new"));
    }

    #[test]
    fn writers_wait_for_the_credentials_lock() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        save(&paths, &creds()).unwrap();
        let held = lock_file(&paths.credentials_lock(), Duration::ZERO).unwrap().unwrap();
        let p2 = paths.clone();
        let t = std::thread::spawn(move || mark_invalid(&p2, "cct_abc"));
        std::thread::sleep(Duration::from_millis(150));
        assert!(matches!(load(&paths), AuthState::LoggedIn(_)), "mark_invalid must wait for the lock");
        drop(held);
        t.join().unwrap().unwrap();
        assert!(matches!(load(&paths), AuthState::Invalid(_)));
    }

    #[test]
    fn bounded_mark_falls_back_to_a_revoked_marker() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        save(&paths, &creds()).unwrap();
        // Lock free: the file itself is marked, no marker.
        mark_invalid_bounded(&paths, "cct_abc", Duration::from_millis(100)).unwrap();
        assert!(matches!(load(&paths), AuthState::Invalid(c) if c.invalid));
        assert!(!revoked_marker(&paths, "cct_abc").exists());

        // Lock held throughout: bounded wait, then the marker.
        save(&paths, &creds()).unwrap();
        let _held = lock_file(&paths.credentials_lock(), Duration::ZERO).unwrap().unwrap();
        let start = std::time::Instant::now();
        mark_invalid_bounded(&paths, "cct_abc", Duration::from_millis(200)).unwrap();
        assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
        let marker = revoked_marker(&paths, "cct_abc");
        assert_eq!(marker.parent(), Some(paths.state_dir.as_path()));
        assert!(marker.file_name().unwrap().to_str().unwrap().strip_prefix("revoked-").is_some_and(|h| h.len() == 16));
        assert!(matches!(load(&paths), AuthState::Invalid(c) if !c.invalid && c.token == "cct_abc"));

        // The marker only covers that token: a newer login is valid.
        drop(_held);
        save(&paths, &Credentials { token: "cct_new".into(), ..creds() }).unwrap();
        assert!(matches!(load(&paths), AuthState::LoggedIn(c) if c.token == "cct_new"));
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

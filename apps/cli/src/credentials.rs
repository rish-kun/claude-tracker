//! The CLI's API token and identity.

use std::fs;
use std::io::ErrorKind;
use std::time::Duration;

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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

pub fn load(paths: &Paths) -> AuthState {
    match read_json::<Credentials>(&paths.credentials()) {
        Some(c) if c.invalid => AuthState::Invalid(c),
        Some(c) if !c.token.is_empty() => AuthState::LoggedIn(c),
        _ => AuthState::LoggedOut,
    }
}

/// Serialises writers of the credentials file (`credentials.lock`), so a
/// stale rejection cannot interleave with saving a new login.
fn with_lock<T>(paths: &Paths, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let Some(_lock) = lock_file(&paths.credentials_lock(), Duration::from_secs(5))? else {
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
    with_lock(paths, || {
        if let AuthState::LoggedIn(mut c) = load(paths)
            && c.token == rejected_token
        {
            c.invalid = true;
            write_json_atomic(&paths.credentials(), &c, Some(0o600))?;
        }
        Ok(())
    })
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
    fn corrupt_file_is_logged_out() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::write(paths.credentials(), b"{not json").unwrap();
        assert!(matches!(load(&paths), AuthState::LoggedOut));
    }
}

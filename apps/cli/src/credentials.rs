//! The CLI's API token and identity.

use std::fs;
use std::io::ErrorKind;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;
use crate::state::{read_json, write_json_atomic};

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

pub fn save(paths: &Paths, creds: &Credentials) -> Result<()> {
    write_json_atomic(&paths.credentials(), creds, Some(0o600))
}

pub fn delete(paths: &Paths) -> Result<()> {
    match fs::remove_file(paths.credentials()) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

pub fn mark_invalid(paths: &Paths) -> Result<()> {
    if let AuthState::LoggedIn(mut c) = load(paths) {
        c.invalid = true;
        save(paths, &c)?;
    }
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
        mark_invalid(&paths).unwrap();
        assert!(matches!(load(&paths), AuthState::Invalid(_)));
        delete(&paths).unwrap();
        delete(&paths).unwrap(); // idempotent
        assert!(matches!(load(&paths), AuthState::LoggedOut));
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

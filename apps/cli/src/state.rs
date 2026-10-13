//! Small JSON state files, written atomically.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Opens (creating it if needed) the lock file at `path` and takes an
/// exclusive lock on it, polling until `wait` elapses. Returns None on
/// timeout. The lock is released when the returned file is dropped.
pub fn lock_file(path: &Path, wait: Duration) -> Result<Option<File>> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let f = OpenOptions::new().create(true).write(true).truncate(false).open(path)?;
    let deadline = Instant::now() + wait;
    loop {
        match f.try_lock() {
            Ok(()) => return Ok(Some(f)),
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Writes to a temp file and renames over `path`. `mode` sets unix permissions
/// (e.g. 0o600 for credentials), also on a leftover temp file from a crash.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T, mode: Option<u32>) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if let Some(m) = mode {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(m);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut f = opts.open(&tmp)?;
    let written = (|| -> Result<()> {
        // `mode()` only applies when the file is created; fix up a leftover.
        #[cfg(unix)]
        if let Some(m) = mode {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(m))?;
        }
        f.write_all(&serde_json::to_vec_pretty(value)?)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LastSession {
    pub session_id: String,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn leftover_tmp_file_does_not_keep_old_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        fs::write(&tmp, "stale").unwrap();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644)).unwrap();

        write_json_atomic(&path, &serde_json::json!({"k": 1}), Some(0o600)).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!tmp.exists());
    }

    #[test]
    fn lock_file_is_exclusive_and_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/x.lock");
        let held = lock_file(&path, Duration::ZERO).unwrap().expect("first lock");
        let start = Instant::now();
        assert!(lock_file(&path, Duration::from_millis(100)).unwrap().is_none());
        assert!(start.elapsed() >= Duration::from_millis(100));
        drop(held);
        assert!(lock_file(&path, Duration::ZERO).unwrap().is_some());
    }
}

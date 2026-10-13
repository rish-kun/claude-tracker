//! All file locations used by cctrack.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub claude_dir: PathBuf,
    pub claude_json: PathBuf,
}

impl Paths {
    pub fn from_env() -> Result<Self> {
        let home = directories::BaseDirs::new()
            .context("cannot determine home directory")?
            .home_dir()
            .to_path_buf();
        let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let config_dir = env("CCTRACK_CONFIG_DIR").unwrap_or_else(|| {
            env("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")).join("cctrack")
        });
        let state_dir = env("CCTRACK_STATE_DIR").unwrap_or_else(|| {
            env("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state")).join("cctrack")
        });
        let (claude_dir, claude_json) = match env("CLAUDE_CONFIG_DIR") {
            Some(d) => (d.clone(), d.join(".claude.json")),
            None => (home.join(".claude"), home.join(".claude.json")),
        };
        Ok(Self { config_dir, state_dir, claude_dir, claude_json })
    }

    /// Every location inside `root`; used by tests.
    pub fn under(root: &Path) -> Self {
        Self {
            config_dir: root.join("config"),
            state_dir: root.join("state"),
            claude_dir: root.join("claude"),
            claude_json: root.join("claude.json"),
        }
    }

    pub fn credentials(&self) -> PathBuf { self.config_dir.join("credentials.json") }
    pub fn credentials_lock(&self) -> PathBuf { self.config_dir.join("credentials.lock") }
    pub fn offsets(&self) -> PathBuf { self.state_dir.join("offsets.json") }
    pub fn sync_lock(&self) -> PathBuf { self.state_dir.join("sync.lock") }
    /// Account timeline for this Claude config dir. Each dir (default vs
    /// `CLAUDE_CONFIG_DIR`) is logged into its own account, so concurrent
    /// sessions in different dirs must not share observations.
    pub fn timeline(&self) -> PathBuf { self.state_dir.join(format!("accounts-{}.json", self.claude_key())) }
    pub fn last_session(&self) -> PathBuf { self.state_dir.join("last_session.json") }
    /// When this config dir's limit snapshot was last uploaded.
    pub fn last_snapshot(&self) -> PathBuf { self.state_dir.join(format!("last_snapshot-{}.json", self.claude_key())) }
    /// Session ids the server deterministically rejected; never re-sent.
    pub fn quarantine(&self) -> PathBuf { self.state_dir.join("quarantine.json") }
    pub fn sync_log(&self) -> PathBuf { self.state_dir.join("sync.log") }
    pub fn projects_dir(&self) -> PathBuf { self.claude_dir.join("projects") }

    /// Stable short key for the Claude config dir: the first 6 bytes of
    /// sha256 of the `.claude.json` path, as hex.
    fn claude_key(&self) -> String {
        let digest = Sha256::digest(self.claude_json.to_string_lossy().as_bytes());
        digest[..6].iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_and_snapshot_are_per_claude_config_dir() {
        let a = Paths::under(Path::new("/tmp/x"));
        let mut b = a.clone();
        b.claude_json = PathBuf::from("/home/u/.claude-b/.claude.json");
        assert_ne!(a.timeline(), b.timeline());
        assert_ne!(a.last_snapshot(), b.last_snapshot());
        // Stable for the same dir, and still under the shared state dir.
        assert_eq!(a.timeline(), Paths::under(Path::new("/tmp/x")).timeline());
        assert_eq!(a.timeline().parent(), Some(a.state_dir.as_path()));
        let name = a.timeline().file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("accounts-") && name.ends_with(".json") && name.len() == "accounts-.json".len() + 12, "{name}");
        // Offsets stay shared: they are keyed by absolute transcript path.
        assert_eq!(a.offsets(), b.offsets());
    }
}

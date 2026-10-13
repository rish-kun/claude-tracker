//! All file locations used by cctrack.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

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
    pub fn offsets(&self) -> PathBuf { self.state_dir.join("offsets.json") }
    pub fn sync_lock(&self) -> PathBuf { self.state_dir.join("sync.lock") }
    pub fn timeline(&self) -> PathBuf { self.state_dir.join("accounts.json") }
    pub fn last_session(&self) -> PathBuf { self.state_dir.join("last_session.json") }
    pub fn last_snapshot(&self) -> PathBuf { self.state_dir.join("last_snapshot.json") }
    pub fn sync_log(&self) -> PathBuf { self.state_dir.join("sync.log") }
    pub fn projects_dir(&self) -> PathBuf { self.claude_dir.join("projects") }
}

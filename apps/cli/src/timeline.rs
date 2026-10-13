//! Which Claude account was active when. Hooks record an observation whenever
//! the signed-in account changes, so a later sync attributes each entry to the
//! account active at its timestamp rather than the account active at sync time.

use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::claude_json::{self, ClaudeState, OAuthAccount};
use crate::paths::Paths;
use crate::state::{read_json, write_json_atomic};

const MAX_OBSERVATIONS: usize = 500;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Observation {
    pub since: DateTime<Utc>,
    /// None means API-key authentication.
    pub account: Option<OAuthAccount>,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Timeline {
    pub observations: Vec<Observation>,
}

fn uuid(a: &Option<OAuthAccount>) -> Option<&str> {
    a.as_ref().map(|a| a.account_uuid.as_str())
}

impl Timeline {
    pub fn load(path: &Path) -> Self {
        read_json(path).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_json_atomic(path, self, None)
    }

    /// Records `account` if it differs from the latest observation.
    pub fn observe(&mut self, now: DateTime<Utc>, account: Option<OAuthAccount>) -> bool {
        if let Some(last) = self.observations.last() {
            if uuid(&last.account) == uuid(&account) {
                return false;
            }
        }
        self.observations.push(Observation { since: now, account });
        if self.observations.len() > MAX_OBSERVATIONS {
            let excess = self.observations.len() - MAX_OBSERVATIONS;
            self.observations.drain(..excess);
        }
        true
    }

    /// The account active at `ts`: the latest observation at or before it, or
    /// the earliest observation for older entries. None if nothing was observed.
    pub fn account_at(&self, ts: DateTime<Utc>) -> Option<Option<&OAuthAccount>> {
        let obs = self.observations.iter().rev().find(|o| o.since <= ts).or(self.observations.first())?;
        Some(obs.account.as_ref())
    }
}

/// Reads the current account from ~/.claude.json and records it. A missing or
/// unparseable file (e.g. a torn read mid-rewrite) records nothing and yields
/// the default state.
pub fn observe_current(paths: &Paths, now: DateTime<Utc>) -> ClaudeState {
    let Some(state) = claude_json::read_checked(&paths.claude_json) else {
        return ClaudeState::default();
    };
    let mut t = Timeline::load(&paths.timeline());
    if t.observe(now, state.account.clone()) {
        let _ = t.save(&paths.timeline());
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn acct(id: &str) -> Option<OAuthAccount> {
        Some(OAuthAccount {
            account_uuid: id.into(),
            organization_uuid: None,
            email_address: None,
            organization_type: None,
            organization_rate_limit_tier: None,
        })
    }

    fn at(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 1, h, 0, 0).unwrap()
    }

    #[test]
    fn observe_dedupes_and_account_at_picks_active() {
        let mut t = Timeline::default();
        assert!(t.account_at(at(5)).is_none());
        assert!(t.observe(at(9), acct("A")));
        assert!(!t.observe(at(10), acct("A")));
        assert!(t.observe(at(12), acct("B")));
        assert!(t.observe(at(14), None));

        assert_eq!(t.account_at(at(8)).unwrap().unwrap().account_uuid, "A"); // before first: earliest
        assert_eq!(t.account_at(at(11)).unwrap().unwrap().account_uuid, "A");
        assert_eq!(t.account_at(at(12)).unwrap().unwrap().account_uuid, "B");
        assert!(t.account_at(at(15)).unwrap().is_none()); // API key period
    }

    #[test]
    fn caps_history() {
        let mut t = Timeline::default();
        for i in 0..(MAX_OBSERVATIONS + 10) {
            t.observe(at(0) + chrono::Duration::minutes(i as i64), acct(&i.to_string()));
        }
        assert_eq!(t.observations.len(), MAX_OBSERVATIONS);
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("accounts.json");
        let mut t = Timeline::default();
        t.observe(at(9), acct("A"));
        t.save(&p).unwrap();
        assert_eq!(Timeline::load(&p).observations, t.observations);
    }

    #[test]
    fn observe_current_skips_unreadable_claude_json() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());

        // Missing ~/.claude.json: nothing recorded.
        assert_eq!(observe_current(&paths, at(8)), ClaudeState::default());
        assert!(Timeline::load(&paths.timeline()).observations.is_empty());

        // Torn write (truncated JSON): nothing recorded, not misread as API key.
        std::fs::write(&paths.claude_json, br#"{"oauthAccount": {"accountUuid": "A", "emailAdd"#).unwrap();
        assert_eq!(observe_current(&paths, at(9)), ClaudeState::default());
        assert!(Timeline::load(&paths.timeline()).observations.is_empty());

        // Valid file: recorded.
        std::fs::write(&paths.claude_json, br#"{"oauthAccount": {"accountUuid": "A"}}"#).unwrap();
        let st = observe_current(&paths, at(10));
        assert_eq!(st.account.unwrap().account_uuid, "A");
        let obs = Timeline::load(&paths.timeline()).observations;
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].since, at(10));

        // Torn again after a valid observation: the A observation stands alone.
        std::fs::write(&paths.claude_json, b"{").unwrap();
        observe_current(&paths, at(11));
        assert_eq!(Timeline::load(&paths.timeline()).observations.len(), 1);
    }
}

//! Per-session sync state (`sessions.json`), written only by syncs (under
//! `sync.lock`): the titles seen in a transcript and the one last sent, the
//! time range of usage uploaded, pool accounts seen for proxied sessions, and
//! what the proxy log says about each session.
//!
//! Titles can sit in bytes an earlier sync already read, so the latest ones
//! are kept here rather than re-read: a title changed later is still sent,
//! on its own if the session has no new usage.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::state::{read_json, write_json_atomic};
use crate::transcript::TitleLine;

/// Sessions untouched for this long are forgotten.
pub const RETENTION: Duration = Duration::days(90);

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct SessionState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_title: Option<String>,
    /// The title the server last accepted for this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_title: Option<String>,
    /// Earliest and latest usage timestamps uploaded from this device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_ts: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ts: Option<DateTime<Utc>>,
    /// How many uploaded entries each pool auth file served (proxied
    /// sessions; counted once the server accepted them).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pool_auths: BTreeMap<String, u64>,
    /// The pool auth file of the latest uploaded entry matched in the log
    /// (CLIProxyAPI keeps a session on one account), and its timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pool_auth: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pool_at: Option<DateTime<Utc>>,
    /// Of the uploaded entries compared with a proxy log, how many ran during
    /// a request of a session with the same first character and model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_evidence: Option<LogEvidence>,
    /// For a session without a hook record or `ANTHROPIC_BASE_URL`: whether
    /// the proxy log showed it proxied. Decided once, then kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inferred: Option<Inferred>,
    /// Why the last sync credited this session to the Claude account rather
    /// than a pool account (None when it was credited from a proxy log).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_proxied: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub touched_at: Option<DateTime<Utc>>,
}

/// Entries compared with a proxy log, and how many had a running request.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct LogEvidence {
    pub matched: u64,
    pub total: u64,
}

/// A decision inferred from the proxy log (see `proxy::infer`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Inferred {
    pub proxied: bool,
    pub matched: u64,
    pub total: u64,
    pub decided_at: DateTime<Utc>,
}

/// The auth with the highest count (ties: the alphabetically first, so the
/// choice is stable).
pub fn top_auth(counts: &BTreeMap<String, u64>) -> Option<&str> {
    let mut best: Option<(&String, u64)> = None;
    for (auth, &n) in counts {
        if best.is_none_or(|(_, b)| n > b) {
            best = Some((auth, n));
        }
    }
    best.map(|(a, _)| a.as_str())
}

impl SessionState {
    /// The session's title: a custom title beats an AI title.
    pub fn title(&self) -> Option<&str> {
        self.custom_title.as_deref().or(self.ai_title.as_deref())
    }

    /// The title, when it differs from the one last sent.
    pub fn pending_title(&self) -> Option<&str> {
        self.title().filter(|t| Some(*t) != self.sent_title.as_deref())
    }

    /// Whether usage of this session was uploaded, so a title-only update
    /// can name a time range the server already has.
    pub fn uploaded_range(&self) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        Some((self.first_ts?, self.last_ts?))
    }

    /// The pool auth file that served most of this session (ties: the
    /// alphabetically first, so the choice is stable).
    pub fn top_pool_auth(&self) -> Option<&str> {
        top_auth(&self.pool_auths)
    }

    /// Records an uploaded entry stamped `ts` that the log says `auth` served.
    pub fn note_pool(&mut self, auth: &str, ts: DateTime<Utc>) {
        *self.pool_auths.entry(auth.to_string()).or_default() += 1;
        if self.last_pool_at.is_none_or(|t| ts >= t) {
            self.last_pool_at = Some(ts);
            self.last_pool_auth = Some(auth.to_string());
        }
    }

    /// Records an uploaded entry compared with a proxy log.
    pub fn note_evidence(&mut self, had_request: bool) {
        let e = self.log_evidence.get_or_insert_default();
        e.total += 1;
        e.matched += u64::from(had_request);
    }
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct SessionStates {
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionState>,
}

impl SessionStates {
    pub fn load(path: &Path) -> Self {
        read_json(path).unwrap_or_default()
    }

    /// Drops sessions untouched for [`RETENTION`] and saves (owner-only: it
    /// holds pool account emails and session titles).
    pub fn save(&mut self, path: &Path, now: DateTime<Utc>) -> Result<()> {
        self.sessions.retain(|_, s| s.touched_at.is_none_or(|t| t + RETENTION >= now));
        write_json_atomic(path, self, Some(0o600))
    }

    pub fn get(&self, session_id: &str) -> Option<&SessionState> {
        self.sessions.get(session_id)
    }

    pub fn touch(&mut self, session_id: &str, now: DateTime<Utc>) -> &mut SessionState {
        let s = self.sessions.entry(session_id.to_string()).or_default();
        s.touched_at = Some(now);
        s
    }

    /// Records a title line (later lines win).
    pub fn note_title(&mut self, t: TitleLine, now: DateTime<Utc>) {
        match t {
            TitleLine::Ai { session_id, title } => self.touch(&session_id, now).ai_title = Some(title),
            TitleLine::Custom { session_id, title } => self.touch(&session_id, now).custom_title = Some(title),
        }
    }

    /// Records that usage of `session_id` stamped `ts` was uploaded.
    pub fn note_uploaded(&mut self, session_id: &str, ts: DateTime<Utc>, now: DateTime<Utc>) {
        let s = self.touch(session_id, now);
        s.first_ts = Some(s.first_ts.map_or(ts, |f| f.min(ts)));
        s.last_ts = Some(s.last_ts.map_or(ts, |l| l.max(ts)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-15T10:00:00Z").unwrap().with_timezone(&Utc)
    }

    fn ai(t: &str) -> TitleLine {
        TitleLine::Ai { session_id: "s".into(), title: t.into() }
    }

    fn custom(t: &str) -> TitleLine {
        TitleLine::Custom { session_id: "s".into(), title: t.into() }
    }

    #[test]
    fn custom_title_beats_ai_and_last_wins() {
        let mut st = SessionStates::default();
        st.note_title(ai("first"), now());
        st.note_title(ai("second"), now());
        assert_eq!(st.get("s").unwrap().title(), Some("second"));
        st.note_title(custom("SOP Report"), now());
        st.note_title(ai("third"), now());
        assert_eq!(st.get("s").unwrap().title(), Some("SOP Report"), "a later AI title does not replace a custom one");
        st.note_title(custom("Renamed"), now());
        assert_eq!(st.get("s").unwrap().title(), Some("Renamed"));
    }

    #[test]
    fn pending_title_until_sent() {
        let mut st = SessionStates::default();
        st.note_title(ai("x"), now());
        assert_eq!(st.get("s").unwrap().pending_title(), Some("x"));
        st.touch("s", now()).sent_title = Some("x".into());
        assert_eq!(st.get("s").unwrap().pending_title(), None);
        st.note_title(ai("x"), now());
        assert_eq!(st.get("s").unwrap().pending_title(), None, "a repeated title is not resent");
        st.note_title(ai("y"), now());
        assert_eq!(st.get("s").unwrap().pending_title(), Some("y"));
    }

    #[test]
    fn uploaded_range_and_pool_counts() {
        let mut st = SessionStates::default();
        assert!(st.get("s").is_none());
        let t = now();
        st.note_uploaded("s", t, t);
        st.note_uploaded("s", t - Duration::hours(1), t);
        st.note_uploaded("s", t + Duration::hours(1), t);
        assert_eq!(st.get("s").unwrap().uploaded_range(), Some((t - Duration::hours(1), t + Duration::hours(1))));
        let s = st.touch("s", t);
        assert_eq!(s.top_pool_auth(), None);
        s.pool_auths.insert("b.json".into(), 3);
        s.pool_auths.insert("a.json".into(), 3);
        s.pool_auths.insert("c.json".into(), 1);
        assert_eq!(s.top_pool_auth(), Some("a.json"));
    }

    #[test]
    fn pool_affinity_and_evidence() {
        let mut st = SessionStates::default();
        let t = now();
        let s = st.touch("s", t);
        s.note_pool("a.json", t);
        s.note_pool("b.json", t - Duration::minutes(1)); // older: affinity stays with a
        assert_eq!(s.last_pool_auth.as_deref(), Some("a.json"));
        s.note_pool("b.json", t + Duration::minutes(1));
        assert_eq!((s.last_pool_auth.as_deref(), s.last_pool_at), (Some("b.json"), Some(t + Duration::minutes(1))));
        assert_eq!(s.pool_auths.get("b.json"), Some(&2));
        s.note_evidence(true);
        s.note_evidence(false);
        assert_eq!(s.log_evidence, Some(LogEvidence { matched: 1, total: 2 }));
    }

    #[test]
    fn roundtrip_and_retention() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.json");
        let mut st = SessionStates::load(&path);
        st.note_title(ai("old"), now() - Duration::days(100));
        st.note_title(TitleLine::Ai { session_id: "new".into(), title: "t".into() }, now());
        st.save(&path, now()).unwrap();
        let back = SessionStates::load(&path);
        assert!(back.get("s").is_none(), "expired");
        assert_eq!(back.get("new").unwrap().title(), Some("t"));
    }

    #[cfg(unix)]
    #[test]
    fn saved_owner_only_even_over_an_older_world_readable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.json");
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let mut st = SessionStates::load(&path);
        st.note_title(ai("t"), now());
        st.save(&path, now()).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

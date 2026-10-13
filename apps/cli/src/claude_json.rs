//! Reads the signed-in Claude account and the cached plan-limit utilisation
//! from Claude Code's config file. Both are undocumented, so parsing is
//! defensive: any unexpected shape yields "unknown", never an error.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OAuthAccount {
    pub account_uuid: String,
    pub organization_uuid: Option<String>,
    pub email_address: Option<String>,
    pub organization_type: Option<String>,
    pub organization_rate_limit_tier: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageSnapshot {
    pub account_uuid: String,
    pub fetched_at: DateTime<Utc>,
    pub five_hour_pct: Option<f64>,
    pub five_hour_resets_at: Option<DateTime<Utc>>,
    pub seven_day_pct: Option<f64>,
    pub seven_day_resets_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClaudeState {
    pub account: Option<OAuthAccount>,
    pub snapshot: Option<UsageSnapshot>,
}

pub fn read(path: &Path) -> ClaudeState {
    std::fs::read(path).map(|b| parse(&b)).unwrap_or_default()
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from)
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

pub fn parse(bytes: &[u8]) -> ClaudeState {
    let Ok(v) = serde_json::from_slice::<Value>(bytes) else {
        return ClaudeState::default();
    };
    let account = v.get("oauthAccount").and_then(|oa| {
        Some(OAuthAccount {
            account_uuid: str_field(oa, "accountUuid")?,
            organization_uuid: str_field(oa, "organizationUuid"),
            email_address: str_field(oa, "emailAddress"),
            organization_type: str_field(oa, "organizationType"),
            organization_rate_limit_tier: str_field(oa, "organizationRateLimitTier"),
        })
    });
    let snapshot = v.get("cachedUsageUtilization").and_then(|cu| {
        let window = |name: &str| cu.get("utilization").and_then(|u| u.get(name));
        let pct = |name: &str| window(name).and_then(|w| w.get("utilization")).and_then(Value::as_f64);
        let reset = |name: &str| {
            window(name).and_then(|w| w.get("resets_at")).and_then(Value::as_str).and_then(parse_ts)
        };
        let fetched_ms = cu.get("fetchedAtMs").and_then(|f| f.as_i64().or_else(|| f.as_f64().map(|x| x as i64)))?;
        let snap = UsageSnapshot {
            account_uuid: str_field(cu, "accountUuid")?,
            fetched_at: DateTime::from_timestamp_millis(fetched_ms)?,
            five_hour_pct: pct("five_hour"),
            five_hour_resets_at: reset("five_hour"),
            seven_day_pct: pct("seven_day"),
            seven_day_resets_at: reset("seven_day"),
        };
        (snap.five_hour_pct.is_some() || snap.seven_day_pct.is_some()).then_some(snap)
    });
    ClaudeState { account, snapshot }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "oauthAccount": {
        "accountUuid": "acc-1", "emailAddress": "shared@example.com", "organizationUuid": "org-1",
        "organizationType": "claude_max", "organizationRateLimitTier": "default_claude_max_20x",
        "displayName": "ignored"
      },
      "cachedUsageUtilization": {
        "fetchedAtMs": 1791481909879, "accountUuid": "acc-1",
        "utilization": {
          "five_hour": {"utilization": 22, "resets_at": "2026-10-13T05:00:00+00:00"},
          "seven_day": {"utilization": 10.5, "resets_at": "2026-10-17T00:00:00Z"},
          "iguana_necktie": {"utilization": 80.6}
        }
      }
    }"#;

    #[test]
    fn parses_account_and_snapshot() {
        let st = parse(SAMPLE.as_bytes());
        let a = st.account.unwrap();
        assert_eq!(a.account_uuid, "acc-1");
        assert_eq!(a.organization_rate_limit_tier.as_deref(), Some("default_claude_max_20x"));
        let s = st.snapshot.unwrap();
        assert_eq!(s.account_uuid, "acc-1");
        assert_eq!(s.five_hour_pct, Some(22.0));
        assert_eq!(s.seven_day_pct, Some(10.5));
        assert_eq!(s.fetched_at.timestamp_millis(), 1791481909879);
        assert_eq!(s.five_hour_resets_at.unwrap().to_rfc3339(), "2026-10-13T05:00:00+00:00");
    }

    #[test]
    fn tolerates_missing_and_malformed() {
        assert_eq!(parse(b"not json"), ClaudeState::default());
        assert_eq!(parse(b"{}"), ClaudeState::default());
        let st = parse(br#"{"oauthAccount": {"emailAddress": "x"}, "cachedUsageUtilization": {"accountUuid": "a"}}"#);
        assert!(st.account.is_none(), "account without uuid is ignored");
        assert!(st.snapshot.is_none(), "snapshot without fetchedAtMs/percentages is ignored");
        let st = parse(br#"{"cachedUsageUtilization": {"accountUuid": "a", "fetchedAtMs": 1, "utilization": {"five_hour": {"utilization": "high"}}}}"#);
        assert!(st.snapshot.is_none(), "non-numeric utilization is ignored");
        assert_eq!(read(std::path::Path::new("/definitely/missing.json")), ClaudeState::default());
    }
}

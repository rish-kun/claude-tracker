//! Request/response types for the cctrack API (api/openapi.yaml).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::credentials::UserInfo;

pub const API_KEY_ACCOUNT: &str = "api_key";

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Device {
    pub hostname: String,
    pub os: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct IngestAccount {
    pub account_uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_uuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_tier: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct IngestSession {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    /// The session's latest title (custom beats AI); never prompt text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct IngestEntry {
    pub session_id: String,
    pub account_uuid: String,
    pub message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub model: String,
    pub ts: DateTime<Utc>,
    pub input: i64,
    pub output: i64,
    pub cache_write_5m: i64,
    pub cache_write_1h: i64,
    pub cache_read: i64,
    pub speed: String,
    pub is_subagent: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LimitSnapshotIn {
    pub account_uuid: String,
    pub captured_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour_resets_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seven_day_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seven_day_resets_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct IngestRequest {
    pub device: Device,
    pub accounts: Vec<IngestAccount>,
    pub sessions: Vec<IngestSession>,
    pub entries: Vec<IngestEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_snapshot: Option<LimitSnapshotIn>,
}

#[derive(Deserialize, Debug)]
pub struct IngestResponse {
    pub accepted: i64,
    pub duplicates: i64,
}

#[derive(Deserialize, Debug)]
pub struct Me {
    pub id: i64,
    pub email: String,
    pub name: String,
    pub is_admin: bool,
}

#[derive(Serialize, Debug)]
pub struct ExchangeRequest<'a> {
    pub code: &'a str,
    pub code_verifier: &'a str,
    pub device_name: &'a str,
}

#[derive(Deserialize, Debug)]
pub struct ExchangeResponse {
    pub token: String,
    pub user: UserInfo,
}

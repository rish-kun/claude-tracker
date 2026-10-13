//! Blocking HTTP client for the cctrack API.

use std::fmt;
use std::time::Duration;

use reqwest::Url;
use reqwest::blocking::{Client, RequestBuilder};
use serde::de::DeserializeOwned;

use crate::wire::{ExchangeRequest, ExchangeResponse, IngestRequest, IngestResponse, Me};

#[derive(Debug)]
pub enum ApiError {
    Unauthorized(String),
    NotFound(String),
    Http(u16, String),
    Network(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Unauthorized(m) => write!(f, "server returned 401: {m}"),
            ApiError::NotFound(m) => write!(f, "server returned 404: {m}"),
            ApiError::Http(code, m) => write!(f, "server returned {code}: {m}"),
            ApiError::Network(m) => write!(f, "network error: {m}"),
        }
    }
}

impl std::error::Error for ApiError {}

pub struct ApiClient {
    base: String,
    token: Option<String>,
    http: Client,
}

impl ApiClient {
    pub fn new(server: &str, token: Option<&str>) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("cctrack/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("build HTTP client");
        Self { base: server.trim_end_matches('/').to_string(), token: token.map(String::from), http }
    }

    fn url(&self, segments: &[&str]) -> Result<Url, ApiError> {
        let mut url = Url::parse(&self.base).map_err(|e| ApiError::Network(format!("bad server URL: {e}")))?;
        url.path_segments_mut()
            .map_err(|_| ApiError::Network("bad server URL".into()))?
            .pop_if_empty()
            .push("api")
            .extend(segments);
        Ok(url)
    }

    fn authed(&self, rb: RequestBuilder) -> RequestBuilder {
        match &self.token {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    /// Sends a request; returns the parsed body, or None for 204.
    fn send<T: DeserializeOwned>(&self, rb: RequestBuilder) -> Result<Option<T>, ApiError> {
        let resp = self.authed(rb).send().map_err(|e| ApiError::Network(e.to_string()))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(None);
        }
        let text = resp.text().unwrap_or_default();
        if status.is_success() {
            return serde_json::from_str(&text).map(Some).map_err(|e| ApiError::Http(status.as_u16(), e.to_string()));
        }
        let msg = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
            .unwrap_or(text);
        Err(match status.as_u16() {
            401 => ApiError::Unauthorized(msg),
            404 => ApiError::NotFound(msg),
            code => ApiError::Http(code, msg),
        })
    }

    pub fn ingest(&self, body: &IngestRequest) -> Result<IngestResponse, ApiError> {
        let url = self.url(&["ingest"])?;
        self.send(self.http.post(url).json(body))?.ok_or_else(|| ApiError::Http(204, "empty response".into()))
    }

    pub fn set_summary(&self, session_id: &str, summary: &str) -> Result<(), ApiError> {
        let url = self.url(&["sessions", session_id, "summary"])?;
        self.send::<serde_json::Value>(self.http.post(url).json(&serde_json::json!({ "summary": summary })))?;
        Ok(())
    }

    pub fn me(&self) -> Result<Me, ApiError> {
        let url = self.url(&["me"])?;
        self.send(self.http.get(url))?.ok_or_else(|| ApiError::Http(204, "empty response".into()))
    }

    pub fn exchange(&self, code: &str, verifier: &str, device: &str) -> Result<ExchangeResponse, ApiError> {
        let url = self.url(&["auth", "cli", "exchange"])?;
        let body = ExchangeRequest { code, code_verifier: verifier, device_name: device };
        self.send(self.http.post(url).json(&body))?.ok_or_else(|| ApiError::Http(204, "empty response".into()))
    }

    pub fn revoke_own_token(&self) -> Result<(), ApiError> {
        let url = self.url(&["auth", "token"])?;
        self.send::<serde_json::Value>(self.http.delete(url))?;
        Ok(())
    }
}

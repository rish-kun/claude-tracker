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
    /// HTTP 426: the server refuses this cctrack version until it is updated.
    /// `message` is the server's explanation; `latest` and `setup_url` come
    /// from the same response when it has them.
    Outdated { message: String, latest: Option<String>, setup_url: Option<String> },
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Unauthorized(m) => write!(f, "server returned 401: {m}"),
            ApiError::NotFound(m) => write!(f, "server returned 404: {m}"),
            ApiError::Http(code, m) => write!(f, "server returned {code}: {m}"),
            ApiError::Network(m) => write!(f, "network error: {m}"),
            ApiError::Outdated { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for ApiError {}

/// Whether credentials may be sent to `url`: https anywhere, plain http only
/// to a loopback host (local development).
pub fn is_secure_server(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        // The URL parser lowercases domains and normalises IPs ("127.1" → "127.0.0.1").
        "http" => matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        _ => false,
    }
}

/// The message for a 426 whose body has no `error` (e.g. from a proxy).
fn outdated_fallback(body: &str) -> String {
    let body = body.trim();
    let detail = if body.is_empty() { String::new() } else { format!(": {}", body.chars().take(200).collect::<String>()) };
    format!(
        "server returned 426{detail}. cctrack {} is out of date: update cctrack (see the \"Update cctrack\" section of the setup.md attached to the latest release) and try again.",
        env!("CARGO_PKG_VERSION")
    )
}

pub struct ApiClient {
    base: String,
    token: Option<String>,
    http: Client,
}

impl ApiClient {
    pub fn new(server: &str, token: Option<&str>) -> Self {
        Self::with_timeout(server, token, Duration::from_secs(30))
    }

    /// A client whose every request (connect, send and read) must finish
    /// within `timeout`.
    pub fn with_timeout(server: &str, token: Option<&str>, timeout: Duration) -> Self {
        let http = Client::builder()
            .timeout(timeout)
            .user_agent(concat!("cctrack/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("build HTTP client");
        Self { base: server.trim_end_matches('/').to_string(), token: token.map(String::from), http }
    }

    fn url(&self, segments: &[&str]) -> Result<Url, ApiError> {
        let mut url = Url::parse(&self.base).map_err(|e| ApiError::Network(format!("bad server URL: {e}")))?;
        if !is_secure_server(&url) {
            return Err(ApiError::Network(format!(
                "refusing to send credentials to {} over plain http; use https (http is only allowed for localhost)",
                self.base
            )));
        }
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
        let body = serde_json::from_str::<serde_json::Value>(&text).ok();
        let field = |k: &str| body.as_ref().and_then(|v| v.get(k)).and_then(|e| e.as_str()).map(String::from);
        let msg = field("error");
        Err(match status.as_u16() {
            401 => ApiError::Unauthorized(msg.unwrap_or(text)),
            404 => ApiError::NotFound(msg.unwrap_or(text)),
            426 => ApiError::Outdated {
                message: msg.unwrap_or_else(|| outdated_fallback(&text)),
                latest: field("latest"),
                setup_url: field("setup_url"),
            },
            code => ApiError::Http(code, msg.unwrap_or(text)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_http_only_for_loopback() {
        for ok in ["https://c.example.com", "http://localhost:3000", "http://127.0.0.1:9", "http://[::1]:8080", "http://LOCALHOST", "http://127.1"] {
            assert!(is_secure_server(&Url::parse(ok).unwrap()), "{ok}");
        }
        for bad in ["http://c.example.com", "http://10.0.0.5", "http://127.0.0.2", "http://localhost.evil.com", "http://[::2]", "ftp://c.example.com"] {
            assert!(!is_secure_server(&Url::parse(bad).unwrap()), "{bad}");
        }
    }

    #[test]
    fn http_426_is_outdated_with_the_servers_message() {
        let mut server = mockito::Server::new();
        let msg = "cctrack 0.2.0 is out of date: the latest is 0.3.0. Update cctrack and try again.";
        let setup = "https://github.com/rish-kun/claude-tracker/releases/download/cli-v0.3.0/setup.md";
        let body = serde_json::json!({ "error": msg, "latest": "0.3.0", "setup_url": setup });
        let m = server
            .mock("GET", "/api/me")
            .match_header("user-agent", concat!("cctrack/", env!("CARGO_PKG_VERSION")))
            .with_status(426)
            .with_body(body.to_string())
            .expect(1)
            .create();
        let err = ApiClient::new(&server.url(), Some("cct_x")).me().unwrap_err();
        m.assert();
        assert_eq!(err.to_string(), msg);
        let ApiError::Outdated { latest, setup_url, .. } = err else { panic!("{err:?}") };
        assert_eq!((latest.as_deref(), setup_url.as_deref()), (Some("0.3.0"), Some(setup)));
    }

    #[test]
    fn http_426_without_a_json_body_still_says_to_update() {
        let mut server = mockito::Server::new();
        server.mock("DELETE", "/api/auth/token").with_status(426).with_body("Upgrade Required").create();
        let err = ApiClient::new(&server.url(), Some("cct_x")).revoke_own_token().unwrap_err();
        let ApiError::Outdated { message, latest: None, setup_url: None } = &err else { panic!("{err:?}") };
        assert!(message.contains("426: Upgrade Required") && message.contains("out of date") && message.contains("Update cctrack"), "{message}");
        assert_eq!(err.to_string(), *message);
    }

    #[test]
    fn client_refuses_plain_http_to_remote_hosts() {
        let err = ApiClient::new("http://c.example.com", Some("cct_x")).me().unwrap_err();
        assert!(matches!(&err, ApiError::Network(m) if m.contains("plain http")), "{err}");
    }
}

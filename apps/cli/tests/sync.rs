use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use serde_json::Value;

use cctrack::client::ApiClient;
use cctrack::credentials::{self, AuthState, Credentials, UserInfo};
use cctrack::paths::Paths;
use cctrack::sync::{self, SyncOptions};

const CLAUDE_JSON: &str = r#"{
  "oauthAccount": {"accountUuid": "acc-1", "emailAddress": "shared@example.com",
                   "organizationType": "claude_max", "organizationRateLimitTier": "default_claude_max_20x"},
  "cachedUsageUtilization": {"fetchedAtMs": 1791481909879, "accountUuid": "acc-1",
    "utilization": {"five_hour": {"utilization": 22, "resets_at": "2026-10-13T05:00:00Z"},
                    "seven_day": {"utilization": 10, "resets_at": "2026-10-17T00:00:00Z"}}}
}"#;

fn assistant(id: &str, ts: &str, input: i64) -> String {
    format!(
        r#"{{"type":"assistant","sessionId":"sess-1","timestamp":"{ts}","cwd":"/home/dev/proj","gitBranch":"main","message":{{"id":"{id}","model":"claude-opus-5-5","usage":{{"input_tokens":{input},"output_tokens":5}}}}}}"#
    )
}

struct Fixture {
    _dir: tempfile::TempDir,
    paths: Paths,
    transcript: std::path::PathBuf,
    creds: Credentials,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    fs::write(&paths.claude_json, CLAUDE_JSON).unwrap();
    let proj = paths.projects_dir().join("-home-dev-proj");
    fs::create_dir_all(proj.join("sess-1/subagents")).unwrap();
    let transcript = proj.join("sess-1.jsonl");
    let lines = [
        r#"{"type":"user","sessionId":"sess-1","message":{"content":"secret prompt"}}"#.to_string(),
        assistant("msg_1", "2026-10-01T10:00:00Z", 100),
        assistant("msg_1", "2026-10-01T10:00:00Z", 100), // same response, second content block
        assistant("msg_old", "2026-09-30T10:00:00Z", 999), // before login
        r#"{"type":"assistant","sessionId":"sess-1","timestamp":"2026-10-01T10:01:00Z","message":{"id":"x","model":"<synthetic>","usage":{"input_tokens":0}}}"#.to_string(),
    ];
    fs::write(&transcript, lines.join("\n") + "\n" + &assistant("msg_4", "2026-10-01T10:05:00Z", 4)).unwrap(); // msg_4 has no trailing newline yet
    fs::write(proj.join("sess-1/subagents/agent-a.jsonl"), assistant("msg_3", "2026-10-01T10:02:00Z", 3) + "\n").unwrap();

    let creds = Credentials {
        server: "http://unused".into(),
        token: "cct_test".into(),
        user: UserInfo { id: 1, email: "dev@example.com".into(), name: "Dev".into() },
        logged_in_at: Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
        invalid: false,
    };
    credentials::save(&paths, &creds).unwrap();
    Fixture { _dir: dir, paths, transcript, creds }
}

fn opts() -> SyncOptions {
    SyncOptions { transcript: None, wait_for_lock: Duration::from_secs(0) }
}

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap()
}

#[test]
fn sync_uploads_new_entries_once_and_resumes() {
    let fx = fixture();
    let mut server = mockito::Server::new();
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sink = bodies.clone();
    let mock = server
        .mock("POST", "/api/ingest")
        .match_header("authorization", "Bearer cct_test")
        .with_status(200)
        .with_body_from_request(move |req| {
            sink.lock().unwrap().push(serde_json::from_slice(req.body().unwrap()).unwrap());
            br#"{"accepted":2,"duplicates":0}"#.to_vec()
        })
        .expect(2)
        .create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));

    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 2);

    {
        let bodies = bodies.lock().unwrap();
        let body = &bodies[0];
        let ids: Vec<&str> = body["entries"].as_array().unwrap().iter().map(|e| e["message_id"].as_str().unwrap()).collect();
        assert_eq!(ids.len(), 2, "msg_1 once + msg_3; got {ids:?}");
        assert!(ids.contains(&"msg_1") && ids.contains(&"msg_3"));
        let sub = body["entries"].as_array().unwrap().iter().find(|e| e["message_id"] == "msg_3").unwrap();
        assert_eq!(sub["is_subagent"], true);
        assert_eq!(sub["session_id"], "sess-1");
        assert!(body["entries"].as_array().unwrap().iter().all(|e| e["account_uuid"] == "acc-1"));
        assert_eq!(body["accounts"][0]["account_uuid"], "acc-1");
        assert_eq!(body["accounts"][0]["rate_limit_tier"], "default_claude_max_20x");
        assert_eq!(body["sessions"][0]["session_id"], "sess-1");
        assert_eq!(body["sessions"][0]["cwd"], "/home/dev/proj");
        assert_eq!(body["limit_snapshot"]["five_hour_pct"], 22.0);
        assert!(!body.to_string().contains("secret prompt"), "no transcript content may be uploaded");
    }

    // Nothing new: no request.
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 0);

    // Completing the partial line makes msg_4 available; snapshot is not re-sent.
    let mut content = fs::read_to_string(&fx.transcript).unwrap();
    content.push('\n');
    fs::write(&fx.transcript, content).unwrap();
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 1);
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies[1]["entries"][0]["message_id"], "msg_4");
    assert!(bodies[1].get("limit_snapshot").is_none());
    mock.assert();
}

#[test]
fn unauthorized_marks_credentials_invalid_and_keeps_offsets() {
    let fx = fixture();
    let mut server = mockito::Server::new();
    let mock = server.mock("POST", "/api/ingest").with_status(401).with_body(r#"{"error":"invalid or revoked token"}"#).expect(2).create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));

    assert!(sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).is_err());
    assert!(matches!(credentials::load(&fx.paths), AuthState::Invalid(_)));
    // Offsets did not advance: the same entries are attempted again.
    let err = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap_err();
    assert!(err.to_string().contains("cctrack login"));
    mock.assert();
}

#[test]
fn concurrent_sync_skips_when_locked() {
    let fx = fixture();
    fs::create_dir_all(&fx.paths.state_dir).unwrap();
    let held = fs::OpenOptions::new().create(true).write(true).truncate(false).open(fx.paths.sync_lock()).unwrap();
    held.lock().unwrap();
    let client = ApiClient::new("http://127.0.0.1:9", Some("cct_test")); // never contacted
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert!(report.skipped_locked);
}

#[test]
fn entries_attributed_to_account_active_at_their_time() {
    use cctrack::claude_json::{ClaudeState, OAuthAccount};
    use cctrack::timeline::Timeline;
    use cctrack::transcript::parse_line;
    use cctrack::wire::Device;

    let acct = |id: &str| Some(OAuthAccount {
        account_uuid: id.into(), organization_uuid: None, email_address: None,
        organization_type: None, organization_rate_limit_tier: None,
    });
    let mut t = Timeline::default();
    t.observe(Utc.with_ymd_and_hms(2026, 10, 1, 9, 0, 0).unwrap(), acct("A"));
    t.observe(Utc.with_ymd_and_hms(2026, 10, 1, 11, 0, 0).unwrap(), acct("B"));
    let early = parse_line(&assistant("m1", "2026-10-01T10:00:00Z", 1), false, "h").unwrap();
    let late = parse_line(&assistant("m2", "2026-10-01T11:30:00Z", 1), false, "h").unwrap();
    let current = ClaudeState { account: acct("B"), snapshot: None };
    let device = Device { hostname: "h".into(), os: "macos".into() };

    let req = sync::build_request(&device, &[early, late], &t, &current, None);
    assert_eq!(req.entries[0].account_uuid, "A");
    assert_eq!(req.entries[1].account_uuid, "B");
    assert_eq!(req.accounts.len(), 2);

    // No timeline and no oauthAccount: API key.
    let req = sync::build_request(&device, &[parse_line(&assistant("m3", "2026-10-01T10:00:00Z", 1), false, "h").unwrap()],
        &Timeline::default(), &ClaudeState::default(), None);
    assert_eq!(req.entries[0].account_uuid, "api_key");
}

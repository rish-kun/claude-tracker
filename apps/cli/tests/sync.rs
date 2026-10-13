use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use serde_json::Value;

use cctrack::client::ApiClient;
use cctrack::credentials::{self, AuthState, Credentials, UserInfo};
use cctrack::paths::Paths;
use cctrack::scan::Offsets;
use cctrack::sync::{self, SyncOptions};
use cctrack::timeline::Timeline;

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

fn subagent_path(fx: &Fixture) -> std::path::PathBuf {
    fx.transcript.parent().unwrap().join("sess-1/subagents/agent-a.jsonl")
}

fn line(session: &str, id: &str, ts: &str) -> String {
    format!(
        r#"{{"type":"assistant","sessionId":"{session}","timestamp":"{ts}","message":{{"id":"{id}","model":"claude-opus-5-5","usage":{{"input_tokens":1,"output_tokens":1}}}}}}"#
    )
}

/// A mock /api/ingest that records every request body and accepts it.
fn recording_server(expect: usize) -> (mockito::ServerGuard, mockito::Mock, Arc<Mutex<Vec<Value>>>) {
    let mut server = mockito::Server::new();
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sink = bodies.clone();
    let mock = server
        .mock("POST", "/api/ingest")
        .with_status(200)
        .with_body_from_request(move |req| {
            let body: Value = serde_json::from_slice(req.body().unwrap()).unwrap();
            let n = body["entries"].as_array().unwrap().len();
            sink.lock().unwrap().push(body);
            format!(r#"{{"accepted":{n},"duplicates":0}}"#).into_bytes()
        })
        .expect(expect)
        .create();
    (server, mock, bodies)
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
    let offsets = Offsets::load(&fx.paths.offsets());
    assert_eq!(offsets.get(&fx.transcript).offset, 0, "main transcript offset must not advance");
    assert_eq!(offsets.get(&subagent_path(&fx)).offset, 0, "subagent offset must not advance");
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

#[test]
fn batches_respect_server_session_limit() {
    let fx = fixture();
    // One file whose 520 entries each belong to a different session (e.g. `claude -p` runs),
    // so the batch must be split inside the file.
    let many = fx.paths.projects_dir().join("-home-dev-auto/auto.jsonl");
    fs::create_dir_all(many.parent().unwrap()).unwrap();
    let lines: Vec<String> = (0..520).map(|i| line(&format!("auto-{i}"), &format!("auto_msg_{i}"), "2026-10-01T10:00:00Z")).collect();
    fs::write(&many, lines.join("\n") + "\n").unwrap();

    let (server, mock, bodies) = recording_server(2);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.batches, 2);
    assert_eq!(report.entries_sent, 522, "520 auto entries + msg_1 + msg_3");

    {
        let bodies = bodies.lock().unwrap();
        let mut ids = std::collections::HashSet::new();
        for b in bodies.iter() {
            assert!(b["sessions"].as_array().unwrap().len() <= sync::MAX_BATCH_SESSIONS);
            assert!(b["accounts"].as_array().unwrap().len() <= sync::MAX_BATCH_ACCOUNTS);
            for e in b["entries"].as_array().unwrap() {
                assert!(ids.insert(e["message_id"].as_str().unwrap().to_string()), "entry sent twice");
            }
        }
        assert!((0..520).all(|i| ids.contains(&format!("auto_msg_{i}"))));
    }
    let offsets = Offsets::load(&fx.paths.offsets());
    assert_eq!(offsets.get(&many).offset, fs::metadata(&many).unwrap().len());

    // Everything was committed: a second run sends nothing.
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 0);
    mock.assert();
}

#[test]
fn batches_respect_server_account_limit() {
    use cctrack::claude_json::OAuthAccount;
    let fx = fixture();
    // 30 accounts, each active for one minute, with one entry in each minute.
    let base = Utc.with_ymd_and_hms(2026, 10, 1, 11, 0, 0).unwrap();
    let mut t = Timeline::default();
    let mut lines = Vec::new();
    for i in 0..30 {
        let since = base + chrono::Duration::minutes(i);
        t.observe(since, Some(OAuthAccount {
            account_uuid: format!("acct-{i}"), organization_uuid: None, email_address: None,
            organization_type: None, organization_rate_limit_tier: None,
        }));
        lines.push(line("sess-many", &format!("acct_msg_{i}"), &(since + chrono::Duration::seconds(30)).to_rfc3339()));
    }
    t.save(&fx.paths.timeline()).unwrap();
    let f = fx.paths.projects_dir().join("-home-dev-many/sess-many.jsonl");
    fs::create_dir_all(f.parent().unwrap()).unwrap();
    fs::write(&f, lines.join("\n") + "\n").unwrap();

    let (server, mock, bodies) = recording_server(2);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 32);
    let bodies = bodies.lock().unwrap();
    let mut accounts = std::collections::HashSet::new();
    for b in bodies.iter() {
        assert!(b["accounts"].as_array().unwrap().len() <= sync::MAX_BATCH_ACCOUNTS);
        for e in b["entries"].as_array().unwrap() {
            accounts.insert(e["account_uuid"].as_str().unwrap().to_string());
        }
    }
    assert!((0..30).all(|i| accounts.contains(&format!("acct-{i}"))));
    assert_eq!(Offsets::load(&fx.paths.offsets()).get(&f).offset, fs::metadata(&f).unwrap().len());
    mock.assert();
}

#[test]
fn torn_claude_json_records_no_observation_and_sends_no_snapshot() {
    let fx = fixture();
    let (server, mock, bodies) = recording_server(2);
    let client = ApiClient::new(&server.url(), Some("cct_test"));

    sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    let obs = Timeline::load(&fx.paths.timeline()).observations;
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].account.as_ref().unwrap().account_uuid, "acc-1");

    // Claude Code is mid-rewrite: the file is truncated. Forget the sent snapshot
    // so a stale one would be sent if the torn read were trusted.
    fs::write(&fx.paths.claude_json, &CLAUDE_JSON[..40]).unwrap();
    fs::remove_file(fx.paths.last_snapshot()).unwrap();
    let mut content = fs::read_to_string(&fx.transcript).unwrap();
    content.push('\n'); // completes msg_4
    fs::write(&fx.transcript, content).unwrap();

    let later = now() + chrono::Duration::hours(1);
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), later).unwrap();
    assert_eq!(report.entries_sent, 1);

    let obs = Timeline::load(&fx.paths.timeline()).observations;
    assert_eq!(obs.len(), 1, "torn read must not add an observation");
    assert!(obs.iter().all(|o| o.account.is_some()), "torn read must not be recorded as API key");
    let bodies = bodies.lock().unwrap();
    assert!(bodies[1].get("limit_snapshot").is_none());
    assert_eq!(bodies[1]["entries"][0]["message_id"], "msg_4");
    assert_eq!(bodies[1]["entries"][0]["account_uuid"], "acc-1", "attributed via the timeline");
    mock.assert();
}

/// A mock /api/ingest that refuses (403) any request containing session "bad",
/// accepts everything else, and records every request body.
fn refusing_server(expect: usize) -> (mockito::ServerGuard, mockito::Mock, Arc<Mutex<Vec<Value>>>) {
    let mut server = mockito::Server::new();
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sink = bodies.clone();
    let has_bad = |req: &mockito::Request| {
        let body: Value = serde_json::from_slice(req.body().unwrap()).unwrap();
        body["sessions"].as_array().unwrap().iter().any(|s| s["session_id"] == "bad")
    };
    let mock = server
        .mock("POST", "/api/ingest")
        .with_status_code_from_request(move |req| if has_bad(req) { 403 } else { 200 })
        .with_body_from_request(move |req| {
            let body: Value = serde_json::from_slice(req.body().unwrap()).unwrap();
            let n = body["entries"].as_array().unwrap().len();
            let bad = has_bad(req);
            sink.lock().unwrap().push(body);
            if bad {
                br#"{"error":"a session in this batch belongs to another user"}"#.to_vec()
            } else {
                format!(r#"{{"accepted":{n},"duplicates":0}}"#).into_bytes()
            }
        })
        .expect(expect)
        .create();
    (server, mock, bodies)
}

#[test]
fn deterministic_rejection_quarantines_only_the_offending_session() {
    let fx = fixture();
    let bad = fx.paths.projects_dir().join("-home-dev-other/bad.jsonl");
    fs::create_dir_all(bad.parent().unwrap()).unwrap();
    fs::write(&bad, [line("bad", "bad_1", "2026-10-01T10:00:00Z"), line("bad", "bad_2", "2026-10-01T10:01:00Z")].join("\n") + "\n").unwrap();

    // Batch (403) → "bad" alone (403) → "sess-1" alone (200) → snapshot alone (200).
    let (server, mock, bodies) = refusing_server(4);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.quarantined, vec!["bad".to_string()]);
    assert_eq!(report.accepted, 2, "msg_1 + msg_3 accepted");
    {
        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 4);
        let ok: Vec<&Value> = bodies.iter().filter(|b| b["sessions"].as_array().unwrap().iter().all(|s| s["session_id"] != "bad")).collect();
        let ids: Vec<&str> = ok.iter().flat_map(|b| b["entries"].as_array().unwrap()).map(|e| e["message_id"].as_str().unwrap()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"msg_1") && ids.contains(&"msg_3"));
        assert!(ok.iter().any(|b| b.get("limit_snapshot").is_some()), "snapshot still delivered");
    }

    // Offsets advanced for every file: accepted or quarantined.
    let offsets = Offsets::load(&fx.paths.offsets());
    assert_eq!(offsets.get(&bad).offset, fs::metadata(&bad).unwrap().len());
    assert_eq!(offsets.get(&subagent_path(&fx)).offset, fs::metadata(subagent_path(&fx)).unwrap().len());
    assert!(offsets.get(&fx.transcript).offset > 0);

    let q = sync::Quarantine::load(&fx.paths.quarantine());
    let scope = sync::Quarantine::scope(&fx.creds);
    assert_eq!(scope, "http://unused|1");
    assert!(q.contains(&scope, "bad") && !q.contains(&scope, "sess-1"));
    let raw: Value = serde_json::from_str(&fs::read_to_string(fx.paths.quarantine()).unwrap()).unwrap();
    assert_eq!(raw, serde_json::json!({ "http://unused|1": ["bad"] }));
    let log = fs::read_to_string(fx.paths.sync_log()).unwrap();
    assert!(log.contains("quarantined session bad") && log.contains("HTTP 403"), "{log}");

    // Nothing is retried; later entries of the quarantined session are skipped too.
    fs::write(&bad, fs::read_to_string(&bad).unwrap() + &line("bad", "bad_3", "2026-10-01T11:00:00Z") + "\n").unwrap();
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 0);
    assert_eq!(report.batches, 0);
    assert_eq!(Offsets::load(&fx.paths.offsets()).get(&bad).offset, fs::metadata(&bad).unwrap().len());
    mock.assert();
}

#[test]
fn server_error_keeps_offsets_and_quarantines_nothing() {
    let fx = fixture();
    let mut server = mockito::Server::new();
    let mock = server.mock("POST", "/api/ingest").with_status(500).with_body(r#"{"error":"db down"}"#).expect(1).create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));

    let err = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap_err();
    assert!(err.to_string().contains("500"), "{err}");
    let offsets = Offsets::load(&fx.paths.offsets());
    assert_eq!(offsets.get(&fx.transcript).offset, 0);
    assert_eq!(offsets.get(&subagent_path(&fx)).offset, 0);
    assert!(!fx.paths.quarantine().exists());
    assert!(!fx.paths.last_snapshot().exists());
    mock.assert();
}

#[test]
fn stale_unauthorized_does_not_invalidate_a_newer_login() {
    let fx = fixture();
    let mut server = mockito::Server::new();
    let mock = server.mock("POST", "/api/ingest").with_status(401).with_body(r#"{"error":"invalid or revoked token"}"#).expect(1).create();
    // The sync started with cct_test, but the user has since logged in again.
    let newer = Credentials { token: "cct_newer".into(), ..fx.creds.clone() };
    credentials::save(&fx.paths, &newer).unwrap();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    assert!(sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).is_err());
    match credentials::load(&fx.paths) {
        AuthState::LoggedIn(c) => assert_eq!(c.token, "cct_newer"),
        other => panic!("newer login must stay valid, got {other:?}"),
    }
    mock.assert();
}

#[test]
fn same_size_rewrite_is_uploaded_again() {
    let fx = fixture();
    let (server, mock, bodies) = recording_server(2);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let f = fx.paths.projects_dir().join("-home-dev-rw/rw.jsonl");
    fs::create_dir_all(f.parent().unwrap()).unwrap();
    fs::write(&f, line("rw", "rw_a", "2026-10-01T10:00:00Z") + "\n").unwrap();
    sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();

    // Rewritten in place with different content of exactly the same length.
    let replaced = line("rw", "rw_b", "2026-10-01T10:00:00Z") + "\n";
    assert_eq!(replaced.len() as u64, fs::metadata(&f).unwrap().len());
    fs::write(&f, &replaced).unwrap();
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 1);
    assert_eq!(bodies.lock().unwrap()[1]["entries"][0]["message_id"], "rw_b");
    mock.assert();
}

// ---- I1: revocation is noticed even with nothing to upload ----

/// Runs a first sync that uploads everything, so the next has nothing pending.
fn drained(fx: &Fixture) {
    let (server, _mock, _) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
}

#[test]
fn nothing_pending_checks_the_token_and_marks_a_revoked_one_invalid() {
    let fx = fixture();
    drained(&fx);
    let mut server = mockito::Server::new();
    let ingest = server.mock("POST", "/api/ingest").expect(0).create();
    let me = server.mock("GET", "/api/me").match_header("authorization", "Bearer cct_test").with_status(401).with_body(r#"{"error":"revoked"}"#).expect(1).create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let err = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap_err();
    assert!(err.to_string().contains("401"), "{err}");
    assert!(matches!(credentials::load(&fx.paths), AuthState::Invalid(_)));
    ingest.assert();
    me.assert();
}

#[test]
fn nothing_pending_with_a_valid_token_or_offline_is_ok() {
    let fx = fixture();
    drained(&fx);
    let mut server = mockito::Server::new();
    let me = server.mock("GET", "/api/me").with_body(r#"{"id":1,"email":"dev@example.com","name":"Dev","is_admin":false}"#).expect(1).create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    assert_eq!(sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap().entries_sent, 0);
    me.assert();
    // Offline: no error, still logged in.
    let offline = ApiClient::new("http://127.0.0.1:9", Some("cct_test"));
    assert!(sync::run(&fx.paths, &fx.creds, &offline, &opts(), now()).is_ok());
    assert!(matches!(credentials::load(&fx.paths), AuthState::LoggedIn(_)));
}

#[test]
fn no_token_check_when_a_batch_was_sent() {
    let fx = fixture();
    let (mut server, mock, _) = recording_server(1);
    let me = server.mock("GET", "/api/me").expect(0).create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    mock.assert();
    me.assert();
}

// ---- I2: byte cap and 413 ----

/// A transcript of `n` entries in session `session` whose message ids are `pad` bytes long.
fn big_file(fx: &Fixture, name: &str, session: &str, n: usize, pad: usize) -> std::path::PathBuf {
    let f = fx.paths.projects_dir().join(format!("-home-dev-big/{name}.jsonl"));
    fs::create_dir_all(f.parent().unwrap()).unwrap();
    let lines: Vec<String> = (0..n).map(|i| line(session, &format!("{i:05}-{}", "x".repeat(pad)), "2026-10-01T10:00:00Z")).collect();
    fs::write(&f, lines.join("\n") + "\n").unwrap();
    f
}

#[test]
fn batches_stay_under_the_byte_cap() {
    let fx = fixture();
    // ~1.5 KB per entry: 1000 entries fit the count limit but not 900 KB.
    let f = big_file(&fx, "big", "big", 1000, 1500);
    let mut server = mockito::Server::new();
    let sizes: Arc<Mutex<Vec<usize>>> = Arc::default();
    let sink = sizes.clone();
    let mock = server
        .mock("POST", "/api/ingest")
        .with_body_from_request(move |req| {
            let body = req.body().unwrap();
            sink.lock().unwrap().push(body.len());
            let n = serde_json::from_slice::<Value>(body).unwrap()["entries"].as_array().unwrap().len();
            format!(r#"{{"accepted":{n},"duplicates":0}}"#).into_bytes()
        })
        .expect_at_least(2)
        .create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 1002);
    let sizes = sizes.lock().unwrap();
    assert!(sizes.len() >= 2 && sizes.iter().all(|&s| s <= sync::MAX_BATCH_BYTES), "{sizes:?}");
    assert_eq!(Offsets::load(&fx.paths.offsets()).get(&f).offset, fs::metadata(&f).unwrap().len());
    mock.assert();
}

#[test]
fn http_413_splits_the_batch_and_never_quarantines() {
    let fx = fixture();
    let f = big_file(&fx, "mid", "mid", 200, 500);
    // A server with a much smaller body limit than ours.
    let mut server = mockito::Server::new();
    let ids: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = ids.clone();
    let too_big = |req: &mockito::Request| req.body().unwrap().len() > 30_000;
    let mock = server
        .mock("POST", "/api/ingest")
        .with_status_code_from_request(move |req| if too_big(req) { 413 } else { 200 })
        .with_body_from_request(move |req| {
            if too_big(req) {
                return br#"{"error":"request body too large"}"#.to_vec();
            }
            let body: Value = serde_json::from_slice(req.body().unwrap()).unwrap();
            let entries = body["entries"].as_array().unwrap();
            sink.lock().unwrap().extend(entries.iter().map(|e| e["message_id"].as_str().unwrap().to_string()));
            format!(r#"{{"accepted":{},"duplicates":0}}"#, entries.len()).into_bytes()
        })
        .expect_at_least(3)
        .create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert!(report.quarantined.is_empty());
    assert_eq!(report.entries_sent, 202);
    let ids = ids.lock().unwrap();
    assert_eq!(ids.len(), 202);
    assert_eq!(ids.iter().collect::<std::collections::HashSet<_>>().len(), 202, "each entry accepted exactly once");
    assert_eq!(Offsets::load(&fx.paths.offsets()).get(&f).offset, fs::metadata(&f).unwrap().len());
    assert!(!fx.paths.quarantine().exists());
    mock.assert();
}

#[test]
fn http_413_for_a_single_entry_stops_without_quarantine() {
    let fx = fixture();
    let mut server = mockito::Server::new();
    // msg_1 + msg_3 (413) → msg_1 + snapshot alone (413): a single entry, so stop.
    let mock = server.mock("POST", "/api/ingest").with_status(413).with_body(r#"{"error":"request body too large"}"#).expect(2).create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let err = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap_err();
    assert!(err.to_string().contains("413"), "{err}");
    assert!(!fx.paths.quarantine().exists());
    assert_eq!(Offsets::load(&fx.paths.offsets()).get(&fx.transcript).offset, 0);
    assert!(matches!(credentials::load(&fx.paths), AuthState::LoggedIn(_)));
    mock.assert();
}

// ---- I3: a file of duplicates commits only with the batch carrying them ----

#[test]
fn duplicate_file_offset_waits_for_the_batch_carrying_its_messages() {
    let fx = fixture();
    let content = [line("dup", "dup_1", "2026-10-01T10:00:00Z"), line("dup", "dup_2", "2026-10-01T10:01:00Z")].join("\n") + "\n";
    let original = fx.paths.projects_dir().join("-home-dev-a/dup.jsonl");
    let copy = fx.paths.projects_dir().join("-home-dev-b/dup.jsonl");
    for f in [&original, &copy] {
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, &content).unwrap();
    }

    let mut server = mockito::Server::new();
    let failing = server.mock("POST", "/api/ingest").with_status(500).with_body(r#"{"error":"db down"}"#).expect(1).create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    assert!(sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).is_err());
    let offsets = Offsets::load(&fx.paths.offsets());
    assert_eq!(offsets.get(&original).offset, 0);
    assert_eq!(offsets.get(&copy).offset, 0, "the duplicate file's offset must not advance before its messages are accepted");
    failing.assert();

    // The original disappears before the retry: the copy still carries the usage.
    fs::remove_file(&original).unwrap();
    let (server, mock, bodies) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    let ids: Vec<String> = bodies.lock().unwrap()[0]["entries"].as_array().unwrap().iter().map(|e| e["message_id"].as_str().unwrap().to_string()).collect();
    assert!(ids.contains(&"dup_1".to_string()) && ids.contains(&"dup_2".to_string()), "{ids:?}");
    assert_eq!(Offsets::load(&fx.paths.offsets()).get(&copy).offset, content.len() as u64);
    mock.assert();
}

#[test]
fn duplicate_file_commits_with_the_original_on_success() {
    let fx = fixture();
    let content = line("dup", "dup_1", "2026-10-01T10:00:00Z") + "\n";
    let original = fx.paths.projects_dir().join("-home-dev-a/dup.jsonl");
    let copy = fx.paths.projects_dir().join("-home-dev-b/dup.jsonl");
    for f in [&original, &copy] {
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, &content).unwrap();
    }
    let (server, mock, bodies) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 3, "dup_1 once + msg_1 + msg_3");
    assert_eq!(bodies.lock().unwrap().len(), 1);
    let offsets = Offsets::load(&fx.paths.offsets());
    assert_eq!(offsets.get(&original).offset, content.len() as u64);
    assert_eq!(offsets.get(&copy).offset, content.len() as u64);
    mock.assert();
}

// ---- I8: quarantine scope, global rejections, one report per session ----

#[test]
fn quarantine_is_scoped_to_server_and_user() {
    let fx = fixture();
    // sess-1 was quarantined for another user and for another server, not for us.
    fs::create_dir_all(&fx.paths.state_dir).unwrap();
    fs::write(fx.paths.quarantine(), r#"{"http://unused|2":["sess-1"],"https://other.example.com|1":["sess-1"]}"#).unwrap();
    let (server, mock, bodies) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 2);
    assert_eq!(bodies.lock().unwrap()[0]["sessions"][0]["session_id"], "sess-1");
    mock.assert();

    // Quarantined for us: skipped, and its offsets still advance.
    let fx = fixture();
    fs::create_dir_all(&fx.paths.state_dir).unwrap();
    fs::write(fx.paths.quarantine(), r#"{"http://unused|1":["sess-1"]}"#).unwrap();
    let client = ApiClient::new("http://127.0.0.1:9", Some("cct_test")); // only /me, offline
    fs::remove_file(&fx.paths.claude_json).unwrap(); // no snapshot to send
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 0);
    assert!(Offsets::load(&fx.paths.offsets()).get(&fx.transcript).offset > 0);
}

#[test]
fn identical_rejection_of_every_session_stops_without_quarantine() {
    let fx = fixture();
    let other = fx.paths.projects_dir().join("-home-dev-other/s2.jsonl");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    fs::write(&other, line("s2", "s2_1", "2026-10-01T10:00:00Z") + "\n").unwrap();
    // E.g. the device clock is a day off: every request is invalid.
    let mut server = mockito::Server::new();
    let mock = server.mock("POST", "/api/ingest").with_status(400).with_body(r#"{"error":"ts too far in the future"}"#).expect(4).create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let err = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap_err().to_string();
    assert!(err.contains("HTTP 400") && err.contains("ts too far in the future") && err.contains("nothing was quarantined"), "{err}");
    assert!(!fx.paths.quarantine().exists());
    let offsets = Offsets::load(&fx.paths.offsets());
    assert_eq!(offsets.get(&fx.transcript).offset, 0);
    assert_eq!(offsets.get(&other).offset, 0);
    assert!(!fx.paths.last_snapshot().exists());
    mock.assert(); // batch, s2 alone, sess-1 alone, snapshot alone
}

#[test]
fn single_session_400_rejection_stops_without_quarantine() {
    let fx = fixture();
    fs::remove_file(&fx.paths.claude_json).unwrap(); // no snapshot
    let mut server = mockito::Server::new();
    let mock = server.mock("POST", "/api/ingest").with_status(400).with_body(r#"{"error":"ts too far in the future"}"#).expect(2).create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let err = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap_err().to_string();
    assert!(err.contains("HTTP 400") && err.contains("nothing was quarantined"), "{err}");
    assert!(!fx.paths.quarantine().exists());
    assert_eq!(Offsets::load(&fx.paths.offsets()).get(&fx.transcript).offset, 0);
    mock.assert();
}

#[test]
fn lone_foreign_session_403_is_quarantined_and_later_batches_upload() {
    let fx = fixture();
    fs::remove_file(&fx.paths.claude_json).unwrap(); // no snapshot: batch 1 is "bad" alone
    // Exactly BATCH_ENTRIES entries of "bad" fill batch 1 on their own; the
    // rest ("good", then sess-1) land in batch 2.
    let bad = fx.paths.projects_dir().join("-home-dev-a/bad.jsonl");
    fs::create_dir_all(bad.parent().unwrap()).unwrap();
    fs::write(&bad, (0..sync::BATCH_ENTRIES).map(|i| line("bad", &format!("bad_{i}"), "2026-10-01T10:00:00Z")).collect::<Vec<_>>().join("\n") + "\n").unwrap();
    let good = fx.paths.projects_dir().join("-home-dev-b/good.jsonl");
    fs::create_dir_all(good.parent().unwrap()).unwrap();
    fs::write(&good, line("good", "good_1", "2026-10-01T10:00:00Z") + "\n").unwrap();

    // batch 1 (403) → bad alone (403: quarantined, not a stop) → batch 2 (200).
    let (server, mock, bodies) = refusing_server(3);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.quarantined, vec!["bad".to_string()]);
    assert_eq!(report.accepted, 3, "good_1 + msg_1 + msg_3");
    {
        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies[0]["sessions"].as_array().unwrap().len(), 1, "batch 1 holds only the foreign session");
        let last = bodies.last().unwrap();
        let ids: Vec<&str> = last["entries"].as_array().unwrap().iter().map(|e| e["message_id"].as_str().unwrap()).collect();
        assert_eq!(ids.len(), 3, "{ids:?}");
        assert!(ids.contains(&"good_1") && ids.contains(&"msg_1") && ids.contains(&"msg_3"), "{ids:?}");
    }
    assert!(sync::Quarantine::load(&fx.paths.quarantine()).contains(&sync::Quarantine::scope(&fx.creds), "bad"));
    let offsets = Offsets::load(&fx.paths.offsets());
    assert_eq!(offsets.get(&bad).offset, fs::metadata(&bad).unwrap().len());
    assert_eq!(offsets.get(&good).offset, fs::metadata(&good).unwrap().len());

    // A second sync sends nothing for it, even for new entries.
    fs::write(&bad, fs::read_to_string(&bad).unwrap() + &line("bad", "bad_new", "2026-10-01T11:00:00Z") + "\n").unwrap();
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 0);
    assert_eq!(report.batches, 0);
    mock.assert();
}

#[test]
fn session_spanning_batches_is_quarantined_and_reported_once() {
    let fx = fixture();
    let good = fx.paths.projects_dir().join("-home-dev-a/good.jsonl");
    fs::create_dir_all(good.parent().unwrap()).unwrap();
    fs::write(&good, (0..10).map(|i| line("good", &format!("good_{i}"), "2026-10-01T10:00:00Z")).collect::<Vec<_>>().join("\n") + "\n").unwrap();
    // 1600 entries of "bad": 1490 land in batch 1 (after good's 10), 110 in batch 2.
    let bad = fx.paths.projects_dir().join("-home-dev-b/bad.jsonl");
    fs::create_dir_all(bad.parent().unwrap()).unwrap();
    fs::write(&bad, (0..1600).map(|i| line("bad", &format!("bad_{i}"), "2026-10-01T10:00:00Z")).collect::<Vec<_>>().join("\n") + "\n").unwrap();

    // batch 1 (403) → bad alone (403) → good alone (200) → snapshot alone (200) → batch 2 without bad (200).
    let (server, mock, bodies) = refusing_server(5);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.quarantined, vec!["bad".to_string()]);
    {
        let bodies = bodies.lock().unwrap();
        let last = bodies.last().unwrap();
        let ids: Vec<&str> = last["entries"].as_array().unwrap().iter().map(|e| e["message_id"].as_str().unwrap()).collect();
        assert_eq!(ids.len(), 2, "batch 2 carries only msg_1 + msg_3: {ids:?}");
    }
    let log = fs::read_to_string(fx.paths.sync_log()).unwrap();
    assert_eq!(log.matches("quarantined session bad").count(), 1, "{log}");
    assert_eq!(Offsets::load(&fx.paths.offsets()).get(&bad).offset, fs::metadata(&bad).unwrap().len());
    mock.assert();
}

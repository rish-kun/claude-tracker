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
fn lone_session_403_from_a_proxy_stops_without_quarantine() {
    let fx = fixture();
    fs::remove_file(&fx.paths.claude_json).unwrap(); // no snapshot
    // E.g. a WAF in front of the server: 403, but not the server's ownership error.
    let mut server = mockito::Server::new();
    let mock = server.mock("POST", "/api/ingest").with_status(403).with_body("Forbidden by firewall policy").expect(2).create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let err = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap_err().to_string();
    assert!(err.contains("HTTP 403") && err.contains("nothing was quarantined"), "{err}");
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

fn append(path: &std::path::Path, s: &str) {
    let mut c = fs::read_to_string(path).unwrap();
    c.push_str(s);
    fs::write(path, c).unwrap();
}

fn sessions_of(body: &Value) -> Vec<Value> {
    body["sessions"].as_array().unwrap().clone()
}

#[test]
fn session_titles_are_sent_and_updated_without_new_usage() {
    let fx = fixture();
    let content = fs::read_to_string(&fx.transcript).unwrap();
    fs::write(
        &fx.transcript,
        format!(
            "{}\n{}\n{content}\n",
            r#"{"type":"ai-title","aiTitle":"First idea","sessionId":"sess-1"}"#,
            r#"{"type":"ai-title","aiTitle":"BITS Pilani SWD website IIS migration","sessionId":"sess-1"}"#
        ),
    )
    .unwrap();
    // Subagent transcripts carry no session title: ignored.
    append(&subagent_path(&fx), "{\"type\":\"custom-title\",\"customTitle\":\"from a subagent\",\"sessionId\":\"sess-1\"}\n");
    // A titled session with no usage is never created on the server by its title.
    let other = fx.paths.projects_dir().join("-home-dev-other/sess-t.jsonl");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    fs::write(&other, "{\"type\":\"ai-title\",\"aiTitle\":\"Lonely\",\"sessionId\":\"sess-t\"}\n").unwrap();

    let (server, mock, bodies) = recording_server(3);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!((report.entries_sent, report.titles_sent), (3, 0));
    {
        let b = &bodies.lock().unwrap()[0];
        let s = sessions_of(b);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0]["title"], "BITS Pilani SWD website IIS migration", "the last AI title wins");
    }

    // A custom title appears later, in bytes after the usage: sent on its own.
    append(&fx.transcript, "{\"type\":\"custom-title\",\"customTitle\":\"SOP Report\",\"sessionId\":\"sess-1\"}\n");
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!((report.entries_sent, report.titles_sent), (0, 1));
    {
        let b = &bodies.lock().unwrap()[1];
        assert!(b["entries"].as_array().unwrap().is_empty() && b["accounts"].as_array().unwrap().is_empty());
        let s = sessions_of(b);
        assert_eq!(s.len(), 1, "{b}");
        assert_eq!(s[0]["session_id"], "sess-1");
        assert_eq!(s[0]["title"], "SOP Report");
        // The range already uploaded, so the server's range is unchanged.
        assert_eq!(s[0]["started_at"], "2026-10-01T10:00:00Z");
        assert_eq!(s[0]["ended_at"], "2026-10-01T10:05:00Z");
        assert!(s[0].get("cwd").is_none());
    }

    // A later AI title does not beat the custom one: nothing to send.
    append(&fx.transcript, "{\"type\":\"ai-title\",\"aiTitle\":\"Newer AI title\",\"sessionId\":\"sess-1\"}\n");
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!((report.entries_sent, report.titles_sent), (0, 0));

    // Renamed again: sent again.
    append(&fx.transcript, "{\"type\":\"custom-title\",\"customTitle\":\"Renamed\",\"sessionId\":\"sess-1\"}\n");
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.titles_sent, 1);
    let bodies = bodies.lock().unwrap();
    assert_eq!(sessions_of(&bodies[2])[0]["title"], "Renamed");
    assert!(bodies.iter().all(|b| !b.to_string().contains("Lonely") && !b.to_string().contains("from a subagent")));
    mock.assert();
}

fn local(ts: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(ts).unwrap().with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string()
}

fn line_model(session: &str, id: &str, ts: &str, model: &str) -> String {
    format!(
        r#"{{"type":"assistant","sessionId":"{session}","timestamp":"{ts}","message":{{"id":"{id}","model":"{model}","usage":{{"input_tokens":1,"output_tokens":1}}}}}}"#
    )
}

/// A CLIProxyAPI home with two pool accounts and a log for session "sess-1".
fn proxy_fixture(fx: &Fixture) {
    use cctrack::proxy::{self, HookPoint, ProxyMode};
    let dir = &fx.paths.cliproxy_dir;
    fs::create_dir_all(dir.join("logs")).unwrap();
    for (name, email, uuid) in [("pool-a.json", "a@pool.org", "pool-a-uuid"), ("pool-b.json", "b@pool.org", "pool-b-uuid")] {
        fs::write(dir.join(name), format!(
            r#"{{"type":"claude","email":"{email}","account_uuid":"{uuid}","organization_uuid":"org-{uuid}","organization_name":"Pool","access_token":"sk-ant-oat-SECRET","refresh_token":"sk-ant-ort-SECRET","id_token":"SECRET"}}"#
        )).unwrap();
    }
    let sel = |ts: &str, req: &str, session: &str, auth: &str, model: &str| format!(
        "[{}] [{req}] [info ] [selector.go:1068] session-affinity: cache hit | session={session} auth={auth} provider=mixed model={model}", local(ts)
    );
    let done = |ts: &str, req: &str| format!("[{}] [{req}] [info ] [gin_logger.go:108] 200 |  5s |  127.0.0.1 | POST \"/v1/messages?beta=true\"", local(ts));
    let lines = [
        sel("2026-10-01T09:59:58Z", "r1", "claude:s...", "pool-a.json", "claude-opus-5-5"),
        done("2026-10-01T10:00:03Z", "r1"),
        sel("2026-10-01T10:00:30Z", "r2", "claude:x...", "pool-a.json", "claude-opus-5-5"),
        sel("2026-10-01T10:01:55Z", "r3", "claude:s...", "pool-b.json", "claude-opus-5-5"),
        done("2026-10-01T10:02:10Z", "r3"),
    ];
    fs::write(dir.join("logs/main.log"), lines.join("\n") + "\n").unwrap();
    // The SessionStart hook ran inside Claude Code started by the proxy wrapper.
    proxy::record_session(&fx.paths, "sess-1", None, Some("http://127.0.0.1:8317"), HookPoint::SessionStart, ProxyMode::Auto, now());
    // msg_4 complete, plus a haiku response that has no log line.
    append(&fx.transcript, "\n");
    append(&fx.transcript, &(line_model("sess-1", "msg_h", "2026-10-01T10:03:00Z", "claude-haiku-5-5") + "\n"));
}

#[test]
fn proxied_usage_is_credited_to_the_pool_account_that_served_it() {
    let fx = fixture();
    proxy_fixture(&fx);

    // Dry run first: same attribution, nothing uploaded or recorded.
    let dry = sync::dry_run(&fx.paths, &fx.creds, &opts(), false, now()).unwrap();
    assert_eq!(dry.sessions.len(), 1);
    let s = &dry.sessions[0];
    assert_eq!(s.entries, 4);
    assert!(matches!(&s.proxy, Some(Ok(why)) if why.contains("Claude Code's environment")), "{:?}", s.proxy);
    let count = |email: &str, how: &str| s.accounts.get(&(email.to_string(), how.to_string())).copied().unwrap_or(0);
    assert_eq!(count("a@pool.org", "CLIProxyAPI log"), 1);
    assert_eq!(count("b@pool.org", "CLIProxyAPI log"), 2);
    assert_eq!(count("b@pool.org", "session's usual pool account"), 1);
    assert!(!fx.paths.offsets().exists() && !fx.paths.sessions().exists(), "a dry run changes no state");

    let (server, mock, bodies) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!(report.entries_sent, 4);
    let bodies = bodies.lock().unwrap();
    let b = &bodies[0];
    let account_of = |id: &str| {
        b["entries"].as_array().unwrap().iter().find(|e| e["message_id"] == id).unwrap()["account_uuid"].as_str().unwrap().to_string()
    };
    assert_eq!(account_of("msg_1"), "pool-a-uuid", "the request in flight at 10:00:00");
    assert_eq!(account_of("msg_3"), "pool-b-uuid", "subagent usage of the same session");
    assert_eq!(account_of("msg_4"), "pool-b-uuid", "latest earlier request of the session");
    assert_eq!(account_of("msg_h"), "pool-b-uuid", "no haiku line: the session's usual pool account");
    let accounts = b["accounts"].as_array().unwrap();
    let a = accounts.iter().find(|a| a["account_uuid"] == "pool-a-uuid").unwrap();
    assert_eq!(a["email"], "a@pool.org");
    assert_eq!(a["org_uuid"], "org-pool-a-uuid");
    assert!(a.get("org_type").is_none() && a.get("rate_limit_tier").is_none(), "plans are set by an admin");
    // The plan-limit snapshot stays with the oauthAccount.
    assert_eq!(b["limit_snapshot"]["account_uuid"], "acc-1");
    assert!(accounts.iter().any(|a| a["account_uuid"] == "acc-1"));
    assert!(!b.to_string().contains("SECRET"), "auth tokens never leave the machine");
    mock.assert();
}

#[test]
fn proxy_mode_off_keeps_the_claude_account() {
    let fx = fixture();
    proxy_fixture(&fx);
    cctrack::proxy::save_mode(&fx.paths, cctrack::proxy::ProxyMode::Off).unwrap();
    let (server, mock, bodies) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    let bodies = bodies.lock().unwrap();
    assert!(bodies[0]["entries"].as_array().unwrap().iter().all(|e| e["account_uuid"] == "acc-1"), "{}", bodies[0]);
    mock.assert();
}

#[test]
fn proxied_session_without_a_log_keeps_the_claude_account() {
    let fx = fixture();
    proxy_fixture(&fx);
    fs::remove_file(fx.paths.cliproxy_dir.join("logs/main.log")).unwrap();
    let dry = sync::dry_run(&fx.paths, &fx.creds, &opts(), false, now()).unwrap();
    assert!(matches!(&dry.sessions[0].proxy, Some(Err(why)) if why.contains("no CLIProxyAPI log")), "{:?}", dry.sessions[0].proxy);
    let (server, mock, bodies) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert!(bodies.lock().unwrap()[0]["entries"].as_array().unwrap().iter().all(|e| e["account_uuid"] == "acc-1"));
    mock.assert();
}

// ---- QA round: pool attribution ----

/// Pool auth files pool-a/pool-b in the proxy home (with secrets that must never leave).
fn pool_home(fx: &Fixture) {
    let dir = &fx.paths.cliproxy_dir;
    fs::create_dir_all(dir.join("logs")).unwrap();
    for (name, email, uuid) in [("pool-a.json", "a@pool.org", "pool-a-uuid"), ("pool-b.json", "b@pool.org", "pool-b-uuid")] {
        fs::write(dir.join(name), format!(
            r#"{{"type":"claude","email":"{email}","account_uuid":"{uuid}","organization_uuid":"org-{uuid}","access_token":"sk-ant-oat-SECRET","refresh_token":"sk-ant-ort-SECRET"}}"#
        )).unwrap();
    }
}

/// A request in the proxy log: (start, end, request id, session key, auth).
type Req<'a> = (&'a str, Option<&'a str>, &'a str, &'a str, &'a str);

/// Writes requests (model claude-opus-5-5) to `logs/<file>`, in time order,
/// with times formatted by `fmt` (the proxy host's local time).
fn write_log_as(fx: &Fixture, file: &str, reqs: &[Req], fmt: &dyn Fn(&str) -> String) {
    let mut lines: Vec<(chrono::DateTime<Utc>, String)> = Vec::new();
    let t = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
    for (start, end, req, session, auth) in reqs {
        lines.push((t(start), format!(
            "[{}] [{req}] [info ] [selector.go:1068] session-affinity: cache hit | session={session} auth={auth} provider=mixed model=claude-opus-5-5",
            fmt(start)
        )));
        if let Some(end) = end {
            lines.push((t(end), format!("[{}] [{req}] [info ] [gin_logger.go:108] 200 |  5s |  127.0.0.1 | POST \"/v1/messages?beta=true\"", fmt(end))));
        }
    }
    lines.sort_by_key(|(t, _)| *t);
    let body: String = lines.into_iter().map(|(_, l)| l + "\n").collect();
    fs::write(fx.paths.cliproxy_dir.join("logs").join(file), body).unwrap();
}

fn write_log(fx: &Fixture, reqs: &[Req]) {
    write_log_as(fx, "main.log", reqs, &local);
}

/// A transcript for `session` with opus entries at `times`.
fn transcript(fx: &Fixture, session: &str, times: &[&str]) -> std::path::PathBuf {
    let f = fx.paths.projects_dir().join(format!("-home-dev-{session}/{session}.jsonl"));
    fs::create_dir_all(f.parent().unwrap()).unwrap();
    let lines: Vec<String> = times.iter().enumerate().map(|(i, ts)| line_model(session, &format!("{session}_{i}"), ts, "claude-opus-5-5")).collect();
    fs::write(&f, lines.join("\n") + "\n").unwrap();
    f
}

fn accounts_by_message(body: &Value) -> std::collections::HashMap<String, String> {
    body["entries"].as_array().unwrap().iter()
        .map(|e| (e["message_id"].as_str().unwrap().to_string(), e["account_uuid"].as_str().unwrap().to_string()))
        .collect()
}

fn all_accounts(bodies: &[Value]) -> std::collections::HashMap<String, String> {
    bodies.iter().flat_map(accounts_by_message).collect()
}

fn session_state(fx: &Fixture, id: &str) -> Value {
    let v: Value = serde_json::from_str(&fs::read_to_string(fx.paths.sessions()).unwrap()).unwrap();
    v["sessions"][id].clone()
}

/// QA HIGH: a session started before the hooks (no record) and without
/// ANTHROPIC_BASE_URL in any settings file was credited to the personal
/// account forever. Now it is decided from the proxy log, and the decision kept.
#[test]
fn sessions_without_a_hook_record_are_inferred_from_the_proxy_log() {
    use cctrack::proxy::{self, HookPoint, ProxyMode};
    let fx = fixture();
    pool_home(&fx);
    write_log(&fx, &[
        // The pre-hook proxied session 9c013753…: every response during one of its requests.
        ("2026-10-01T10:00:00Z", Some("2026-10-01T10:00:21Z"), "p1", "claude:9...", "pool-a.json"),
        ("2026-10-01T10:01:00Z", Some("2026-10-01T10:01:21Z"), "p2", "claude:9...", "pool-a.json"),
        ("2026-10-01T10:02:00Z", Some("2026-10-01T10:02:21Z"), "p3", "claude:9...", "pool-a.json"),
        ("2026-10-01T10:03:00Z", Some("2026-10-01T10:03:21Z"), "p4", "claude:9...", "pool-b.json"),
        ("2026-10-01T10:04:00Z", Some("2026-10-01T10:04:21Z"), "p5", "claude:9...", "pool-b.json"),
        // Only two entries so far: too few to tell.
        ("2026-10-01T10:00:00Z", Some("2026-10-01T10:00:21Z"), "f1", "claude:2...", "pool-a.json"),
        ("2026-10-01T10:01:00Z", Some("2026-10-01T10:01:21Z"), "f2", "claude:2...", "pool-a.json"),
        // A session a hook saw without ANTHROPIC_BASE_URL, yet its responses match.
        ("2026-10-01T10:00:00Z", Some("2026-10-01T10:00:21Z"), "e1", "claude:e...", "pool-a.json"),
        ("2026-10-01T10:01:00Z", Some("2026-10-01T10:01:21Z"), "e2", "claude:e...", "pool-a.json"),
        ("2026-10-01T10:02:00Z", Some("2026-10-01T10:02:21Z"), "e3", "claude:e...", "pool-a.json"),
    ]);
    transcript(&fx, "9c013753-pre", &["2026-10-01T10:00:20Z", "2026-10-01T10:01:20Z", "2026-10-01T10:02:20Z", "2026-10-01T10:03:20Z", "2026-10-01T10:04:20Z"]);
    // A plain `cli` session: nothing of it in the log.
    transcript(&fx, "1abc-plain", &["2026-10-01T10:00:20Z", "2026-10-01T10:01:20Z", "2026-10-01T10:02:20Z", "2026-10-01T10:03:20Z"]);
    // Same first character as the proxied one, but never while a '9' request runs.
    transcript(&fx, "9d00-plain", &["2026-10-01T10:30:00Z", "2026-10-01T10:40:00Z", "2026-10-01T10:50:00Z"]);
    transcript(&fx, "2few", &["2026-10-01T10:00:20Z", "2026-10-01T10:01:20Z"]);
    transcript(&fx, "e0hooked", &["2026-10-01T10:00:20Z", "2026-10-01T10:01:20Z", "2026-10-01T10:02:20Z"]);
    proxy::record_session(&fx.paths, "e0hooked", None, None, HookPoint::SessionStart, ProxyMode::Auto, now());

    let dry = sync::dry_run(&fx.paths, &fx.creds, &opts(), false, now()).unwrap();
    let get = |id: &str| dry.sessions.iter().find(|s| s.session_id == id).unwrap();
    let s = get("9c013753-pre");
    assert!(matches!(&s.proxy, Some(Ok(why)) if why.contains("inferred from the proxy log") && why.contains("5 of 5")), "{:?}", s.proxy);
    assert_eq!(s.accounts.get(&("a@pool.org".to_string(), "CLIProxyAPI log".to_string())), Some(&3));
    assert_eq!(s.accounts.get(&("b@pool.org".to_string(), "CLIProxyAPI log".to_string())), Some(&2));
    for id in ["1abc-plain", "9d00-plain"] {
        let s = get(id);
        assert!(matches!(&s.proxy, Some(Err(why)) if why.contains("inferred from the proxy log as not proxied")), "{id}: {:?}", s.proxy);
        assert!(s.accounts.keys().all(|(a, _)| a == "shared@example.com"), "{id}: {:?}", s.accounts);
        assert!(s.looks_proxied.is_none());
    }
    assert!(matches!(&get("2few").proxy, Some(Err(why)) if why.contains("only 2 entries")), "{:?}", get("2few").proxy);
    let e = get("e0hooked");
    assert!(matches!(&e.proxy, Some(Err(why)) if why.contains("ANTHROPIC_BASE_URL is not set")), "{:?}", e.proxy);
    assert!(e.looks_proxied.as_deref().is_some_and(|n| n.contains("looks proxied: 3 of 3")), "{:?}", e.looks_proxied);
    assert!(!fx.paths.sessions().exists(), "a dry run changes no state");

    let (server, mock, bodies) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    let got = all_accounts(&bodies.lock().unwrap());
    for i in 0..3 {
        assert_eq!(got[&format!("9c013753-pre_{i}")], "pool-a-uuid");
    }
    assert_eq!(got["9c013753-pre_3"], "pool-b-uuid");
    for id in ["1abc-plain_0", "9d00-plain_2", "2few_1", "e0hooked_0", "msg_1"] {
        assert_eq!(got[id], "acc-1", "{id}");
    }
    mock.assert();
    // The decisions are kept; too few entries decide nothing yet.
    let st = session_state(&fx, "9c013753-pre");
    assert_eq!((st["inferred"]["proxied"].as_bool(), st["inferred"]["matched"].as_u64()), (Some(true), Some(5)), "{st}");
    assert_eq!(session_state(&fx, "1abc-plain")["inferred"]["proxied"], false);
    assert!(session_state(&fx, "2few")["inferred"].is_null());
    assert_eq!(session_state(&fx, "2few")["log_evidence"], serde_json::json!({ "matched": 2, "total": 2 }));
    let status = proxy::status_report(&fx.paths, None, None, None, now());
    assert!(status.contains("9c013753-pre [pool] decided"), "{status}");
    assert!(status.contains("look proxied but were credited to the Claude account (1):\n  e0hooked: 3 of 3"), "{status}");

    // Stable: a later response with no log line stays with the pool (the
    // session's usual account), and the 2-entry session is decided once it has 3.
    let pre = fx.paths.projects_dir().join("-home-dev-9c013753-pre/9c013753-pre.jsonl");
    append(&pre, &(line_model("9c013753-pre", "late", "2026-10-01T11:00:00Z", "claude-opus-5-5") + "\n"));
    let few = fx.paths.projects_dir().join("-home-dev-2few/2few.jsonl");
    append(&few, &(line_model("2few", "2few_2", "2026-10-01T10:01:20.500Z", "claude-opus-5-5") + "\n"));
    let dry = sync::dry_run(&fx.paths, &fx.creds, &opts(), false, now()).unwrap();
    let s = dry.sessions.iter().find(|s| s.session_id == "9c013753-pre").unwrap();
    assert!(matches!(&s.proxy, Some(Ok(why)) if why.contains("inferred from the proxy log")), "{:?}", s.proxy);
    assert_eq!(s.accounts.get(&("a@pool.org".to_string(), "session's usual pool account".to_string())), Some(&1), "{:?}", s.accounts);
    let s = dry.sessions.iter().find(|s| s.session_id == "2few").unwrap();
    assert!(matches!(&s.proxy, Some(Ok(why)) if why.contains("3 of 3")), "{:?}", s.proxy);
}

/// QA MEDIUM repro: overlapping requests of two sessions sharing a first
/// character on different pool accounts were both credited to pool-b.
#[test]
fn overlapping_sessions_with_the_same_first_char_get_their_own_pool_account() {
    use cctrack::proxy::{self, HookPoint, ProxyMode};
    let fx = fixture();
    pool_home(&fx);
    write_log(&fx, &[
        ("2026-10-01T10:30:00Z", Some("2026-10-01T10:30:30Z"), "q5", "claude:c...", "pool-a.json"),
        ("2026-10-01T10:30:20Z", Some("2026-10-01T10:30:40Z"), "q6", "claude:c...", "pool-b.json"),
    ]);
    for id in ["cccc0005-aaaa", "cccc0006-bbbb"] {
        proxy::record_session(&fx.paths, id, None, Some("http://127.0.0.1:8317"), HookPoint::SessionStart, ProxyMode::Auto, now());
    }
    transcript(&fx, "cccc0005-aaaa", &["2026-10-01T10:30:29.500Z"]);
    transcript(&fx, "cccc0006-bbbb", &["2026-10-01T10:30:39.500Z"]);
    let (server, mock, bodies) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    let got = all_accounts(&bodies.lock().unwrap());
    assert_eq!(got["cccc0005-aaaa_0"], "pool-a-uuid");
    assert_eq!(got["cccc0006-bbbb_0"], "pool-b-uuid");
    mock.assert();
    // Affinity is recorded once accepted.
    assert_eq!(session_state(&fx, "cccc0005-aaaa")["last_pool_auth"], "pool-a.json");
}

/// QA MEDIUM: a deleted or renamed auth file. Its cached identity is used;
/// an auth that cannot be identified never borrows another account.
#[test]
fn deleted_auth_files_use_the_cached_identity_and_never_another_account() {
    let fx = fixture();
    proxy_fixture(&fx);
    let (server, mock, _) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    mock.assert();
    let cache = fs::read_to_string(fx.paths.proxy_auths()).unwrap();
    assert!(cache.contains("pool-b-uuid") && cache.contains("b@pool.org") && !cache.contains("SECRET"), "{cache}");

    // pool-b's auth file is deleted; later requests name it and one never seen.
    fs::remove_file(fx.paths.cliproxy_dir.join("pool-b.json")).unwrap();
    let mut log = fs::read_to_string(fx.paths.cliproxy_dir.join("logs/main.log")).unwrap();
    for (start, end, req, auth) in [
        ("2026-10-01T10:20:00Z", "2026-10-01T10:20:10Z", "r4", "pool-b.json"),
        ("2026-10-01T10:30:00Z", "2026-10-01T10:30:10Z", "r5", "gone.json"),
    ] {
        log += &format!("[{}] [{req}] [info ] [selector.go:1068] session-affinity: cache hit but auth unavailable, reselected | session=claude:s... auth={auth} provider=mixed model=claude-opus-5-5\n", local(start));
        log += &format!("[{}] [{req}] [info ] [gin_logger.go:108] 200 |  5s |  127.0.0.1 | POST \"/v1/messages?beta=true\"\n", local(end));
    }
    fs::write(fx.paths.cliproxy_dir.join("logs/main.log"), log).unwrap();
    append(&fx.transcript, &(line_model("sess-1", "msg_5", "2026-10-01T10:20:09Z", "claude-opus-5-5") + "\n"));
    append(&fx.transcript, &(line_model("sess-1", "msg_6", "2026-10-01T10:30:09Z", "claude-opus-5-5") + "\n"));

    let dry = sync::dry_run(&fx.paths, &fx.creds, &opts(), false, now()).unwrap();
    let s = &dry.sessions[0];
    assert_eq!(s.accounts.get(&("b@pool.org".to_string(), "CLIProxyAPI log; auth file pool-b.json gone, identity from cache".to_string())), Some(&1), "{:?}", s.accounts);
    let claude = s.accounts.iter().find(|((a, _), _)| a == "shared@example.com").unwrap();
    assert!(claude.0.1.contains("gone.json") && claude.0.1.contains("never cached"), "{:?}", s.accounts);
    assert_eq!(*claude.1, 1);

    let (server, mock, bodies) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    let got = all_accounts(&bodies.lock().unwrap());
    assert_eq!(got["msg_5"], "pool-b-uuid", "cached identity");
    assert_eq!(got["msg_6"], "acc-1", "not the session's usual pool-b: the log names another auth");
    mock.assert();
}

/// QA LOW-MED: pool account counts were saved before the upload, so a
/// failed upload that was retried counted twice.
#[test]
fn pool_counts_are_kept_only_once_the_upload_is_accepted() {
    let fx = fixture();
    proxy_fixture(&fx);
    let mut server = mockito::Server::new();
    let failing = server.mock("POST", "/api/ingest").with_status(500).with_body(r#"{"error":"db down"}"#).expect(1).create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    assert!(sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).is_err());
    failing.assert();
    assert!(session_state(&fx, "sess-1")["pool_auths"].is_null(), "{}", session_state(&fx, "sess-1"));

    let (server, mock, _) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    mock.assert();
    let st = session_state(&fx, "sess-1");
    assert_eq!(st["pool_auths"], serde_json::json!({ "pool-a.json": 1, "pool-b.json": 2 }), "{st}");
    assert_eq!(st["last_pool_auth"], "pool-b.json");
    assert_eq!(st["log_evidence"], serde_json::json!({ "matched": 2, "total": 4 }), "{st}");
}

/// The proxy host's local time, ±3 h from ours.
fn other_tz() -> chrono::FixedOffset {
    use chrono::Offset;
    let ours = chrono::Local.offset_from_utc_datetime(&now().naive_utc()).fix().local_minus_utc();
    chrono::FixedOffset::east_opt(if ours >= 0 { ours - 3 * 3600 } else { ours + 3 * 3600 }).unwrap()
}

/// Two hook-recorded proxied sessions (9tz, 7tz) whose requests are logged
/// in [`other_tz`].
fn calibration_fixture(fx: &Fixture, hook_7tz: bool) {
    use cctrack::proxy::{self, HookPoint, ProxyMode};
    pool_home(fx);
    let theirs = other_tz();
    let fmt = move |ts: &str| chrono::DateTime::parse_from_rfc3339(ts).unwrap().with_timezone(&theirs).format("%Y-%m-%d %H:%M:%S").to_string();
    write_log_as(fx, "main.log", &[
        ("2026-10-01T10:00:00Z", Some("2026-10-01T10:00:21Z"), "t1", "claude:9...", "pool-a.json"),
        ("2026-10-01T10:01:00Z", Some("2026-10-01T10:01:21Z"), "t2", "claude:9...", "pool-a.json"),
        ("2026-10-01T10:02:00Z", Some("2026-10-01T10:02:21Z"), "t3", "claude:9...", "pool-b.json"),
        ("2026-10-01T10:05:00Z", Some("2026-10-01T10:05:21Z"), "t4", "claude:7...", "pool-b.json"),
        ("2026-10-01T10:06:00Z", Some("2026-10-01T10:06:21Z"), "t5", "claude:7...", "pool-b.json"),
    ], &fmt);
    let mut hooked = vec!["9tz"];
    if hook_7tz {
        hooked.push("7tz");
    }
    for id in hooked {
        proxy::record_session(&fx.paths, id, None, Some("http://127.0.0.1:8317"), HookPoint::SessionStart, ProxyMode::Auto, now());
    }
    transcript(fx, "9tz", &["2026-10-01T10:00:20Z", "2026-10-01T10:01:20Z", "2026-10-01T10:02:20Z"]);
    transcript(fx, "7tz", &["2026-10-01T10:05:20Z", "2026-10-01T10:06:20Z"]);
}

fn clock_warning(dry: &sync::DryRun) -> bool {
    dry.warnings.iter().any(|w| w.contains("read as"))
}

/// QA LOW: the log is in the proxy host's local time, which may not be ours.
#[test]
fn a_proxy_log_in_another_time_zone_is_calibrated() {
    use cctrack::proxy;
    let fx = fixture();
    calibration_fixture(&fx, true);
    let theirs = other_tz();

    // A per-turn sync of one session has one session to go by: never calibrates.
    let one = SyncOptions { transcript: Some(fx.paths.projects_dir().join("-home-dev-9tz/9tz.jsonl")), wait_for_lock: Duration::ZERO };
    assert!(!clock_warning(&sync::dry_run(&fx.paths, &fx.creds, &one, false, now()).unwrap()));

    let dry = sync::dry_run(&fx.paths, &fx.creds, &opts(), false, now()).unwrap();
    let offset = cctrack::proxy::fmt_offset(theirs.local_minus_utc() / 60);
    assert!(dry.warnings.iter().any(|w| w.contains(&format!("read as {offset}"))), "{:?}", dry.warnings);
    let s = dry.sessions.iter().find(|s| s.session_id == "9tz").unwrap();
    assert_eq!(s.accounts.get(&("a@pool.org".to_string(), "CLIProxyAPI log".to_string())), Some(&2), "{:?}", s.accounts);
    assert_eq!(s.accounts.get(&("b@pool.org".to_string(), "CLIProxyAPI log".to_string())), Some(&1), "{:?}", s.accounts);
    assert!(!fx.paths.proxy_log_clock().exists(), "a dry run changes no state");

    let (server, mock, bodies) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    let got = all_accounts(&bodies.lock().unwrap());
    assert_eq!((got["9tz_2"].as_str(), got["7tz_1"].as_str()), ("pool-b-uuid", "pool-b-uuid"));
    mock.assert();
    // Cached per log, and shown by status.
    let status = proxy::status_report(&fx.paths, None, None, None, now());
    assert!(status.contains(&format!("read as {offset} instead of this machine's local time")), "{status}");
}

/// Review MED: only sessions a hook recorded as proxied calibrate: a session
/// credited from the log only because mode is on does not count.
#[test]
fn calibration_needs_two_hook_recorded_sessions() {
    let fx = fixture();
    calibration_fixture(&fx, false);
    let dry = sync::dry_run_with(&fx.paths, &fx.creds, &opts(), false, Some("on"), now()).unwrap();
    assert!(!clock_warning(&dry), "{:?}", dry.warnings);
    let (server, mock, _) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    sync::run_with(&fx.paths, &fx.creds, &client, &opts(), Some("on"), now()).unwrap();
    mock.assert();
    assert!(!fx.paths.proxy_log_clock().exists(), "nothing tried, nothing recorded");
}

/// Review MED: an offset that would cost a session that matches now its
/// matches is not kept.
#[test]
fn calibration_never_costs_a_matching_session_its_matches() {
    let fx = fixture();
    calibration_fixture(&fx, true);
    // A session without a hook record whose request is logged in our local
    // time (in a rotated file) matches with the offset in use.
    write_log_as(&fx, "main-2026-10-01T09-00-00.log", &[
        ("2026-10-01T10:20:00Z", Some("2026-10-01T10:20:21Z"), "l1", "claude:5...", "pool-a.json"),
    ], &local);
    transcript(&fx, "5inf", &["2026-10-01T10:20:20Z"]);
    let dry = sync::dry_run(&fx.paths, &fx.creds, &opts(), false, now()).unwrap();
    assert!(!clock_warning(&dry), "{:?}", dry.warnings);
    let s = dry.sessions.iter().find(|s| s.session_id == "9tz").unwrap();
    assert!(s.accounts.keys().all(|(a, _)| a == "shared@example.com"), "{:?}", s.accounts);
    // Without that session the same log is calibrated.
    fs::remove_file(fx.paths.projects_dir().join("-home-dev-5inf/5inf.jsonl")).unwrap();
    assert!(clock_warning(&sync::dry_run(&fx.paths, &fx.creds, &opts(), false, now()).unwrap()));
}

#[test]
fn a_missing_transcript_is_an_error() {
    let fx = fixture();
    let missing = SyncOptions { transcript: Some(fx.paths.projects_dir().join("nope/none.jsonl")), wait_for_lock: Duration::ZERO };
    let client = ApiClient::new("http://127.0.0.1:9", Some("cct_test")); // never contacted
    let err = sync::run(&fx.paths, &fx.creds, &client, &missing, now()).unwrap_err().to_string();
    assert!(err.contains("no transcript at") && err.contains("none.jsonl"), "{err}");
    assert!(sync::dry_run(&fx.paths, &fx.creds, &missing, false, now()).is_err());
}

#[test]
fn dry_run_warns_about_an_invalid_proxy_mode_override() {
    let fx = fixture();
    let dry = sync::dry_run_with(&fx.paths, &fx.creds, &opts(), false, Some("maybe"), now()).unwrap();
    assert!(dry.warnings.iter().any(|w| w.contains("ignoring CCTRACK_PROXY_MODE=\"maybe\"")), "{:?}", dry.warnings);
    assert_eq!(dry.mode.unwrap().0, cctrack::proxy::ProxyMode::Auto);
    let dry = sync::dry_run_with(&fx.paths, &fx.creds, &opts(), false, Some("off"), now()).unwrap();
    assert!(dry.warnings.is_empty(), "{:?}", dry.warnings);
}

#[test]
fn failed_title_updates_are_reported() {
    let fx = fixture();
    append(&fx.transcript, "\n{\"type\":\"ai-title\",\"aiTitle\":\"First\",\"sessionId\":\"sess-1\"}\n");
    let (server, mock, _) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    mock.assert();
    append(&fx.transcript, "{\"type\":\"custom-title\",\"customTitle\":\"Renamed\",\"sessionId\":\"sess-1\"}\n");
    let mut server = mockito::Server::new();
    let down = server.mock("POST", "/api/ingest").with_status(500).with_body(r#"{"error":"db down"}"#).expect(1).create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    down.assert();
    assert_eq!(report.titles_sent, 0);
    assert!(report.title_errors.len() == 1 && report.title_errors[0].contains("title update failed"), "{:?}", report.title_errors);
    assert!(fs::read_to_string(fx.paths.sync_log()).unwrap().contains("title update failed"));
}

#[test]
fn an_invalid_proxy_mode_override_is_logged_by_a_real_sync() {
    let fx = fixture();
    let (server, mock, _) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    sync::run_with(&fx.paths, &fx.creds, &client, &opts(), Some("maybe"), now()).unwrap();
    mock.assert();
    let log = fs::read_to_string(fx.paths.sync_log()).unwrap();
    assert!(log.contains("ignoring CCTRACK_PROXY_MODE=\"maybe\""), "{log}");
}

/// Review MED: sessions uploaded before sessions.json existed (cctrack < 0.2.0)
/// had their titles in bytes already read and no uploaded range: a one-time
/// pass sends their titles without re-uploading anything or moving offsets.
#[test]
fn titles_of_sessions_uploaded_before_session_state_existed_are_sent_once() {
    let fx = fixture();
    let content = fs::read_to_string(&fx.transcript).unwrap();
    fs::write(&fx.transcript, format!("{}\n{content}\n", r#"{"type":"ai-title","aiTitle":"Old session","sessionId":"sess-1"}"#)).unwrap();
    // Uploaded by an older cctrack: offsets, but no sessions.json.
    let (server, mock, _) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    mock.assert();
    fs::remove_file(fx.paths.sessions()).unwrap();
    let offsets_before = fs::read(fx.paths.offsets()).unwrap();

    // The dry run shows it and changes nothing.
    let dry = sync::dry_run(&fx.paths, &fx.creds, &opts(), false, now()).unwrap();
    let s = dry.sessions.iter().find(|s| s.session_id == "sess-1").unwrap();
    assert_eq!((s.title.as_deref(), s.title_pending, s.entries), (Some("Old session"), true, 0), "{s:?}");
    assert!(!fx.paths.sessions().exists(), "a dry run changes no state");

    // A per-turn sync of another session also catches it up.
    let other = transcript(&fx, "sess-2", &["2026-10-01T11:00:00Z"]);
    let one = SyncOptions { transcript: Some(other), wait_for_lock: Duration::ZERO };
    let (server, mock, bodies) = recording_server(2);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let report = sync::run(&fx.paths, &fx.creds, &client, &one, now()).unwrap();
    mock.assert();
    assert_eq!((report.entries_sent, report.titles_sent), (1, 1));
    let bodies = bodies.lock().unwrap();
    let title_only = bodies.iter().find(|b| b["entries"].as_array().unwrap().is_empty()).unwrap();
    let s = sessions_of(title_only);
    assert_eq!(s.len(), 1, "{title_only}");
    assert_eq!((s[0]["session_id"].as_str(), s[0]["title"].as_str()), (Some("sess-1"), Some("Old session")));
    // The range uploaded before (msg_1 to msg_4; msg_old predates login).
    assert_eq!((s[0]["started_at"].as_str(), s[0]["ended_at"].as_str()), (Some("2026-10-01T10:00:00Z"), Some("2026-10-01T10:05:00Z")));
    assert!(bodies.iter().all(|b| b["entries"].as_array().unwrap().iter().all(|e| e["session_id"] == "sess-2")), "nothing re-uploaded");
    let offsets = Offsets::load(&fx.paths.offsets());
    let before: Offsets = serde_json::from_slice(&offsets_before).unwrap();
    assert_eq!(offsets.get(&fx.transcript), before.get(&fx.transcript), "the cursor stays");
    drop(bodies);

    // Once: the next sync has nothing to send.
    let (server, mock, _) = recording_server(0);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let report = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap();
    assert_eq!((report.entries_sent, report.titles_sent), (0, 0));
    mock.assert();
}

// ---- HTTP 426: this cctrack is out of date ----

const OUTDATED_SETUP: &str = "https://github.com/rish-kun/claude-tracker/releases/download/cli-v9.9.0/setup.md";
const OUTDATED_MSG: &str = "cctrack 0.3.0 is out of date: the latest is 9.9.0. Update cctrack (see \
    https://github.com/rish-kun/claude-tracker/releases/download/cli-v9.9.0/setup.md, section \"Update cctrack\") and try again.";

fn outdated_body() -> String {
    serde_json::json!({ "error": OUTDATED_MSG, "latest": "9.9.0", "setup_url": OUTDATED_SETUP }).to_string()
}

/// A 426 is retried later: nothing advanced, nothing quarantined, the login stands.
fn assert_kept_for_retry(fx: &Fixture) {
    let offsets = Offsets::load(&fx.paths.offsets());
    assert_eq!(offsets.get(&fx.transcript).offset, 0, "main transcript offset must not advance");
    assert_eq!(offsets.get(&subagent_path(fx)).offset, 0, "subagent offset must not advance");
    assert!(!fx.paths.quarantine().exists(), "nothing may be quarantined");
    assert!(!fx.paths.last_snapshot().exists());
    assert!(matches!(credentials::load(&fx.paths), AuthState::LoggedIn(c) if !c.invalid), "credentials must stay valid");
}

#[test]
fn outdated_cli_keeps_offsets_quarantines_nothing_and_keeps_the_login() {
    let fx = fixture();
    let mut server = mockito::Server::new();
    let mock = server
        .mock("POST", "/api/ingest")
        .match_header("user-agent", concat!("cctrack/", env!("CARGO_PKG_VERSION")))
        .with_status(426)
        .with_body(outdated_body())
        .expect(2)
        .create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    for _ in 0..2 {
        let err = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap_err();
        assert_eq!(err.to_string(), OUTDATED_MSG, "the server's message reaches the user");
        assert_kept_for_retry(&fx);
    }
    mock.assert();

    // Once cctrack is updated, the same usage uploads.
    let (server, mock, _) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    assert_eq!(sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap().entries_sent, 2);
    mock.assert();
}

#[test]
fn outdated_cli_with_nothing_pending_reports_it_and_keeps_the_login() {
    let fx = fixture();
    drained(&fx);
    let mut server = mockito::Server::new();
    let me = server.mock("GET", "/api/me").with_status(426).with_body(outdated_body()).expect(1).create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let err = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap_err();
    assert_eq!(err.to_string(), OUTDATED_MSG);
    assert!(matches!(credentials::load(&fx.paths), AuthState::LoggedIn(c) if !c.invalid));
    me.assert();
}

#[test]
fn outdated_cli_title_update_fails_the_sync_and_is_retried() {
    let fx = fixture();
    append(&fx.transcript, "\n");
    drained(&fx);
    // A title change with no new usage: sent on its own.
    append(&fx.transcript, "{\"type\":\"custom-title\",\"customTitle\":\"Later title\",\"sessionId\":\"sess-1\"}\n");
    let mut server = mockito::Server::new();
    let mock = server.mock("POST", "/api/ingest").with_status(426).with_body(outdated_body()).expect(1).create();
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    let err = sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap_err();
    assert_eq!(err.to_string(), OUTDATED_MSG);
    assert!(!fx.paths.quarantine().exists());
    mock.assert();

    let (server, mock, bodies) = recording_server(1);
    let client = ApiClient::new(&server.url(), Some("cct_test"));
    assert_eq!(sync::run(&fx.paths, &fx.creds, &client, &opts(), now()).unwrap().titles_sent, 1);
    assert_eq!(sessions_of(&bodies.lock().unwrap()[0])[0]["title"], "Later title");
    mock.assert();
}

/// The cctrack binary with every location inside the fixture's directory.
fn cctrack_cmd(fx: &Fixture, args: &[&str]) -> std::process::Command {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_cctrack"));
    cmd.args(args)
        .env("HOME", fx._dir.path())
        .env("CCTRACK_CONFIG_DIR", &fx.paths.config_dir)
        .env("CCTRACK_STATE_DIR", &fx.paths.state_dir)
        .env("CLAUDE_CONFIG_DIR", &fx.paths.claude_dir)
        .env("CCTRACK_CLIPROXY_DIR", &fx.paths.cliproxy_dir)
        .stdin(std::process::Stdio::null());
    for var in ["CCTRACK_SERVER", "CCTRACK_PROXY_MODE", "ANTHROPIC_BASE_URL", "XDG_CONFIG_HOME", "XDG_STATE_HOME"] {
        cmd.env_remove(var);
    }
    cmd
}

#[test]
fn outdated_cli_commands_print_the_servers_message_and_fail() {
    let fx = fixture();
    fs::write(fx.paths.claude_dir.join(".claude.json"), CLAUDE_JSON).unwrap();
    let mut server = mockito::Server::new();
    credentials::save(&fx.paths, &Credentials { server: server.url(), ..fx.creds.clone() }).unwrap();
    let ingest = server.mock("POST", "/api/ingest").with_status(426).with_body(outdated_body()).expect(3).create();
    let me = server.mock("GET", "/api/me").with_status(426).with_body(outdated_body()).expect(2).create();
    let summary = server.mock("POST", mockito::Matcher::Regex("^/api/sessions/".into())).expect(0).create();

    // Like any other server error: the message on stderr, exit code 1, no success claim.
    for args in [&["sync"][..], &["whoami"], &["upload", "--session", "sess-1", "--summary", "did things"]] {
        let out = cctrack_cmd(&fx, args).output().unwrap();
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stdout} {stderr}");
        assert_eq!(stderr.trim(), format!("cctrack: {OUTDATED_MSG}"), "{args:?}");
        assert_eq!(stdout, "", "{args:?}");
    }
    // The hooks' quiet sync fails too, and logs the message.
    let out = cctrack_cmd(&fx, &["sync", "--quiet"]).output().unwrap();
    assert_eq!((out.status.code(), out.stderr.len()), (Some(1), 0));
    assert!(fs::read_to_string(fx.paths.sync_log()).unwrap().contains(OUTDATED_MSG));
    assert_kept_for_retry(&fx);

    // SessionStart tells the agent to update cctrack first and keeps the login.
    let mut child = cctrack_cmd(&fx, &["hook", "session-start"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), br#"{"session_id":"sess-1"}"#).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let ctx = v["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    assert!(ctx.starts_with("cctrack: OUT OF DATE. Before doing any other work"), "{ctx}");
    assert!(ctx.contains(&format!("cctrack {} is out of date (the latest is 9.9.0)", env!("CARGO_PKG_VERSION"))), "{ctx}");
    assert!(ctx.contains(&format!("the \"Update cctrack\" section of {OUTDATED_SETUP}")), "{ctx}");
    assert!(ctx.contains("restart Claude Code") && ctx.contains("Claude Code session id: sess-1"), "{ctx}");
    assert_kept_for_retry(&fx);

    ingest.assert();
    me.assert();
    summary.assert();
}

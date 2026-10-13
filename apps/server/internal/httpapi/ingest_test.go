package httpapi

import (
	"context"
	"net/http"
	"strings"
	"testing"
	"time"

	"cctrack/server/internal/api"
)

func str(s string) *string { return &s }

// ingestOK posts a batch, fails the test unless it returns 200, and decodes the result.
func (h *harness) ingestOK(tok string, req api.IngestRequest) api.IngestResponse {
	h.t.Helper()
	rec := h.do("POST", "/api/ingest", req, bearer(tok))
	if rec.Code != http.StatusOK {
		h.t.Fatalf("ingest: %d %s", rec.Code, rec.Body)
	}
	return decode[api.IngestResponse](h.t, rec)
}

// scan runs a single-row query and fails the test on any error.
func (h *harness) scan(query string, dest ...any) {
	h.t.Helper()
	if err := h.store.Pool().QueryRow(context.Background(), query).Scan(dest...); err != nil {
		h.t.Fatalf("%s: %v", query, err)
	}
}

func (h *harness) exec(query string) {
	h.t.Helper()
	if _, err := h.store.Pool().Exec(context.Background(), query); err != nil {
		h.t.Fatalf("%s: %v", query, err)
	}
}

func TestIngestIsIdempotent(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	req := ingestFixture("s1", "acc-1",
		entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 100, 50),
		entry("s1", "acc-1", "m2", "claude-opus-4-5", t0.Add(time.Minute), 10, 5))
	if got := h.ingestOK(tok, req); got.Accepted != 2 || got.Duplicates != 0 {
		t.Fatalf("first = %+v", got)
	}
	if got := h.ingestOK(tok, req); got.Accepted != 0 || got.Duplicates != 2 {
		t.Fatalf("resend = %+v", got)
	}
}

func TestIngestRejectsOtherUsersSession(t *testing.T) {
	h := newHarness(t)
	alice := h.loginCLI("alice@example.com")
	bob := h.loginCLI("bob@example.com")
	h.ingestOK(alice, ingestFixture("shared", "acc-1",
		entry("shared", "acc-1", "m1", "claude-opus-4-5", t0, 1, 1)))
	rec := h.do("POST", "/api/ingest", ingestFixture("shared", "acc-1",
		entry("shared", "acc-1", "m2", "claude-opus-4-5", t0, 1, 1)), bearer(bob))
	if rec.Code != http.StatusForbidden {
		t.Fatalf("got %d, want 403", rec.Code)
	}
	var n int
	h.scan("SELECT count(*) FROM usage_entries", &n)
	if n != 1 {
		t.Fatalf("entries = %d, want 1 (bob's batch must be rolled back)", n)
	}
	var owner string
	h.scan("SELECT u.email FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.session_id = 'shared'", &owner)
	if owner != "alice@example.com" {
		t.Fatalf("session owner = %q", owner)
	}
}

func TestIngestAccountPlanGuessAndAdminEditsSurvive(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	req := ingestFixture("s1", "acc-max", entry("s1", "acc-max", "m1", "claude-opus-4-5", t0, 1, 1))
	req.Accounts[0].OrgType = str("claude_max")
	req.Accounts[0].RateLimitTier = str("default_claude_max_20x")
	req.Accounts[0].Email = str("shared@example.com")
	h.ingestOK(tok, req)

	const q = "SELECT plan, label FROM claude_accounts WHERE account_uuid='acc-max'"
	var plan, label string
	h.scan(q, &plan, &label)
	if plan != "max_20x" {
		t.Fatalf("plan = %q", plan)
	}
	h.exec("UPDATE claude_accounts SET plan='team', label='Shared Max' WHERE account_uuid='acc-max'")
	req.Entries[0].MessageId = "m2"
	if got := h.ingestOK(tok, req); got.Accepted != 1 {
		t.Fatalf("second ingest = %+v, want 1 accepted", got)
	}
	h.scan(q, &plan, &label)
	if plan != "team" || label != "Shared Max" {
		t.Fatalf("admin edits overwritten: plan=%q label=%q", plan, label)
	}
}

// A plan first seen without a tier is "unknown"; a later ingest that carries
// the tier fills it in, but once set (by guess or admin) it is never replaced.
func TestIngestPlanUpgradesFromUnknown(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	const q = "SELECT plan FROM claude_accounts WHERE account_uuid='acc-max'"
	var plan string

	req := ingestFixture("s1", "acc-max", entry("s1", "acc-max", "m1", "claude-opus-4-5", t0, 1, 1))
	req.Accounts[0].OrgType = str("claude_max")
	h.ingestOK(tok, req)
	if h.scan(q, &plan); plan != "unknown" {
		t.Fatalf("no tier: plan = %q, want unknown", plan)
	}

	req.Accounts[0].RateLimitTier = str("default_claude_max_20x")
	req.Entries[0].MessageId = "m2"
	h.ingestOK(tok, req)
	if h.scan(q, &plan); plan != "max_20x" {
		t.Fatalf("with tier: plan = %q, want max_20x", plan)
	}

	h.exec("UPDATE claude_accounts SET plan='team' WHERE account_uuid='acc-max'")
	req.Entries[0].MessageId = "m3"
	h.ingestOK(tok, req)
	if h.scan(q, &plan); plan != "team" {
		t.Fatalf("after admin edit: plan = %q, want team", plan)
	}
}

func TestIngestAPIKeyAccount(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	req := ingestFixture("s1", "api_key", entry("s1", "api_key", "m1", "claude-opus-4-5", t0, 1, 1))
	h.ingestOK(tok, req)
	var plan, label, uuid string
	h.scan("SELECT plan, label, account_uuid FROM claude_accounts", &plan, &label, &uuid)
	if plan != "api_key" || label != "API key (laptop)" || !strings.HasPrefix(uuid, "api_key:") {
		t.Fatalf("got plan=%q label=%q uuid=%q", plan, label, uuid)
	}
}

func TestIngestLimitSnapshotDedup(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	pct := 42.0
	req := ingestFixture("s1", "acc-1", entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 1, 1))
	req.LimitSnapshot = &api.LimitSnapshotIn{AccountUuid: "acc-1", CapturedAt: t0, FiveHourPct: &pct}
	h.ingestOK(tok, req)
	h.ingestOK(tok, req)
	var n int
	h.scan("SELECT count(*) FROM limit_snapshots", &n)
	if n != 1 {
		t.Fatalf("snapshots = %d, want 1", n)
	}
}

func TestIngestRequiresBearer(t *testing.T) {
	h := newHarness(t)
	if rec := h.do("POST", "/api/ingest", ingestFixture("s1", "a")); rec.Code != http.StatusUnauthorized {
		t.Fatalf("got %d", rec.Code)
	}
}

func TestSessionSummary(t *testing.T) {
	h := newHarness(t)
	alice := h.loginCLI("alice@example.com")
	bob := h.loginCLI("bob@example.com")
	h.ingestOK(alice, ingestFixture("s1", "acc-1",
		entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 1, 1)))

	if rec := h.do("POST", "/api/sessions/s1/summary", map[string]string{"summary": "Added login flow."}, bearer(alice)); rec.Code != http.StatusNoContent {
		t.Fatalf("set: %d %s", rec.Code, rec.Body)
	}
	if rec := h.do("POST", "/api/sessions/s1/summary", map[string]string{"summary": "hijack"}, bearer(bob)); rec.Code != http.StatusNotFound {
		t.Fatalf("other user: %d, want 404", rec.Code)
	}
	if rec := h.do("POST", "/api/sessions/s1/summary", map[string]string{"summary": strings.Repeat("x", 501)}, bearer(alice)); rec.Code != http.StatusBadRequest {
		t.Fatalf("too long: %d, want 400", rec.Code)
	}
	var summary string
	h.scan("SELECT summary FROM sessions WHERE session_id='s1'", &summary)
	if summary != "Added login flow." {
		t.Fatalf("summary = %q", summary)
	}
}

// A real browser session cookie (with the CSRF header) is refused on the
// CLI-only write endpoints and nothing is stored.
func TestIngestAndSummaryRejectSessionCookie(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	h.ingestOK(tok, ingestFixture("s1", "acc-1", entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 1, 1)))
	c := h.loginWeb("dev@example.com")

	rec := h.do("POST", "/api/ingest", ingestFixture("s2", "acc-1",
		entry("s2", "acc-1", "m2", "claude-opus-4-5", t0, 1, 1)), withCookie(c), withCSRF())
	if rec.Code != http.StatusForbidden || !strings.Contains(rec.Body.String(), "use the cctrack CLI") {
		t.Fatalf("cookie ingest: %d %s", rec.Code, rec.Body)
	}
	rec = h.do("POST", "/api/sessions/s1/summary", map[string]string{"summary": "from browser"}, withCookie(c), withCSRF())
	if rec.Code != http.StatusForbidden {
		t.Fatalf("cookie summary: %d %s", rec.Code, rec.Body)
	}
	var n int
	h.scan("SELECT count(*) FROM usage_entries", &n)
	if n != 1 {
		t.Fatalf("entries = %d, want 1", n)
	}
	h.scan("SELECT count(*) FROM sessions WHERE summary IS NOT NULL", &n)
	if n != 0 {
		t.Fatalf("summary written via cookie")
	}
}

// TestIngestSessionMetadataLatestWins: replaying an older batch for a session
// must not regress its account, device, cwd/project or branch; a newer batch
// still updates them. started_at/ended_at only ever widen.
func TestIngestSessionMetadataLatestWins(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	batch := func(acct, host, cwd, branch, msg string, ts time.Time) api.IngestRequest {
		req := ingestFixture("s1", acct, entry("s1", acct, msg, "claude-opus-4-5", ts, 1, 1))
		req.Device.Hostname = host
		req.Sessions[0].Cwd, req.Sessions[0].GitBranch = str(cwd), str(branch)
		req.Sessions[0].StartedAt, req.Sessions[0].EndedAt = ts, ts
		return req
	}
	const q = `SELECT a.account_uuid, d.hostname, s.cwd, s.project, s.git_branch, s.started_at, s.ended_at
		FROM sessions s JOIN claude_accounts a ON a.id = s.account_id JOIN devices d ON d.id = s.device_id
		WHERE s.session_id = 's1'`
	type row struct {
		acct, host, cwd, project, branch string
		start, end                       time.Time
	}
	get := func() row {
		var r row
		h.scan(q, &r.acct, &r.host, &r.cwd, &r.project, &r.branch, &r.start, &r.end)
		return r
	}

	newer := batch("acc-new", "laptop", "/home/dev/new-proj", "feature", "m2", t0.Add(2*time.Hour))
	h.ingestOK(tok, newer)
	h.ingestOK(tok, batch("acc-old", "desktop", "/home/dev/old-proj", "main", "m1", t0))
	got := get()
	if got.acct != "acc-new" || got.host != "laptop" || got.cwd != "/home/dev/new-proj" ||
		got.project != "new-proj" || got.branch != "feature" {
		t.Fatalf("older batch regressed metadata: %+v", got)
	}
	if !got.start.Equal(t0) || !got.end.Equal(t0.Add(2*time.Hour)) {
		t.Fatalf("bounds = %v..%v, want widened to %v..%v", got.start, got.end, t0, t0.Add(2*time.Hour))
	}

	// Replaying the newer batch verbatim is a no-op for metadata.
	h.ingestOK(tok, newer)
	if got := get(); got.acct != "acc-new" || got.branch != "feature" {
		t.Fatalf("replay of newest batch: %+v", got)
	}

	h.ingestOK(tok, batch("acc-old", "desktop", "/home/dev/old-proj", "main", "m3", t0.Add(3*time.Hour)))
	if got := get(); got.acct != "acc-old" || got.host != "desktop" || got.branch != "main" || got.project != "old-proj" {
		t.Fatalf("newer batch did not update metadata: %+v", got)
	}
}

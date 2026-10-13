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

func TestIngestIsIdempotent(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	req := ingestFixture("s1", "acc-1",
		entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 100, 50),
		entry("s1", "acc-1", "m2", "claude-opus-4-5", t0.Add(time.Minute), 10, 5))
	rec := h.do("POST", "/api/ingest", req, bearer(tok))
	if rec.Code != 200 {
		t.Fatalf("ingest: %d %s", rec.Code, rec.Body)
	}
	if got := decode[api.IngestResponse](t, rec); got.Accepted != 2 || got.Duplicates != 0 {
		t.Fatalf("first = %+v", got)
	}
	rec = h.do("POST", "/api/ingest", req, bearer(tok))
	if got := decode[api.IngestResponse](t, rec); got.Accepted != 0 || got.Duplicates != 2 {
		t.Fatalf("resend = %+v", got)
	}
}

func TestIngestRejectsOtherUsersSession(t *testing.T) {
	h := newHarness(t)
	alice := h.loginCLI("alice@example.com")
	bob := h.loginCLI("bob@example.com")
	h.do("POST", "/api/ingest", ingestFixture("shared", "acc-1",
		entry("shared", "acc-1", "m1", "claude-opus-4-5", t0, 1, 1)), bearer(alice))
	rec := h.do("POST", "/api/ingest", ingestFixture("shared", "acc-1",
		entry("shared", "acc-1", "m2", "claude-opus-4-5", t0, 1, 1)), bearer(bob))
	if rec.Code != http.StatusForbidden {
		t.Fatalf("got %d, want 403", rec.Code)
	}
	var n int
	_ = h.store.Pool().QueryRow(context.Background(), "SELECT count(*) FROM usage_entries").Scan(&n)
	if n != 1 {
		t.Fatalf("entries = %d, want 1 (bob's batch must be rolled back)", n)
	}
}

func TestIngestAccountPlanGuessAndAdminEditsSurvive(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	req := ingestFixture("s1", "acc-max", entry("s1", "acc-max", "m1", "claude-opus-4-5", t0, 1, 1))
	req.Accounts[0].OrgType = str("claude_max")
	req.Accounts[0].RateLimitTier = str("default_claude_max_20x")
	req.Accounts[0].Email = str("shared@example.com")
	h.do("POST", "/api/ingest", req, bearer(tok))

	ctx := context.Background()
	var plan, label string
	_ = h.store.Pool().QueryRow(ctx, "SELECT plan, label FROM claude_accounts WHERE account_uuid='acc-max'").Scan(&plan, &label)
	if plan != "max_20x" {
		t.Fatalf("plan = %q", plan)
	}
	_, _ = h.store.Pool().Exec(ctx, "UPDATE claude_accounts SET plan='team', label='Shared Max' WHERE account_uuid='acc-max'")
	req.Entries[0].MessageId = "m2"
	h.do("POST", "/api/ingest", req, bearer(tok))
	_ = h.store.Pool().QueryRow(ctx, "SELECT plan, label FROM claude_accounts WHERE account_uuid='acc-max'").Scan(&plan, &label)
	if plan != "team" || label != "Shared Max" {
		t.Fatalf("admin edits overwritten: plan=%q label=%q", plan, label)
	}
}

func TestIngestAPIKeyAccount(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	req := ingestFixture("s1", "api_key", entry("s1", "api_key", "m1", "claude-opus-4-5", t0, 1, 1))
	if rec := h.do("POST", "/api/ingest", req, bearer(tok)); rec.Code != 200 {
		t.Fatalf("%d %s", rec.Code, rec.Body)
	}
	var plan, label, uuid string
	_ = h.store.Pool().QueryRow(context.Background(),
		"SELECT plan, label, account_uuid FROM claude_accounts").Scan(&plan, &label, &uuid)
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
	h.do("POST", "/api/ingest", req, bearer(tok))
	h.do("POST", "/api/ingest", req, bearer(tok))
	var n int
	_ = h.store.Pool().QueryRow(context.Background(), "SELECT count(*) FROM limit_snapshots").Scan(&n)
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
	h.do("POST", "/api/ingest", ingestFixture("s1", "acc-1",
		entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 1, 1)), bearer(alice))

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
	_ = h.store.Pool().QueryRow(context.Background(), "SELECT summary FROM sessions WHERE session_id='s1'").Scan(&summary)
	if summary != "Added login flow." {
		t.Fatalf("summary = %q", summary)
	}
}

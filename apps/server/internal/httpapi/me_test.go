package httpapi

import (
	"math"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"testing"
	"time"

	"cctrack/server/internal/api"
)

func usageURL(from, to time.Time, tz string) string {
	q := url.Values{"from": {from.Format(time.RFC3339)}, "to": {to.Format(time.RFC3339)}}
	if tz != "" {
		q.Set("tz", tz)
	}
	return "/api/me/usage?" + q.Encode()
}

// getOK performs an authenticated GET, fails unless it returns 200, and decodes the body.
func getOK[T any](h *harness, path, tok string) T {
	h.t.Helper()
	rec := h.do("GET", path, nil, bearer(tok))
	if rec.Code != http.StatusOK {
		h.t.Fatalf("GET %s: %d %s", path, rec.Code, rec.Body)
	}
	return decode[T](h.t, rec)
}

func TestMyUsageTotalsCostAndBreakdowns(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	other := h.loginCLI("other@example.com")

	req := ingestFixture("s1", "acc-1",
		entry("s1", "acc-1", "m1", "claude-opus-4-5-20251101", t0, 1_000_000, 100_000), // $5 + $2.5
		entry("s1", "acc-1", "m2", "mystery-model", t0, 10, 0))                         // unpriced
	req.Accounts = append(req.Accounts, ingestFixture("s2", "acc-2").Accounts...)
	req.Sessions = append(req.Sessions, api.IngestSession{SessionId: "s2", StartedAt: t0, EndedAt: t0})
	req.Entries = append(req.Entries, entry("s2", "acc-2", "m3", "claude-opus-4-5", t0, 2_000_000, 0)) // $10
	h.ingestOK(tok, req)
	// Another user's usage must not leak into mine.
	h.ingestOK(other, ingestFixture("s9", "acc-1",
		entry("s9", "acc-1", "m9", "claude-opus-4-5", t0, 9_000_000, 0)))

	u := getOK[api.Usage](h, usageURL(t0.Add(-time.Hour), t0.Add(time.Hour), ""), tok)
	if math.Abs(u.Totals.CostUsd-17.5) > 1e-9 || u.Totals.UnpricedTokens != 10 {
		t.Fatalf("totals = %+v", u.Totals)
	}
	if len(u.ByAccount) != 2 || u.ByAccount[0].Label != "acc-2" {
		t.Fatalf("by_account = %+v", u.ByAccount)
	}
	if len(u.ByModel) != 2 || u.ByModel[0].Key != "claude-opus-4-5" {
		t.Fatalf("by_model = %+v", u.ByModel)
	}
}

func TestMyUsageBucketsDaysInRequestedTZ(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	late := time.Date(2026, 10, 1, 23, 30, 0, 0, time.UTC) // 05:00 on Oct 2 in Asia/Kolkata
	h.ingestOK(tok, ingestFixture("s1", "acc-1",
		entry("s1", "acc-1", "m1", "claude-opus-4-5", late, 1, 1)))

	u := getOK[api.Usage](h, usageURL(late.Add(-24*time.Hour), late.Add(24*time.Hour), "Asia/Kolkata"), tok)
	if len(u.DailyByModel) != 1 || u.DailyByModel[0].Date != "2026-10-02" {
		t.Fatalf("daily = %+v", u.DailyByModel)
	}
	u = getOK[api.Usage](h, usageURL(late.Add(-24*time.Hour), late.Add(24*time.Hour), ""), tok)
	if len(u.DailyByModel) != 1 || u.DailyByModel[0].Date != "2026-10-01" {
		t.Fatalf("UTC daily = %+v", u.DailyByModel)
	}
}

func TestMyUsageRejectsBadRange(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	for _, u := range []string{
		usageURL(t0, t0.Add(time.Hour), "Mars/Olympus"),
		usageURL(t0, t0.Add(time.Hour), "Local"),
		usageURL(t0, t0.Add(-time.Hour), ""),
		usageURL(t0, t0.Add(500*24*time.Hour), ""),
		"/api/me/usage?from=yesterday&to=today",
	} {
		if rec := h.do("GET", u, nil, bearer(tok)); rec.Code != http.StatusBadRequest {
			t.Errorf("%s: got %d, want 400", u, rec.Code)
		}
	}
}

// Required arrays must serialise as [] rather than null when there is no usage.
func TestMyUsageEmptyHasArrays(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	rec := h.do("GET", usageURL(t0.Add(-time.Hour), t0.Add(time.Hour), ""), nil, bearer(tok))
	if rec.Code != http.StatusOK {
		t.Fatalf("usage: %d %s", rec.Code, rec.Body)
	}
	body := rec.Body.String()
	for _, k := range []string{`"by_account":[]`, `"by_model":[]`, `"daily_by_model":[]`, `"limits":[]`} {
		if !strings.Contains(body, k) {
			t.Errorf("missing %s in %s", k, body)
		}
	}
	rec = h.do("GET", "/api/me/sessions", nil, bearer(tok))
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), `"items":[]`) {
		t.Fatalf("sessions: %d %s", rec.Code, rec.Body)
	}
}

func TestMyUsageLatestLimit(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	for i, pct := range []float64{10, 55} {
		p := pct
		req := ingestFixture("s1", "acc-1", entry("s1", "acc-1", "m"+string(rune('a'+i)), "claude-opus-4-5", t0, 1, 1))
		req.LimitSnapshot = &api.LimitSnapshotIn{AccountUuid: "acc-1", CapturedAt: t0.Add(time.Duration(i) * time.Minute), FiveHourPct: &p}
		h.ingestOK(tok, req)
	}
	u := getOK[api.Usage](h, usageURL(t0.Add(-time.Hour), t0.Add(time.Hour), ""), tok)
	if len(u.Limits) != 1 || u.Limits[0].FiveHourPct == nil || *u.Limits[0].FiveHourPct != 55 {
		t.Fatalf("limits = %+v", u.Limits)
	}
	// Cross-track contract: the web app joins limits to by_account rows on
	// Key == decimal account id, so every limit must have a matching row.
	keys := map[string]bool{}
	for _, row := range u.ByAccount {
		keys[row.Key] = true
	}
	for _, l := range u.Limits {
		if !keys[strconv.FormatInt(l.AccountId, 10)] {
			t.Errorf("limit for account %d has no by_account row; by_account = %+v", l.AccountId, u.ByAccount)
		}
	}
}

func TestMySessionsPagedWithCostAndSummary(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	h.ingestOK(tok, ingestFixture("old", "acc-1",
		entry("old", "acc-1", "m1", "claude-opus-4-5", t0, 1_000_000, 0)))
	h.ingestOK(tok, ingestFixture("new", "acc-1",
		entry("new", "acc-1", "m2", "claude-opus-4-5", t0.Add(time.Hour), 2_000_000, 0)))
	if rec := h.do("POST", "/api/sessions/new/summary", map[string]string{"summary": "Built the thing."}, bearer(tok)); rec.Code/100 != 2 {
		t.Fatalf("summary: %d %s", rec.Code, rec.Body)
	}

	page := getOK[api.SessionPage](h, "/api/me/sessions?limit=1", tok)
	if page.Total != 2 || len(page.Items) != 1 {
		t.Fatalf("page = %+v", page)
	}
	s := page.Items[0]
	if s.SessionId != "new" || s.Summary == nil || *s.Summary != "Built the thing." || math.Abs(s.Totals.CostUsd-10) > 1e-9 ||
		s.Account == nil || s.Project == nil || *s.Project != "proj" || s.DeviceHostname == nil || *s.DeviceHostname != "laptop" {
		t.Fatalf("item = %+v", s)
	}
	page = getOK[api.SessionPage](h, "/api/me/sessions?limit=1&offset=1", tok)
	if len(page.Items) != 1 || page.Items[0].SessionId != "old" {
		t.Fatalf("second page = %+v", page.Items)
	}
}

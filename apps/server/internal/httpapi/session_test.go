package httpapi

import (
	"encoding/json"
	"fmt"
	"math"
	"net/http"
	"strings"
	"testing"

	"cctrack/server/internal/api"
	"cctrack/server/internal/pricing"
	"cctrack/server/internal/store"
)

// seedTitledSession has alice ingest session s1 (titled, with main-thread and
// subagent usage) and an untitled, cwd-less session s2.
//
//	m1 main     claude-opus-4-5           1M input                 $5
//	m2 main     claude-opus-4-5-20251101  100k output + 2M cache   $2.5 + $1
//	m3 subagent claude-haiku-4-5          1M input                 $1
//	m4 subagent mystery-model             10 input + 2 output      unpriced
func seedTitledSession(t *testing.T, h *harness) (alice string) {
	t.Helper()
	alice = h.loginCLI("alice@example.com")
	m2 := entry("s1", "acc-1", "m2", "claude-opus-4-5-20251101", t0, 0, 100_000)
	m2.CacheRead = 2_000_000
	m3 := entry("s1", "acc-1", "m3", "claude-haiku-4-5", t0, 1_000_000, 0)
	m3.IsSubagent = true
	m4 := entry("s1", "acc-1", "m4", "mystery-model", t0, 10, 2)
	m4.IsSubagent = true
	req := ingestFixture("s1", "acc-1", entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 1_000_000, 0), m2, m3, m4)
	req.Sessions[0].Title = str("  Add session detail endpoint  ")
	req.Sessions = append(req.Sessions, api.IngestSession{SessionId: "s2", StartedAt: t0, EndedAt: t0})
	req.Entries = append(req.Entries, entry("s2", "acc-1", "m5", "claude-opus-4-5", t0, 1, 1))
	h.ingestOK(alice, req)
	return alice
}

func TestGetSessionAccess(t *testing.T) {
	h := newHarness(t)
	alice := seedTitledSession(t, h)
	aliceWeb := h.loginWeb("alice@example.com")
	bob := h.loginCLI("bob@example.com")
	bobWeb := h.loginWeb("bob@example.com")
	adminWeb := h.loginWeb("admin@example.com")
	adminCLI := h.loginCLI("admin@example.com")

	for _, c := range []struct {
		name string
		auth func(*http.Request)
		want int
	}{
		{"owner bearer", bearer(alice), 200},
		{"owner cookie", withCookie(aliceWeb), 200},
		{"admin cookie", withCookie(adminWeb), 200},
		{"other user bearer", bearer(bob), 404},
		{"other user cookie", withCookie(bobWeb), 404},
		// Admin powers are browser-session only, as on the /admin endpoints.
		{"admin CLI token", bearer(adminCLI), 404},
		{"anonymous", func(*http.Request) {}, 401},
	} {
		t.Run(c.name, func(t *testing.T) {
			rec := h.do("GET", "/api/sessions/s1", nil, c.auth)
			if rec.Code != c.want {
				t.Fatalf("status %d, want %d: %s", rec.Code, c.want, rec.Body)
			}
			if c.want == 200 {
				if d := decode[api.SessionDetail](t, rec); d.Session.SessionId != "s1" || d.Session.User.Email != "alice@example.com" {
					t.Fatalf("detail = %+v", d.Session)
				}
			}
		})
	}

	// A foreign session is indistinguishable from a missing one.
	foreign := h.do("GET", "/api/sessions/s1", nil, bearer(bob))
	missing := h.do("GET", "/api/sessions/no-such-session", nil, bearer(bob))
	if missing.Code != http.StatusNotFound || foreign.Body.String() != missing.Body.String() {
		t.Fatalf("foreign %d %q vs missing %d %q", foreign.Code, foreign.Body, missing.Code, missing.Body)
	}
	if rec := h.do("GET", "/api/sessions/bad%00id", nil, bearer(alice)); rec.Code != http.StatusNotFound {
		t.Fatalf("NUL id: %d %s", rec.Code, rec.Body)
	}
}

func TestGetSessionDetail(t *testing.T) {
	h := newHarness(t)
	alice := seedTitledSession(t, h)
	d := getOK[api.SessionDetail](h, "/api/sessions/s1", alice)

	s := d.Session
	if s.Title == nil || *s.Title != "Add session detail endpoint" || s.Cwd == nil || *s.Cwd != "/home/dev/proj" ||
		s.Project == nil || *s.Project != "proj" || s.Account == nil || s.DeviceHostname == nil {
		t.Fatalf("session = %+v", s)
	}
	near := func(a, b float64) bool { return math.Abs(a-b) < 1e-9 }
	if !near(s.Totals.CostUsd, 9.5) || s.Totals.UnpricedTokens != 12 || s.Totals.TotalTokens != 4_100_012 {
		t.Fatalf("session totals = %+v", s.Totals)
	}
	if d.Calls != 4 || d.PeakContextTokens != 2_000_000 {
		t.Fatalf("calls = %d, peak = %d", d.Calls, d.PeakContextTokens)
	}
	if d.Main.Calls != 2 || !near(d.Main.Totals.CostUsd, 8.5) || d.Main.Totals.UnpricedTokens != 0 ||
		d.Main.Totals.CacheRead != 2_000_000 || d.Main.Totals.Output != 100_000 {
		t.Fatalf("main = %+v", d.Main)
	}
	if d.Subagents.Calls != 2 || !near(d.Subagents.Totals.CostUsd, 1) || d.Subagents.Totals.UnpricedTokens != 12 ||
		d.Subagents.Totals.Input != 1_000_010 {
		t.Fatalf("subagents = %+v", d.Subagents)
	}
	want := []struct {
		model string
		cost  float64
		calls int64
	}{{"claude-opus-4-5", 8.5, 2}, {"claude-haiku-4-5", 1, 1}, {"mystery-model", 0, 1}}
	if len(d.ByModel) != len(want) {
		t.Fatalf("by_model = %+v", d.ByModel)
	}
	for i, w := range want {
		if m := d.ByModel[i]; m.Model != w.model || !near(m.Totals.CostUsd, w.cost) || m.Calls != w.calls {
			t.Fatalf("by_model[%d] = %+v, want %+v", i, m, w)
		}
	}
	if u := d.ByModel[2].Totals.UnpricedTokens; u != 12 {
		t.Fatalf("mystery-model unpriced = %d", u)
	}

	// Totals match what the session list reports for the same session.
	page := getOK[api.SessionPage](h, "/api/me/sessions", alice)
	for _, item := range page.Items {
		if item.SessionId == "s1" && item.Totals != s.Totals {
			t.Fatalf("list totals %+v != detail totals %+v", item.Totals, s.Totals)
		}
	}
}

// Session lists carry cwd and title, omitting them when unknown.
func TestSessionListsIncludeCwdAndTitle(t *testing.T) {
	h := newHarness(t)
	alice := seedTitledSession(t, h)
	admin := h.loginWeb("admin@example.com")

	check := func(name string, body []byte) {
		t.Helper()
		var page struct {
			Items []map[string]json.RawMessage `json:"items"`
		}
		if err := json.Unmarshal(body, &page); err != nil {
			t.Fatal(err)
		}
		if len(page.Items) != 2 {
			t.Fatalf("%s: items = %d", name, len(page.Items))
		}
		for _, it := range page.Items {
			var id string
			_ = json.Unmarshal(it["session_id"], &id)
			_, hasTitle := it["title"]
			_, hasCwd := it["cwd"]
			switch id {
			case "s1":
				if string(it["title"]) != `"Add session detail endpoint"` || string(it["cwd"]) != `"/home/dev/proj"` {
					t.Fatalf("%s: s1 = %s / %s", name, it["title"], it["cwd"])
				}
			case "s2":
				if hasTitle || hasCwd {
					t.Fatalf("%s: s2 should omit title/cwd: %v", name, it)
				}
			}
		}
	}
	rec := h.do("GET", "/api/me/sessions", nil, bearer(alice))
	if rec.Code != http.StatusOK {
		t.Fatalf("me sessions: %d %s", rec.Code, rec.Body)
	}
	check("me", rec.Body.Bytes())

	var aliceID int64
	for _, u := range adminGet[[]api.AdminUser](h, "/api/admin/users", admin) {
		if u.Email == "alice@example.com" {
			aliceID = u.Id
		}
	}
	rec = h.do("GET", fmt.Sprintf("/api/admin/users/%d/sessions", aliceID), nil, withCookie(admin))
	if rec.Code != http.StatusOK {
		t.Fatalf("admin sessions: %d %s", rec.Code, rec.Body)
	}
	check("admin", rec.Body.Bytes())
}

func TestIngestRejectsNULTitle(t *testing.T) {
	req := ingestFixture("s1", "acc-1", entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 1, 1))
	req.Sessions[0].Title = str("bad\x00title")
	if err := validateIngest(&req, t0); err == nil || !strings.Contains(err.Error(), "NUL") {
		t.Fatalf("err = %v", err)
	}
}

// TestSessionDetailAggregation runs without a database: the split, peak,
// call counts and by_model ordering (cost desc, then model).
func TestSessionDetailAggregation(t *testing.T) {
	table := pricing.NewTable([]pricing.Price{
		{Model: "claude-opus-4-5", Base: pricing.Rates{Input: 5, Output: 25}},
		{Model: "a-model", Base: pricing.Rates{Input: 5, Output: 25}},
	})
	row := func(sub bool, model string, calls, peak int64, c pricing.Counts) store.SessionAggRow {
		return store.SessionAggRow{AggRow: store.AggRow{SessionID: "s1", Model: model, Speed: "standard", Counts: c},
			IsSubagent: sub, Calls: calls, PeakPrompt: peak}
	}
	agg := []store.SessionAggRow{
		row(false, "claude-opus-4-5-20251101", 3, 700, pricing.Counts{Input: 1_000_000}), // $5
		row(true, "claude-opus-4-5", 2, 900, pricing.Counts{Output: 40_000}),             // $1
		row(true, "a-model", 1, 50, pricing.Counts{Input: 1_200_000}),                    // $6
		row(false, "zz-unpriced", 4, 10, pricing.Counts{Input: 7, CacheRead: 3}),         // unpriced
		row(false, "aa-unpriced", 1, 20, pricing.Counts{Output: 1_000_000_000}),          // unpriced, more tokens
	}
	d := sessionDetail(store.SessionRow{SessionID: "s1"}, nil, table, agg)
	if d.Calls != 11 || d.PeakContextTokens != 900 {
		t.Fatalf("calls = %d, peak = %d", d.Calls, d.PeakContextTokens)
	}
	if d.Main.Calls != 8 || d.Main.Totals.CostUsd != 5 || d.Main.Totals.UnpricedTokens != 1_000_000_010 {
		t.Fatalf("main = %+v", d.Main)
	}
	if d.Subagents.Calls != 3 || d.Subagents.Totals.CostUsd != 7 || d.Subagents.Totals.UnpricedTokens != 0 {
		t.Fatalf("subagents = %+v", d.Subagents)
	}
	if d.Session.Totals.CostUsd != 12 || d.Session.Totals.TotalTokens != 1_002_240_010 {
		t.Fatalf("session totals = %+v", d.Session.Totals)
	}
	var order []string
	for _, m := range d.ByModel {
		order = append(order, fmt.Sprintf("%s:%d", m.Model, m.Calls))
	}
	if got := strings.Join(order, ","); got != "a-model:1,claude-opus-4-5:5,aa-unpriced:1,zz-unpriced:4" {
		t.Fatalf("by_model order = %s", got)
	}

	empty := sessionDetail(store.SessionRow{SessionID: "s1"}, nil, table, nil)
	b, _ := json.Marshal(empty)
	if !strings.Contains(string(b), `"by_model":[]`) || empty.Calls != 0 {
		t.Fatalf("empty detail = %s", b)
	}
}

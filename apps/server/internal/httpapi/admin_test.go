package httpapi

import (
	"fmt"
	"math"
	"net/http"
	"net/url"
	"testing"
	"time"

	"cctrack/server/internal/api"
)

func rangeQ(path string) string {
	q := url.Values{"from": {t0.Add(-time.Hour).Format(time.RFC3339)}, "to": {t0.Add(time.Hour).Format(time.RFC3339)}}
	return path + "?" + q.Encode()
}

// adminGet performs a cookie-authenticated GET, fails unless it returns 200, and decodes the body.
func adminGet[T any](h *harness, path string, c *http.Cookie) T {
	h.t.Helper()
	rec := h.do("GET", path, nil, withCookie(c))
	if rec.Code != http.StatusOK {
		h.t.Fatalf("GET %s: %d %s", path, rec.Code, rec.Body)
	}
	return decode[T](h.t, rec)
}

// seedTwoUsersTwoAccounts: alice uses acc-1 ($5) and acc-2 ($10); bob uses acc-1 ($15).
func seedTwoUsersTwoAccounts(t *testing.T, h *harness) (alice, bob string) {
	alice = h.loginCLI("alice@example.com")
	bob = h.loginCLI("bob@example.com")
	req := ingestFixture("a1", "acc-1", entry("a1", "acc-1", "m1", "claude-opus-4-5", t0, 1_000_000, 0))
	req.Accounts = append(req.Accounts, api.IngestAccount{AccountUuid: "acc-2"})
	req.Sessions = append(req.Sessions, api.IngestSession{SessionId: "a2", StartedAt: t0, EndedAt: t0})
	req.Entries = append(req.Entries, entry("a2", "acc-2", "m2", "claude-opus-4-5", t0, 2_000_000, 0))
	h.ingestOK(alice, req)
	h.ingestOK(bob, ingestFixture("b1", "acc-1", entry("b1", "acc-1", "m3", "claude-opus-4-5", t0, 3_000_000, 0)))
	return alice, bob
}

func TestAdminEndpointsRequireAdmin(t *testing.T) {
	h := newHarness(t)
	c := h.loginWeb("dev@example.com")
	tok := h.loginCLI("dev@example.com")
	rates := map[string]float64{"input": 7.77, "cache_write_5m": 1, "cache_write_1h": 1, "cache_read": 1, "output": 1}
	for _, rt := range []struct {
		method, path string
		body         any
	}{
		{"GET", rangeQ("/api/admin/by-login"), nil},
		{"GET", rangeQ("/api/admin/by-account"), nil},
		{"GET", "/api/admin/accounts", nil},
		{"PATCH", "/api/admin/accounts/1", map[string]string{"label": "x"}},
		{"GET", "/api/admin/users", nil},
		{"GET", "/api/admin/users/1/sessions", nil},
		{"GET", "/api/admin/tokens", nil},
		{"DELETE", "/api/admin/tokens/1", nil},
		{"GET", "/api/admin/prices", nil},
		{"PUT", "/api/admin/prices/claude-opus-4-5", rates},
	} {
		if rec := h.do(rt.method, rt.path, rt.body, withCookie(c), withCSRF()); rec.Code != http.StatusForbidden {
			t.Errorf("cookie %s %s: got %d, want 403", rt.method, rt.path, rec.Code)
		}
		if rec := h.do(rt.method, rt.path, rt.body, bearer(tok)); rec.Code != http.StatusForbidden {
			t.Errorf("bearer %s %s: got %d, want 403", rt.method, rt.path, rec.Code)
		}
	}
	// Nothing a non-admin sent may have taken effect.
	var revoked, priced int
	h.scan(`SELECT count(*) FROM api_tokens WHERE revoked_at IS NOT NULL`, &revoked)
	h.scan(`SELECT count(*) FROM model_prices WHERE input = 7.77`, &priced)
	if revoked != 0 || priced != 0 {
		t.Fatalf("non-admin writes took effect: revoked=%d priced=%d", revoked, priced)
	}
}

func TestAdminByLogin(t *testing.T) {
	h := newHarness(t)
	seedTwoUsersTwoAccounts(t, h)
	admin := h.loginWeb("admin@example.com")
	out := adminGet[api.ByLogin](h, rangeQ("/api/admin/by-login"), admin)
	if len(out.Users) != 2 {
		t.Fatalf("users = %+v", out.Users)
	}
	// Both spent $15; alice's split is acc-2 $10 then acc-1 $5.
	for _, u := range out.Users {
		if math.Abs(u.Totals.CostUsd-15) > 1e-9 {
			t.Fatalf("%s cost = %v", u.User.Email, u.Totals.CostUsd)
		}
		if u.User.Email == "alice@example.com" {
			if len(u.ByAccount) != 2 || u.ByAccount[0].Account.Label != "acc-2" {
				t.Fatalf("alice by_account = %+v", u.ByAccount)
			}
		}
	}
	if len(out.DailyByUser) != 2 {
		t.Fatalf("daily = %+v", out.DailyByUser)
	}
	if rec := h.do("GET", rangeQ("/api/admin/by-login")+"&tz=Mars/Olympus", nil, withCookie(admin)); rec.Code != http.StatusBadRequest {
		t.Fatalf("bad tz: %d %s", rec.Code, rec.Body)
	}
}

func TestAdminByLoginEmptyHasArrays(t *testing.T) {
	h := newHarness(t)
	admin := h.loginWeb("admin@example.com")
	rec := h.do("GET", rangeQ("/api/admin/by-login"), nil, withCookie(admin))
	if rec.Code != http.StatusOK || rec.Body.String() != `{"daily_by_user":[],"users":[]}`+"\n" {
		t.Fatalf("empty by-login: %d %s", rec.Code, rec.Body)
	}
	rec = h.do("GET", rangeQ("/api/admin/by-account"), nil, withCookie(admin))
	if rec.Code != http.StatusOK || rec.Body.String() != `{"accounts":[]}`+"\n" {
		t.Fatalf("empty by-account: %d %s", rec.Code, rec.Body)
	}
}

func TestAdminByAccountShares(t *testing.T) {
	h := newHarness(t)
	seedTwoUsersTwoAccounts(t, h)
	admin := h.loginWeb("admin@example.com")
	out := adminGet[api.ByAccount](h, rangeQ("/api/admin/by-account"), admin)
	if len(out.Accounts) != 2 {
		t.Fatalf("accounts = %+v", out.Accounts)
	}
	acc1 := out.Accounts[0]
	if acc1.Account.AccountUuid != "acc-1" || math.Abs(acc1.Totals.CostUsd-20) > 1e-9 {
		t.Fatalf("acc-1 = %+v", acc1)
	}
	sum := 0.0
	for _, u := range acc1.ByUser {
		sum += u.Share
	}
	if len(acc1.ByUser) != 2 || acc1.ByUser[0].User.Email != "bob@example.com" ||
		math.Abs(acc1.ByUser[0].Share-0.75) > 1e-9 || math.Abs(sum-1) > 1e-9 {
		t.Fatalf("acc-1 by_user = %+v", acc1.ByUser)
	}
	if acc1.Limits == nil || len(acc1.DailyByUser) != 2 {
		t.Fatalf("acc-1 limits/daily = %+v / %+v", acc1.Limits, acc1.DailyByUser)
	}
}

func TestAdminUpdateAccount(t *testing.T) {
	h := newHarness(t)
	seedTwoUsersTwoAccounts(t, h)
	admin := h.loginWeb("admin@example.com")
	accounts := adminGet[[]api.Account](h, "/api/admin/accounts", admin)
	id := accounts[0].Id
	path := fmt.Sprintf("/api/admin/accounts/%d", id)

	rec := h.do("PATCH", path, map[string]string{"label": "Team Max", "plan": "max_20x"}, withCookie(admin), withCSRF())
	if rec.Code != 200 {
		t.Fatalf("patch: %d %s", rec.Code, rec.Body)
	}
	if a := decode[api.Account](t, rec); a.Label != "Team Max" || a.Plan != "max_20x" {
		t.Fatalf("account = %+v", a)
	}
	// Omitted fields are left alone.
	rec = h.do("PATCH", path, map[string]string{"label": "Renamed"}, withCookie(admin), withCSRF())
	if rec.Code != 200 {
		t.Fatalf("patch label: %d %s", rec.Code, rec.Body)
	}
	if a := decode[api.Account](t, rec); a.Label != "Renamed" || a.Plan != "max_20x" {
		t.Fatalf("account = %+v", a)
	}
	if rec := h.do("PATCH", path, map[string]string{"plan": "gold"}, withCookie(admin), withCSRF()); rec.Code != http.StatusBadRequest {
		t.Fatalf("invalid plan: %d", rec.Code)
	}
	if rec := h.do("PATCH", "/api/admin/accounts/999999", map[string]string{"label": "x"}, withCookie(admin), withCSRF()); rec.Code != http.StatusNotFound {
		t.Fatalf("missing: %d", rec.Code)
	}
	if rec := h.do("PATCH", path, map[string]string{"label": "y"}, withCookie(admin)); rec.Code != http.StatusForbidden {
		t.Fatalf("cookie PATCH without CSRF header: %d", rec.Code)
	}
}

func TestAdminRevokeTokenLocksCLI(t *testing.T) {
	h := newHarness(t)
	alice, bob := seedTwoUsersTwoAccounts(t, h)
	admin := h.loginWeb("admin@example.com")
	tokens := adminGet[[]api.ApiToken](h, "/api/admin/tokens", admin)
	var bobID int64
	for _, tk := range tokens {
		if tk.User.Email == "bob@example.com" {
			bobID = tk.Id
		}
	}
	if bobID == 0 {
		t.Fatalf("bob's token not listed: %+v", tokens)
	}
	revoke := fmt.Sprintf("/api/admin/tokens/%d", bobID)
	if rec := h.do("DELETE", revoke, nil, withCookie(admin), withCSRF()); rec.Code != http.StatusNoContent {
		t.Fatalf("revoke: %d %s", rec.Code, rec.Body)
	}
	// A revoked token is rejected on every authenticated route, not just /api/me.
	for _, rt := range []struct {
		method, path string
		body         any
	}{
		{"GET", "/api/me", nil},
		{"GET", rangeQ("/api/me/usage"), nil},
		{"GET", "/api/me/sessions", nil},
		{"POST", "/api/ingest", ingestFixture("b2", "acc-1", entry("b2", "acc-1", "m9", "claude-opus-4-5", t0, 1, 0))},
		{"POST", "/api/sessions/b1/summary", map[string]string{"summary": "x"}},
		{"GET", "/api/admin/prices", nil},
		{"DELETE", "/api/auth/token", nil},
	} {
		if rec := h.do(rt.method, rt.path, rt.body, bearer(bob)); rec.Code != http.StatusUnauthorized {
			t.Errorf("bob %s %s after revoke: %d", rt.method, rt.path, rec.Code)
		}
	}
	if rec := h.do("GET", "/api/me", nil, bearer(alice)); rec.Code != http.StatusOK {
		t.Fatalf("alice after bob's revoke: %d", rec.Code)
	}
	if rec := h.do("DELETE", revoke, nil, withCookie(admin), withCSRF()); rec.Code != http.StatusNotFound {
		t.Fatalf("second revoke: %d", rec.Code)
	}
	tokens = adminGet[[]api.ApiToken](h, "/api/admin/tokens", admin)
	if last := tokens[len(tokens)-1]; last.Id != bobID || last.RevokedAt == nil {
		t.Fatalf("revoked token should sort last with revoked_at: %+v", tokens)
	}
}

func TestAdminPutPriceChangesCost(t *testing.T) {
	h := newHarness(t)
	seedTwoUsersTwoAccounts(t, h)
	admin := h.loginWeb("admin@example.com")
	rates := map[string]float64{"input": 10, "cache_write_5m": 12.5, "cache_write_1h": 20, "cache_read": 1, "output": 50}
	rec := h.do("PUT", "/api/admin/prices/claude-opus-4-5-20251101", rates, withCookie(admin), withCSRF())
	if rec.Code != 200 {
		t.Fatalf("put: %d %s", rec.Code, rec.Body)
	}
	if p := decode[api.ModelPrice](t, rec); p.Model != "claude-opus-4-5" || p.Rates.Input != 10 || p.Rates.LongInput != nil {
		t.Fatalf("put returned %+v", p)
	}
	out := adminGet[api.ByLogin](h, rangeQ("/api/admin/by-login"), admin)
	if math.Abs(out.Users[0].Totals.CostUsd-30) > 1e-9 {
		t.Fatalf("cost after price change = %v, want 30", out.Users[0].Totals.CostUsd)
	}
	found := false
	for _, p := range adminGet[[]api.ModelPrice](h, "/api/admin/prices", admin) {
		if p.Model == "claude-opus-4-5" {
			found = p.Rates.Input == 10
		}
	}
	if !found {
		t.Fatal("listed price not updated")
	}
	bad := map[string]float64{"input": 1, "cache_write_5m": 1, "cache_write_1h": 1, "cache_read": 1, "output": 1, "long_input": 2}
	if rec := h.do("PUT", "/api/admin/prices/x", bad, withCookie(admin), withCSRF()); rec.Code != http.StatusBadRequest {
		t.Fatalf("partial long rates: %d", rec.Code)
	}
}

func TestAdminUsersAndUserSessions(t *testing.T) {
	h := newHarness(t)
	seedTwoUsersTwoAccounts(t, h)
	admin := h.loginWeb("admin@example.com")
	users := adminGet[[]api.AdminUser](h, "/api/admin/users", admin)
	var aliceID int64
	for _, u := range users {
		switch u.Email {
		case "alice@example.com":
			aliceID = u.Id
			if u.LastSeenAt == nil || u.IsAdmin {
				t.Fatalf("alice = %+v", u)
			}
		case "admin@example.com":
			if !u.IsAdmin {
				t.Fatalf("admin = %+v", u)
			}
		}
	}
	if aliceID == 0 {
		t.Fatalf("alice not listed: %+v", users)
	}
	page := adminGet[api.SessionPage](h, fmt.Sprintf("/api/admin/users/%d/sessions", aliceID), admin)
	if page.Total != 2 || len(page.Items) != 2 {
		t.Fatalf("alice sessions = %+v", page)
	}
	for _, s := range page.Items {
		if s.User.Email != "alice@example.com" {
			t.Fatalf("foreign session in alice's page: %+v", s)
		}
	}
}

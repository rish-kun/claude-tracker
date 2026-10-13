package httpapi

import (
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"testing"

	"cctrack/server/internal/api"
)

func TestHealth(t *testing.T) {
	h := newHarness(t)
	rec := h.do("GET", "/api/healthz", nil)
	if rec.Code != 200 || decode[api.Health](t, rec).Status != "ok" {
		t.Fatalf("got %d %s", rec.Code, rec.Body)
	}
}

func TestCLILoginFlowIssuesWorkingToken(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	rec := h.do("GET", "/api/me", nil, bearer(tok))
	if rec.Code != 200 {
		t.Fatalf("me: %d %s", rec.Code, rec.Body)
	}
	me := decode[api.Me](t, rec)
	if me.Email != "dev@example.com" || me.IsAdmin {
		t.Fatalf("me = %+v", me)
	}
}

func TestCLIStartRedirectsLoopbackTo127(t *testing.T) {
	h := newHarness(t)
	_, challenge := pkcePair()
	rec := h.do("GET", "/api/auth/cli/start?port=5555&state=abcdefghijklmnop&code_challenge="+challenge, nil)
	state := locationParam(t, rec, "state")
	h.idp.id = identity("dev@example.com")
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil)
	if loc := rec.Header().Get("Location"); !strings.HasPrefix(loc, "http://127.0.0.1:5555/callback?") {
		t.Fatalf("Location = %q", loc)
	}
}

func TestCLIStartValidation(t *testing.T) {
	h := newHarness(t)
	_, challenge := pkcePair()
	for _, q := range []string{
		"state=abcdefghijklmnop&code_challenge=" + challenge,         // loopback without port
		"port=80&state=abcdefghijklmnop&code_challenge=" + challenge, // privileged port
		"port=5555&state=short&code_challenge=" + challenge,          // short state
		"port=5555&state=abcdefghijklmnop&code_challenge=tooshort",   // short challenge
		"mode=weird&port=5555&state=abcdefghijklmnop&code_challenge=" + challenge,
	} {
		if rec := h.do("GET", "/api/auth/cli/start?"+q, nil); rec.Code != http.StatusBadRequest {
			t.Errorf("%s: got %d, want 400", q, rec.Code)
		}
	}
}

func TestPasteModeShowsCode(t *testing.T) {
	h := newHarness(t)
	_, challenge := pkcePair()
	rec := h.do("GET", "/api/auth/cli/start?mode=paste&state=abcdefghijklmnop&code_challenge="+challenge, nil)
	state := locationParam(t, rec, "state")
	h.idp.id = identity("dev@example.com")
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil)
	if rec.Code != 200 || !strings.Contains(rec.Body.String(), "<code>") {
		t.Fatalf("paste page: %d %s", rec.Code, rec.Body)
	}
	if !strings.Contains(rec.Body.String(), "Never share this code; paste it only into your own terminal.") {
		t.Fatalf("paste page lacks the anti-phishing warning: %s", rec.Body)
	}
}

func TestExchangeRejectsWrongVerifierAndReuse(t *testing.T) {
	h := newHarness(t)
	verifier, challenge := pkcePair()
	rec := h.do("GET", "/api/auth/cli/start?port=5555&state=abcdefghijklmnop&code_challenge="+challenge, nil)
	state := locationParam(t, rec, "state")
	h.idp.id = identity("dev@example.com")
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil)
	code := locationParam(t, rec, "code")

	wrong, _ := pkcePair()
	rec = h.do("POST", "/api/auth/cli/exchange", map[string]string{"code": code, "code_verifier": wrong, "device_name": "x"})
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("wrong verifier: %d", rec.Code)
	}
	// The code was burned by the failed attempt.
	rec = h.do("POST", "/api/auth/cli/exchange", map[string]string{"code": code, "code_verifier": verifier, "device_name": "x"})
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("reuse: %d", rec.Code)
	}
}

func TestCallbackStateIsSingleUse(t *testing.T) {
	h := newHarness(t)
	_, challenge := pkcePair()
	rec := h.do("GET", "/api/auth/cli/start?port=5555&state=abcdefghijklmnop&code_challenge="+challenge, nil)
	state := locationParam(t, rec, "state")
	h.idp.id = identity("dev@example.com")
	h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil)
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("second callback: %d", rec.Code)
	}
}

func TestCallbackRejectsDisallowedAccount(t *testing.T) {
	h := newHarness(t)
	_, challenge := pkcePair()
	rec := h.do("GET", "/api/auth/cli/start?port=5555&state=abcdefghijklmnop&code_challenge="+challenge, nil)
	state := locationParam(t, rec, "state")
	h.idp.id = identity("stranger@other.com")
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil)
	if rec.Code != http.StatusForbidden {
		t.Fatalf("got %d, want 403", rec.Code)
	}
}

func TestWebLoginCookieAndCSRF(t *testing.T) {
	h := newHarness(t)
	c := h.loginWeb("admin@example.com")
	rec := h.do("GET", "/api/me", nil, withCookie(c))
	if rec.Code != 200 || !decode[api.Me](t, rec).IsAdmin {
		t.Fatalf("me: %d %s", rec.Code, rec.Body)
	}
	if rec := h.do("POST", "/api/auth/logout", nil, withCookie(c)); rec.Code != http.StatusForbidden {
		t.Fatalf("logout without CSRF header: %d, want 403", rec.Code)
	}
	if rec := h.do("POST", "/api/auth/logout", nil, withCookie(c), withCSRF()); rec.Code != http.StatusNoContent {
		t.Fatalf("logout: %d", rec.Code)
	}
}

func TestWebCallbackRequiresStateCookie(t *testing.T) {
	h := newHarness(t)
	rec := h.do("GET", "/api/auth/web/start", nil)
	state := locationParam(t, rec, "state")
	h.idp.id = identity("dev@example.com")
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil) // no cookie
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("got %d, want 400", rec.Code)
	}
}

func TestRevokedTokenIs401(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	if rec := h.do("DELETE", "/api/auth/token", nil, bearer(tok)); rec.Code != http.StatusNoContent {
		t.Fatalf("revoke: %d", rec.Code)
	}
	if rec := h.do("GET", "/api/me", nil, bearer(tok)); rec.Code != http.StatusUnauthorized {
		t.Fatalf("me after revoke: %d, want 401", rec.Code)
	}
}

func TestBadBearerIs401AndAnonymousIs401(t *testing.T) {
	h := newHarness(t)
	if rec := h.do("GET", "/api/me", nil, bearer("cct_nope")); rec.Code != http.StatusUnauthorized {
		t.Fatalf("bad bearer: %d", rec.Code)
	}
	if rec := h.do("GET", "/api/me", nil); rec.Code != http.StatusUnauthorized {
		t.Fatalf("anonymous: %d", rec.Code)
	}
}

// TestLoginStartPendingCap: once maxPendingAuthRequests sign-ins are pending,
// both login starts answer 503 without inserting; expired rows do not count.
func TestLoginStartPendingCap(t *testing.T) {
	h := newHarness(t)
	h.exec(fmt.Sprintf(`INSERT INTO auth_requests (state, mode, expires_at)
		SELECT 'flood-' || g, 'web', now() + interval '10 minutes' FROM generate_series(1, %d) g`, maxPendingAuthRequests))
	_, challenge := pkcePair()
	cli := "/api/auth/cli/start?port=5555&state=abcdefghijklmnop&code_challenge=" + challenge
	for _, path := range []string{cli, "/api/auth/web/start"} {
		rec := h.do("GET", path, nil)
		if rec.Code != http.StatusServiceUnavailable ||
			decode[map[string]string](t, rec)["error"] != "too many pending sign-ins, try again shortly" {
			t.Fatalf("%s at cap: %d %s", path, rec.Code, rec.Body)
		}
	}
	var n int
	h.scan("SELECT count(*) FROM auth_requests", &n)
	if n != maxPendingAuthRequests {
		t.Fatalf("rows = %d, want %d (no insert at cap)", n, maxPendingAuthRequests)
	}
	h.exec("UPDATE auth_requests SET expires_at = now() - interval '1 minute' WHERE state = 'flood-1'")
	if rec := h.do("GET", "/api/auth/web/start", nil); rec.Code != http.StatusFound {
		t.Fatalf("below cap: %d %s", rec.Code, rec.Body)
	}
}

package httpapi

import (
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

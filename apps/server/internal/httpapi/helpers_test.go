package httpapi

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"cctrack/server/internal/auth"
	"cctrack/server/internal/config"
	"cctrack/server/internal/store"
	"cctrack/server/internal/testdb"
)

type fakeIDP struct {
	id  auth.Identity
	err error
}

func (f *fakeIDP) AuthCodeURL(state, redirectURL string) string {
	return "https://idp.test/auth?state=" + url.QueryEscape(state)
}

func (f *fakeIDP) Exchange(ctx context.Context, code, redirectURL string) (auth.Identity, error) {
	return f.id, f.err
}

func testConfig() config.Config {
	return config.Config{
		SessionSecret:  []byte(strings.Repeat("k", 32)),
		PublicURL:      "https://cctrack.test",
		AllowedDomains: []string{"example.com"},
		AdminEmails:    []string{"admin@example.com"},
	}
}

type harness struct {
	t     *testing.T
	srv   *Server
	h     http.Handler
	idp   *fakeIDP
	store *store.Store
}

func newHarness(t *testing.T) *harness {
	t.Helper()
	st := store.New(testdb.New(t))
	idp := &fakeIDP{}
	srv := New(testConfig(), st, idp)
	return &harness{t: t, srv: srv, h: srv.Handler(), idp: idp, store: st}
}

func bearer(token string) func(*http.Request) {
	return func(r *http.Request) { r.Header.Set("Authorization", "Bearer "+token) }
}

func withCookie(c *http.Cookie) func(*http.Request) {
	return func(r *http.Request) { r.AddCookie(c) }
}

func withCSRF() func(*http.Request) {
	return func(r *http.Request) { r.Header.Set(csrfHeader, "1") }
}

func (h *harness) do(method, path string, body any, mods ...func(*http.Request)) *httptest.ResponseRecorder {
	h.t.Helper()
	var buf bytes.Buffer
	if body != nil {
		if err := json.NewEncoder(&buf).Encode(body); err != nil {
			h.t.Fatal(err)
		}
	}
	req := httptest.NewRequest(method, path, &buf)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	for _, m := range mods {
		m(req)
	}
	rec := httptest.NewRecorder()
	h.h.ServeHTTP(rec, req)
	return rec
}

func decode[T any](t *testing.T, rec *httptest.ResponseRecorder) T {
	t.Helper()
	var v T
	if err := json.Unmarshal(rec.Body.Bytes(), &v); err != nil {
		t.Fatalf("decode %q: %v", rec.Body.String(), err)
	}
	return v
}

func identity(email string) auth.Identity {
	domain := email[strings.IndexByte(email, '@')+1:]
	return auth.Identity{Subject: "sub-" + email, Email: email, Name: strings.Split(email, "@")[0],
		HostedDomain: domain, EmailVerified: true}
}

func pkcePair() (verifier, challenge string) {
	verifier = auth.RandomString(48)
	sum := sha256.Sum256([]byte(verifier))
	return verifier, base64.RawURLEncoding.EncodeToString(sum[:])
}

func locationParam(t *testing.T, rec *httptest.ResponseRecorder, key string) string {
	t.Helper()
	u, err := url.Parse(rec.Header().Get("Location"))
	if err != nil {
		t.Fatal(err)
	}
	return u.Query().Get(key)
}

// loginCLI runs the full loopback login flow and returns the issued API token.
func (h *harness) loginCLI(email string) string {
	h.t.Helper()
	verifier, challenge := pkcePair()
	cliState := auth.RandomString(16)
	rec := h.do("GET", "/api/auth/cli/start?port=5555&state="+cliState+"&code_challenge="+challenge, nil)
	if rec.Code != http.StatusFound {
		h.t.Fatalf("start: %d %s", rec.Code, rec.Body)
	}
	serverState := locationParam(h.t, rec, "state")
	h.idp.id = identity(email)
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(serverState), nil)
	if rec.Code != http.StatusFound {
		h.t.Fatalf("callback: %d %s", rec.Code, rec.Body)
	}
	if got := locationParam(h.t, rec, "state"); got != cliState {
		h.t.Fatalf("loopback state = %q, want %q", got, cliState)
	}
	code := locationParam(h.t, rec, "code")
	rec = h.do("POST", "/api/auth/cli/exchange", map[string]string{
		"code": code, "code_verifier": verifier, "device_name": "laptop"})
	if rec.Code != http.StatusOK {
		h.t.Fatalf("exchange: %d %s", rec.Code, rec.Body)
	}
	return decode[map[string]any](h.t, rec)["token"].(string)
}

// loginWeb runs the browser login flow and returns the session cookie.
func (h *harness) loginWeb(email string) *http.Cookie {
	h.t.Helper()
	rec := h.do("GET", "/api/auth/web/start", nil)
	if rec.Code != http.StatusFound {
		h.t.Fatalf("web start: %d", rec.Code)
	}
	var stateC *http.Cookie
	for _, c := range rec.Result().Cookies() {
		if c.Name == stateCookie {
			stateC = c
		}
	}
	state := locationParam(h.t, rec, "state")
	h.idp.id = identity(email)
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil, withCookie(stateC))
	if rec.Code != http.StatusFound {
		h.t.Fatalf("web callback: %d %s", rec.Code, rec.Body)
	}
	for _, c := range rec.Result().Cookies() {
		if c.Name == sessionCookie {
			return c
		}
	}
	h.t.Fatal("no session cookie")
	return nil
}

package httpapi

// Tests in this file run without TEST_DATABASE_URL. They only exercise paths
// that return before touching the store, or use a real pgx pool pointed at a
// closed port to observe how genuine database failures are handled.

import (
	"bytes"
	"context"
	"log"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/jackc/pgx/v5/pgxpool"

	"cctrack/server/internal/api"
	"cctrack/server/internal/store"
)

func noDBRequest(t *testing.T, h http.Handler, method, path, body string, hdr ...string) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(method, path, strings.NewReader(body))
	for i := 0; i+1 < len(hdr); i += 2 {
		req.Header.Set(hdr[i], hdr[i+1])
	}
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	return rec
}

// unreachableStore returns a store backed by a real pool whose server refuses
// connections, so every query fails with a non-ErrNotFound error.
func unreachableStore(t *testing.T) *store.Store {
	t.Helper()
	pool, err := pgxpool.New(context.Background(), "postgres://u:p@127.0.0.1:1/x?connect_timeout=2&sslmode=disable")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(pool.Close)
	return store.New(pool)
}

// TestNoDBRequestHandling covers routing, validation, CSRF and body-limit
// behaviour on paths that never reach the store (nil store).
func TestNoDBRequestHandling(t *testing.T) {
	h := New(testConfig(), nil, &fakeIDP{}).Handler()
	_, ch := pkcePair()
	for _, c := range []struct {
		name, method, path, body string
		hdr                      []string
		want                     int
		wantErr                  string // substring of the JSON "error" field
	}{
		{"health", "GET", "/api/healthz", "", nil, 200, ""},
		{"anonymous me", "GET", "/api/me", "", nil, 401, "not logged in"},
		{"non-bearer auth", "GET", "/api/me", "", []string{"Authorization", "Basic x"}, 401, "invalid token"},
		{"bad session cookie is anonymous", "GET", "/api/me", "", []string{"Cookie", sessionCookie + "=bogus"}, 401, "not logged in"},
		{"loopback without port", "GET", "/api/auth/cli/start?state=abcdefghijklmnop&code_challenge=" + ch, "", nil, 400, "port"},
		{"privileged port", "GET", "/api/auth/cli/start?port=80&state=abcdefghijklmnop&code_challenge=" + ch, "", nil, 400, "port"},
		{"bad mode", "GET", "/api/auth/cli/start?mode=weird&port=5555&state=abcdefghijklmnop&code_challenge=" + ch, "", nil, 400, "mode"},
		{"short state", "GET", "/api/auth/cli/start?port=5555&state=short&code_challenge=" + ch, "", nil, 400, "state"},
		{"short challenge", "GET", "/api/auth/cli/start?port=5555&state=abcdefghijklmnop&code_challenge=tooshort", "", nil, 400, "code_challenge"},
		{"param bind error is JSON 400", "GET", "/api/auth/cli/start?port=abc&state=abcdefghijklmnop&code_challenge=" + ch, "", nil, 400, "port"},
		{"anonymous admin by-login", "GET", "/api/admin/by-login?from=2026-10-01T00:00:00Z&to=2026-10-02T00:00:00Z", "", nil, 401, "not logged in"},
		{"anonymous admin by-account", "GET", "/api/admin/by-account?from=2026-10-01T00:00:00Z&to=2026-10-02T00:00:00Z", "", nil, 401, "not logged in"},
		{"anonymous admin accounts", "GET", "/api/admin/accounts", "", nil, 401, "not logged in"},
		{"anonymous admin account patch", "PATCH", "/api/admin/accounts/1", `{"label":"x"}`, nil, 401, "not logged in"},
		{"anonymous admin users", "GET", "/api/admin/users", "", nil, 401, "not logged in"},
		{"anonymous admin user sessions", "GET", "/api/admin/users/1/sessions", "", nil, 401, "not logged in"},
		{"anonymous admin tokens", "GET", "/api/admin/tokens", "", nil, 401, "not logged in"},
		{"anonymous admin token revoke", "DELETE", "/api/admin/tokens/1", "", nil, 401, "not logged in"},
		{"anonymous admin prices", "GET", "/api/admin/prices", "", nil, 401, "not logged in"},
		{"anonymous admin price put", "PUT", "/api/admin/prices/claude-opus-4-5", `{"input":1}`, nil, 401, "not logged in"},
		{"admin PATCH cookie without CSRF header", "PATCH", "/api/admin/accounts/1", `{"label":"x"}`, []string{"Cookie", sessionCookie + "=bogus"}, 403, "X-CCTrack"},
		{"anonymous usage", "GET", "/api/me/usage?from=2026-10-01T00:00:00Z&to=2026-10-02T00:00:00Z", "", nil, 401, "not logged in"},
		{"anonymous sessions", "GET", "/api/me/sessions", "", nil, 401, "not logged in"},
		{"anonymous ingest", "POST", "/api/ingest", "{}", nil, 401, "not logged in"},
		{"anonymous summary", "POST", "/api/sessions/s1/summary", `{"summary":"x"}`, nil, 401, "not logged in"},
		{"cookie POST without CSRF header", "POST", "/api/auth/logout", "", []string{"Cookie", sessionCookie + "=bogus"}, 403, "X-CCTrack"},
		{"cookie POST with CSRF header", "POST", "/api/auth/logout", "", []string{"Cookie", sessionCookie + "=bogus", csrfHeader, "1"}, 204, ""},
		{"bad JSON", "POST", "/api/auth/cli/exchange", "not json", nil, 400, "invalid JSON"},
		{"body over 1 MB", "POST", "/api/auth/cli/exchange", `{"code":"` + strings.Repeat("a", 2<<20) + `"}`, nil, 413, "too large"},
	} {
		t.Run(c.name, func(t *testing.T) {
			rec := noDBRequest(t, h, c.method, c.path, c.body, c.hdr...)
			if rec.Code != c.want {
				t.Fatalf("status %d, want %d: %s", rec.Code, c.want, rec.Body)
			}
			if c.wantErr == "" {
				return
			}
			if ct := rec.Header().Get("Content-Type"); ct != "application/json" {
				t.Fatalf("Content-Type = %q", ct)
			}
			if msg := decode[map[string]string](t, rec)["error"]; !strings.Contains(msg, c.wantErr) {
				t.Fatalf("error = %q, want substring %q", msg, c.wantErr)
			}
		})
	}
}

// A valid session cookie plus a failing database must be a 500, not a silent
// downgrade to anonymous.
func TestSessionCookieDBErrorIs500(t *testing.T) {
	srv := New(testConfig(), unreachableStore(t), &fakeIDP{})
	cookie := srv.sessions.Encode(42, time.Now().Add(time.Hour))
	rec := noDBRequest(t, srv.Handler(), "GET", "/api/me", "", "Cookie", sessionCookie+"="+cookie)
	if rec.Code != http.StatusInternalServerError {
		t.Fatalf("status %d, want 500: %s", rec.Code, rec.Body)
	}
}

func TestPurgeFailureIsLogged(t *testing.T) {
	var buf bytes.Buffer
	prev := log.Writer()
	log.SetOutput(&buf)
	t.Cleanup(func() { log.SetOutput(prev) })
	srv := New(testConfig(), unreachableStore(t), &fakeIDP{})
	noDBRequest(t, srv.Handler(), "GET", "/api/auth/web/start", "")
	if !strings.Contains(buf.String(), "purge expired auth rows") {
		t.Fatalf("purge failure not logged; log = %q", buf.String())
	}
}

func TestTruncateDeviceName(t *testing.T) {
	long := strings.Repeat("Rishit’s MacBook Pro ", 20) // U+2019 is 3 bytes
	for _, c := range []struct{ name, in string }{
		{"long multi-byte", long},
		{"multi-byte straddling byte 100", strings.Repeat("a", 99) + "’’’"},
		{"invalid UTF-8", "bad\xff\xfename"},
		{"NUL byte", "a\x00b"},
	} {
		t.Run(c.name, func(t *testing.T) {
			got := truncateDeviceName(c.in)
			if !utf8.ValidString(got) {
				t.Fatalf("invalid UTF-8: %q", got)
			}
			if n := utf8.RuneCountInString(got); n > maxDeviceNameRunes {
				t.Fatalf("%d runes, want <= %d", n, maxDeviceNameRunes)
			}
			if strings.ContainsRune(got, 0) {
				t.Fatalf("contains NUL: %q", got)
			}
		})
	}
	if got := truncateDeviceName(strings.Repeat("’", 150)); utf8.RuneCountInString(got) != maxDeviceNameRunes {
		t.Fatalf("got %d runes, want exactly %d", utf8.RuneCountInString(got), maxDeviceNameRunes)
	}
	if got := truncateDeviceName("  Rishit’s MacBook Pro  "); got != "Rishit’s MacBook Pro" {
		t.Fatalf("got %q", got)
	}
	if got := truncateDeviceName(" \x00 "); got != "unknown" {
		t.Fatalf("empty name: got %q, want unknown", got)
	}
}

func TestPriceFromBody(t *testing.T) {
	f := func(v float64) *float64 { return &v }
	base := func() api.PriceRates {
		return api.PriceRates{Input: 3, CacheWrite5m: 3.75, CacheWrite1h: 6, CacheRead: 0.3, Output: 15}
	}
	withFast := base()
	withFast.FastInput, withFast.FastOutput = f(6), f(30)
	withLong := base()
	withLong.LongInput, withLong.LongCacheWrite5m, withLong.LongCacheWrite1h = f(6), f(7.5), f(12)
	withLong.LongCacheRead, withLong.LongOutput = f(0.6), f(22.5)

	for _, c := range []struct {
		name    string
		model   string
		mod     func(*api.PriceRates)
		wantErr string
	}{
		{"base only", "m", nil, ""},
		{"empty model", "", nil, "model is required"},
		{"negative base", "m", func(r *api.PriceRates) { r.Output = -1 }, ">= 0"},
		{"fast pair", "m", func(r *api.PriceRates) { *r = withFast }, ""},
		{"fast input only", "m", func(r *api.PriceRates) { r.FastInput = f(6) }, "fast_input and fast_output"},
		{"fast output only", "m", func(r *api.PriceRates) { r.FastOutput = f(6) }, "fast_input and fast_output"},
		{"negative fast", "m", func(r *api.PriceRates) { *r = withFast; r.FastOutput = f(-1) }, ">= 0"},
		{"all long", "m", func(r *api.PriceRates) { *r = withLong }, ""},
		{"partial long", "m", func(r *api.PriceRates) { r.LongInput = f(6) }, "long_*"},
		{"four of five long", "m", func(r *api.PriceRates) { *r = withLong; r.LongOutput = nil }, "long_*"},
		{"negative long", "m", func(r *api.PriceRates) { *r = withLong; r.LongCacheRead = f(-0.1) }, ">= 0"},
	} {
		t.Run(c.name, func(t *testing.T) {
			body := base()
			if c.mod != nil {
				c.mod(&body)
			}
			p, msg := priceFromBody(c.model, body)
			if c.wantErr != "" {
				if !strings.Contains(msg, c.wantErr) {
					t.Fatalf("msg = %q, want substring %q", msg, c.wantErr)
				}
				return
			}
			if msg != "" {
				t.Fatalf("unexpected error %q", msg)
			}
			if p.Model != c.model || p.Base.Output != body.Output {
				t.Fatalf("price = %+v", p)
			}
			if (p.Long != nil) != (body.LongInput != nil) || (p.Long != nil && p.Long.Output != *body.LongOutput) {
				t.Fatalf("long = %+v", p.Long)
			}
			if (p.FastInput == nil) != (body.FastInput == nil) {
				t.Fatalf("fast = %v/%v", p.FastInput, p.FastOutput)
			}
		})
	}
}

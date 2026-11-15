package httpapi

import (
	"net/http"
	"strings"
	"testing"
)

// TestProxySyncAuth runs without a database: the sync endpoint checks its own
// header before touching the store, and is not blocked by authenticate.
func TestProxySyncAuth(t *testing.T) {
	cfg := testConfig()
	cfg.ProxySyncToken = "sync-secret-0123456789"
	h := New(cfg, unreachableStore(t), &fakeIDP{}).Handler()
	for _, c := range []struct {
		name string
		hdr  []string
		want int
	}{
		{"missing header", nil, http.StatusUnauthorized},
		{"empty header", []string{proxyTokenHeader, ""}, http.StatusUnauthorized},
		{"wrong token", []string{proxyTokenHeader, "sync-secret-0123456788"}, http.StatusUnauthorized},
		{"prefix of token", []string{proxyTokenHeader, "sync-secret"}, http.StatusUnauthorized},
		{"token as bearer is not accepted", []string{"Authorization", "Bearer sync-secret-0123456789"}, http.StatusUnauthorized},
		// Past auth it reaches the (unreachable) store: 500, not 401/404.
		{"right token", []string{proxyTokenHeader, "sync-secret-0123456789"}, http.StatusInternalServerError},
		{"right token, CLI user agent", []string{proxyTokenHeader, "sync-secret-0123456789", "User-Agent", "cctrack/0.0.1"},
			http.StatusInternalServerError},
		// Unrelated user credentials are ignored, not checked first.
		{"right token, malformed bearer", []string{proxyTokenHeader, "sync-secret-0123456789", "Authorization", "Basic eA=="},
			http.StatusInternalServerError},
		{"right token, session cookie", []string{proxyTokenHeader, "sync-secret-0123456789", "Cookie", sessionCookie + "=x"},
			http.StatusInternalServerError},
	} {
		t.Run(c.name, func(t *testing.T) {
			rec := noDBRequest(t, h, "GET", "/api/proxy/keys", "", c.hdr...)
			if rec.Code != c.want {
				t.Fatalf("status %d, want %d: %s", rec.Code, c.want, rec.Body)
			}
			if c.want == http.StatusUnauthorized && !strings.Contains(rec.Body.String(), "token") {
				t.Fatalf("body %s", rec.Body)
			}
		})
	}
}

// TestProxySyncDisabled: with PROXY_SYNC_TOKEN unset the endpoint is 404,
// whatever other credentials the request carries.
func TestProxySyncDisabled(t *testing.T) {
	h := New(testConfig(), unreachableStore(t), &fakeIDP{}).Handler()
	for _, hdr := range [][]string{nil, {"Authorization", "Basic eA=="}, {proxyTokenHeader, "anything"}} {
		if rec := noDBRequest(t, h, "GET", "/api/proxy/keys", "", hdr...); rec.Code != http.StatusNotFound {
			t.Fatalf("headers %v: status %d, want 404: %s", hdr, rec.Code, rec.Body)
		}
	}
}

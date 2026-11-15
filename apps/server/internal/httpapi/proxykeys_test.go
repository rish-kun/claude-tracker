package httpapi

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"net/http"
	"strings"
	"testing"

	"cctrack/server/internal/api"
)

const testSyncToken = "sync-secret-0123456789"

func newProxyHarness(t *testing.T) *harness {
	t.Helper()
	h := newHarness(t)
	cfg := testConfig()
	cfg.ProxySyncToken = testSyncToken
	h.srv = New(cfg, h.store, h.idp)
	h.h = h.srv.Handler()
	return h
}

func (h *harness) syncKeys() []api.ProxySyncKey {
	h.t.Helper()
	rec := h.do("GET", "/api/proxy/keys", nil, func(r *http.Request) { r.Header.Set(proxyTokenHeader, testSyncToken) })
	if rec.Code != http.StatusOK {
		h.t.Fatalf("sync: %d %s", rec.Code, rec.Body)
	}
	if cc := rec.Header().Get("Cache-Control"); cc != "no-store" {
		h.t.Fatalf("sync Cache-Control = %q", cc)
	}
	return decode[api.ProxySyncKeys](h.t, rec).Keys
}

// requestKey is `cctrack proxy connect`: POST /proxy-keys/self with the CLI token.
func (h *harness) requestKey(token string, body any) api.ProxyKeyRequested {
	h.t.Helper()
	rec := h.do("POST", "/api/proxy-keys/self", body, bearer(token))
	if rec.Code != http.StatusCreated {
		h.t.Fatalf("request key: %d %s", rec.Code, rec.Body)
	}
	if cc := rec.Header().Get("Cache-Control"); cc != "no-store" {
		h.t.Fatalf("request key Cache-Control = %q", cc)
	}
	return decode[api.ProxyKeyRequested](h.t, rec)
}

func hexSHA(s string) string {
	sum := sha256.Sum256([]byte(s))
	return hex.EncodeToString(sum[:])
}

func TestProxyKeysRequestApproveRevokeSync(t *testing.T) {
	h := newProxyHarness(t)
	alice := h.loginCLI("alice@example.com")
	admin := h.loginWeb("admin@example.com")

	a := h.requestKey(alice, map[string]any{"device_name": "  alice-mac  "})
	if !strings.HasPrefix(a.Key, "ccp_") || len(a.Key) != 4+43 {
		t.Fatalf("key format: prefix %q len %d", a.Key[:4], len(a.Key))
	}
	if a.Status != api.ProxyKeyRequestedStatusPending || a.ProxyKey.Status != api.ProxyKeyStatusPending ||
		a.ProxyKey.User.Email != "alice@example.com" || a.ProxyKey.DeviceName != "alice-mac" || !a.ProxyKey.Linked ||
		a.ProxyKey.ApprovedAt != nil {
		t.Fatalf("requested %+v", a)
	}

	// The plain key is never stored or listed.
	var stored int
	h.scan(fmt.Sprintf(`SELECT count(*) FROM proxy_keys WHERE key_hash = decode('%s', 'hex')`, hexSHA(a.Key)), &stored)
	if stored != 1 {
		t.Fatalf("hash of key stored %d times", stored)
	}
	if rec := h.do("GET", "/api/admin/proxy-keys", nil, withCookie(admin)); strings.Contains(rec.Body.String(), a.Key) {
		t.Fatal("list leaks the plain key")
	}

	// Pending keys are synced as pending (the proxy answers 403).
	got := h.syncKeys()
	if len(got) != 1 || got[0].Hash != hexSHA(a.Key) || got[0].Email != "alice@example.com" || got[0].Label != "alice-mac" ||
		got[0].Status != api.ProxySyncKeyStatusPending {
		t.Fatalf("sync pending = %+v", got)
	}

	approve := fmt.Sprintf("/api/admin/proxy-keys/%d/approve", a.ProxyKey.Id)
	if rec := h.do("POST", approve, nil, withCookie(admin)); rec.Code != http.StatusForbidden {
		t.Fatalf("approve without CSRF header: %d", rec.Code)
	}
	if rec := h.do("POST", approve, nil, withCookie(admin), withCSRF()); rec.Code != http.StatusNoContent {
		t.Fatalf("approve: %d %s", rec.Code, rec.Body)
	}
	if rec := h.do("POST", approve, nil, withCookie(admin), withCSRF()); rec.Code != http.StatusNotFound {
		t.Fatalf("second approve: %d", rec.Code)
	}
	if rec := h.do("POST", "/api/admin/proxy-keys/999999/approve", nil, withCookie(admin), withCSRF()); rec.Code != http.StatusNotFound {
		t.Fatalf("approve missing: %d", rec.Code)
	}
	if got := h.syncKeys(); len(got) != 1 || got[0].Status != api.ProxySyncKeyStatusApproved {
		t.Fatalf("sync approved = %+v", got)
	}
	keys := adminGet[[]api.ProxyKey](h, "/api/admin/proxy-keys", admin)
	if len(keys) != 1 || keys[0].Status != api.ProxyKeyStatusApproved || keys[0].ApprovedAt == nil || keys[0].DeviceName != "alice-mac" {
		t.Fatalf("list after approve = %+v", keys)
	}

	// Another device gets its own key; the same device again replaces (revokes) the old one.
	pc := h.requestKey(alice, map[string]any{"device_name": "alice-pc"})
	again := h.requestKey(alice, map[string]any{"device_name": "alice-mac"})
	if again.Status != api.ProxyKeyRequestedStatusPending || again.Key == a.Key {
		t.Fatalf("re-request %+v", again)
	}
	if rec := h.do("POST", approve, nil, withCookie(admin), withCSRF()); rec.Code != http.StatusNotFound {
		t.Fatalf("approve replaced key: %d", rec.Code)
	}
	keys = adminGet[[]api.ProxyKey](h, "/api/admin/proxy-keys", admin)
	if len(keys) != 3 || keys[0].Id != pc.ProxyKey.Id || keys[1].Id != again.ProxyKey.Id ||
		keys[2].Id != a.ProxyKey.Id || keys[2].Status != api.ProxyKeyStatusRevoked || keys[2].RevokedAt == nil {
		t.Fatalf("list: pending oldest first, then revoked: %+v", keys)
	}
	got = h.syncKeys()
	if len(got) != 2 || got[0].Hash != hexSHA(pc.Key) || got[1].Hash != hexSHA(again.Key) {
		t.Fatalf("sync after replace = %+v", got)
	}

	// Admin revoke works on pending keys too.
	revoke := fmt.Sprintf("/api/admin/proxy-keys/%d", pc.ProxyKey.Id)
	if rec := h.do("DELETE", revoke, nil, withCookie(admin)); rec.Code != http.StatusForbidden {
		t.Fatalf("revoke without CSRF header: %d", rec.Code)
	}
	if rec := h.do("DELETE", revoke, nil, withCookie(admin), withCSRF()); rec.Code != http.StatusNoContent {
		t.Fatalf("revoke: %d %s", rec.Code, rec.Body)
	}
	if rec := h.do("DELETE", revoke, nil, withCookie(admin), withCSRF()); rec.Code != http.StatusNotFound {
		t.Fatalf("second revoke: %d", rec.Code)
	}
	if got := h.syncKeys(); len(got) != 1 || got[0].Hash != hexSHA(again.Key) {
		t.Fatalf("sync after revoke = %+v", got)
	}

	// Alice logs the CLI out: their keys drop from the sync list and show "not linked".
	if rec := h.do("DELETE", "/api/auth/token", nil, bearer(alice)); rec.Code != http.StatusNoContent {
		t.Fatalf("logout: %d %s", rec.Code, rec.Body)
	}
	if got := h.syncKeys(); len(got) != 0 {
		t.Fatalf("sync after unlink = %+v", got)
	}
	for _, k := range adminGet[[]api.ProxyKey](h, "/api/admin/proxy-keys", admin) {
		if k.Linked {
			t.Fatalf("still linked: %+v", k)
		}
	}
	if rec := h.do("GET", "/api/proxy/keys", nil); rec.Code != http.StatusUnauthorized {
		t.Fatalf("sync without header: %d", rec.Code)
	}
}

func TestProxyKeyRequestDeviceName(t *testing.T) {
	h := newProxyHarness(t)
	tok := h.loginCLI("dev@example.com")
	for _, c := range []struct {
		name string
		body any
		want string
	}{
		{"missing", map[string]any{}, "unknown"},
		{"blank", map[string]any{"device_name": "   "}, "unknown"},
		{"NUL dropped", map[string]any{"device_name": "a\x00b"}, "ab"},
		{"truncated", map[string]any{"device_name": strings.Repeat("é", 120)}, strings.Repeat("é", 100)},
	} {
		if got := h.requestKey(tok, c.body).ProxyKey.DeviceName; got != c.want {
			t.Errorf("%s: device %q, want %q", c.name, got, c.want)
		}
	}
	if rec := h.do("POST", "/api/proxy-keys/self", map[string]any{"device_name": 7}, bearer(tok)); rec.Code != http.StatusBadRequest {
		t.Fatalf("bad JSON type: %d %s", rec.Code, rec.Body)
	}
}

// Only a CLI bearer token may request a key, so every key is tied to a
// Google-verified email the admin can see.
func TestProxyKeyRequestRequiresCLI(t *testing.T) {
	h := newProxyHarness(t)
	admin := h.loginWeb("admin@example.com")
	body := map[string]any{"device_name": "x"}
	if rec := h.do("POST", "/api/proxy-keys/self", body); rec.Code != http.StatusUnauthorized {
		t.Fatalf("anonymous: %d", rec.Code)
	}
	if rec := h.do("POST", "/api/proxy-keys/self", body, withCookie(admin), withCSRF()); rec.Code != http.StatusForbidden {
		t.Fatalf("cookie: %d", rec.Code)
	}
	if rec := h.do("POST", "/api/proxy-keys/self", body, bearer("cct_bogus")); rec.Code != http.StatusUnauthorized {
		t.Fatalf("bogus bearer: %d", rec.Code)
	}
	var n int
	h.scan(`SELECT count(*) FROM proxy_keys`, &n)
	if n != 0 {
		t.Fatalf("non-CLI requests created %d keys", n)
	}
}

func TestProxyKeyEndpointsRequireAdmin(t *testing.T) {
	h := newProxyHarness(t)
	dev := h.loginWeb("dev@example.com")
	devTok := h.loginCLI("dev@example.com")
	adminTok := h.loginCLI("admin@example.com")
	k := h.requestKey(devTok, map[string]any{"device_name": "dev-mac"})
	for _, rt := range []struct{ method, path string }{
		{"GET", "/api/admin/proxy-keys"},
		{"POST", fmt.Sprintf("/api/admin/proxy-keys/%d/approve", k.ProxyKey.Id)},
		{"DELETE", fmt.Sprintf("/api/admin/proxy-keys/%d", k.ProxyKey.Id)},
	} {
		if rec := h.do(rt.method, rt.path, nil, withCookie(dev), withCSRF()); rec.Code != http.StatusForbidden {
			t.Errorf("non-admin cookie %s %s: %d", rt.method, rt.path, rec.Code)
		}
		for _, tok := range []string{devTok, adminTok} {
			if rec := h.do(rt.method, rt.path, nil, bearer(tok)); rec.Code != http.StatusForbidden {
				t.Errorf("bearer %s %s: %d", rt.method, rt.path, rec.Code)
			}
		}
	}
	var approved, revoked int
	h.scan(`SELECT count(approved_at), count(revoked_at) FROM proxy_keys`, &approved, &revoked)
	if approved != 0 || revoked != 0 {
		t.Fatalf("non-admin requests approved %d / revoked %d keys", approved, revoked)
	}
	// The old admin issue endpoint is gone.
	admin := h.loginWeb("admin@example.com")
	if rec := h.do("POST", "/api/admin/proxy-keys", map[string]any{"user_id": 1}, withCookie(admin), withCSRF()); rec.Code != http.StatusMethodNotAllowed && rec.Code != http.StatusNotFound {
		t.Fatalf("admin issue endpoint still routed: %d", rec.Code)
	}
	// An admin session cookie is not a sync credential.
	if rec := h.do("GET", "/api/proxy/keys", nil, withCookie(admin)); rec.Code != http.StatusUnauthorized {
		t.Fatalf("sync with admin cookie: %d", rec.Code)
	}
}

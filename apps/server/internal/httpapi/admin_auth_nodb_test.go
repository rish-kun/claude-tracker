package httpapi

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"cctrack/server/internal/store"
)

// TestRequireAdminRejectsBearer runs without a database: it places principals
// directly in the request context, as authenticate would.
func TestRequireAdminRejectsBearer(t *testing.T) {
	s := New(testConfig(), nil, &fakeIDP{})
	admin := store.User{ID: 1, Email: "admin@example.com"}
	dev := store.User{ID: 2, Email: "dev@example.com"}
	for _, c := range []struct {
		name    string
		p       *principal
		want    int
		wantErr string
	}{
		{"anonymous", nil, http.StatusUnauthorized, "not logged in"},
		{"admin cookie", &principal{User: admin}, http.StatusOK, ""},
		{"admin bearer", &principal{User: admin, TokenID: 7}, http.StatusForbidden, "admin endpoints require a browser session"},
		{"non-admin cookie", &principal{User: dev}, http.StatusForbidden, "admin only"},
		{"non-admin bearer", &principal{User: dev, TokenID: 8}, http.StatusForbidden, "browser session"},
	} {
		t.Run(c.name, func(t *testing.T) {
			r := httptest.NewRequest("GET", "/api/admin/accounts", nil)
			if c.p != nil {
				r = r.WithContext(context.WithValue(r.Context(), ctxKey{}, *c.p))
			}
			rec := httptest.NewRecorder()
			_, ok := s.requireAdmin(rec, r)
			if ok != (c.want == http.StatusOK) || (!ok && rec.Code != c.want) {
				t.Fatalf("ok=%v code=%d, want %d", ok, rec.Code, c.want)
			}
			if c.wantErr != "" && !strings.Contains(rec.Body.String(), c.wantErr) {
				t.Fatalf("body %q, want %q", rec.Body, c.wantErr)
			}
		})
	}
}

// TestAccountToAPIReturnsRawLabel: the admin settings form edits the stored
// label, so the API must not substitute the email/uuid fallback.
func TestAccountToAPIReturnsRawLabel(t *testing.T) {
	email := "shared@example.com"
	a := store.Account{ID: 1, AccountUUID: "acc-1", Email: &email, Plan: "pro"}
	if got := accountToAPI(a).Label; got != "" {
		t.Fatalf("Account.Label = %q, want empty", got)
	}
	if got := accountRef(a).Label; got != email {
		t.Fatalf("AccountRef.Label = %q, want display fallback %q", got, email)
	}
	a.Label = "Team Max"
	if got := accountToAPI(a).Label; got != "Team Max" {
		t.Fatalf("Account.Label = %q", got)
	}
}

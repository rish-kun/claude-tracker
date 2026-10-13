package httpapi

// No-DB tests for request-body checks that run before the store is touched:
// handlers are called directly with a principal in the context (as
// authenticate would place it) and a nil store.

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"log"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"cctrack/server/internal/api"
	"cctrack/server/internal/store"
)

var (
	adminCookie = principal{User: store.User{ID: 1, Email: "admin@example.com"}}
	cliBearer   = principal{User: store.User{ID: 2, Email: "dev@example.com"}, TokenID: 9}
)

func requestAs(p principal, method, path, body string) *http.Request {
	r := httptest.NewRequest(method, path, strings.NewReader(body))
	return r.WithContext(context.WithValue(r.Context(), ctxKey{}, p))
}

func wantError(t *testing.T, rec *httptest.ResponseRecorder, code int, substr string) {
	t.Helper()
	if rec.Code != code {
		t.Fatalf("status %d, want %d: %s", rec.Code, code, rec.Body)
	}
	if msg := decode[map[string]string](t, rec)["error"]; !strings.Contains(msg, substr) {
		t.Fatalf("error = %q, want substring %q", msg, substr)
	}
}

const fullRates = `{"input":3,"cache_write_5m":3.75,"cache_write_1h":6,"cache_read":0.3,"output":15}`

func TestCheckPriceKeys(t *testing.T) {
	for _, c := range []struct{ name, body, want string }{
		{"all base rates", fullRates, ""},
		{"optional nulls allowed", `{"input":3,"cache_write_5m":3.75,"cache_write_1h":6,"cache_read":0.3,"output":15,"fast_input":null}`, ""},
		{"empty object", `{}`, "input is required"},
		{"null body", `null`, "input is required"},
		{"missing output", `{"input":3,"cache_write_5m":3.75,"cache_write_1h":6,"cache_read":0.3}`, "output is required"},
		{"null cache_read", `{"input":3,"cache_write_5m":3.75,"cache_write_1h":6,"cache_read":null,"output":15}`, "cache_read is required"},
		{"zero is a value", `{"input":0,"cache_write_5m":0,"cache_write_1h":0,"cache_read":0,"output":0}`, ""},
	} {
		t.Run(c.name, func(t *testing.T) {
			err := checkPriceKeys([]byte(c.body))
			if c.want == "" {
				if err != nil {
					t.Fatalf("unexpected error %v", err)
				}
				return
			}
			if err == nil || !strings.Contains(err.Error(), c.want) {
				t.Fatalf("err = %v, want %q", err, c.want)
			}
		})
	}
}

// ingestJSON returns the fixture as generic JSON so tests can drop keys.
func ingestJSON(t *testing.T) map[string]any {
	t.Helper()
	req := ingestFixture("s1", "acc-1", entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 10, 5))
	b, err := json.Marshal(req)
	if err != nil {
		t.Fatal(err)
	}
	var m map[string]any
	if err := json.Unmarshal(b, &m); err != nil {
		t.Fatal(err)
	}
	return m
}

func marshal(t *testing.T, v any) string {
	t.Helper()
	b, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

func TestCheckIngestKeys(t *testing.T) {
	if err := checkIngestKeys([]byte(marshal(t, ingestJSON(t)))); err != nil {
		t.Fatalf("complete batch: %v", err)
	}
	for _, key := range entryRequiredKeys {
		for _, null := range []bool{false, true} {
			m := ingestJSON(t)
			e := m["entries"].([]any)[0].(map[string]any)
			if null {
				e[key] = nil
			} else {
				delete(e, key)
			}
			err := checkIngestKeys([]byte(marshal(t, m)))
			if want := "entries[0]." + key + " is required"; err == nil || err.Error() != want {
				t.Errorf("%s (null=%v): err = %v, want %q", key, null, err, want)
			}
		}
	}
	// Optional keys may be omitted.
	m := ingestJSON(t)
	e := m["entries"].([]any)[0].(map[string]any)
	delete(e, "request_id")
	delete(e, "speed")
	delete(e, "is_subagent")
	if err := checkIngestKeys([]byte(marshal(t, m))); err != nil {
		t.Fatalf("optional keys omitted: %v", err)
	}
}

func TestIngestHandlerRejectsMissingTokenKey(t *testing.T) {
	s := New(testConfig(), nil, &fakeIDP{})
	m := ingestJSON(t)
	delete(m["entries"].([]any)[0].(map[string]any), "cache_read")
	rec := httptest.NewRecorder()
	s.Ingest(rec, requestAs(cliBearer, "POST", "/api/ingest", marshal(t, m)))
	wantError(t, rec, http.StatusBadRequest, "entries[0].cache_read is required")
}

func TestIngestHandlerRejectsCapsAndNUL(t *testing.T) {
	s := New(testConfig(), nil, &fakeIDP{})
	for _, c := range []struct {
		name, want string
		mutate     func(e map[string]any)
	}{
		{"input over cap", "each token count", func(e map[string]any) { e["input"] = maxTokensPerField + 1 }},
		{"NUL model", "must not contain NUL", func(e map[string]any) { e["model"] = "claude\x00" }},
	} {
		t.Run(c.name, func(t *testing.T) {
			m := ingestJSON(t)
			c.mutate(m["entries"].([]any)[0].(map[string]any))
			rec := httptest.NewRecorder()
			s.Ingest(rec, requestAs(cliBearer, "POST", "/api/ingest", marshal(t, m)))
			wantError(t, rec, http.StatusBadRequest, c.want)
		})
	}
}

func TestAdminPutPriceRequiresBaseRates(t *testing.T) {
	s := New(testConfig(), nil, &fakeIDP{})
	for _, c := range []struct{ name, model, body, want string }{
		{"empty body", "m", `{}`, "input is required"},
		{"missing cache_write_1h", "m", `{"input":3,"cache_write_5m":3.75,"cache_read":0.3,"output":15}`, "cache_write_1h is required"},
		{"null input", "m", `{"input":null,"cache_write_5m":3.75,"cache_write_1h":6,"cache_read":0.3,"output":15}`, "input is required"},
		{"NUL model", "bad\x00model", fullRates, "must not contain NUL"},
	} {
		t.Run(c.name, func(t *testing.T) {
			rec := httptest.NewRecorder()
			s.AdminPutPrice(rec, requestAs(adminCookie, "PUT", "/api/admin/prices/m", c.body), c.model, api.AdminPutPriceParams{})
			wantError(t, rec, http.StatusBadRequest, c.want)
		})
	}
}

func TestNULRejectedInSummaryAndLabel(t *testing.T) {
	s := New(testConfig(), nil, &fakeIDP{})
	rec := httptest.NewRecorder()
	s.SetSessionSummary(rec, requestAs(cliBearer, "POST", "/api/sessions/s1/summary", `{"summary":"did \u0000 things"}`), "s1")
	wantError(t, rec, http.StatusBadRequest, "must not contain NUL")

	rec = httptest.NewRecorder()
	s.SetSessionSummary(rec, requestAs(cliBearer, "POST", "/api/sessions/x/summary", `{"summary":"ok"}`), "s\x001")
	wantError(t, rec, http.StatusBadRequest, "must not contain NUL")

	rec = httptest.NewRecorder()
	s.AdminUpdateAccount(rec, requestAs(adminCookie, "PATCH", "/api/admin/accounts/1", `{"label":"Team\u0000Max"}`), 1)
	wantError(t, rec, http.StatusBadRequest, "must not contain NUL")
}

// TestIngestAndSummaryRequireBearer: a browser session (TokenID == 0) must not
// write usage or summaries, even with the CSRF header.
func TestIngestAndSummaryRequireBearer(t *testing.T) {
	s := New(testConfig(), nil, &fakeIDP{})
	cookieUser := principal{User: store.User{ID: 2, Email: "dev@example.com"}}

	rec := httptest.NewRecorder()
	s.Ingest(rec, requestAs(cookieUser, "POST", "/api/ingest", marshal(t, ingestJSON(t))))
	wantError(t, rec, http.StatusForbidden, "use the cctrack CLI")

	rec = httptest.NewRecorder()
	s.SetSessionSummary(rec, requestAs(cookieUser, "POST", "/api/sessions/s1/summary", `{"summary":"x"}`), "s1")
	wantError(t, rec, http.StatusForbidden, "use the cctrack CLI")

	rec = httptest.NewRecorder()
	s.Ingest(rec, httptest.NewRequest("POST", "/api/ingest", strings.NewReader("{}")))
	wantError(t, rec, http.StatusUnauthorized, "not logged in")

	// requireCLI admits a bearer principal.
	if _, ok := s.requireCLI(httptest.NewRecorder(), requestAs(cliBearer, "POST", "/api/ingest", "")); !ok {
		t.Fatal("bearer principal rejected")
	}
}

// captureLog redirects the standard logger for the rest of the test.
func captureLog(t *testing.T) *bytes.Buffer {
	t.Helper()
	var buf bytes.Buffer
	prev := log.Writer()
	log.SetOutput(&buf)
	t.Cleanup(func() { log.SetOutput(prev) })
	return &buf
}

func TestExchangeFailedPageIsGeneric(t *testing.T) {
	logs := captureLog(t)
	rec := httptest.NewRecorder()
	exchangeFailed(rec, errors.New(`exchange code: oauth2: "invalid_client" secret-detail-xyz`))
	if rec.Code != http.StatusBadGateway {
		t.Fatalf("status %d", rec.Code)
	}
	body := rec.Body.String()
	if !strings.Contains(body, "Google sign-in failed; please try again.") || strings.Contains(body, "secret-detail-xyz") {
		t.Fatalf("page leaks or lacks the generic message: %s", body)
	}
	if !strings.Contains(logs.String(), "secret-detail-xyz") {
		t.Fatalf("detail not logged: %q", logs)
	}
}

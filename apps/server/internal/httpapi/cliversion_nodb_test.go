package httpapi

// Tests in this file run without TEST_DATABASE_URL: the version check runs
// before authentication, and requests that pass it stop at "not logged in".

import (
	"context"
	"errors"
	"net/http"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"cctrack/server/internal/api"
	"cctrack/server/internal/cliversion"
)

const (
	testSetupURL    = "https://github.com/acme/cctrack/releases/download/cli-v0.3.0/setup.md"
	testDownloadURL = "https://github.com/acme/cctrack/releases/download/cli-v0.3.0"
)

// versionServer returns a server whose latest CLI release is cli-v0.3.0, or
// unknown when fail is set, and a count of lookups.
func versionServer(t *testing.T, fail bool) (*Server, *atomic.Int32) {
	t.Helper()
	cfg := testConfig()
	cfg.CLIReleasesRepo = "acme/cctrack"
	srv := New(cfg, nil, &fakeIDP{})
	var calls atomic.Int32
	srv.cliVersion = cliversion.New(cfg.CLIReleasesRepo, func(ctx context.Context) (cliversion.Release, error) {
		calls.Add(1)
		if fail {
			return cliversion.Release{}, errors.New("GitHub down")
		}
		return cliversion.Release{Version: cliversion.Version{Major: 0, Minor: 3, Patch: 0}, Tag: "cli-v0.3.0"}, nil
	}, func() time.Time { return srv.now() })
	return srv, &calls
}

func TestCLIVersionGate(t *testing.T) {
	h := func() http.Handler { srv, _ := versionServer(t, false); return srv.Handler() }()
	bearer := []string{"Authorization", "Bearer cct_whatever"} // nil store: must never be looked up
	ua := func(v string, extra ...string) []string { return append([]string{"User-Agent", v}, extra...) }
	for _, c := range []struct {
		name, method, path string
		hdr                []string
		want               int
		wantErr            string
	}{
		{"older CLI", "GET", "/api/me", ua("cctrack/0.2.0"), 426, "cctrack 0.2.0 is out of date: the latest is 0.3.0"},
		{"older CLI with bearer skips auth", "POST", "/api/ingest", ua("cctrack/0.2.9", bearer...), 426, "out of date"},
		{"0.2.10 is still older than 0.3.0", "GET", "/api/me", ua("cctrack/0.2.10"), 426, "out of date"},
		{"equal CLI", "GET", "/api/me", ua("cctrack/0.3.0"), 401, "not logged in"},
		{"newer CLI", "GET", "/api/me", ua("cctrack/0.10.0"), 401, "not logged in"},
		{"version then comment", "GET", "/api/me", ua("cctrack/0.3.0 (darwin)"), 401, "not logged in"},
		{"garbage version", "GET", "/api/me", ua("cctrack/banana"), 426, "cctrack banana is out of date"},
		{"prerelease version", "GET", "/api/me", ua("cctrack/0.4.0-beta.1"), 426, "out of date"},
		{"empty version", "GET", "/api/me", ua("cctrack/"), 426, "out of date"},
		{"version endpoint exempt", "GET", "/api/cli/version", ua("cctrack/0.1.0"), 200, ""},
		{"logout exempt", "DELETE", "/api/auth/token", ua("cctrack/0.1.0"), 401, "not logged in"},
		{"other auth/token method not exempt", "GET", "/api/auth/token", ua("cctrack/0.1.0"), 426, "out of date"},
		{"browser", "GET", "/api/me", ua("Mozilla/5.0 (Macintosh) cctrack/0.1.0"), 401, "not logged in"},
		{"curl", "GET", "/api/me", ua("curl/8.7.1"), 401, "not logged in"},
		{"no user agent", "GET", "/api/me", nil, 401, "not logged in"},
		{"case-sensitive prefix", "GET", "/api/me", ua("CCTrack/0.1.0"), 401, "not logged in"},
	} {
		t.Run(c.name, func(t *testing.T) {
			rec := noDBRequest(t, h, c.method, c.path, "", c.hdr...)
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

func TestCLIVersionGateBody(t *testing.T) {
	srv, _ := versionServer(t, false)
	rec := noDBRequest(t, srv.Handler(), "GET", "/api/me", "", "User-Agent", "cctrack/0.2.0")
	if rec.Code != http.StatusUpgradeRequired {
		t.Fatalf("status %d: %s", rec.Code, rec.Body)
	}
	if !strings.HasPrefix(rec.Body.String(), `{"error":`) {
		t.Fatalf("error is not the first field: %s", rec.Body)
	}
	got := decode[api.CliOutdated](t, rec)
	want := api.CliOutdated{
		Error: `cctrack 0.2.0 is out of date: the latest is 0.3.0. Update cctrack (see ` + testSetupURL +
			`, section "Update cctrack") and try again.`,
		Latest:   "0.3.0",
		SetupUrl: testSetupURL,
	}
	if got != want {
		t.Fatalf("body = %+v\nwant %+v", got, want)
	}
	if cc := rec.Header().Get("Cache-Control"); cc != "no-store" {
		t.Fatalf("Cache-Control = %q", cc)
	}
}

// Browsers and other clients never wait on the GitHub lookup.
func TestCLIVersionGateSkipsLookupForNonCLI(t *testing.T) {
	srv, calls := versionServer(t, false)
	h := srv.Handler()
	noDBRequest(t, h, "GET", "/api/me", "", "User-Agent", "Mozilla/5.0")
	noDBRequest(t, h, "GET", "/api/healthz", "")
	if n := calls.Load(); n != 0 {
		t.Fatalf("%d lookups for non-CLI requests", n)
	}
	noDBRequest(t, h, "GET", "/api/me", "", "User-Agent", "cctrack/0.3.0")
	noDBRequest(t, h, "GET", "/api/me", "", "User-Agent", "cctrack/0.2.0")
	if n := calls.Load(); n != 1 {
		t.Fatalf("%d lookups for two CLI requests, want 1 (cached)", n)
	}
}

func TestCLIVersionUnknownFailsOpen(t *testing.T) {
	srv, _ := versionServer(t, true)
	h := srv.Handler()
	rec := noDBRequest(t, h, "GET", "/api/me", "", "User-Agent", "cctrack/0.0.1")
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("status %d, want 401 (not enforced): %s", rec.Code, rec.Body)
	}
	rec = noDBRequest(t, h, "GET", "/api/cli/version", "")
	if rec.Code != http.StatusServiceUnavailable {
		t.Fatalf("status %d, want 503: %s", rec.Code, rec.Body)
	}
	if msg := decode[map[string]string](t, rec)["error"]; msg != "latest CLI version unavailable" {
		t.Fatalf("error = %q", msg)
	}
}

func TestCLIVersionDisabled(t *testing.T) {
	srv := New(testConfig(), nil, &fakeIDP{}) // no CLIReleasesRepo
	if srv.cliVersion != nil {
		t.Fatal("version check enabled without a repo")
	}
	h := srv.Handler()
	rec := noDBRequest(t, h, "GET", "/api/me", "", "User-Agent", "cctrack/0.0.1")
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("status %d, want 401 (not enforced): %s", rec.Code, rec.Body)
	}
	rec = noDBRequest(t, h, "GET", "/api/cli/version", "")
	if rec.Code != http.StatusServiceUnavailable {
		t.Fatalf("status %d, want 503: %s", rec.Code, rec.Body)
	}
	if msg := decode[map[string]string](t, rec)["error"]; msg != "CLI version check not configured" {
		t.Fatalf("error = %q", msg)
	}
}

func TestGetCliVersion(t *testing.T) {
	srv, _ := versionServer(t, false)
	rec := noDBRequest(t, srv.Handler(), "GET", "/api/cli/version", "")
	if rec.Code != http.StatusOK {
		t.Fatalf("status %d: %s", rec.Code, rec.Body)
	}
	if cc := rec.Header().Get("Cache-Control"); cc != "public, max-age=300" {
		t.Fatalf("Cache-Control = %q", cc)
	}
	if ct := rec.Header().Get("Content-Type"); ct != "application/json" {
		t.Fatalf("Content-Type = %q", ct)
	}
	want := api.CliVersion{Latest: "0.3.0", Tag: "cli-v0.3.0", DownloadUrl: testDownloadURL, SetupUrl: testSetupURL}
	if got := decode[api.CliVersion](t, rec); got != want {
		t.Fatalf("body = %+v\nwant %+v", got, want)
	}
}

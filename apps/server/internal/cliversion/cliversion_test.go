package cliversion

import (
	"context"
	"errors"
	"io"
	"net/http"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func TestParse(t *testing.T) {
	for _, c := range []struct {
		in   string
		want Version
		ok   bool
	}{
		{"0.2.0", Version{0, 2, 0}, true},
		{"10.20.30", Version{10, 20, 30}, true},
		{"1.2", Version{}, false},
		{"1.2.3.4", Version{}, false},
		{"v1.2.3", Version{}, false},
		{"1.2.3-beta.1", Version{}, false},
		{"1.2.3+build", Version{}, false},
		{"01.2.3", Version{}, false},
		{"1..3", Version{}, false},
		{"-1.2.3", Version{}, false},
		{"+1.2.3", Version{}, false},
		{" 1.2.3", Version{}, false},
		{"1.2.99999999999999999999", Version{}, false},
		{"", Version{}, false},
		{"garbage", Version{}, false},
	} {
		got, ok := Parse(c.in)
		if ok != c.ok || got != c.want {
			t.Errorf("Parse(%q) = %v, %v; want %v, %v", c.in, got, ok, c.want, c.ok)
		}
	}
}

func TestCompare(t *testing.T) {
	v := func(s string) Version {
		t.Helper()
		x, ok := Parse(s)
		if !ok {
			t.Fatalf("parse %q", s)
		}
		return x
	}
	for _, c := range []struct {
		a, b string
		want int
	}{
		{"0.10.0", "0.9.0", 1},
		{"0.9.0", "0.10.0", -1},
		{"1.0.0", "0.99.99", 1},
		{"0.2.1", "0.2.0", 1},
		{"0.2.0", "0.2.0", 0},
	} {
		if got := v(c.a).Compare(v(c.b)); got != c.want {
			t.Errorf("%s vs %s = %d, want %d", c.a, c.b, got, c.want)
		}
	}
	if s := (Version{0, 10, 3}).String(); s != "0.10.3" {
		t.Errorf("String = %q", s)
	}
}

func TestPick(t *testing.T) {
	rel, ok := Pick([]GitHubRelease{
		{TagName: "cli-v0.9.0"},
		{TagName: "cli-v0.10.0"},
		{TagName: "cli-v0.11.0", Draft: true},
		{TagName: "cli-v0.12.0", Prerelease: true},
		{TagName: "cli-v0.13.0-rc.1"},
		{TagName: "v9.9.9"},
		{TagName: "plugin-v5.0.0"},
		{TagName: "cli-v1.0"},
		{TagName: "cli-v0.2.0"},
	})
	if !ok || rel.Tag != "cli-v0.10.0" || rel.Version != (Version{0, 10, 0}) {
		t.Fatalf("Pick = %+v, %v; want cli-v0.10.0", rel, ok)
	}
	if _, ok := Pick([]GitHubRelease{{TagName: "v1.0.0"}, {TagName: "cli-v2.0.0", Draft: true}}); ok {
		t.Fatal("Pick found a release among none eligible")
	}
	if _, ok := Pick(nil); ok {
		t.Fatal("Pick(nil) found a release")
	}
}

type roundTrip func(*http.Request) (*http.Response, error)

func (f roundTrip) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func TestGitHubFetcher(t *testing.T) {
	var got *http.Request
	status, body := http.StatusOK, `[{"tag_name":"cli-v0.2.0","draft":false,"prerelease":false},
		{"tag_name":"cli-v0.3.0","draft":false,"prerelease":false,"name":"x","body":"notes"}]`
	client := &http.Client{Transport: roundTrip(func(r *http.Request) (*http.Response, error) {
		got = r
		return &http.Response{StatusCode: status, Body: io.NopCloser(strings.NewReader(body)), Header: http.Header{}}, nil
	})}

	rel, err := GitHubFetcher(client, "acme/cctrack", "")(context.Background())
	if err != nil || rel.Tag != "cli-v0.3.0" {
		t.Fatalf("rel = %+v, err %v", rel, err)
	}
	if u := got.URL.String(); u != "https://api.github.com/repos/acme/cctrack/releases?per_page=30" {
		t.Fatalf("URL = %s", u)
	}
	if got.Header.Get("Accept") != "application/vnd.github+json" || got.Header.Get("User-Agent") == "" {
		t.Fatalf("headers = %v", got.Header)
	}
	if a := got.Header.Get("Authorization"); a != "" {
		t.Fatalf("Authorization sent without a token: %q", a)
	}

	if _, err := GitHubFetcher(client, "acme/cctrack", "ghp_secret")(context.Background()); err != nil {
		t.Fatal(err)
	}
	if a := got.Header.Get("Authorization"); a != "Bearer ghp_secret" {
		t.Fatalf("Authorization = %q", a)
	}

	status = http.StatusForbidden
	if _, err := GitHubFetcher(client, "acme/cctrack", "")(context.Background()); err == nil || !strings.Contains(err.Error(), "403") {
		t.Fatalf("err = %v, want HTTP 403", err)
	}
	status, body = http.StatusOK, `[{"tag_name":"v1.0.0"}]`
	if _, err := GitHubFetcher(client, "acme/cctrack", "")(context.Background()); err == nil {
		t.Fatal("expected error when no cli release exists")
	}
	body = `not json`
	if _, err := GitHubFetcher(client, "acme/cctrack", "")(context.Background()); err == nil {
		t.Fatal("expected error for invalid JSON")
	}
}

// fakeFetch returns results in order (the last one repeats) and counts calls.
type fakeFetch struct {
	mu      sync.Mutex
	calls   int
	results []result
}

type result struct {
	tag string
	err error
}

func (f *fakeFetch) fetch(ctx context.Context) (Release, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if _, ok := ctx.Deadline(); !ok {
		return Release{}, errors.New("fetch without a deadline")
	}
	r := f.results[min(f.calls, len(f.results)-1)]
	f.calls++
	if r.err != nil {
		return Release{}, r.err
	}
	v, _ := Parse(strings.TrimPrefix(r.tag, TagPrefix))
	return Release{Version: v, Tag: r.tag}, nil
}

func (f *fakeFetch) count() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.calls
}

type clock struct{ t time.Time }

func (c *clock) now() time.Time          { return c.t }
func (c *clock) advance(d time.Duration) { c.t = c.t.Add(d) }
func newClock() *clock                   { return &clock{time.Date(2026, 10, 16, 12, 0, 0, 0, time.UTC)} }
func expect(t *testing.T, s *Source, tag string, ok bool, calls int, f *fakeFetch) {
	t.Helper()
	rel, got := s.Latest(context.Background())
	if got != ok || rel.Tag != tag || f.count() != calls {
		t.Fatalf("Latest = %q, %v after %d fetches; want %q, %v after %d", rel.Tag, got, f.count(), tag, ok, calls)
	}
}

func TestSourceCachesForTenMinutes(t *testing.T) {
	c := newClock()
	f := &fakeFetch{results: []result{{tag: "cli-v0.2.0"}, {tag: "cli-v0.3.0"}}}
	s := New("acme/cctrack", f.fetch, c.now)
	expect(t, s, "cli-v0.2.0", true, 1, f)
	c.advance(9*time.Minute + 59*time.Second)
	expect(t, s, "cli-v0.2.0", true, 1, f)
	c.advance(time.Second)
	expect(t, s, "cli-v0.3.0", true, 2, f)
	expect(t, s, "cli-v0.3.0", true, 2, f)
}

func TestSourceStaleOnErrorAndRetrySpacing(t *testing.T) {
	c := newClock()
	boom := errors.New("boom")
	f := &fakeFetch{results: []result{{tag: "cli-v0.2.0"}, {err: boom}, {err: boom}, {tag: "cli-v0.4.0"}}}
	s := New("acme/cctrack", f.fetch, c.now)
	expect(t, s, "cli-v0.2.0", true, 1, f)
	c.advance(10 * time.Minute)
	expect(t, s, "cli-v0.2.0", true, 2, f) // failed: last known value
	c.advance(59 * time.Second)
	expect(t, s, "cli-v0.2.0", true, 2, f) // within a minute: no retry
	c.advance(time.Second)
	expect(t, s, "cli-v0.2.0", true, 3, f) // retried, failed again
	c.advance(time.Minute)
	expect(t, s, "cli-v0.4.0", true, 4, f) // retried, succeeded
	c.advance(5 * time.Minute)
	expect(t, s, "cli-v0.4.0", true, 4, f) // fresh again for ten minutes
}

func TestSourceUnknownUntilFirstSuccess(t *testing.T) {
	c := newClock()
	f := &fakeFetch{results: []result{{err: errors.New("rate limited")}, {tag: "cli-v0.2.0"}}}
	s := New("acme/cctrack", f.fetch, c.now)
	expect(t, s, "", false, 1, f)
	c.advance(30 * time.Second)
	expect(t, s, "", false, 1, f)
	c.advance(30 * time.Second)
	expect(t, s, "cli-v0.2.0", true, 2, f)
}

func TestSourceNoStampede(t *testing.T) {
	release := make(chan struct{})
	var calls atomic.Int32
	s := New("acme/cctrack", func(ctx context.Context) (Release, error) {
		calls.Add(1)
		<-release
		return Release{Version: Version{0, 2, 0}, Tag: "cli-v0.2.0"}, nil
	}, nil)
	var wg sync.WaitGroup
	for range 20 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if rel, ok := s.Latest(context.Background()); !ok || rel.Tag != "cli-v0.2.0" {
				t.Errorf("Latest = %+v, %v", rel, ok)
			}
		}()
	}
	time.Sleep(20 * time.Millisecond)
	close(release)
	wg.Wait()
	if n := calls.Load(); n != 1 {
		t.Fatalf("%d fetches for concurrent callers, want 1", n)
	}
}

// A cancelled request must not abort the shared lookup.
func TestSourceIgnoresCallerCancellation(t *testing.T) {
	s := New("acme/cctrack", func(ctx context.Context) (Release, error) {
		if err := ctx.Err(); err != nil {
			return Release{}, err
		}
		return Release{Version: Version{0, 2, 0}, Tag: "cli-v0.2.0"}, nil
	}, nil)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, ok := s.Latest(ctx); !ok {
		t.Fatal("lookup failed for a cancelled caller")
	}
}

func TestURLs(t *testing.T) {
	s := New("acme/cctrack", nil, nil)
	rel := Release{Version: Version{0, 3, 0}, Tag: "cli-v0.3.0"}
	if u := s.DownloadURL(rel); u != "https://github.com/acme/cctrack/releases/download/cli-v0.3.0" {
		t.Errorf("DownloadURL = %s", u)
	}
	if u := s.SetupURL(rel); u != "https://github.com/acme/cctrack/releases/download/cli-v0.3.0/setup.md" {
		t.Errorf("SetupURL = %s", u)
	}
}

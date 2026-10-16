// Package cliversion tracks the latest published cctrack CLI release, read
// from the GitHub releases of the repository the server is deployed from.
package cliversion

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	// TagPrefix marks CLI releases among the repository's GitHub releases.
	TagPrefix = "cli-v"

	cacheTTL      = 10 * time.Minute
	retryInterval = time.Minute
	fetchTimeout  = 2 * time.Second
)

// Version is a strict X.Y.Z semantic version (no pre-release or build parts).
type Version struct{ Major, Minor, Patch int }

// Parse accepts exactly three dot-separated decimal integers without signs
// or leading zeros.
func Parse(s string) (Version, bool) {
	parts := strings.Split(s, ".")
	if len(parts) != 3 {
		return Version{}, false
	}
	var n [3]int
	for i, p := range parts {
		if p == "" || len(p) > 9 || (len(p) > 1 && p[0] == '0') {
			return Version{}, false
		}
		for _, c := range p {
			if c < '0' || c > '9' {
				return Version{}, false
			}
		}
		n[i], _ = strconv.Atoi(p) // digits only and <= 9 of them: cannot fail
	}
	return Version{n[0], n[1], n[2]}, true
}

func (v Version) String() string { return fmt.Sprintf("%d.%d.%d", v.Major, v.Minor, v.Patch) }

// Compare returns -1, 0 or +1 as v is lower than, equal to or higher than o.
func (v Version) Compare(o Version) int {
	for _, d := range [3]int{v.Major - o.Major, v.Minor - o.Minor, v.Patch - o.Patch} {
		if d < 0 {
			return -1
		}
		if d > 0 {
			return 1
		}
	}
	return 0
}

// Release is a published CLI release.
type Release struct {
	Version Version
	Tag     string // TagPrefix + Version
}

// FetchFunc looks up the latest CLI release.
type FetchFunc func(ctx context.Context) (Release, error)

// GitHubRelease is the subset of a GitHub release object that matters here.
type GitHubRelease struct {
	TagName    string `json:"tag_name"`
	Draft      bool   `json:"draft"`
	Prerelease bool   `json:"prerelease"`
}

// Pick returns the highest-versioned published (not draft, not prerelease)
// release tagged cli-vX.Y.Z.
func Pick(releases []GitHubRelease) (Release, bool) {
	var best Release
	found := false
	for _, r := range releases {
		if r.Draft || r.Prerelease {
			continue
		}
		raw, ok := strings.CutPrefix(r.TagName, TagPrefix)
		if !ok {
			continue
		}
		v, ok := Parse(raw)
		if !ok {
			continue
		}
		if !found || v.Compare(best.Version) > 0 {
			best, found = Release{Version: v, Tag: r.TagName}, true
		}
	}
	return best, found
}

// GitHubFetcher reads the latest CLI release of repo ("owner/name") from the
// GitHub REST API. token is optional; it raises the API rate limit.
func GitHubFetcher(client *http.Client, repo, token string) FetchFunc {
	url := "https://api.github.com/repos/" + repo + "/releases?per_page=30"
	return func(ctx context.Context) (Release, error) {
		req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
		if err != nil {
			return Release{}, err
		}
		req.Header.Set("Accept", "application/vnd.github+json")
		req.Header.Set("X-GitHub-Api-Version", "2022-11-28")
		req.Header.Set("User-Agent", "cctrack-server")
		if token != "" {
			req.Header.Set("Authorization", "Bearer "+token)
		}
		resp, err := client.Do(req)
		if err != nil {
			return Release{}, err
		}
		defer resp.Body.Close()
		if resp.StatusCode != http.StatusOK {
			return Release{}, fmt.Errorf("GitHub releases: HTTP %d", resp.StatusCode)
		}
		var releases []GitHubRelease
		if err := json.NewDecoder(io.LimitReader(resp.Body, 8<<20)).Decode(&releases); err != nil {
			return Release{}, fmt.Errorf("GitHub releases: %w", err)
		}
		rel, ok := Pick(releases)
		if !ok {
			return Release{}, errors.New("GitHub releases: no published " + TagPrefix + "X.Y.Z release")
		}
		return rel, nil
	}
}

// Source caches the latest release for ten minutes. A failed lookup keeps
// serving the last known release and is retried at most once a minute.
// Callers are serialized, so concurrent requests share one lookup.
type Source struct {
	repo  string
	fetch FetchFunc
	now   func() time.Time

	mu        sync.Mutex
	latest    Release
	have      bool
	fetchedAt time.Time // last successful lookup
	triedAt   time.Time // last lookup attempt, successful or not
}

// New returns a Source for repo ("owner/name"). now defaults to time.Now.
func New(repo string, fetch FetchFunc, now func() time.Time) *Source {
	if now == nil {
		now = time.Now
	}
	return &Source{repo: repo, fetch: fetch, now: now}
}

// Latest returns the latest release, or false when it has never been
// successfully looked up.
func (s *Source) Latest(ctx context.Context) (Release, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	now := s.now()
	if s.have && now.Sub(s.fetchedAt) < cacheTTL {
		return s.latest, true
	}
	if !s.triedAt.IsZero() && now.Sub(s.triedAt) < retryInterval {
		return s.latest, s.have
	}
	s.triedAt = now
	// The lookup outlives a cancelled request: its result serves everyone.
	fctx, cancel := context.WithTimeout(context.WithoutCancel(ctx), fetchTimeout)
	defer cancel()
	rel, err := s.fetch(fctx)
	if err != nil {
		log.Printf("latest CLI version lookup failed: %v", err)
		return s.latest, s.have
	}
	s.latest, s.have, s.fetchedAt = rel, true, now
	return s.latest, true
}

// DownloadURL is the base URL of rel's release assets (append an asset name).
func (s *Source) DownloadURL(rel Release) string {
	return "https://github.com/" + s.repo + "/releases/download/" + rel.Tag
}

// SetupURL is rel's setup guide, which includes the update instructions.
func (s *Source) SetupURL(rel Release) string { return s.DownloadURL(rel) + "/setup.md" }

package httpapi

import (
	"encoding/json"
	"net/http"
	"strings"

	"cctrack/server/internal/api"
	"cctrack/server/internal/cliversion"
)

const cliUserAgentPrefix = "cctrack/"

func (s *Server) GetCliVersion(w http.ResponseWriter, r *http.Request) {
	if s.cliVersion == nil {
		writeError(w, http.StatusServiceUnavailable, "CLI version check not configured")
		return
	}
	rel, ok := s.cliVersion.Latest(r.Context())
	if !ok {
		writeError(w, http.StatusServiceUnavailable, "latest CLI version unavailable")
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "public, max-age=300")
	w.WriteHeader(http.StatusOK)
	_ = json.NewEncoder(w).Encode(api.CliVersion{
		Latest:      rel.Version.String(),
		Tag:         rel.Tag,
		DownloadUrl: s.cliVersion.DownloadURL(rel),
		SetupUrl:    s.cliVersion.SetupURL(rel),
	})
}

// requireCurrentCLI refuses requests from cctrack CLIs older than the latest
// release with 426, which the CLI treats as retryable (it keeps its data).
// Only cctrack/ User-Agents pay for the latest-version lookup; an unknown
// latest version lets everything through.
func (s *Server) requireCurrentCLI(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		rest, isCLI := strings.CutPrefix(r.Header.Get("User-Agent"), cliUserAgentPrefix)
		if !isCLI || s.cliVersion == nil || cliVersionExempt(r) {
			next.ServeHTTP(w, r)
			return
		}
		rel, known := s.cliVersion.Latest(r.Context())
		if !known {
			next.ServeHTTP(w, r)
			return
		}
		client, _, _ := strings.Cut(rest, " ")
		if v, ok := cliversion.Parse(client); ok && v.Compare(rel.Version) >= 0 {
			next.ServeHTTP(w, r)
			return
		}
		if client == "" {
			client = "(no version)"
		}
		latest, setup := rel.Version.String(), s.cliVersion.SetupURL(rel)
		writeJSON(w, http.StatusUpgradeRequired, api.CliOutdated{
			Error: "cctrack " + client + " is out of date: the latest is " + latest +
				". Update cctrack (see " + setup + ", section \"Update cctrack\") and try again.",
			Latest:   latest,
			SetupUrl: setup,
		})
	})
}

// cliVersionExempt: an outdated CLI may still look up the latest version and
// log out.
func cliVersionExempt(r *http.Request) bool {
	switch r.URL.Path {
	case "/api/cli/version":
		return true
	case "/api/auth/token":
		return r.Method == http.MethodDelete
	}
	return false
}

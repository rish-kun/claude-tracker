// Package httpapi implements the cctrack HTTP API.
package httpapi

import (
	"net/http"
	"strings"
	"time"

	"cctrack/server/internal/api"
	"cctrack/server/internal/auth"
	"cctrack/server/internal/cliversion"
	"cctrack/server/internal/config"
	"cctrack/server/internal/store"
)

type Server struct {
	cfg      config.Config
	store    *store.Store
	idp      auth.IdentityProvider
	allow    auth.Allowlist
	sessions auth.SessionCodec
	admins   map[string]bool
	now      func() time.Time
	// cliVersion is the latest CLI release; nil disables the version check.
	cliVersion *cliversion.Source
}

var _ api.ServerInterface = (*Server)(nil)

func New(cfg config.Config, st *store.Store, idp auth.IdentityProvider) *Server {
	admins := map[string]bool{}
	for _, e := range cfg.AdminEmails {
		admins[strings.ToLower(e)] = true
	}
	s := &Server{
		cfg:      cfg,
		store:    st,
		idp:      idp,
		allow:    auth.NewAllowlist(cfg.AllowedDomains, cfg.AllowedEmails),
		sessions: auth.NewSessionCodec(cfg.SessionSecret),
		admins:   admins,
		now:      time.Now,
	}
	if cfg.CLIReleasesRepo != "" {
		fetch := cliversion.GitHubFetcher(&http.Client{}, cfg.CLIReleasesRepo, cfg.GitHubToken)
		s.cliVersion = cliversion.New(cfg.CLIReleasesRepo, fetch, func() time.Time { return s.now() })
	}
	return s
}

func (s *Server) isAdmin(email string) bool { return s.admins[strings.ToLower(email)] }

func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	api.HandlerWithOptions(s, api.StdHTTPServerOptions{
		BaseURL:    "/api",
		BaseRouter: mux,
		ErrorHandlerFunc: func(w http.ResponseWriter, r *http.Request, err error) {
			writeError(w, http.StatusBadRequest, err.Error())
		},
	})
	var h http.Handler = mux
	h = s.csrf(h)
	h = s.authenticate(h)
	h = s.requireCurrentCLI(h) // before authenticate: no DB lookup for outdated CLIs
	h = limitBody(h, 1<<20)
	return h
}

func (s *Server) GetHealth(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, api.Health{Status: "ok"})
}

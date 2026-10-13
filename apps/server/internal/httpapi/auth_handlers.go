package httpapi

import (
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"regexp"
	"strings"
	"time"

	"cctrack/server/internal/api"
	"cctrack/server/internal/auth"
	"cctrack/server/internal/store"
)

const (
	authRequestTTL = 10 * time.Minute
	cliCodeTTL     = 60 * time.Second
	sessionTTL     = 30 * 24 * time.Hour
)

var b64url = regexp.MustCompile(`^[A-Za-z0-9_-]+$`)

func (s *Server) redirectURL() string { return s.cfg.PublicURL + "/api/auth/google/callback" }

func (s *Server) CliStart(w http.ResponseWriter, r *http.Request, params api.CliStartParams) {
	mode := "loopback"
	if params.Mode != nil {
		mode = string(*params.Mode)
	}
	port := 0
	switch mode {
	case "loopback":
		if params.Port == nil || *params.Port < 1024 || *params.Port > 65535 {
			writeError(w, http.StatusBadRequest, "port must be 1024-65535 in loopback mode")
			return
		}
		port = *params.Port
	case "paste":
	default:
		writeError(w, http.StatusBadRequest, "mode must be loopback or paste")
		return
	}
	if len(params.State) < 16 || len(params.State) > 128 || !b64url.MatchString(params.State) {
		writeError(w, http.StatusBadRequest, "state must be 16-128 base64url characters")
		return
	}
	if len(params.CodeChallenge) < 43 || len(params.CodeChallenge) > 128 || !b64url.MatchString(params.CodeChallenge) {
		writeError(w, http.StatusBadRequest, "code_challenge must be 43-128 base64url characters")
		return
	}
	_ = s.store.PurgeExpired(r.Context())
	state := auth.RandomString(32)
	err := s.store.CreateAuthRequest(r.Context(), store.AuthRequest{
		State: state, Mode: mode, CLIState: params.State, CLIPort: port,
		CodeChallenge: params.CodeChallenge, ExpiresAt: s.now().Add(authRequestTTL),
	})
	if err != nil {
		s.internal(w, err)
		return
	}
	http.Redirect(w, r, s.idp.AuthCodeURL(state, s.redirectURL()), http.StatusFound)
}

func (s *Server) WebStart(w http.ResponseWriter, r *http.Request) {
	state := auth.RandomString(32)
	err := s.store.CreateAuthRequest(r.Context(), store.AuthRequest{
		State: state, Mode: "web", ExpiresAt: s.now().Add(authRequestTTL),
	})
	if err != nil {
		s.internal(w, err)
		return
	}
	http.SetCookie(w, &http.Cookie{
		Name: stateCookie, Value: state, Path: "/api/auth", MaxAge: int(authRequestTTL.Seconds()),
		HttpOnly: true, Secure: s.cfg.SecureCookies(), SameSite: http.SameSiteLaxMode,
	})
	http.Redirect(w, r, s.idp.AuthCodeURL(state, s.redirectURL()), http.StatusFound)
}

func (s *Server) GoogleCallback(w http.ResponseWriter, r *http.Request, params api.GoogleCallbackParams) {
	ctx := r.Context()
	if params.Error != nil {
		htmlPage(w, http.StatusBadRequest, "Sign-in cancelled", "Google returned: "+*params.Error, "")
		return
	}
	if params.State == nil || params.Code == nil {
		htmlPage(w, http.StatusBadRequest, "Invalid request", "Missing code or state.", "")
		return
	}
	req, err := s.store.TakeAuthRequest(ctx, *params.State)
	if errors.Is(err, store.ErrNotFound) {
		htmlPage(w, http.StatusBadRequest, "Link expired", "This sign-in link has expired or was already used. Start again.", "")
		return
	}
	if err != nil {
		s.internal(w, err)
		return
	}
	if req.Mode == "web" {
		c, err := r.Cookie(stateCookie)
		if err != nil || c.Value != req.State {
			htmlPage(w, http.StatusBadRequest, "Invalid request", "Sign-in must finish in the browser that started it.", "")
			return
		}
		http.SetCookie(w, &http.Cookie{Name: stateCookie, Value: "", Path: "/api/auth", MaxAge: -1})
	}

	id, err := s.idp.Exchange(ctx, *params.Code, s.redirectURL())
	if err != nil {
		htmlPage(w, http.StatusBadGateway, "Google sign-in failed", err.Error(), "")
		return
	}
	if !s.allow.Allows(id) {
		htmlPage(w, http.StatusForbidden, "Not allowed",
			fmt.Sprintf("%s is not allowed to use this cctrack server. Ask your admin to add your email or Google Workspace domain.", id.Email), "")
		return
	}
	user, err := s.store.UpsertUser(ctx, id.Subject, id.Email, id.Name)
	if err != nil {
		s.internal(w, err)
		return
	}

	if req.Mode == "web" {
		http.SetCookie(w, &http.Cookie{
			Name: sessionCookie, Value: s.sessions.Encode(user.ID, s.now().Add(sessionTTL)),
			Path: "/", MaxAge: int(sessionTTL.Seconds()),
			HttpOnly: true, Secure: s.cfg.SecureCookies(), SameSite: http.SameSiteLaxMode,
		})
		http.Redirect(w, r, s.cfg.PublicURL+"/", http.StatusFound)
		return
	}

	code := auth.RandomString(32)
	if err := s.store.CreateCLICode(ctx, auth.HashToken(code), user.ID, req.CodeChallenge, s.now().Add(cliCodeTTL)); err != nil {
		s.internal(w, err)
		return
	}
	if req.Mode == "paste" {
		htmlPage(w, http.StatusOK, "Paste this code into your terminal",
			"Signed in as "+user.Email+". The code expires in 60 seconds.", code)
		return
	}
	loopback := fmt.Sprintf("http://127.0.0.1:%d/callback?code=%s&state=%s",
		req.CLIPort, url.QueryEscape(code), url.QueryEscape(req.CLIState))
	http.Redirect(w, r, loopback, http.StatusFound)
}

func (s *Server) CliExchange(w http.ResponseWriter, r *http.Request) {
	var body api.CliExchangeRequest
	if !decodeJSON(w, r, &body) {
		return
	}
	device := strings.TrimSpace(body.DeviceName)
	if device == "" {
		device = "unknown"
	}
	if len(device) > 100 {
		device = device[:100]
	}
	userID, challenge, err := s.store.ConsumeCLICode(r.Context(), auth.HashToken(body.Code))
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusBadRequest, "invalid or expired code")
		return
	}
	if err != nil {
		s.internal(w, err)
		return
	}
	if !auth.VerifyPKCE(body.CodeVerifier, challenge) {
		writeError(w, http.StatusBadRequest, "code_verifier does not match")
		return
	}
	user, err := s.store.UserByID(r.Context(), userID)
	if err != nil {
		s.internal(w, err)
		return
	}
	plain, hash := auth.NewToken()
	if _, err := s.store.CreateToken(r.Context(), userID, hash, device); err != nil {
		s.internal(w, err)
		return
	}
	writeJSON(w, http.StatusOK, api.CliExchangeResponse{
		Token: plain,
		User:  api.User{Id: user.ID, Email: user.Email, Name: user.Name},
	})
}

func (s *Server) RevokeOwnToken(w http.ResponseWriter, r *http.Request) {
	p, ok := s.requireUser(w, r)
	if !ok {
		return
	}
	if p.TokenID == 0 {
		writeError(w, http.StatusUnauthorized, "bearer token required")
		return
	}
	if err := s.store.RevokeToken(r.Context(), p.TokenID); err != nil && !errors.Is(err, store.ErrNotFound) {
		s.internal(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) WebLogout(w http.ResponseWriter, r *http.Request) {
	http.SetCookie(w, &http.Cookie{
		Name: sessionCookie, Value: "", Path: "/", MaxAge: -1,
		HttpOnly: true, Secure: s.cfg.SecureCookies(), SameSite: http.SameSiteLaxMode,
	})
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) GetMe(w http.ResponseWriter, r *http.Request) {
	p, ok := s.requireUser(w, r)
	if !ok {
		return
	}
	writeJSON(w, http.StatusOK, api.Me{
		Id: p.User.ID, Email: p.User.Email, Name: p.User.Name, IsAdmin: s.isAdmin(p.User.Email),
	})
}

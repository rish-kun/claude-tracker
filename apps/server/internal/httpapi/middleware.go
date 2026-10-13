package httpapi

import (
	"context"
	"errors"
	"net/http"
	"strings"

	"cctrack/server/internal/auth"
	"cctrack/server/internal/store"
)

const (
	sessionCookie = "cctrack_session"
	stateCookie   = "cctrack_oauth_state"
	csrfHeader    = "X-CCTrack"
)

type principal struct {
	User    store.User
	TokenID int64 // non-zero when authenticated by bearer token
}

type ctxKey struct{}

func principalFrom(r *http.Request) (principal, bool) {
	p, ok := r.Context().Value(ctxKey{}).(principal)
	return p, ok
}

func limitBody(next http.Handler, n int64) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		r.Body = http.MaxBytesReader(w, r.Body, n)
		next.ServeHTTP(w, r)
	})
}

// authenticate resolves a bearer token or session cookie into a principal.
// An invalid bearer token is rejected immediately with 401; an invalid cookie
// is ignored (treated as anonymous).
func (s *Server) authenticate(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if h := r.Header.Get("Authorization"); h != "" {
			tok, ok := strings.CutPrefix(h, "Bearer ")
			if !ok || !strings.HasPrefix(tok, auth.TokenPrefix) {
				writeError(w, http.StatusUnauthorized, "invalid token")
				return
			}
			u, tokenID, err := s.store.UserByToken(r.Context(), auth.HashToken(tok))
			if errors.Is(err, store.ErrNotFound) {
				writeError(w, http.StatusUnauthorized, "invalid or revoked token")
				return
			}
			if err != nil {
				s.internal(w, err)
				return
			}
			r = r.WithContext(context.WithValue(r.Context(), ctxKey{}, principal{User: u, TokenID: tokenID}))
		} else if c, err := r.Cookie(sessionCookie); err == nil {
			// An invalid/expired cookie or a deleted user means anonymous;
			// any other store error is a real failure, not a logged-out user.
			if uid, err := s.sessions.Decode(c.Value, s.now()); err == nil {
				u, err := s.store.UserByID(r.Context(), uid)
				switch {
				case err == nil:
					r = r.WithContext(context.WithValue(r.Context(), ctxKey{}, principal{User: u}))
				case errors.Is(err, store.ErrNotFound):
				default:
					s.internal(w, err)
					return
				}
			}
		}
		next.ServeHTTP(w, r)
	})
}

// csrf requires a custom header on cookie-authenticated state-changing
// requests; browsers cannot send it cross-site without a CORS preflight,
// which this server never grants.
func (s *Server) csrf(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodGet, http.MethodHead, http.MethodOptions:
		default:
			if _, err := r.Cookie(sessionCookie); err == nil && r.Header.Get("Authorization") == "" &&
				r.Header.Get(csrfHeader) != "1" {
				writeError(w, http.StatusForbidden, "missing "+csrfHeader+" header")
				return
			}
		}
		next.ServeHTTP(w, r)
	})
}

func (s *Server) requireUser(w http.ResponseWriter, r *http.Request) (principal, bool) {
	p, ok := principalFrom(r)
	if !ok {
		writeError(w, http.StatusUnauthorized, "not logged in")
	}
	return p, ok
}

func (s *Server) requireAdmin(w http.ResponseWriter, r *http.Request) (principal, bool) {
	p, ok := s.requireUser(w, r)
	if !ok {
		return p, false
	}
	// Admin endpoints are cookie-only (openapi.yaml): a leaked CLI token must
	// not grant admin access.
	if p.TokenID != 0 {
		writeError(w, http.StatusForbidden, "admin endpoints require a browser session")
		return p, false
	}
	if !s.isAdmin(p.User.Email) {
		writeError(w, http.StatusForbidden, "admin only")
		return p, false
	}
	return p, true
}

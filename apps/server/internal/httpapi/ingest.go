package httpapi

import (
	"errors"
	"net/http"
	"strings"
	"unicode/utf8"

	"cctrack/server/internal/api"
	"cctrack/server/internal/store"
)

const maxSummaryRunes = 500

func (s *Server) Ingest(w http.ResponseWriter, r *http.Request) {
	p, ok := s.requireUser(w, r)
	if !ok {
		return
	}
	var req api.IngestRequest
	raw, ok := readJSON(w, r, &req)
	if !ok {
		return
	}
	if err := checkIngestKeys(raw); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if err := validateIngest(&req, s.now()); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	res, err := s.store.Ingest(r.Context(), p.User.ID, &req)
	if errors.Is(err, store.ErrForbidden) {
		writeError(w, http.StatusForbidden, "a session in this batch belongs to another user")
		return
	}
	if err != nil {
		s.internal(w, err)
		return
	}
	writeJSON(w, http.StatusOK, api.IngestResponse{Accepted: res.Accepted, Duplicates: res.Duplicates})
}

func (s *Server) SetSessionSummary(w http.ResponseWriter, r *http.Request, sessionId string) {
	p, ok := s.requireUser(w, r)
	if !ok {
		return
	}
	var body api.SummaryRequest
	if !decodeJSON(w, r, &body) {
		return
	}
	summary := strings.TrimSpace(body.Summary)
	if summary == "" || utf8.RuneCountInString(summary) > maxSummaryRunes {
		writeError(w, http.StatusBadRequest, "summary must be 1-500 characters")
		return
	}
	if hasNUL(summary, sessionId) {
		writeError(w, http.StatusBadRequest, "summary and session id must not contain NUL")
		return
	}
	err := s.store.SetSummary(r.Context(), p.User.ID, sessionId, summary)
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, "session not found")
		return
	}
	if err != nil {
		s.internal(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

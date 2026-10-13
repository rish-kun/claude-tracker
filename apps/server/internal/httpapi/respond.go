package httpapi

import (
	"encoding/json"
	"errors"
	"log"
	"net/http"

	"github.com/jackc/pgx/v5/pgconn"
)

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func writeError(w http.ResponseWriter, status int, msg string) {
	writeJSON(w, status, map[string]string{"error": msg})
}

// decodeJSON decodes the request body into dst, writing a 400/413 on failure.
func decodeJSON(w http.ResponseWriter, r *http.Request, dst any) bool {
	if err := json.NewDecoder(r.Body).Decode(dst); err != nil {
		var tooBig *http.MaxBytesError
		if errors.As(err, &tooBig) {
			writeError(w, http.StatusRequestEntityTooLarge, "request body too large")
			return false
		}
		writeError(w, http.StatusBadRequest, "invalid JSON: "+err.Error())
		return false
	}
	return true
}

func (s *Server) internal(w http.ResponseWriter, err error) {
	log.Printf("internal error: %v", err)
	writeError(w, http.StatusInternalServerError, "internal error")
}

// isInvalidParam reports whether err is a Postgres invalid_parameter_value
// (SQLSTATE 22023), which `AT TIME ZONE` raises for a zone Postgres does not
// know even though Go's tzdata accepted it.
func isInvalidParam(err error) bool {
	var pgErr *pgconn.PgError
	return errors.As(err, &pgErr) && pgErr.Code == "22023"
}

// aggregateFailed reports a failed tz-bucketed query: 400 when Postgres
// rejected the time zone, 500 otherwise.
func (s *Server) aggregateFailed(w http.ResponseWriter, err error, tz string) {
	if isInvalidParam(err) {
		writeError(w, http.StatusBadRequest, "unknown tz: "+tz)
		return
	}
	s.internal(w, err)
}

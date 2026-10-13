package httpapi

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
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
	_, ok := readJSON(w, r, dst)
	return ok
}

// readJSON is decodeJSON that also returns the raw body, for handlers that
// must tell a missing (or null) key from a zero value.
func readJSON(w http.ResponseWriter, r *http.Request, dst any) ([]byte, bool) {
	raw, err := io.ReadAll(r.Body)
	if err != nil {
		var tooBig *http.MaxBytesError
		if errors.As(err, &tooBig) {
			writeError(w, http.StatusRequestEntityTooLarge, "request body too large")
			return nil, false
		}
		writeError(w, http.StatusBadRequest, "could not read request body")
		return nil, false
	}
	if err := json.Unmarshal(raw, dst); err != nil {
		writeError(w, http.StatusBadRequest, "invalid JSON: "+err.Error())
		return nil, false
	}
	return raw, true
}

// missingKey returns the first of keys that obj lacks or holds as null, or "".
func missingKey(obj map[string]json.RawMessage, keys ...string) string {
	for _, k := range keys {
		v, ok := obj[k]
		if !ok || bytes.Equal(bytes.TrimSpace(v), []byte("null")) {
			return k
		}
	}
	return ""
}

var priceBaseKeys = []string{"input", "cache_write_5m", "cache_write_1h", "cache_read", "output"}

// checkPriceKeys requires every base rate to be present and non-null in a
// PriceRates body; otherwise a missing rate would silently become 0.
func checkPriceKeys(raw []byte) error {
	var obj map[string]json.RawMessage
	if err := json.Unmarshal(raw, &obj); err != nil {
		return fmt.Errorf("body must be a JSON object")
	}
	if k := missingKey(obj, priceBaseKeys...); k != "" {
		return fmt.Errorf("%s is required", k)
	}
	return nil
}

var entryRequiredKeys = []string{"session_id", "account_uuid", "message_id", "model", "ts",
	"input", "output", "cache_write_5m", "cache_write_1h", "cache_read"}

// checkIngestKeys requires every entry to carry its required keys (non-null),
// so an omitted token count is rejected instead of being stored as 0.
func checkIngestKeys(raw []byte) error {
	var shape struct {
		Entries []map[string]json.RawMessage `json:"entries"`
	}
	if err := json.Unmarshal(raw, &shape); err != nil {
		return fmt.Errorf("body must be a JSON object")
	}
	for i, e := range shape.Entries {
		if k := missingKey(e, entryRequiredKeys...); k != "" {
			return fmt.Errorf("entries[%d].%s is required", i, k)
		}
	}
	return nil
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

package httpapi

import (
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
	"errors"
	"net/http"

	"cctrack/server/internal/api"
	"cctrack/server/internal/auth"
	"cctrack/server/internal/store"
)

const proxyTokenHeader = "X-CCTrack-Proxy-Token"

func proxyKeyToAPI(k store.ProxyKey) api.ProxyKey {
	return api.ProxyKey{Id: k.ID, User: userRef(k.User), DeviceName: k.DeviceName, Status: api.ProxyKeyStatus(k.Status()),
		CreatedAt: k.CreatedAt, ApprovedAt: k.ApprovedAt, RevokedAt: k.RevokedAt, Linked: k.Linked}
}

// RequestProxyKey creates a pending key for the CLI user's device (`cctrack
// proxy connect`), replacing that device's previous key. Only a CLI bearer
// token may request one, so the key is tied to a Google-verified email.
func (s *Server) RequestProxyKey(w http.ResponseWriter, r *http.Request) {
	p, ok := s.requireCLI(w, r)
	if !ok {
		return
	}
	var body api.ProxyKeyRequest
	if !decodeJSON(w, r, &body) {
		return
	}
	device := ""
	if body.DeviceName != nil {
		device = *body.DeviceName
	}
	plain, hash := auth.NewProxyKey()
	k, err := s.store.RequestProxyKey(r.Context(), p.User.ID, hash, truncateDeviceName(device))
	if err != nil {
		s.internal(w, err) // the user exists: requireCLI just resolved their token
		return
	}
	w.Header().Set("Cache-Control", "no-store")
	writeJSON(w, http.StatusCreated, api.ProxyKeyRequested{
		Key: plain, Status: api.ProxyKeyRequestedStatus(k.Status()), ProxyKey: proxyKeyToAPI(k)})
}

func (s *Server) AdminListProxyKeys(w http.ResponseWriter, r *http.Request) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	keys, err := s.store.ListProxyKeys(r.Context())
	if err != nil {
		s.internal(w, err)
		return
	}
	out := make([]api.ProxyKey, 0, len(keys))
	for _, k := range keys {
		out = append(out, proxyKeyToAPI(k))
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) AdminApproveProxyKey(w http.ResponseWriter, r *http.Request, id int64) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	err := s.store.ApproveProxyKey(r.Context(), id)
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, "proxy key not found, revoked or already approved")
		return
	}
	if err != nil {
		s.internal(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) AdminRevokeProxyKey(w http.ResponseWriter, r *http.Request, id int64) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	err := s.store.RevokeProxyKey(r.Context(), id)
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, "proxy key not found or already revoked")
		return
	}
	if err != nil {
		s.internal(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// proxySyncPath is served outside bearer/cookie authentication (see Handler).
const proxySyncPath = "/api/proxy/keys"

// proxyTokenOK compares the sync header with PROXY_SYNC_TOKEN in constant
// time; hashing first keeps the comparison independent of the lengths.
func (s *Server) proxyTokenOK(r *http.Request) bool {
	got := sha256.Sum256([]byte(r.Header.Get(proxyTokenHeader)))
	want := sha256.Sum256([]byte(s.cfg.ProxySyncToken))
	return subtle.ConstantTimeCompare(got[:], want[:]) == 1
}

// ProxySyncKeys lists the keys the admin's CLIProxyAPI should know, approved
// (allowed) or pending (403). It is
// authenticated only by X-CCTrack-Proxy-Token, so it does not depend on the
// bearer/cookie principal that authenticate resolves.
func (s *Server) ProxySyncKeys(w http.ResponseWriter, r *http.Request) {
	if s.cfg.ProxySyncToken == "" {
		writeError(w, http.StatusNotFound, "not found")
		return
	}
	if r.Header.Get(proxyTokenHeader) == "" || !s.proxyTokenOK(r) {
		writeError(w, http.StatusUnauthorized, "invalid proxy token")
		return
	}
	keys, err := s.store.SyncProxyKeys(r.Context())
	if err != nil {
		s.internal(w, err)
		return
	}
	out := api.ProxySyncKeys{Keys: make([]api.ProxySyncKey, 0, len(keys))}
	for _, k := range keys {
		status := api.ProxySyncKeyStatusPending
		if k.Approved {
			status = api.ProxySyncKeyStatusApproved
		}
		out.Keys = append(out.Keys, api.ProxySyncKey{Hash: hex.EncodeToString(k.Hash), Email: k.Email,
			Label: k.DeviceName, Status: status})
	}
	w.Header().Set("Cache-Control", "no-store")
	writeJSON(w, http.StatusOK, out)
}

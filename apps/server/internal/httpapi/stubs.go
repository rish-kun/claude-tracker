package httpapi

import (
	"net/http"

	"cctrack/server/internal/api"
)

func notImplemented(w http.ResponseWriter) {
	writeError(w, http.StatusNotImplemented, "not implemented")
}

// Task 7
func (s *Server) GetMyUsage(w http.ResponseWriter, r *http.Request, params api.GetMyUsageParams) {
	notImplemented(w)
}
func (s *Server) ListMySessions(w http.ResponseWriter, r *http.Request, params api.ListMySessionsParams) {
	notImplemented(w)
}

// Task 8
func (s *Server) AdminByLogin(w http.ResponseWriter, r *http.Request, params api.AdminByLoginParams) {
	notImplemented(w)
}
func (s *Server) AdminByAccount(w http.ResponseWriter, r *http.Request, params api.AdminByAccountParams) {
	notImplemented(w)
}
func (s *Server) AdminListAccounts(w http.ResponseWriter, r *http.Request) { notImplemented(w) }
func (s *Server) AdminUpdateAccount(w http.ResponseWriter, r *http.Request, id int64) {
	notImplemented(w)
}
func (s *Server) AdminListUsers(w http.ResponseWriter, r *http.Request) { notImplemented(w) }
func (s *Server) AdminListUserSessions(w http.ResponseWriter, r *http.Request, id int64, params api.AdminListUserSessionsParams) {
	notImplemented(w)
}
func (s *Server) AdminListTokens(w http.ResponseWriter, r *http.Request) { notImplemented(w) }
func (s *Server) AdminRevokeToken(w http.ResponseWriter, r *http.Request, id int64) {
	notImplemented(w)
}
func (s *Server) AdminListPrices(w http.ResponseWriter, r *http.Request) { notImplemented(w) }
func (s *Server) AdminPutPrice(w http.ResponseWriter, r *http.Request, model string) {
	notImplemented(w)
}

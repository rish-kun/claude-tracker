package httpapi

import (
	"errors"
	"net/http"
	"unicode/utf8"

	"cctrack/server/internal/api"
	"cctrack/server/internal/pricing"
	"cctrack/server/internal/report"
	"cctrack/server/internal/store"
)

// canViewSession reports whether p may read a session owned by ownerID: its
// owner always, and admins only through a browser session, matching the
// cookie-only admin endpoints (a leaked CLI token must not widen access).
func (s *Server) canViewSession(p principal, ownerID int64) bool {
	return p.User.ID == ownerID || (p.TokenID == 0 && s.isAdmin(p.User.Email))
}

func (s *Server) GetSession(w http.ResponseWriter, r *http.Request, sessionId string) {
	p, ok := s.requireUser(w, r)
	if !ok {
		return
	}
	// Every refusal is the same 404, so a session id is never confirmed to
	// exist for someone who may not see it. Ids ingest could never store (NUL
	// or invalid UTF-8, which Postgres text rejects, or over 100 bytes) are
	// 404 too, not a 500.
	notFound := func() { writeError(w, http.StatusNotFound, "session not found") }
	if hasNUL(sessionId) || !utf8.ValidString(sessionId) || len(sessionId) > 100 {
		notFound()
		return
	}
	ctx := r.Context()
	row, err := s.store.SessionByID(ctx, sessionId)
	if errors.Is(err, store.ErrNotFound) {
		notFound()
		return
	}
	if err != nil {
		s.internal(w, err)
		return
	}
	if !s.canViewSession(p, row.UserID) {
		notFound()
		return
	}

	table, err := s.store.PriceTable(ctx)
	if err != nil {
		s.internal(w, err)
		return
	}
	agg, err := s.store.SessionAggregate(ctx, sessionId)
	if err != nil {
		s.internal(w, err)
		return
	}
	acctIDs := []int64{}
	if row.AccountID != nil {
		acctIDs = append(acctIDs, *row.AccountID)
	}
	accounts, err := s.store.AccountsByIDs(ctx, acctIDs)
	if err != nil {
		s.internal(w, err)
		return
	}
	writeJSON(w, http.StatusOK, sessionDetail(row, accounts, table, agg))
}

// sessionDetail prices a session's aggregated rows exactly as the usage
// endpoints do and splits them into main thread vs subagents and by model.
func sessionDetail(row store.SessionRow, accounts map[int64]store.Account, table *pricing.Table,
	agg []store.SessionAggRow) api.SessionDetail {
	var total, main, sub report.Bucket
	var calls, mainCalls, subCalls, peak int64
	byModel := report.NewGroup[string]()
	modelCalls := map[string]int64{}
	for _, a := range agg {
		model := pricing.Normalize(a.Model)
		total.Add(table, a.AggRow)
		byModel.Add(model, table, a.AggRow)
		modelCalls[model] += a.Calls
		calls += a.Calls
		peak = max(peak, a.PeakPrompt)
		if a.IsSubagent {
			sub.Add(table, a.AggRow)
			subCalls += a.Calls
		} else {
			main.Add(table, a.AggRow)
			mainCalls += a.Calls
		}
	}
	out := api.SessionDetail{
		Session:           sessionToAPI(row, accounts, total.Totals()),
		Calls:             calls,
		PeakContextTokens: peak,
		Main:              api.SessionPart{Totals: main.Totals(), Calls: mainCalls},
		Subagents:         api.SessionPart{Totals: sub.Totals(), Calls: subCalls},
		ByModel:           []api.SessionModelUsage{},
	}
	models := byModel.SortedKeys(func(a, b string) bool {
		if ca, cb := byModel.Get(a).Cost, byModel.Get(b).Cost; ca != cb {
			return ca > cb
		}
		return a < b
	})
	for _, m := range models {
		out.ByModel = append(out.ByModel, api.SessionModelUsage{Model: m, Totals: byModel.Get(m).Totals(), Calls: modelCalls[m]})
	}
	return out
}

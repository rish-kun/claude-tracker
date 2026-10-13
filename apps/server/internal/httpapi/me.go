package httpapi

import (
	"cmp"
	"net/http"
	"strconv"

	"cctrack/server/internal/api"
	"cctrack/server/internal/pricing"
	"cctrack/server/internal/report"
	"cctrack/server/internal/store"
)

type dayKey struct {
	Day, Key string
}

func series(g *report.Group[dayKey], label func(key string) string) []api.SeriesPoint {
	keys := g.SortedKeys(func(a, b dayKey) bool {
		if a.Day != b.Day {
			return a.Day < b.Day
		}
		return a.Key < b.Key
	})
	out := make([]api.SeriesPoint, 0, len(keys))
	for _, k := range keys {
		b := g.Get(k)
		out = append(out, api.SeriesPoint{Date: k.Day, Key: k.Key, Label: label(k.Key),
			TotalTokens: b.Counts.Total(), CostUsd: b.Cost, UnpricedTokens: b.Unpriced})
	}
	return out
}

// byCostDesc orders keys by cost, then total tokens (both descending), then
// key ascending so ties are deterministic whatever order SQL returned rows in.
func byCostDesc[K cmp.Ordered](g *report.Group[K]) []K {
	return byCostDescThen(g, cmp.Less[K])
}

// byCostDescThen is byCostDesc for keys that are not cmp.Ordered: keyLess
// breaks ties after cost and total tokens.
func byCostDescThen[K comparable](g *report.Group[K], keyLess func(a, b K) bool) []K {
	return g.SortedKeys(func(a, b K) bool {
		ga, gb := g.Get(a), g.Get(b)
		if ga.Cost != gb.Cost {
			return ga.Cost > gb.Cost
		}
		if ta, tb := ga.Counts.Total(), gb.Counts.Total(); ta != tb {
			return ta > tb
		}
		return keyLess(a, b)
	})
}

func (s *Server) GetMyUsage(w http.ResponseWriter, r *http.Request, params api.GetMyUsageParams) {
	p, ok := s.requireUser(w, r)
	if !ok {
		return
	}
	f, err := parseRange(params.From, params.To, params.Tz)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	f.UserID = &p.User.ID
	ctx := r.Context()
	table, err := s.store.PriceTable(ctx)
	if err != nil {
		s.internal(w, err)
		return
	}
	rows, err := s.store.Aggregate(ctx, store.GroupBy{Day: true, Account: true}, f)
	if err != nil {
		s.aggregateFailed(w, err, f.TZ)
		return
	}
	ids := accountIDs(rows)
	accounts, err := s.store.AccountsByIDs(ctx, ids)
	if err != nil {
		s.internal(w, err)
		return
	}
	limits, err := s.store.LatestLimits(ctx, ids)
	if err != nil {
		s.internal(w, err)
		return
	}

	var total report.Bucket
	byModel := report.NewGroup[string]()
	byAccount := report.NewGroup[int64]()
	daily := report.NewGroup[dayKey]()
	for _, row := range rows {
		model := pricing.Normalize(row.Model)
		total.Add(table, row)
		byModel.Add(model, table, row)
		byAccount.Add(row.AccountID, table, row)
		daily.Add(dayKey{row.Day, model}, table, row)
	}

	out := api.Usage{
		Totals:       total.Totals(),
		ByModel:      []api.BreakdownRow{},
		ByAccount:    []api.BreakdownRow{},
		DailyByModel: series(daily, func(k string) string { return k }),
		Limits:       limitsToAPI(limits),
	}
	for _, m := range byCostDesc(byModel) {
		out.ByModel = append(out.ByModel, api.BreakdownRow{Key: m, Label: m, Totals: byModel.Get(m).Totals()})
	}
	for _, id := range byCostDesc(byAccount) {
		out.ByAccount = append(out.ByAccount, api.BreakdownRow{
			Key: strconv.FormatInt(id, 10), Label: accounts[id].DisplayLabel(), Totals: byAccount.Get(id).Totals()})
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) ListMySessions(w http.ResponseWriter, r *http.Request, params api.ListMySessionsParams) {
	p, ok := s.requireUser(w, r)
	if !ok {
		return
	}
	limit, offset := pageParams(params.Limit, params.Offset)
	rows, total, err := s.store.ListSessions(r.Context(), p.User.ID, limit, offset)
	if err != nil {
		s.internal(w, err)
		return
	}
	page, err := s.sessionPage(r.Context(), rows, total)
	if err != nil {
		s.internal(w, err)
		return
	}
	writeJSON(w, http.StatusOK, page)
}

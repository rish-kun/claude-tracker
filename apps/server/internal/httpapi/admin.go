package httpapi

import (
	"errors"
	"net/http"
	"strconv"
	"strings"
	"unicode/utf8"

	"cctrack/server/internal/api"
	"cctrack/server/internal/pricing"
	"cctrack/server/internal/report"
	"cctrack/server/internal/store"
)

const maxLabelRunes = 100

type userAcct struct{ User, Account int64 }

// byUserAcctCostDesc orders (user, account) keys by cost desc, tokens desc,
// then (User, Account) ascending.
func byUserAcctCostDesc(g *report.Group[userAcct]) []userAcct {
	return byCostDescThen(g, func(a, b userAcct) bool {
		if a.User != b.User {
			return a.User < b.User
		}
		return a.Account < b.Account
	})
}

func userIDs(rows []store.AggRow) []int64 {
	seen := map[int64]bool{}
	ids := []int64{}
	for _, r := range rows {
		if !seen[r.UserID] {
			seen[r.UserID] = true
			ids = append(ids, r.UserID)
		}
	}
	return ids
}

func emailLabel(users map[int64]store.UserRef) func(string) string {
	return func(k string) string {
		id, _ := strconv.ParseInt(k, 10, 64)
		return users[id].Email
	}
}

func (s *Server) AdminByLogin(w http.ResponseWriter, r *http.Request, params api.AdminByLoginParams) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	f, err := parseRange(params.From, params.To, params.Tz)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	ctx := r.Context()
	table, err := s.store.PriceTable(ctx)
	if err != nil {
		s.internal(w, err)
		return
	}
	rows, err := s.store.Aggregate(ctx, store.GroupBy{Day: true, User: true, Account: true}, f)
	if err != nil {
		s.aggregateFailed(w, err, f.TZ)
		return
	}
	users, err := s.store.UsersByIDs(ctx, userIDs(rows))
	if err != nil {
		s.internal(w, err)
		return
	}
	accounts, err := s.store.AccountsByIDs(ctx, accountIDs(rows))
	if err != nil {
		s.internal(w, err)
		return
	}

	byUser := report.NewGroup[int64]()
	byUserAcct := report.NewGroup[userAcct]()
	daily := report.NewGroup[dayKey]()
	for _, row := range rows {
		byUser.Add(row.UserID, table, row)
		byUserAcct.Add(userAcct{row.UserID, row.AccountID}, table, row)
		daily.Add(dayKey{row.Day, strconv.FormatInt(row.UserID, 10)}, table, row)
	}
	// Sorted once; each user's slice keeps the cost-desc order.
	acctsOf := map[int64][]userAcct{}
	for _, k := range byUserAcctCostDesc(byUserAcct) {
		acctsOf[k.User] = append(acctsOf[k.User], k)
	}

	out := api.ByLogin{Users: []api.LoginUsage{}, DailyByUser: series(daily, emailLabel(users))}
	for _, uid := range byCostDesc(byUser) {
		lu := api.LoginUsage{User: userRef(users[uid]), Totals: byUser.Get(uid).Totals(), ByAccount: []api.AccountUsage{}}
		for _, k := range acctsOf[uid] {
			lu.ByAccount = append(lu.ByAccount, api.AccountUsage{
				Account: accountRef(accounts[k.Account]), Totals: byUserAcct.Get(k).Totals()})
		}
		out.Users = append(out.Users, lu)
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) AdminByAccount(w http.ResponseWriter, r *http.Request, params api.AdminByAccountParams) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	f, err := parseRange(params.From, params.To, params.Tz)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	ctx := r.Context()
	table, err := s.store.PriceTable(ctx)
	if err != nil {
		s.internal(w, err)
		return
	}
	rows, err := s.store.Aggregate(ctx, store.GroupBy{Day: true, User: true, Account: true}, f)
	if err != nil {
		s.aggregateFailed(w, err, f.TZ)
		return
	}
	accounts, err := s.store.ListAccounts(ctx)
	if err != nil {
		s.internal(w, err)
		return
	}
	users, err := s.store.UsersByIDs(ctx, userIDs(rows))
	if err != nil {
		s.internal(w, err)
		return
	}

	byAcct := report.NewGroup[int64]()
	byAcctUser := report.NewGroup[userAcct]()
	daily := map[int64]*report.Group[dayKey]{}
	for _, row := range rows {
		byAcct.Add(row.AccountID, table, row)
		byAcctUser.Add(userAcct{row.UserID, row.AccountID}, table, row)
		if daily[row.AccountID] == nil {
			daily[row.AccountID] = report.NewGroup[dayKey]()
		}
		daily[row.AccountID].Add(dayKey{row.Day, strconv.FormatInt(row.UserID, 10)}, table, row)
	}
	usersOf := map[int64][]userAcct{}
	for _, k := range byUserAcctCostDesc(byAcctUser) {
		usersOf[k.Account] = append(usersOf[k.Account], k)
	}
	label := emailLabel(users)

	out := api.ByAccount{Accounts: []api.AccountDetail{}}
	for _, a := range accounts {
		total := byAcct.Get(a.ID)
		detail := api.AccountDetail{
			Account: accountToAPI(a), Totals: total.Totals(),
			ByUser: []api.UserShare{}, DailyByUser: []api.SeriesPoint{},
		}
		for _, k := range usersOf[a.ID] {
			b := byAcctUser.Get(k)
			share := 0.0
			if total.Cost > 0 {
				share = b.Cost / total.Cost
			} else if total.Counts.Total() > 0 {
				share = float64(b.Counts.Total()) / float64(total.Counts.Total())
			}
			detail.ByUser = append(detail.ByUser, api.UserShare{User: userRef(users[k.User]), Totals: b.Totals(), Share: share})
		}
		if g := daily[a.ID]; g != nil {
			detail.DailyByUser = series(g, label)
		}
		limits, err := s.store.LimitHistory(ctx, a.ID, f.From, f.To)
		if err != nil {
			s.internal(w, err)
			return
		}
		detail.Limits = limitsToAPI(limits)
		out.Accounts = append(out.Accounts, detail)
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) AdminListAccounts(w http.ResponseWriter, r *http.Request) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	accounts, err := s.store.ListAccounts(r.Context())
	if err != nil {
		s.internal(w, err)
		return
	}
	out := make([]api.Account, 0, len(accounts))
	for _, a := range accounts {
		out = append(out, accountToAPI(a))
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) AdminUpdateAccount(w http.ResponseWriter, r *http.Request, id int64) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	var body api.AccountPatch
	if !decodeJSON(w, r, &body) {
		return
	}
	var label, plan *string
	if body.Label != nil {
		l := strings.TrimSpace(*body.Label)
		if utf8.RuneCountInString(l) > maxLabelRunes {
			writeError(w, http.StatusBadRequest, "label must be at most 100 characters")
			return
		}
		label = &l
	}
	if body.Plan != nil {
		if !body.Plan.Valid() {
			writeError(w, http.StatusBadRequest, "invalid plan")
			return
		}
		p := string(*body.Plan)
		plan = &p
	}
	a, err := s.store.UpdateAccount(r.Context(), id, label, plan)
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, "account not found")
		return
	}
	if err != nil {
		s.internal(w, err)
		return
	}
	writeJSON(w, http.StatusOK, accountToAPI(a))
}

func (s *Server) AdminListUsers(w http.ResponseWriter, r *http.Request) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	users, err := s.store.ListUsers(r.Context())
	if err != nil {
		s.internal(w, err)
		return
	}
	out := make([]api.AdminUser, 0, len(users))
	for _, u := range users {
		out = append(out, api.AdminUser{Id: u.ID, Email: u.Email, Name: u.Name, IsAdmin: s.isAdmin(u.Email),
			CreatedAt: u.CreatedAt, LastSeenAt: u.LastSeenAt})
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) AdminListUserSessions(w http.ResponseWriter, r *http.Request, id int64, params api.AdminListUserSessionsParams) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	limit, offset := pageParams(params.Limit, params.Offset)
	rows, total, err := s.store.ListSessions(r.Context(), id, limit, offset)
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

func (s *Server) AdminListTokens(w http.ResponseWriter, r *http.Request) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	tokens, err := s.store.ListTokens(r.Context())
	if err != nil {
		s.internal(w, err)
		return
	}
	out := make([]api.ApiToken, 0, len(tokens))
	for _, t := range tokens {
		out = append(out, api.ApiToken{Id: t.ID, User: userRef(t.User), DeviceName: t.DeviceName,
			CreatedAt: t.CreatedAt, LastUsedAt: t.LastUsedAt, RevokedAt: t.RevokedAt})
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) AdminRevokeToken(w http.ResponseWriter, r *http.Request, id int64) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	err := s.store.RevokeToken(r.Context(), id)
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, "token not found or already revoked")
		return
	}
	if err != nil {
		s.internal(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func priceToAPI(p pricing.Price) api.ModelPrice {
	rates := api.PriceRates{
		Input: p.Base.Input, CacheWrite5m: p.Base.CacheWrite5m, CacheWrite1h: p.Base.CacheWrite1h,
		CacheRead: p.Base.CacheRead, Output: p.Base.Output, FastInput: p.FastInput, FastOutput: p.FastOutput,
	}
	if p.Long != nil {
		l := *p.Long
		rates.LongInput, rates.LongCacheWrite5m, rates.LongCacheWrite1h = &l.Input, &l.CacheWrite5m, &l.CacheWrite1h
		rates.LongCacheRead, rates.LongOutput = &l.CacheRead, &l.Output
	}
	return api.ModelPrice{Model: p.Model, Rates: rates, UpdatedAt: p.UpdatedAt}
}

func (s *Server) AdminListPrices(w http.ResponseWriter, r *http.Request) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	prices, err := s.store.Prices(r.Context())
	if err != nil {
		s.internal(w, err)
		return
	}
	out := make([]api.ModelPrice, 0, len(prices))
	for _, p := range prices {
		out = append(out, priceToAPI(p))
	}
	writeJSON(w, http.StatusOK, out)
}

// priceFromBody validates rates for model and builds a pricing.Price. It
// returns a client-facing error message when the body is invalid.
func priceFromBody(model string, body api.PriceRates) (pricing.Price, string) {
	if model == "" {
		return pricing.Price{}, "model is required"
	}
	rates := []float64{body.Input, body.CacheWrite5m, body.CacheWrite1h, body.CacheRead, body.Output}
	optional := []*float64{body.FastInput, body.FastOutput,
		body.LongInput, body.LongCacheWrite5m, body.LongCacheWrite1h, body.LongCacheRead, body.LongOutput}
	for _, v := range optional {
		if v != nil {
			rates = append(rates, *v)
		}
	}
	for _, v := range rates {
		if v < 0 {
			return pricing.Price{}, "rates must be >= 0"
		}
	}
	if (body.FastInput == nil) != (body.FastOutput == nil) {
		return pricing.Price{}, "set both fast_input and fast_output, or neither"
	}
	p := pricing.Price{
		Model: model,
		Base: pricing.Rates{Input: body.Input, CacheWrite5m: body.CacheWrite5m, CacheWrite1h: body.CacheWrite1h,
			CacheRead: body.CacheRead, Output: body.Output},
		FastInput: body.FastInput, FastOutput: body.FastOutput,
	}
	longs := optional[2:]
	set := 0
	for _, v := range longs {
		if v != nil {
			set++
		}
	}
	switch set {
	case 0:
	case len(longs):
		p.Long = &pricing.Rates{Input: *body.LongInput, CacheWrite5m: *body.LongCacheWrite5m,
			CacheWrite1h: *body.LongCacheWrite1h, CacheRead: *body.LongCacheRead, Output: *body.LongOutput}
	default:
		return pricing.Price{}, "set all long_* rates, or none"
	}
	return p, ""
}

func (s *Server) AdminPutPrice(w http.ResponseWriter, r *http.Request, model string) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	var body api.PriceRates
	if !decodeJSON(w, r, &body) {
		return
	}
	p, msg := priceFromBody(pricing.Normalize(model), body)
	if msg != "" {
		writeError(w, http.StatusBadRequest, msg)
		return
	}
	saved, err := s.store.UpsertPrice(r.Context(), p)
	if err != nil {
		s.internal(w, err)
		return
	}
	writeJSON(w, http.StatusOK, priceToAPI(saved))
}

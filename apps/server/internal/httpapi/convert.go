package httpapi

import (
	"context"
	"time"

	"cctrack/server/internal/api"
	"cctrack/server/internal/report"
	"cctrack/server/internal/store"
)

var (
	epoch     = time.Unix(0, 0).UTC()
	farFuture = time.Date(9999, 1, 1, 0, 0, 0, 0, time.UTC)
)

func accountRef(a store.Account) api.AccountRef {
	return api.AccountRef{Id: a.ID, Label: a.DisplayLabel(), Plan: api.Plan(a.Plan), Email: a.Email}
}

// accountToAPI returns the stored admin label as-is (possibly ""), so the
// settings page edits the label rather than its email/uuid fallback.
func accountToAPI(a store.Account) api.Account {
	return api.Account{
		Id: a.ID, AccountUuid: a.AccountUUID, OrgUuid: a.OrgUUID, Email: a.Email, OrgType: a.OrgType,
		RateLimitTier: a.RateLimitTier, Label: a.Label, Plan: api.Plan(a.Plan), CreatedAt: a.CreatedAt,
	}
}

func userRef(u store.UserRef) api.UserRef { return api.UserRef{Id: u.ID, Email: u.Email, Name: u.Name} }

func limitToAPI(l store.LimitSnapshot) api.LimitSnapshot {
	return api.LimitSnapshot{
		AccountId: l.AccountID, CapturedAt: l.CapturedAt,
		FiveHourPct: l.FiveHourPct, FiveHourResetsAt: l.FiveHourResetsAt,
		SevenDayPct: l.SevenDayPct, SevenDayResetsAt: l.SevenDayResetsAt,
	}
}

func limitsToAPI(ls []store.LimitSnapshot) []api.LimitSnapshot {
	out := make([]api.LimitSnapshot, 0, len(ls))
	for _, l := range ls {
		out = append(out, limitToAPI(l))
	}
	return out
}

func accountIDs(rows []store.AggRow) []int64 {
	seen := map[int64]bool{}
	ids := []int64{}
	for _, r := range rows {
		if !seen[r.AccountID] {
			seen[r.AccountID] = true
			ids = append(ids, r.AccountID)
		}
	}
	return ids
}

// sessionPage prices each session in rows and converts to the API shape.
func (s *Server) sessionPage(ctx context.Context, rows []store.SessionRow, total int) (api.SessionPage, error) {
	page := api.SessionPage{Items: []api.Session{}, Total: total}
	if len(rows) == 0 {
		return page, nil
	}
	ids := make([]string, len(rows))
	acctIDs := []int64{}
	for i, r := range rows {
		ids[i] = r.SessionID
		if r.AccountID != nil {
			acctIDs = append(acctIDs, *r.AccountID)
		}
	}
	table, err := s.store.PriceTable(ctx)
	if err != nil {
		return page, err
	}
	agg, err := s.store.Aggregate(ctx, store.GroupBy{Session: true}, store.Filter{From: epoch, To: farFuture, SessionIDs: ids})
	if err != nil {
		return page, err
	}
	accounts, err := s.store.AccountsByIDs(ctx, acctIDs)
	if err != nil {
		return page, err
	}
	g := report.NewGroup[string]()
	for _, r := range agg {
		g.Add(r.SessionID, table, r)
	}
	for _, r := range rows {
		item := api.Session{
			SessionId:      r.SessionID,
			User:           api.UserRef{Id: r.UserID, Email: r.UserEmail, Name: r.UserName},
			DeviceHostname: r.DeviceHostname, Project: r.Project, GitBranch: r.GitBranch, Summary: r.Summary,
			StartedAt: r.StartedAt, EndedAt: r.EndedAt,
			Totals: g.Get(r.SessionID).Totals(),
		}
		if r.AccountID != nil {
			if a, ok := accounts[*r.AccountID]; ok {
				ref := accountRef(a)
				item.Account = &ref
			}
		}
		page.Items = append(page.Items, item)
	}
	return page, nil
}

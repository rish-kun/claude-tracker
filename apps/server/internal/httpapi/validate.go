package httpapi

import (
	"fmt"
	"time"

	"cctrack/server/internal/api"
	"cctrack/server/internal/store"
)

const (
	maxEntries  = 5000
	maxSessions = 500
	maxAccounts = 20
)

var earliestTS = time.Date(2024, 1, 1, 0, 0, 0, 0, time.UTC)

func validateIngest(req *api.IngestRequest, now time.Time) error {
	if req.Device.Hostname == "" || len(req.Device.Hostname) > 255 {
		return fmt.Errorf("device.hostname is required (max 255 chars)")
	}
	if len(req.Entries) > maxEntries || len(req.Sessions) > maxSessions || len(req.Accounts) > maxAccounts {
		return fmt.Errorf("batch too large: max %d entries, %d sessions, %d accounts", maxEntries, maxSessions, maxAccounts)
	}
	latest := now.Add(24 * time.Hour)

	accounts := map[string]bool{store.APIKeyAccount: true}
	for i, a := range req.Accounts {
		if a.AccountUuid == "" || len(a.AccountUuid) > 100 {
			return fmt.Errorf("accounts[%d].account_uuid is required (max 100 chars)", i)
		}
		accounts[a.AccountUuid] = true
	}
	sessions := map[string]bool{}
	for i, s := range req.Sessions {
		if s.SessionId == "" || len(s.SessionId) > 100 {
			return fmt.Errorf("sessions[%d].session_id is required (max 100 chars)", i)
		}
		if s.EndedAt.Before(s.StartedAt) {
			return fmt.Errorf("sessions[%d]: ended_at before started_at", i)
		}
		sessions[s.SessionId] = true
	}
	for i, e := range req.Entries {
		switch {
		case !sessions[e.SessionId]:
			return fmt.Errorf("entries[%d].session_id %q not listed in sessions", i, e.SessionId)
		case !accounts[e.AccountUuid]:
			return fmt.Errorf("entries[%d].account_uuid %q not listed in accounts", i, e.AccountUuid)
		case e.MessageId == "" || len(e.MessageId) > 200:
			return fmt.Errorf("entries[%d].message_id is required (max 200 chars)", i)
		case e.Model == "" || len(e.Model) > 200:
			return fmt.Errorf("entries[%d].model is required (max 200 chars)", i)
		case e.Input < 0 || e.Output < 0 || e.CacheWrite5m < 0 || e.CacheWrite1h < 0 || e.CacheRead < 0:
			return fmt.Errorf("entries[%d]: token counts must be >= 0", i)
		case e.Ts.Before(earliestTS) || e.Ts.After(latest):
			return fmt.Errorf("entries[%d].ts out of range", i)
		}
	}
	if s := req.LimitSnapshot; s != nil {
		if !accounts[s.AccountUuid] || s.AccountUuid == store.APIKeyAccount {
			return fmt.Errorf("limit_snapshot.account_uuid %q not listed in accounts", s.AccountUuid)
		}
		for _, p := range []*float64{s.FiveHourPct, s.SevenDayPct} {
			if p != nil && (*p < 0 || *p > 1000) {
				return fmt.Errorf("limit_snapshot percentages must be within 0-1000")
			}
		}
		if s.CapturedAt.After(latest) {
			return fmt.Errorf("limit_snapshot.captured_at out of range")
		}
	}
	return nil
}

package httpapi

import (
	"fmt"
	"strings"
	"time"

	"cctrack/server/internal/api"
	"cctrack/server/internal/store"
)

const (
	maxEntries  = 5000
	maxSessions = 500
	maxAccounts = 20

	// maxTokensPerField caps each token count of one entry, and
	// maxPromptTokens the prompt side (input + cache writes + cache reads), so
	// no sum the server computes can overflow int64 or Postgres bigint.
	maxTokensPerField = 50_000_000
	maxPromptTokens   = 100_000_000

	// syntheticAccountPrefix namespaces the per-user, per-device accounts the
	// server creates for API-key usage; clients must not claim it.
	syntheticAccountPrefix = "api_key:"
)

var earliestTS = time.Date(2024, 1, 1, 0, 0, 0, 0, time.UTC)

// hasNUL reports whether any string contains a NUL character, which Postgres
// text columns reject.
func hasNUL(ss ...string) bool {
	for _, s := range ss {
		if strings.IndexByte(s, 0) >= 0 {
			return true
		}
	}
	return false
}

func derefStr(p *string) string {
	if p == nil {
		return ""
	}
	return *p
}

func validateIngest(req *api.IngestRequest, now time.Time) error {
	if req.Device.Hostname == "" || len(req.Device.Hostname) > 255 {
		return fmt.Errorf("device.hostname is required (max 255 chars)")
	}
	if hasNUL(req.Device.Hostname, req.Device.Os) {
		return fmt.Errorf("device: strings must not contain NUL")
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
		if strings.HasPrefix(a.AccountUuid, syntheticAccountPrefix) {
			return fmt.Errorf("accounts[%d].account_uuid must not start with %q", i, syntheticAccountPrefix)
		}
		if hasNUL(a.AccountUuid, derefStr(a.OrgUuid), derefStr(a.Email), derefStr(a.OrgType), derefStr(a.RateLimitTier)) {
			return fmt.Errorf("accounts[%d]: strings must not contain NUL", i)
		}
		accounts[a.AccountUuid] = true
	}
	sessions := map[string]bool{}
	for i, s := range req.Sessions {
		if s.SessionId == "" || len(s.SessionId) > 100 {
			return fmt.Errorf("sessions[%d].session_id is required (max 100 chars)", i)
		}
		if hasNUL(s.SessionId, derefStr(s.Cwd), derefStr(s.GitBranch), derefStr(s.Title)) {
			return fmt.Errorf("sessions[%d]: strings must not contain NUL", i)
		}
		if !inRange(s.StartedAt, latest) || !inRange(s.EndedAt, latest) {
			return fmt.Errorf("sessions[%d]: started_at/ended_at out of range", i)
		}
		if s.EndedAt.Before(s.StartedAt) {
			return fmt.Errorf("sessions[%d]: ended_at before started_at", i)
		}
		sessions[s.SessionId] = true
	}
	for i, e := range req.Entries {
		switch {
		case hasNUL(e.SessionId, e.AccountUuid, e.MessageId, derefStr(e.RequestId), e.Model, derefStr(e.Speed)):
			return fmt.Errorf("entries[%d]: strings must not contain NUL", i)
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
		case e.Input > maxTokensPerField || e.Output > maxTokensPerField || e.CacheWrite5m > maxTokensPerField ||
			e.CacheWrite1h > maxTokensPerField || e.CacheRead > maxTokensPerField:
			return fmt.Errorf("entries[%d]: each token count must be <= %d", i, maxTokensPerField)
		case e.Input+e.CacheWrite5m+e.CacheWrite1h+e.CacheRead > maxPromptTokens:
			return fmt.Errorf("entries[%d]: input + cache tokens must be <= %d", i, maxPromptTokens)
		case !inRange(e.Ts, latest):
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
		if !inRange(s.CapturedAt, latest) {
			return fmt.Errorf("limit_snapshot.captured_at out of range")
		}
	}
	return nil
}

// inRange reports whether t lies within [earliestTS, latest]. Zero (omitted)
// times fail, which matters for sessions: the upsert keeps LEAST(started_at)
// and GREATEST(ended_at), so a bad bound could never be corrected later.
func inRange(t, latest time.Time) bool {
	return !t.Before(earliestTS) && !t.After(latest)
}

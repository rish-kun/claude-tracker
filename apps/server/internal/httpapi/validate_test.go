package httpapi

import (
	"math"
	"strings"
	"testing"
	"time"

	"cctrack/server/internal/api"
)

var t0 = time.Date(2026, 10, 1, 12, 0, 0, 0, time.UTC)

func entry(sessionID, accountUUID, msgID, model string, ts time.Time, input, output int64) api.IngestEntry {
	return api.IngestEntry{SessionId: sessionID, AccountUuid: accountUUID, MessageId: msgID, Model: model,
		Ts: ts, Input: input, Output: output}
}

func ingestFixture(sessionID, accountUUID string, entries ...api.IngestEntry) api.IngestRequest {
	start, end := t0, t0
	for _, e := range entries {
		if e.Ts.Before(start) {
			start = e.Ts
		}
		if e.Ts.After(end) {
			end = e.Ts
		}
	}
	cwd := "/home/dev/proj"
	return api.IngestRequest{
		Device:   api.Device{Hostname: "laptop", Os: "macos"},
		Accounts: []api.IngestAccount{{AccountUuid: accountUUID}},
		Sessions: []api.IngestSession{{SessionId: sessionID, Cwd: &cwd, StartedAt: start, EndedAt: end}},
		Entries:  entries,
	}
}

func TestValidateIngestOK(t *testing.T) {
	req := ingestFixture("s1", "acc-1", entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 10, 5))
	pct := 42.0
	req.LimitSnapshot = &api.LimitSnapshotIn{AccountUuid: "acc-1", CapturedAt: t0, FiveHourPct: &pct}
	if err := validateIngest(&req, t0); err != nil {
		t.Fatal(err)
	}
}

func TestValidateIngestErrors(t *testing.T) {
	cases := map[string]func(*api.IngestRequest){
		"session_id":          func(r *api.IngestRequest) { r.Entries[0].SessionId = "other" },
		"account_uuid":        func(r *api.IngestRequest) { r.Entries[0].AccountUuid = "nope" },
		"message_id":          func(r *api.IngestRequest) { r.Entries[0].MessageId = "" },
		"negative":            func(r *api.IngestRequest) { r.Entries[0].Output = -1 },
		"future":              func(r *api.IngestRequest) { r.Entries[0].Ts = t0.Add(48 * time.Hour) },
		"model":               func(r *api.IngestRequest) { r.Entries[0].Model = "" },
		"hostname":            func(r *api.IngestRequest) { r.Device.Hostname = "" },
		"too many":            func(r *api.IngestRequest) { r.Entries = make([]api.IngestEntry, 5001) },
		"ended_at":            func(r *api.IngestRequest) { r.Sessions[0].EndedAt = r.Sessions[0].StartedAt.Add(-time.Hour) },
		"zero started_at":     func(r *api.IngestRequest) { r.Sessions[0].StartedAt = time.Time{} },
		"far-future ended_at": func(r *api.IngestRequest) { r.Sessions[0].EndedAt = t0.Add(48 * time.Hour) },
		"zero captured_at":    func(r *api.IngestRequest) { r.LimitSnapshot = &api.LimitSnapshotIn{AccountUuid: "acc-1"} },
		"far-future captured_at": func(r *api.IngestRequest) {
			r.LimitSnapshot = &api.LimitSnapshotIn{AccountUuid: "acc-1", CapturedAt: t0.Add(48 * time.Hour)}
		},
		"snapshot acct": func(r *api.IngestRequest) { r.LimitSnapshot = &api.LimitSnapshotIn{AccountUuid: "zzz", CapturedAt: t0} },
		"snapshot pct": func(r *api.IngestRequest) {
			p := -5.0
			r.LimitSnapshot = &api.LimitSnapshotIn{AccountUuid: "acc-1", CapturedAt: t0, FiveHourPct: &p}
		},
	}
	for name, mutate := range cases {
		req := ingestFixture("s1", "acc-1", entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 10, 5))
		mutate(&req)
		if err := validateIngest(&req, t0); err == nil {
			t.Errorf("%s: expected error", name)
		}
	}
}

func TestValidateIngestAllowsAPIKeyAccount(t *testing.T) {
	req := ingestFixture("s1", "api_key", entry("s1", "api_key", "m1", "claude-opus-4-5", t0, 1, 1))
	if err := validateIngest(&req, t0); err != nil {
		t.Fatal(err)
	}
}

// Counts exactly at the caps are accepted.
func TestValidateIngestTokenCapsInclusive(t *testing.T) {
	req := ingestFixture("s1", "acc-1", entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, maxTokensPerField, maxTokensPerField))
	e := &req.Entries[0]
	e.CacheWrite5m, e.CacheRead = maxTokensPerField, 0
	if err := validateIngest(&req, t0); err != nil {
		t.Fatalf("prompt exactly at cap: %v", err)
	}
	e.CacheWrite1h = 1
	if err := validateIngest(&req, t0); err == nil {
		t.Fatal("prompt one over cap accepted")
	}
}

// TestValidateIngestLimits covers token caps (S1), the reserved api_key:
// namespace (S6) and NUL rejection (S10), checking the reported reason.
func TestValidateIngestLimits(t *testing.T) {
	const capMsg, nulMsg = "each token count must be <=", "must not contain NUL"
	for _, c := range []struct {
		name, want string
		mutate     func(*api.IngestRequest)
	}{
		{"input over cap", capMsg, func(r *api.IngestRequest) { r.Entries[0].Input = maxTokensPerField + 1 }},
		{"output over cap", capMsg, func(r *api.IngestRequest) { r.Entries[0].Output = maxTokensPerField + 1 }},
		{"cache_write_5m over cap", capMsg, func(r *api.IngestRequest) { r.Entries[0].CacheWrite5m = maxTokensPerField + 1 }},
		{"cache_write_1h over cap", capMsg, func(r *api.IngestRequest) { r.Entries[0].CacheWrite1h = maxTokensPerField + 1 }},
		{"cache_read over cap", capMsg, func(r *api.IngestRequest) { r.Entries[0].CacheRead = maxTokensPerField + 1 }},
		{"max int64 input", capMsg, func(r *api.IngestRequest) { r.Entries[0].Input = math.MaxInt64 }},
		{"max int64 cache_read", capMsg, func(r *api.IngestRequest) { r.Entries[0].CacheRead = math.MaxInt64 }},
		{"prompt over cap", "input + cache tokens must be <=", func(r *api.IngestRequest) {
			e := &r.Entries[0]
			e.Input, e.CacheWrite5m, e.CacheWrite1h, e.CacheRead = maxTokensPerField, maxTokensPerField, 0, 1
		}},
		{"synthetic account_uuid", `must not start with "api_key:"`, func(r *api.IngestRequest) {
			r.Accounts[0].AccountUuid = "api_key:1:1"
			r.Entries[0].AccountUuid = "api_key:1:1"
		}},
		{"NUL hostname", nulMsg, func(r *api.IngestRequest) { r.Device.Hostname = "lap\x00top" }},
		{"NUL os", nulMsg, func(r *api.IngestRequest) { r.Device.Os = "mac\x00" }},
		{"NUL account_uuid", nulMsg, func(r *api.IngestRequest) {
			r.Accounts[0].AccountUuid = "acc\x00"
			r.Entries[0].AccountUuid = "acc\x00"
		}},
		{"NUL org_uuid", nulMsg, func(r *api.IngestRequest) { r.Accounts[0].OrgUuid = str("o\x00") }},
		{"NUL account email", nulMsg, func(r *api.IngestRequest) { r.Accounts[0].Email = str("a\x00@b") }},
		{"NUL org_type", nulMsg, func(r *api.IngestRequest) { r.Accounts[0].OrgType = str("\x00") }},
		{"NUL rate_limit_tier", nulMsg, func(r *api.IngestRequest) { r.Accounts[0].RateLimitTier = str("t\x00") }},
		{"NUL session_id", nulMsg, func(r *api.IngestRequest) {
			r.Sessions[0].SessionId = "s\x001"
			r.Entries[0].SessionId = "s\x001"
		}},
		{"NUL cwd", nulMsg, func(r *api.IngestRequest) { r.Sessions[0].Cwd = str("/a\x00b") }},
		{"NUL git_branch", nulMsg, func(r *api.IngestRequest) { r.Sessions[0].GitBranch = str("main\x00") }},
		{"NUL message_id", nulMsg, func(r *api.IngestRequest) { r.Entries[0].MessageId = "m\x00" }},
		{"NUL request_id", nulMsg, func(r *api.IngestRequest) { r.Entries[0].RequestId = str("r\x00") }},
		{"NUL model", nulMsg, func(r *api.IngestRequest) { r.Entries[0].Model = "claude\x00" }},
		{"NUL speed", nulMsg, func(r *api.IngestRequest) { r.Entries[0].Speed = str("fast\x00") }},
	} {
		t.Run(c.name, func(t *testing.T) {
			req := ingestFixture("s1", "acc-1", entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 10, 5))
			c.mutate(&req)
			err := validateIngest(&req, t0)
			if err == nil || !strings.Contains(err.Error(), c.want) {
				t.Fatalf("err = %v, want substring %q", err, c.want)
			}
		})
	}
}

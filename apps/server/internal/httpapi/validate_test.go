package httpapi

import (
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

package store_test

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"cctrack/server/internal/api"
	"cctrack/server/internal/store"
	"cctrack/server/internal/testdb"
)

var ts0 = time.Date(2026, 10, 1, 12, 0, 0, 0, time.UTC)

func strp(s string) *string { return &s }

func TestSessionTitle(t *testing.T) {
	long := strings.Repeat("é", store.MaxTitleRunes+50)
	for _, c := range []struct {
		name string
		in   *string
		want *string
	}{
		{"absent", nil, nil},
		{"empty", strp(""), nil},
		{"blank", strp(" \t\n "), nil},
		{"trimmed", strp("  Fix login  "), strp("Fix login")},
		{"truncated by runes", &long, strp(strings.Repeat("é", store.MaxTitleRunes))},
		{"no trailing space after cut", strp(strings.Repeat("a", store.MaxTitleRunes-1) + " tail"), strp(strings.Repeat("a", store.MaxTitleRunes-1))},
	} {
		got := store.SessionTitle(c.in)
		switch {
		case (got == nil) != (c.want == nil):
			t.Errorf("%s: got %v, want %v", c.name, got, c.want)
		case got != nil && *got != *c.want:
			t.Errorf("%s: got %q, want %q", c.name, *got, *c.want)
		case got != nil && utf8.RuneCountInString(*got) > store.MaxTitleRunes:
			t.Errorf("%s: %d runes", c.name, utf8.RuneCountInString(*got))
		}
	}
}

// newUser inserts a user and returns its id.
func newUser(t *testing.T, st *store.Store, email string) int64 {
	t.Helper()
	var id int64
	if err := st.Pool().QueryRow(context.Background(),
		`INSERT INTO users (google_sub, email) VALUES ($1, $1) RETURNING id`, email).Scan(&id); err != nil {
		t.Fatal(err)
	}
	return id
}

func batch(sessionID string, title *string, entries ...api.IngestEntry) *api.IngestRequest {
	return &api.IngestRequest{
		Device:   api.Device{Hostname: "laptop", Os: "macos"},
		Accounts: []api.IngestAccount{{AccountUuid: "acc-1"}},
		Sessions: []api.IngestSession{{SessionId: sessionID, Cwd: strp("/home/dev/proj"), Title: title,
			StartedAt: ts0, EndedAt: ts0}},
		Entries: entries,
	}
}

func ingestEntry(msg, model string, sub bool, in, out, w5, w1, cr int64) api.IngestEntry {
	return api.IngestEntry{SessionId: "s1", AccountUuid: "acc-1", MessageId: msg, Model: model, Ts: ts0,
		Input: in, Output: out, CacheWrite5m: w5, CacheWrite1h: w1, CacheRead: cr, IsSubagent: sub}
}

// TestIngestSessionTitleUpsert: a non-empty title replaces the stored one;
// an absent or blank title never clears it.
func TestIngestSessionTitleUpsert(t *testing.T) {
	st := store.New(testdb.New(t))
	ctx := context.Background()
	uid := newUser(t, st, "dev@example.com")
	title := func() *string {
		t.Helper()
		r, err := st.SessionByID(ctx, "s1")
		if err != nil {
			t.Fatal(err)
		}
		return r.Title
	}
	ingest := func(tt *string) {
		t.Helper()
		if _, err := st.Ingest(ctx, uid, batch("s1", tt)); err != nil {
			t.Fatal(err)
		}
	}

	ingest(nil)
	if got := title(); got != nil {
		t.Fatalf("untitled session: title = %q", *got)
	}
	ingest(strp("  Fix the login flow  "))
	if got := title(); got == nil || *got != "Fix the login flow" {
		t.Fatalf("title = %v", got)
	}
	ingest(nil)
	ingest(strp(""))
	ingest(strp("   "))
	if got := title(); got == nil || *got != "Fix the login flow" {
		t.Fatalf("absent/blank title cleared it: %v", got)
	}
	ingest(strp("Fix login and add tests"))
	if got := title(); got == nil || *got != "Fix login and add tests" {
		t.Fatalf("new title not applied: %v", got)
	}
	ingest(strp(strings.Repeat("界", 300)))
	if got := title(); got == nil || *got != strings.Repeat("界", store.MaxTitleRunes) {
		t.Fatalf("long title not truncated to %d runes: %v", store.MaxTitleRunes, got)
	}

	r, err := st.SessionByID(ctx, "s1")
	if err != nil {
		t.Fatal(err)
	}
	if r.UserID != uid || r.Cwd == nil || *r.Cwd != "/home/dev/proj" || r.Project == nil || *r.Project != "proj" ||
		r.DeviceHostname == nil || *r.DeviceHostname != "laptop" {
		t.Fatalf("session row = %+v", r)
	}
	list, _, err := st.ListSessions(ctx, uid, 10, 0)
	if err != nil || len(list) != 1 || list[0].Title == nil || list[0].Cwd == nil {
		t.Fatalf("ListSessions = %+v, %v", list, err)
	}
	if _, err := st.SessionByID(ctx, "nope"); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("missing session: err = %v, want ErrNotFound", err)
	}
}

// TestIngestTitleOnlyUpdate: the CLI's title-only update (no cwd, branch,
// accounts or entries) sets the title and leaves everything else alone.
func TestIngestTitleOnlyUpdate(t *testing.T) {
	st := store.New(testdb.New(t))
	ctx := context.Background()
	uid := newUser(t, st, "dev@example.com")
	full := batch("s1", nil, ingestEntry("m1", "claude-opus-4-5", false, 1, 1, 0, 0, 0))
	full.Sessions[0].GitBranch = strp("main")
	if _, err := st.Ingest(ctx, uid, full); err != nil {
		t.Fatal(err)
	}
	before, err := st.SessionByID(ctx, "s1")
	if err != nil {
		t.Fatal(err)
	}
	titleOnly := &api.IngestRequest{
		Device:   api.Device{Hostname: "laptop", Os: "macos"},
		Accounts: []api.IngestAccount{},
		Sessions: []api.IngestSession{{SessionId: "s1", Title: strp("Renamed"), StartedAt: ts0, EndedAt: ts0}},
		Entries:  []api.IngestEntry{},
	}
	if _, err := st.Ingest(ctx, uid, titleOnly); err != nil {
		t.Fatal(err)
	}
	after, err := st.SessionByID(ctx, "s1")
	if err != nil {
		t.Fatal(err)
	}
	if after.Title == nil || *after.Title != "Renamed" {
		t.Fatalf("title = %v", after.Title)
	}
	after.Title = before.Title
	if before.AccountID == nil || after.AccountID == nil || *before.AccountID != *after.AccountID ||
		derefS(after.Cwd) != derefS(before.Cwd) || derefS(after.Project) != derefS(before.Project) ||
		derefS(after.GitBranch) != "main" || !after.StartedAt.Equal(before.StartedAt) ||
		!after.EndedAt.Equal(before.EndedAt) {
		t.Fatalf("title-only update changed the session:\nbefore %+v\nafter  %+v", before, after)
	}
}

func derefS(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

// TestSessionAggregate: rows are split by subagent flag and model, with the
// entry count and the largest single prompt of each group.
func TestSessionAggregate(t *testing.T) {
	st := store.New(testdb.New(t))
	ctx := context.Background()
	uid := newUser(t, st, "dev@example.com")
	if _, err := st.Ingest(ctx, uid, batch("s1", nil,
		ingestEntry("m1", "claude-opus-4-5", false, 100, 10, 1000, 0, 5000),       // prompt 6100
		ingestEntry("m2", "claude-opus-4-5", false, 200, 20, 0, 3000, 40_000),     // prompt 43200
		ingestEntry("m3", "claude-haiku-4-5", true, 50, 5, 0, 0, 10),              // prompt 60
		ingestEntry("m4", "claude-opus-4-5", true, 300, 30, 0, 0, 0),              // prompt 300
		ingestEntry("m5", "claude-opus-4-5-20251101", false, 1, 1, 0, 0, 200_000), // long context
	)); err != nil {
		t.Fatal(err)
	}
	// Another session's entries must not leak in.
	other := batch("s2", nil, ingestEntry("x1", "claude-opus-4-5", false, 9, 9, 9, 9, 9))
	other.Entries[0].SessionId = "s2"
	if _, err := st.Ingest(ctx, uid, other); err != nil {
		t.Fatal(err)
	}

	rows, err := st.SessionAggregate(ctx, "s1")
	if err != nil {
		t.Fatal(err)
	}
	type key struct {
		sub     bool
		model   string
		longCtx bool
	}
	got := map[key]store.SessionAggRow{}
	for _, r := range rows {
		if r.SessionID != "s1" || r.Speed != "standard" {
			t.Fatalf("row = %+v", r)
		}
		got[key{r.IsSubagent, r.Model, r.LongCtx}] = r
	}
	if len(got) != 4 || len(rows) != 4 {
		t.Fatalf("rows = %+v", rows)
	}
	main := got[key{false, "claude-opus-4-5", false}]
	if main.Calls != 2 || main.PeakPrompt != 43_200 || main.Counts.Input != 300 || main.Counts.Output != 30 ||
		main.Counts.CacheWrite5m != 1000 || main.Counts.CacheWrite1h != 3000 || main.Counts.CacheRead != 45_000 {
		t.Fatalf("main opus = %+v", main)
	}
	if r := got[key{true, "claude-haiku-4-5", false}]; r.Calls != 1 || r.PeakPrompt != 60 || r.Counts.Input != 50 {
		t.Fatalf("subagent haiku = %+v", r)
	}
	if r := got[key{true, "claude-opus-4-5", false}]; r.Calls != 1 || r.PeakPrompt != 300 {
		t.Fatalf("subagent opus = %+v", r)
	}
	if r := got[key{false, "claude-opus-4-5-20251101", true}]; r.Calls != 1 || r.PeakPrompt != 200_001 {
		t.Fatalf("long-context row = %+v", r)
	}

	if rows, err := st.SessionAggregate(ctx, "empty"); err != nil || len(rows) != 0 {
		t.Fatalf("unknown session: %+v, %v", rows, err)
	}
}

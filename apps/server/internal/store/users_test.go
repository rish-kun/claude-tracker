package store_test

import (
	"context"
	"testing"

	"cctrack/server/internal/store"
	"cctrack/server/internal/testdb"
)

// TestPurgeExpiredIsBounded: each call deletes at most PurgeBatch expired rows
// per table and never touches live ones.
func TestPurgeExpiredIsBounded(t *testing.T) {
	pool := testdb.New(t)
	st := store.New(pool)
	ctx := context.Background()
	exec := func(q string) {
		t.Helper()
		if _, err := pool.Exec(ctx, q); err != nil {
			t.Fatalf("%s: %v", q, err)
		}
	}
	count := func(q string) int {
		t.Helper()
		var n int
		if err := pool.QueryRow(ctx, q).Scan(&n); err != nil {
			t.Fatalf("%s: %v", q, err)
		}
		return n
	}
	exec(`INSERT INTO users (google_sub, email) VALUES ('sub', 'dev@example.com')`)
	exec(`INSERT INTO auth_requests (state, mode, expires_at)
		SELECT 'old-' || g, 'web', now() - interval '1 hour' FROM generate_series(1, 1500) g`)
	exec(`INSERT INTO auth_requests (state, mode, expires_at)
		SELECT 'live-' || g, 'web', now() + interval '1 hour' FROM generate_series(1, 5) g`)
	exec(`INSERT INTO cli_auth_codes (code_hash, user_id, code_challenge, expires_at)
		SELECT decode(md5('old' || g), 'hex'), u.id, 'c', now() - interval '2 days'
		FROM generate_series(1, 1500) g, users u`)
	exec(`INSERT INTO cli_auth_codes (code_hash, user_id, code_challenge, expires_at)
		SELECT decode(md5('recent' || g), 'hex'), u.id, 'c', now() - interval '1 hour'
		FROM generate_series(1, 3) g, users u`)

	const oldReqs = "SELECT count(*) FROM auth_requests WHERE state LIKE 'old-%'"
	const oldCodes = "SELECT count(*) FROM cli_auth_codes WHERE expires_at < now() - interval '1 day'"
	for i, want := range []int{1500 - store.PurgeBatch, 0} {
		if err := st.PurgeExpired(ctx); err != nil {
			t.Fatalf("purge %d: %v", i, err)
		}
		if got := count(oldReqs); got != want {
			t.Fatalf("after purge %d: expired auth_requests = %d, want %d", i, got, want)
		}
		if got := count(oldCodes); got != want {
			t.Fatalf("after purge %d: old cli codes = %d, want %d", i, got, want)
		}
	}
	if got := count("SELECT count(*) FROM auth_requests"); got != 5 {
		t.Fatalf("live auth_requests = %d, want 5", got)
	}
	if got := count("SELECT count(*) FROM cli_auth_codes"); got != 3 {
		t.Fatalf("codes within the 1-day grace = %d, want 3", got)
	}

	for _, c := range []struct {
		n    int
		want bool
	}{{5, true}, {6, false}} {
		got, err := st.PendingAuthRequestsAtLeast(ctx, c.n)
		if err != nil || got != c.want {
			t.Fatalf("PendingAuthRequestsAtLeast(%d) = %v, %v; want %v", c.n, got, err, c.want)
		}
	}
}

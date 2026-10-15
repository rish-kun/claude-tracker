package store

import (
	"cmp"
	"context"
	"errors"
	"fmt"
	"slices"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/jackc/pgx/v5"

	"cctrack/server/internal/api"
)

type IngestResult struct {
	Accepted, Duplicates int
}

func project(cwd *string) *string {
	if cwd == nil || *cwd == "" {
		return nil
	}
	p := strings.TrimRight(strings.ReplaceAll(*cwd, `\`, "/"), "/")
	if i := strings.LastIndexByte(p, '/'); i >= 0 {
		p = p[i+1:]
	}
	return &p
}

// MaxTitleRunes caps a stored session title.
const MaxTitleRunes = 200

// SessionTitle normalises a client-sent session title: trimmed and cut to
// MaxTitleRunes characters. Empty (or absent) is nil, which the upsert treats
// as "keep the stored title".
func SessionTitle(t *string) *string {
	if t == nil {
		return nil
	}
	v := strings.TrimSpace(*t)
	if utf8.RuneCountInString(v) > MaxTitleRunes {
		v = strings.TrimSpace(string([]rune(v)[:MaxTitleRunes]))
	}
	if v == "" {
		return nil
	}
	return &v
}

func ptrOrNil(s *string) *string {
	if s == nil || *s == "" {
		return nil
	}
	return s
}

// Ingest stores a batch atomically. Entries are deduplicated on message_id.
func (s *Store) Ingest(ctx context.Context, userID int64, req *api.IngestRequest) (IngestResult, error) {
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return IngestResult{}, err
	}
	defer tx.Rollback(ctx)

	var deviceID int64
	if err := tx.QueryRow(ctx, `
		INSERT INTO devices (user_id, hostname, os) VALUES ($1, $2, $3)
		ON CONFLICT (user_id, hostname) DO UPDATE SET os = EXCLUDED.os, last_seen = now()
		RETURNING id`, userID, req.Device.Hostname, req.Device.Os).Scan(&deviceID); err != nil {
		return IngestResult{}, fmt.Errorf("device: %w", err)
	}

	accountIDs := map[string]int64{}
	upsertAccount := func(uuid string, a api.IngestAccount, label, plan string) error {
		var id int64
		err := tx.QueryRow(ctx, `
			INSERT INTO claude_accounts (account_uuid, org_uuid, email, org_type, rate_limit_tier, label, plan)
			VALUES ($1, $2, $3, $4, $5, $6, $7)
			ON CONFLICT (account_uuid) DO UPDATE SET
			  org_uuid = COALESCE(EXCLUDED.org_uuid, claude_accounts.org_uuid),
			  email = COALESCE(EXCLUDED.email, claude_accounts.email),
			  org_type = COALESCE(EXCLUDED.org_type, claude_accounts.org_type),
			  rate_limit_tier = COALESCE(EXCLUDED.rate_limit_tier, claude_accounts.rate_limit_tier),
			  -- Fill in a plan first seen without a tier; never touch one an admin (or an
			  -- earlier guess) already set.
			  plan = CASE WHEN claude_accounts.plan = 'unknown' THEN EXCLUDED.plan ELSE claude_accounts.plan END
			RETURNING id`,
			uuid, ptrOrNil(a.OrgUuid), ptrOrNil(a.Email), ptrOrNil(a.OrgType), ptrOrNil(a.RateLimitTier), label, plan).Scan(&id)
		accountIDs[a.AccountUuid] = id
		return err
	}
	// Rows are locked in a fixed order (accounts by uuid, sessions by id, entries
	// by message id) so concurrent batches touching the same rows cannot deadlock.
	accounts := slices.SortedFunc(slices.Values(req.Accounts), func(a, b api.IngestAccount) int {
		return cmp.Compare(a.AccountUuid, b.AccountUuid)
	})
	for _, a := range accounts {
		if a.AccountUuid == APIKeyAccount {
			continue
		}
		orgType, tier := deref(a.OrgType), deref(a.RateLimitTier)
		if err := upsertAccount(a.AccountUuid, a, "", GuessPlan(orgType, tier)); err != nil {
			return IngestResult{}, fmt.Errorf("account: %w", err)
		}
	}
	needsAPIKey := false
	for _, e := range req.Entries {
		if e.AccountUuid == APIKeyAccount {
			needsAPIKey = true
			break
		}
	}
	if needsAPIKey {
		uuid := fmt.Sprintf("api_key:%d:%d", userID, deviceID)
		label := "API key (" + req.Device.Hostname + ")"
		if err := upsertAccount(uuid, api.IngestAccount{AccountUuid: APIKeyAccount}, label, "api_key"); err != nil {
			return IngestResult{}, fmt.Errorf("api key account: %w", err)
		}
	}

	// Each session's account is the account of its latest entry in this batch.
	sessionAccount := map[string]int64{}
	latest := map[string]time.Time{}
	for _, e := range req.Entries {
		if e.Ts.After(latest[e.SessionId]) || latest[e.SessionId].IsZero() {
			latest[e.SessionId] = e.Ts
			sessionAccount[e.SessionId] = accountIDs[e.AccountUuid]
		}
	}
	sessions := slices.SortedFunc(slices.Values(req.Sessions), func(a, b api.IngestSession) int {
		return cmp.Compare(a.SessionId, b.SessionId)
	})
	for _, sess := range sessions {
		var acct *int64
		if id, ok := sessionAccount[sess.SessionId]; ok {
			acct = &id
		}
		// Metadata is latest-wins by observed time: a batch whose ended_at is
		// older than the stored one (a replay or a late upload) only fills
		// gaps and never overwrites newer account/device/cwd/branch values.
		// A non-empty title always replaces the stored one (Claude Code
		// regenerates titles); an absent/empty one never clears it.
		var got string
		err := tx.QueryRow(ctx, `
			INSERT INTO sessions (session_id, user_id, device_id, account_id, cwd, project, git_branch, started_at, ended_at, title)
			VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
			ON CONFLICT (session_id) DO UPDATE SET
			  device_id = CASE WHEN EXCLUDED.ended_at >= sessions.ended_at
			    THEN COALESCE(EXCLUDED.device_id, sessions.device_id) ELSE COALESCE(sessions.device_id, EXCLUDED.device_id) END,
			  account_id = CASE WHEN EXCLUDED.ended_at >= sessions.ended_at
			    THEN COALESCE(EXCLUDED.account_id, sessions.account_id) ELSE COALESCE(sessions.account_id, EXCLUDED.account_id) END,
			  cwd = CASE WHEN EXCLUDED.ended_at >= sessions.ended_at
			    THEN COALESCE(EXCLUDED.cwd, sessions.cwd) ELSE COALESCE(sessions.cwd, EXCLUDED.cwd) END,
			  project = CASE WHEN EXCLUDED.ended_at >= sessions.ended_at
			    THEN COALESCE(EXCLUDED.project, sessions.project) ELSE COALESCE(sessions.project, EXCLUDED.project) END,
			  git_branch = CASE WHEN EXCLUDED.ended_at >= sessions.ended_at
			    THEN COALESCE(EXCLUDED.git_branch, sessions.git_branch) ELSE COALESCE(sessions.git_branch, EXCLUDED.git_branch) END,
			  title = COALESCE(EXCLUDED.title, sessions.title),
			  started_at = LEAST(sessions.started_at, EXCLUDED.started_at),
			  ended_at = GREATEST(sessions.ended_at, EXCLUDED.ended_at)
			WHERE sessions.user_id = EXCLUDED.user_id
			RETURNING session_id`,
			sess.SessionId, userID, deviceID, acct, ptrOrNil(sess.Cwd), project(sess.Cwd), ptrOrNil(sess.GitBranch),
			sess.StartedAt, sess.EndedAt, SessionTitle(sess.Title)).Scan(&got)
		if errors.Is(err, pgx.ErrNoRows) {
			return IngestResult{}, ErrForbidden
		}
		if err != nil {
			return IngestResult{}, fmt.Errorf("session: %w", err)
		}
	}

	var res IngestResult
	if n := len(req.Entries); n > 0 {
		cols := struct {
			session, msg, req, model, speed []string
			account                         []int64
			ts                              []time.Time
			in, out, w5, w1, cr             []int64
			sub                             []bool
		}{}
		entries := slices.SortedFunc(slices.Values(req.Entries), func(a, b api.IngestEntry) int {
			return cmp.Compare(a.MessageId, b.MessageId)
		})
		for _, e := range entries {
			speed := deref(e.Speed)
			if speed == "" {
				speed = "standard"
			}
			cols.session = append(cols.session, e.SessionId)
			cols.account = append(cols.account, accountIDs[e.AccountUuid])
			cols.msg = append(cols.msg, e.MessageId)
			cols.req = append(cols.req, deref(e.RequestId))
			cols.model = append(cols.model, e.Model)
			cols.ts = append(cols.ts, e.Ts)
			cols.in = append(cols.in, e.Input)
			cols.out = append(cols.out, e.Output)
			cols.w5 = append(cols.w5, e.CacheWrite5m)
			cols.w1 = append(cols.w1, e.CacheWrite1h)
			cols.cr = append(cols.cr, e.CacheRead)
			cols.speed = append(cols.speed, speed)
			cols.sub = append(cols.sub, e.IsSubagent)
		}
		tag, err := tx.Exec(ctx, `
			INSERT INTO usage_entries (session_id, user_id, account_id, message_id, request_id, model, ts,
			  input, output, cache_write_5m, cache_write_1h, cache_read, speed, is_subagent)
			SELECT s, $2, a, m, rq, md, t, i, o, w5, w1, cr, sp, sb
			FROM unnest($1::text[], $3::bigint[], $4::text[], $5::text[], $6::text[], $7::timestamptz[],
			            $8::bigint[], $9::bigint[], $10::bigint[], $11::bigint[], $12::bigint[], $13::text[], $14::bool[])
			  AS x(s, a, m, rq, md, t, i, o, w5, w1, cr, sp, sb)
			ON CONFLICT (message_id) DO NOTHING`,
			cols.session, userID, cols.account, cols.msg, cols.req, cols.model, cols.ts,
			cols.in, cols.out, cols.w5, cols.w1, cols.cr, cols.speed, cols.sub)
		if err != nil {
			return IngestResult{}, fmt.Errorf("entries: %w", err)
		}
		res.Accepted = int(tag.RowsAffected())
		res.Duplicates = n - res.Accepted
	}

	if snap := req.LimitSnapshot; snap != nil {
		if _, err := tx.Exec(ctx, `
			INSERT INTO limit_snapshots (account_id, user_id, captured_at, five_hour_pct, five_hour_resets_at,
			  seven_day_pct, seven_day_resets_at)
			VALUES ($1, $2, $3, $4, $5, $6, $7)
			ON CONFLICT (account_id, captured_at) DO NOTHING`,
			accountIDs[snap.AccountUuid], userID, snap.CapturedAt, snap.FiveHourPct, snap.FiveHourResetsAt,
			snap.SevenDayPct, snap.SevenDayResetsAt); err != nil {
			return IngestResult{}, fmt.Errorf("snapshot: %w", err)
		}
	}
	return res, tx.Commit(ctx)
}

func deref(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

func (s *Store) SetSummary(ctx context.Context, userID int64, sessionID, summary string) error {
	tag, err := s.pool.Exec(ctx, `
		UPDATE sessions SET summary = $3, summary_at = now()
		WHERE session_id = $1 AND user_id = $2`, sessionID, userID, summary)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

package store

import (
	"context"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5"

	"cctrack/server/internal/pricing"
)

type GroupBy struct {
	Day, User, Account, Session bool
}

type Filter struct {
	From, To   time.Time
	TZ         string
	UserID     *int64
	AccountID  *int64
	SessionIDs []string
}

type AggRow struct {
	Day       string
	UserID    int64
	AccountID int64
	SessionID string
	Model     string
	Speed     string
	LongCtx   bool
	Counts    pricing.Counts
}

// Aggregate sums tokens over the filter, grouped by the requested dimensions
// plus model, speed and the long-context flag (all needed for pricing).
func (s *Store) Aggregate(ctx context.Context, g GroupBy, f Filter) ([]AggRow, error) {
	pick := func(on bool, expr, zero string) string {
		if on {
			return expr
		}
		return zero
	}
	q := fmt.Sprintf(`
		SELECT %s, %s, %s, %s, model, speed,
		       (input + cache_write_5m + cache_write_1h + cache_read) > %d AS long_ctx,
		       sum(input)::bigint, sum(output)::bigint, sum(cache_write_5m)::bigint,
		       sum(cache_write_1h)::bigint, sum(cache_read)::bigint
		FROM usage_entries
		WHERE ts >= $1 AND ts < $2 AND $3::text IS NOT NULL
		  AND ($4::bigint IS NULL OR user_id = $4)
		  AND ($5::bigint IS NULL OR account_id = $5)
		  AND (cardinality($6::text[]) = 0 OR session_id = ANY($6))
		GROUP BY 1, 2, 3, 4, 5, 6, 7`,
		pick(g.Day, "to_char(ts AT TIME ZONE $3::text, 'YYYY-MM-DD')", "''::text"),
		pick(g.User, "user_id", "0::bigint"),
		pick(g.Account, "account_id", "0::bigint"),
		pick(g.Session, "session_id", "''::text"),
		pricing.LongContextThreshold)
	// Grouping by a constant select-list column ("GROUP BY 1" over ''::text) is valid
	// Postgres; if a future version rejects it, build GROUP BY from the enabled dims only.
	tz := f.TZ
	if tz == "" {
		tz = "UTC"
	}
	ids := f.SessionIDs
	if ids == nil {
		ids = []string{}
	}
	rows, err := s.pool.Query(ctx, q, f.From, f.To, tz, f.UserID, f.AccountID, ids)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (AggRow, error) {
		var r AggRow
		err := row.Scan(&r.Day, &r.UserID, &r.AccountID, &r.SessionID, &r.Model, &r.Speed, &r.LongCtx,
			&r.Counts.Input, &r.Counts.Output, &r.Counts.CacheWrite5m, &r.Counts.CacheWrite1h, &r.Counts.CacheRead)
		return r, err
	})
}

func (s *Store) Prices(ctx context.Context) ([]pricing.Price, error) {
	rows, err := s.pool.Query(ctx, `
		SELECT model, input, cache_write_5m, cache_write_1h, cache_read, output, fast_input, fast_output,
		       long_input, long_cache_write_5m, long_cache_write_1h, long_cache_read, long_output, updated_at
		FROM model_prices ORDER BY model`)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, scanPrice)
}

func scanPrice(row pgx.CollectableRow) (pricing.Price, error) {
	var p pricing.Price
	var li, l5, l1, lr, lo *float64
	err := row.Scan(&p.Model, &p.Base.Input, &p.Base.CacheWrite5m, &p.Base.CacheWrite1h, &p.Base.CacheRead,
		&p.Base.Output, &p.FastInput, &p.FastOutput, &li, &l5, &l1, &lr, &lo, &p.UpdatedAt)
	if li != nil && l5 != nil && l1 != nil && lr != nil && lo != nil {
		p.Long = &pricing.Rates{Input: *li, CacheWrite5m: *l5, CacheWrite1h: *l1, CacheRead: *lr, Output: *lo}
	}
	return p, err
}

func (s *Store) PriceTable(ctx context.Context) (*pricing.Table, error) {
	prices, err := s.Prices(ctx)
	if err != nil {
		return nil, err
	}
	return pricing.NewTable(prices), nil
}

type Account struct {
	ID            int64
	AccountUUID   string
	OrgUUID       *string
	Email         *string
	OrgType       *string
	RateLimitTier *string
	Label         string
	Plan          string
	CreatedAt     time.Time
}

// DisplayLabel is the admin label, else the account email, else its uuid.
func (a Account) DisplayLabel() string {
	if a.Label != "" {
		return a.Label
	}
	if a.Email != nil && *a.Email != "" {
		return *a.Email
	}
	return a.AccountUUID
}

const accountCols = "id, account_uuid, org_uuid, email, org_type, rate_limit_tier, label, plan, created_at"

func scanAccount(row pgx.CollectableRow) (Account, error) {
	var a Account
	err := row.Scan(&a.ID, &a.AccountUUID, &a.OrgUUID, &a.Email, &a.OrgType, &a.RateLimitTier,
		&a.Label, &a.Plan, &a.CreatedAt)
	return a, err
}

func (s *Store) ListAccounts(ctx context.Context) ([]Account, error) {
	rows, err := s.pool.Query(ctx, `SELECT `+accountCols+` FROM claude_accounts ORDER BY id`)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, scanAccount)
}

func (s *Store) AccountsByIDs(ctx context.Context, ids []int64) (map[int64]Account, error) {
	rows, err := s.pool.Query(ctx, `SELECT `+accountCols+` FROM claude_accounts WHERE id = ANY($1)`, ids)
	if err != nil {
		return nil, err
	}
	list, err := pgx.CollectRows(rows, scanAccount)
	out := make(map[int64]Account, len(list))
	for _, a := range list {
		out[a.ID] = a
	}
	return out, err
}

type UserRef struct {
	ID          int64
	Email, Name string
}

func (s *Store) UsersByIDs(ctx context.Context, ids []int64) (map[int64]UserRef, error) {
	rows, err := s.pool.Query(ctx, `SELECT id, email, name FROM users WHERE id = ANY($1)`, ids)
	if err != nil {
		return nil, err
	}
	list, err := pgx.CollectRows(rows, func(row pgx.CollectableRow) (UserRef, error) {
		var u UserRef
		return u, row.Scan(&u.ID, &u.Email, &u.Name)
	})
	out := make(map[int64]UserRef, len(list))
	for _, u := range list {
		out[u.ID] = u
	}
	return out, err
}

type LimitSnapshot struct {
	AccountID        int64
	CapturedAt       time.Time
	FiveHourPct      *float64
	FiveHourResetsAt *time.Time
	SevenDayPct      *float64
	SevenDayResetsAt *time.Time
}

func scanLimit(row pgx.CollectableRow) (LimitSnapshot, error) {
	var l LimitSnapshot
	err := row.Scan(&l.AccountID, &l.CapturedAt, &l.FiveHourPct, &l.FiveHourResetsAt, &l.SevenDayPct, &l.SevenDayResetsAt)
	return l, err
}

const limitCols = "account_id, captured_at, five_hour_pct, five_hour_resets_at, seven_day_pct, seven_day_resets_at"

func (s *Store) LatestLimits(ctx context.Context, accountIDs []int64) ([]LimitSnapshot, error) {
	rows, err := s.pool.Query(ctx, `
		SELECT DISTINCT ON (account_id) `+limitCols+`
		FROM limit_snapshots WHERE account_id = ANY($1)
		ORDER BY account_id, captured_at DESC`, accountIDs)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, scanLimit)
}

func (s *Store) LimitHistory(ctx context.Context, accountID int64, from, to time.Time) ([]LimitSnapshot, error) {
	rows, err := s.pool.Query(ctx, `
		SELECT `+limitCols+` FROM limit_snapshots
		WHERE account_id = $1 AND captured_at >= $2 AND captured_at < $3
		ORDER BY captured_at`, accountID, from, to)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, scanLimit)
}

type SessionRow struct {
	SessionID      string
	UserID         int64
	UserEmail      string
	UserName       string
	AccountID      *int64
	DeviceHostname *string
	Project        *string
	GitBranch      *string
	Summary        *string
	StartedAt      time.Time
	EndedAt        time.Time
}

// ListSessions returns one page of a user's sessions (newest first) and the total count.
func (s *Store) ListSessions(ctx context.Context, userID int64, limit, offset int) ([]SessionRow, int, error) {
	var total int
	if err := s.pool.QueryRow(ctx, `SELECT count(*) FROM sessions WHERE user_id = $1`, userID).Scan(&total); err != nil {
		return nil, 0, err
	}
	rows, err := s.pool.Query(ctx, `
		SELECT s.session_id, s.user_id, u.email, u.name, s.account_id, d.hostname, s.project, s.git_branch,
		       s.summary, s.started_at, s.ended_at
		FROM sessions s
		JOIN users u ON u.id = s.user_id
		LEFT JOIN devices d ON d.id = s.device_id
		WHERE s.user_id = $1
		ORDER BY s.started_at DESC, s.session_id
		LIMIT $2 OFFSET $3`, userID, limit, offset)
	if err != nil {
		return nil, 0, err
	}
	list, err := pgx.CollectRows(rows, func(row pgx.CollectableRow) (SessionRow, error) {
		var r SessionRow
		err := row.Scan(&r.SessionID, &r.UserID, &r.UserEmail, &r.UserName, &r.AccountID, &r.DeviceHostname,
			&r.Project, &r.GitBranch, &r.Summary, &r.StartedAt, &r.EndedAt)
		return r, err
	})
	return list, total, err
}

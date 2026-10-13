package store

import (
	"context"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"

	"cctrack/server/internal/pricing"
)

type AdminUser struct {
	ID         int64
	Email      string
	Name       string
	CreatedAt  time.Time
	LastSeenAt *time.Time
}

// ListUsers returns every user with the most recent last_seen of their devices.
func (s *Store) ListUsers(ctx context.Context) ([]AdminUser, error) {
	rows, err := s.pool.Query(ctx, `
		SELECT u.id, u.email, u.name, u.created_at, max(d.last_seen)
		FROM users u LEFT JOIN devices d ON d.user_id = u.id
		GROUP BY u.id ORDER BY u.email, u.id`)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (AdminUser, error) {
		var u AdminUser
		return u, row.Scan(&u.ID, &u.Email, &u.Name, &u.CreatedAt, &u.LastSeenAt)
	})
}

type Token struct {
	ID         int64
	User       UserRef
	DeviceName string
	CreatedAt  time.Time
	LastUsedAt *time.Time
	RevokedAt  *time.Time
}

// ListTokens returns active tokens first, each group newest first.
func (s *Store) ListTokens(ctx context.Context) ([]Token, error) {
	rows, err := s.pool.Query(ctx, `
		SELECT t.id, u.id, u.email, u.name, t.device_name, t.created_at, t.last_used_at, t.revoked_at
		FROM api_tokens t JOIN users u ON u.id = t.user_id
		ORDER BY t.revoked_at IS NOT NULL, t.created_at DESC, t.id DESC`)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (Token, error) {
		var t Token
		return t, row.Scan(&t.ID, &t.User.ID, &t.User.Email, &t.User.Name, &t.DeviceName, &t.CreatedAt,
			&t.LastUsedAt, &t.RevokedAt)
	})
}

// UpdateAccount sets label and/or plan (nil leaves the column unchanged).
func (s *Store) UpdateAccount(ctx context.Context, id int64, label, plan *string) (Account, error) {
	rows, err := s.pool.Query(ctx, `
		UPDATE claude_accounts SET label = COALESCE($2, label), plan = COALESCE($3, plan)
		WHERE id = $1 RETURNING `+accountCols, id, label, plan)
	if err != nil {
		return Account{}, err
	}
	a, err := pgx.CollectExactlyOneRow(rows, scanAccount)
	if errors.Is(err, pgx.ErrNoRows) {
		return Account{}, ErrNotFound
	}
	return a, err
}

// UpsertPrice inserts or replaces the price row for p.Model.
func (s *Store) UpsertPrice(ctx context.Context, p pricing.Price) (pricing.Price, error) {
	var li, l5, l1, lr, lo *float64
	if p.Long != nil {
		li, l5, l1, lr, lo = &p.Long.Input, &p.Long.CacheWrite5m, &p.Long.CacheWrite1h, &p.Long.CacheRead, &p.Long.Output
	}
	rows, err := s.pool.Query(ctx, `
		INSERT INTO model_prices (model, input, cache_write_5m, cache_write_1h, cache_read, output,
		  fast_input, fast_output, long_input, long_cache_write_5m, long_cache_write_1h, long_cache_read, long_output, updated_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, now())
		ON CONFLICT (model) DO UPDATE SET
		  input = EXCLUDED.input, cache_write_5m = EXCLUDED.cache_write_5m, cache_write_1h = EXCLUDED.cache_write_1h,
		  cache_read = EXCLUDED.cache_read, output = EXCLUDED.output, fast_input = EXCLUDED.fast_input,
		  fast_output = EXCLUDED.fast_output, long_input = EXCLUDED.long_input,
		  long_cache_write_5m = EXCLUDED.long_cache_write_5m, long_cache_write_1h = EXCLUDED.long_cache_write_1h,
		  long_cache_read = EXCLUDED.long_cache_read, long_output = EXCLUDED.long_output, updated_at = now()
		RETURNING model, input, cache_write_5m, cache_write_1h, cache_read, output, fast_input, fast_output,
		  long_input, long_cache_write_5m, long_cache_write_1h, long_cache_read, long_output, updated_at`,
		p.Model, p.Base.Input, p.Base.CacheWrite5m, p.Base.CacheWrite1h, p.Base.CacheRead, p.Base.Output,
		p.FastInput, p.FastOutput, li, l5, l1, lr, lo)
	if err != nil {
		return pricing.Price{}, err
	}
	return pgx.CollectExactlyOneRow(rows, scanPrice)
}

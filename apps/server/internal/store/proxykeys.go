package store

import (
	"context"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
)

// Proxy key statuses. A key is requested pending by its owner's CLI; the
// proxy accepts it only once an admin approves it.
const (
	ProxyKeyPending  = "pending"
	ProxyKeyApproved = "approved"
	ProxyKeyRevoked  = "revoked"
)

// ProxyKey is a per-device key for the admin's CLIProxyAPI. Linked reports
// whether the owner still has at least one unrevoked CLI token; the proxy
// sees the key only while it is linked and not revoked.
type ProxyKey struct {
	ID         int64
	User       UserRef
	DeviceName string
	CreatedAt  time.Time
	ApprovedAt *time.Time
	RevokedAt  *time.Time
	Linked     bool
}

// Status is revoked, approved or pending, in that order of precedence.
func (k ProxyKey) Status() string {
	switch {
	case k.RevokedAt != nil:
		return ProxyKeyRevoked
	case k.ApprovedAt != nil:
		return ProxyKeyApproved
	default:
		return ProxyKeyPending
	}
}

// SyncProxyKey is what the proxy syncs: the key's SHA-256, owner, device and
// whether it is approved or pending.
type SyncProxyKey struct {
	Hash       []byte
	Email      string
	DeviceName string
	Approved   bool
}

const linkedExpr = `EXISTS (SELECT 1 FROM api_tokens t WHERE t.user_id = u.id AND t.revoked_at IS NULL)`

const proxyKeyCols = `k.id, u.id, u.email, u.name, k.device_name, k.created_at, k.approved_at, k.revoked_at, ` + linkedExpr

func scanProxyKey(row pgx.CollectableRow) (ProxyKey, error) {
	var k ProxyKey
	return k, row.Scan(&k.ID, &k.User.ID, &k.User.Email, &k.User.Name, &k.DeviceName, &k.CreatedAt,
		&k.ApprovedAt, &k.RevokedAt, &k.Linked)
}

// RequestProxyKey stores a new pending key for userID's device, revoking the
// unrevoked key that device already had, so each user has at most one active
// key per device. ErrNotFound when the user does not exist.
func (s *Store) RequestProxyKey(ctx context.Context, userID int64, hash []byte, device string) (ProxyKey, error) {
	var k ProxyKey
	err := pgx.BeginFunc(ctx, s.pool, func(tx pgx.Tx) error {
		// Serialise requests per user so two concurrent connects for one
		// device cannot both pass the revoke and trip the unique index.
		var one int
		if err := tx.QueryRow(ctx, `SELECT 1 FROM users WHERE id = $1 FOR UPDATE`, userID).Scan(&one); err != nil {
			if errors.Is(err, pgx.ErrNoRows) {
				return ErrNotFound
			}
			return err
		}
		if _, err := tx.Exec(ctx, `
			UPDATE proxy_keys SET revoked_at = now()
			WHERE user_id = $1 AND device_name = $2 AND revoked_at IS NULL`, userID, device); err != nil {
			return err
		}
		rows, err := tx.Query(ctx, `
			WITH k AS (
			  INSERT INTO proxy_keys (user_id, key_hash, device_name) VALUES ($1, $2, $3)
			  RETURNING id, user_id, device_name, created_at, approved_at, revoked_at)
			SELECT `+proxyKeyCols+` FROM k JOIN users u ON u.id = k.user_id`, userID, hash, device)
		if err != nil {
			return err
		}
		k, err = pgx.CollectExactlyOneRow(rows, scanProxyKey)
		return err
	})
	var pgErr *pgconn.PgError
	if errors.As(err, &pgErr) && pgErr.Code == "23503" { // foreign_key_violation: user deleted meanwhile
		return ProxyKey{}, ErrNotFound
	}
	return k, err
}

// ListProxyKeys returns pending keys first, then approved, then revoked;
// pending oldest first (the queue), the others newest first.
func (s *Store) ListProxyKeys(ctx context.Context) ([]ProxyKey, error) {
	rows, err := s.pool.Query(ctx, `
		SELECT `+proxyKeyCols+`
		FROM proxy_keys k JOIN users u ON u.id = k.user_id
		ORDER BY CASE WHEN k.revoked_at IS NOT NULL THEN 2 WHEN k.approved_at IS NOT NULL THEN 1 ELSE 0 END,
		  CASE WHEN k.revoked_at IS NULL AND k.approved_at IS NULL THEN k.created_at END ASC,
		  k.created_at DESC, k.id DESC`)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, scanProxyKey)
}

// ApproveProxyKey approves a pending key; ErrNotFound when it is missing,
// revoked or already approved.
func (s *Store) ApproveProxyKey(ctx context.Context, id int64) error {
	tag, err := s.pool.Exec(ctx, `
		UPDATE proxy_keys SET approved_at = now()
		WHERE id = $1 AND revoked_at IS NULL AND approved_at IS NULL`, id)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

// RevokeProxyKey revokes a pending or approved key; ErrNotFound when it is
// missing or already revoked.
func (s *Store) RevokeProxyKey(ctx context.Context, id int64) error {
	tag, err := s.pool.Exec(ctx,
		`UPDATE proxy_keys SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL`, id)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

// SyncProxyKeys returns the keys the proxy should know: not revoked, and
// whose owner still has an unrevoked CLI token. Approved keys are allowed;
// pending ones get 403.
func (s *Store) SyncProxyKeys(ctx context.Context) ([]SyncProxyKey, error) {
	rows, err := s.pool.Query(ctx, `
		SELECT k.key_hash, u.email, k.device_name, k.approved_at IS NOT NULL
		FROM proxy_keys k JOIN users u ON u.id = k.user_id
		WHERE k.revoked_at IS NULL AND `+linkedExpr+`
		ORDER BY k.id`)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (SyncProxyKey, error) {
		var k SyncProxyKey
		return k, row.Scan(&k.Hash, &k.Email, &k.DeviceName, &k.Approved)
	})
}

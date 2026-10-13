package store

import (
	"context"
	"errors"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
)

type User struct {
	ID        int64
	GoogleSub string
	Email     string
	Name      string
	CreatedAt time.Time
}

const userCols = "id, google_sub, email, name, created_at"

func scanUser(row pgx.Row) (User, error) {
	var u User
	err := row.Scan(&u.ID, &u.GoogleSub, &u.Email, &u.Name, &u.CreatedAt)
	if errors.Is(err, pgx.ErrNoRows) {
		return u, ErrNotFound
	}
	return u, err
}

func (s *Store) UpsertUser(ctx context.Context, sub, email, name string) (User, error) {
	return scanUser(s.pool.QueryRow(ctx, `
		INSERT INTO users (google_sub, email, name) VALUES ($1, $2, $3)
		ON CONFLICT (google_sub) DO UPDATE SET email = EXCLUDED.email, name = EXCLUDED.name
		RETURNING `+userCols, sub, strings.ToLower(email), name))
}

func (s *Store) UserByID(ctx context.Context, id int64) (User, error) {
	return scanUser(s.pool.QueryRow(ctx, `SELECT `+userCols+` FROM users WHERE id = $1`, id))
}

// UserByToken resolves a non-revoked token hash, touching last_used_at.
func (s *Store) UserByToken(ctx context.Context, hash []byte) (User, int64, error) {
	var u User
	var tokenID int64
	err := s.pool.QueryRow(ctx, `
		UPDATE api_tokens t SET last_used_at = now()
		FROM users u
		WHERE t.token_hash = $1 AND t.revoked_at IS NULL AND u.id = t.user_id
		RETURNING t.id, u.id, u.google_sub, u.email, u.name, u.created_at`, hash).
		Scan(&tokenID, &u.ID, &u.GoogleSub, &u.Email, &u.Name, &u.CreatedAt)
	if errors.Is(err, pgx.ErrNoRows) {
		return User{}, 0, ErrNotFound
	}
	return u, tokenID, err
}

func (s *Store) CreateToken(ctx context.Context, userID int64, hash []byte, device string) (int64, error) {
	var id int64
	err := s.pool.QueryRow(ctx,
		`INSERT INTO api_tokens (user_id, token_hash, device_name) VALUES ($1, $2, $3) RETURNING id`,
		userID, hash, device).Scan(&id)
	return id, err
}

func (s *Store) RevokeToken(ctx context.Context, id int64) error {
	tag, err := s.pool.Exec(ctx,
		`UPDATE api_tokens SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL`, id)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

type AuthRequest struct {
	State         string
	Mode          string // loopback | paste | web
	CLIState      string
	CLIPort       int
	CodeChallenge string
	ExpiresAt     time.Time
}

func (s *Store) CreateAuthRequest(ctx context.Context, r AuthRequest) error {
	_, err := s.pool.Exec(ctx, `
		INSERT INTO auth_requests (state, mode, cli_state, cli_port, code_challenge, expires_at)
		VALUES ($1, $2, $3, $4, $5, $6)`,
		r.State, r.Mode, r.CLIState, r.CLIPort, r.CodeChallenge, r.ExpiresAt)
	return err
}

// TakeAuthRequest deletes and returns an unexpired auth request (single use).
func (s *Store) TakeAuthRequest(ctx context.Context, state string) (AuthRequest, error) {
	var r AuthRequest
	err := s.pool.QueryRow(ctx, `
		DELETE FROM auth_requests WHERE state = $1 AND expires_at > now()
		RETURNING state, mode, cli_state, cli_port, code_challenge, expires_at`, state).
		Scan(&r.State, &r.Mode, &r.CLIState, &r.CLIPort, &r.CodeChallenge, &r.ExpiresAt)
	if errors.Is(err, pgx.ErrNoRows) {
		return r, ErrNotFound
	}
	return r, err
}

func (s *Store) CreateCLICode(ctx context.Context, hash []byte, userID int64, challenge string, exp time.Time) error {
	_, err := s.pool.Exec(ctx, `
		INSERT INTO cli_auth_codes (code_hash, user_id, code_challenge, expires_at)
		VALUES ($1, $2, $3, $4)`, hash, userID, challenge, exp)
	return err
}

// ConsumeCLICode marks an unexpired, unused code as used and returns its owner.
// The code is burned even if the caller's PKCE check later fails.
func (s *Store) ConsumeCLICode(ctx context.Context, hash []byte) (int64, string, error) {
	var userID int64
	var challenge string
	err := s.pool.QueryRow(ctx, `
		UPDATE cli_auth_codes SET used_at = now()
		WHERE code_hash = $1 AND used_at IS NULL AND expires_at > now()
		RETURNING user_id, code_challenge`, hash).Scan(&userID, &challenge)
	if errors.Is(err, pgx.ErrNoRows) {
		return 0, "", ErrNotFound
	}
	return userID, challenge, err
}

func (s *Store) PurgeExpired(ctx context.Context) error {
	_, err := s.pool.Exec(ctx, `
		DELETE FROM auth_requests WHERE expires_at < now();
		DELETE FROM cli_auth_codes WHERE expires_at < now() - interval '1 day';`)
	return err
}

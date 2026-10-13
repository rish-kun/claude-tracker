// Package db opens the Postgres pool and runs embedded migrations.
package db

import (
	"context"
	"fmt"
	"net/url"
	"strings"

	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/jackc/pgx/v5/stdlib"
	"github.com/pressly/goose/v3"
	"github.com/pressly/goose/v3/database"
	"github.com/pressly/goose/v3/lock"

	"cctrack/server/migrations"
)

// Open creates a small pool suitable for serverless instances.
func Open(ctx context.Context, url string) (*pgxpool.Pool, error) {
	cfg, err := pgxpool.ParseConfig(url)
	if err != nil {
		return nil, fmt.Errorf("parse DATABASE_URL: %w", err)
	}
	cfg.MaxConns = 4
	return pgxpool.NewWithConfig(ctx, cfg)
}

// Migrate applies all pending migrations under a Postgres advisory lock.
func Migrate(ctx context.Context, pool *pgxpool.Pool) error {
	sqlDB := stdlib.OpenDBFromPool(pool)
	defer sqlDB.Close()
	locker, err := lock.NewPostgresSessionLocker()
	if err != nil {
		return err
	}
	provider, err := goose.NewProvider(database.DialectPostgres, sqlDB, migrations.FS,
		goose.WithSessionLocker(locker))
	if err != nil {
		return err
	}
	_, err = provider.Up(ctx)
	return err
}

// MigrateURL opens a temporary pool for url and migrates it. It refuses a
// Neon pooled host (see CheckMigrateURL).
func MigrateURL(ctx context.Context, url string) error {
	if err := CheckMigrateURL(url); err != nil {
		return err
	}
	pool, err := Open(ctx, url)
	if err != nil {
		return err
	}
	defer pool.Close()
	return Migrate(ctx, pool)
}

// CheckMigrateURL fails when raw targets a Neon pooled hostname ("-pooler"):
// goose's session advisory lock does not work through a transaction pooler.
func CheckMigrateURL(raw string) error {
	for _, host := range dbHosts(raw) {
		if strings.Contains(strings.ToLower(host), "-pooler") {
			return fmt.Errorf("migration database host %q is a Neon pooled endpoint; migrations need a direct "+
				"connection: set DATABASE_URL_UNPOOLED to the unpooled connection string (host without -pooler)", host)
		}
	}
	return nil
}

// dbHosts extracts the host(s) from a postgres:// URL or a keyword/value DSN.
func dbHosts(raw string) []string {
	raw = strings.TrimSpace(raw)
	if strings.HasPrefix(raw, "postgres://") || strings.HasPrefix(raw, "postgresql://") {
		u, err := url.Parse(raw)
		if err != nil {
			return []string{raw}
		}
		hosts := []string{u.Host}
		if h := u.Query().Get("host"); h != "" {
			hosts = append(hosts, h)
		}
		return hosts
	}
	var hosts []string
	for _, f := range strings.Fields(raw) {
		if v, ok := strings.CutPrefix(f, "host="); ok {
			hosts = append(hosts, strings.Trim(v, "'"))
		}
	}
	return hosts
}

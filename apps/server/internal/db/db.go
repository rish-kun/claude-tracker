// Package db opens the Postgres pool and runs embedded migrations.
package db

import (
	"context"
	"fmt"

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

// MigrateURL opens a temporary pool for url and migrates it.
func MigrateURL(ctx context.Context, url string) error {
	pool, err := Open(ctx, url)
	if err != nil {
		return err
	}
	defer pool.Close()
	return Migrate(ctx, pool)
}

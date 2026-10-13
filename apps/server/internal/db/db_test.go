package db_test

import (
	"context"
	"testing"

	"cctrack/server/internal/db"
	"cctrack/server/internal/testdb"
)

func TestMigrateSeedsPricesAndIsIdempotent(t *testing.T) {
	pool := testdb.New(t)
	ctx := context.Background()
	var n int
	if err := pool.QueryRow(ctx, "SELECT count(*) FROM model_prices").Scan(&n); err != nil {
		t.Fatal(err)
	}
	if n != 19 {
		t.Fatalf("seeded %d prices, want 19", n)
	}
	if err := db.Migrate(ctx, pool); err != nil {
		t.Fatalf("second migrate: %v", err)
	}
}

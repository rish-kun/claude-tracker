package migrations

import (
	"regexp"
	"strconv"
	"strings"
	"testing"
)

// seedRow matches one VALUES tuple of 00002_seed_prices.sql:
// ('model', input, cache_write_5m, cache_write_1h, cache_read, output, ...).
var seedRow = regexp.MustCompile(`^\s*\('([a-z0-9-]+)',\s*([^)]*)\)\s*[,;]\s*$`)

// parseSeed returns model -> its comma-separated numeric/NULL columns.
func parseSeed(t *testing.T) map[string][]string {
	t.Helper()
	b, err := FS.ReadFile("00002_seed_prices.sql")
	if err != nil {
		t.Fatal(err)
	}
	up, _, ok := strings.Cut(string(b), "-- +goose Down")
	if !ok {
		t.Fatal("seed migration has no goose Down section")
	}
	rows := map[string][]string{}
	for _, line := range strings.Split(up, "\n") {
		m := seedRow.FindStringSubmatch(line)
		if m == nil {
			continue
		}
		if _, dup := rows[m[1]]; dup {
			t.Fatalf("duplicate seed row for %s", m[1])
		}
		cols := strings.Split(m[2], ",")
		for i := range cols {
			cols[i] = strings.TrimSpace(cols[i])
		}
		rows[m[1]] = cols
	}
	return rows
}

func TestSeedPrices(t *testing.T) {
	rows := parseSeed(t)
	if len(rows) != 19 {
		t.Fatalf("seed rows = %d, want 19", len(rows))
	}
	for model, cols := range rows {
		// 12 columns after model: 5 base, 2 fast, 5 long.
		if len(cols) != 12 {
			t.Fatalf("%s: %d price columns, want 12", model, len(cols))
		}
		for i, c := range cols[:5] {
			v, err := strconv.ParseFloat(c, 64)
			if err != nil || v <= 0 {
				t.Errorf("%s: base column %d = %q, want a positive number", model, i, c)
			}
		}
	}
	const cacheRead = 3 // input, 5m, 1h, cache_read, output
	if got := rows["claude-sonnet-5-5"][cacheRead]; got != "0.20" {
		t.Errorf("claude-sonnet-5-5 cache_read = %s, want 0.20", got)
	}
}

package httpapi

import (
	"strings"
	"testing"
	"time"
	_ "time/tzdata" // tz tests must not depend on the host's zoneinfo
)

func TestParseRange(t *testing.T) {
	from := time.Date(2026, 10, 1, 0, 0, 0, 0, time.UTC)
	tz := func(s string) *string { return &s }

	f, err := parseRange(from, from.Add(time.Hour), nil)
	if err != nil || f.TZ != "UTC" || !f.From.Equal(from) || !f.To.Equal(from.Add(time.Hour)) {
		t.Fatalf("default = %+v, %v", f, err)
	}
	if f, err := parseRange(from, from.Add(time.Hour), tz("")); err != nil || f.TZ != "UTC" {
		t.Fatalf("empty tz = %+v, %v", f, err)
	}
	if f, err := parseRange(from, from.Add(time.Hour), tz("Asia/Kolkata")); err != nil || f.TZ != "Asia/Kolkata" {
		t.Fatalf("kolkata = %+v, %v", f, err)
	}
	if _, err := parseRange(from, from.Add(400*24*time.Hour), nil); err != nil {
		t.Fatalf("exactly 400 days: %v", err)
	}
	for _, c := range []struct {
		name    string
		to      time.Time
		tz      *string
		wantErr string
	}{
		{"unknown tz", from.Add(time.Hour), tz("Mars/Olympus"), "unknown tz"},
		// Go resolves "Local" to the server's zone; Postgres does not know it.
		{"Local tz", from.Add(time.Hour), tz("Local"), "unknown tz"},
		{"empty range", from, nil, "after"},
		{"reversed range", from.Add(-time.Hour), nil, "after"},
		{"too long", from.Add(400*24*time.Hour + time.Second), nil, "400 days"},
	} {
		t.Run(c.name, func(t *testing.T) {
			_, err := parseRange(from, c.to, c.tz)
			if err == nil || !strings.Contains(err.Error(), c.wantErr) {
				t.Fatalf("err = %v, want %q", err, c.wantErr)
			}
		})
	}
}

func TestPageParams(t *testing.T) {
	n := func(i int) *int { return &i }
	for _, c := range []struct {
		limit, offset *int
		wantL, wantO  int
	}{
		{nil, nil, 50, 0},
		{n(10), n(20), 10, 20},
		{n(0), n(-5), 50, 0},
		{n(1000), nil, 200, 0},
	} {
		if l, o := pageParams(c.limit, c.offset); l != c.wantL || o != c.wantO {
			t.Errorf("pageParams(%v, %v) = %d, %d; want %d, %d", c.limit, c.offset, l, o, c.wantL, c.wantO)
		}
	}
}

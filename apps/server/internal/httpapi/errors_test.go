package httpapi

import (
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"slices"
	"testing"

	"github.com/jackc/pgx/v5/pgconn"

	"cctrack/server/internal/pricing"
	"cctrack/server/internal/report"
	"cctrack/server/internal/store"
)

func TestIsInvalidParam(t *testing.T) {
	pgErr := &pgconn.PgError{Code: "22023", Message: `time zone "Foo/Bar" not recognized`}
	for _, c := range []struct {
		name string
		err  error
		want bool
	}{
		{"22023", pgErr, true},
		{"wrapped 22023", fmt.Errorf("aggregate: %w", pgErr), true},
		{"other SQLSTATE", &pgconn.PgError{Code: "42P01"}, false},
		{"not a PgError", errors.New("boom"), false},
		{"nil", nil, false},
	} {
		if got := isInvalidParam(c.err); got != c.want {
			t.Errorf("%s: isInvalidParam = %v, want %v", c.name, got, c.want)
		}
	}
}

func TestAggregateFailedMapsUnknownTZTo400(t *testing.T) {
	s := &Server{}
	rec := httptest.NewRecorder()
	s.aggregateFailed(rec, &pgconn.PgError{Code: "22023"}, "Foo/Bar")
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("status %d, want 400", rec.Code)
	}
	if msg := decode[map[string]string](t, rec)["error"]; msg != "unknown tz: Foo/Bar" {
		t.Fatalf("error = %q", msg)
	}
	rec = httptest.NewRecorder()
	s.aggregateFailed(rec, errors.New("connection refused"), "UTC")
	if rec.Code != http.StatusInternalServerError {
		t.Fatalf("status %d, want 500", rec.Code)
	}
}

func TestByCostDescTieBreaksOnKey(t *testing.T) {
	tbl := pricing.NewTable([]pricing.Price{{Model: "claude-opus-4-5", Base: pricing.Rates{Input: 5}}})
	row := func(input int64) store.AggRow {
		return store.AggRow{Model: "claude-opus-4-5", Counts: pricing.Counts{Input: input}}
	}

	s := report.NewGroup[string]()
	for _, k := range []string{"c", "a", "big", "b"} {
		in := int64(1000)
		if k == "big" {
			in = 5000
		}
		s.Add(k, tbl, row(in))
	}
	if got, want := byCostDesc(s), []string{"big", "a", "b", "c"}; !slices.Equal(got, want) {
		t.Fatalf("string keys = %v, want %v", got, want)
	}

	// int64 keys tie-break numerically, not lexically ("100" < "9").
	n := report.NewGroup[int64]()
	for _, k := range []int64{100, 9, 10} {
		n.Add(k, tbl, row(1000))
	}
	if got, want := byCostDesc(n), []int64{9, 10, 100}; !slices.Equal(got, want) {
		t.Fatalf("int64 keys = %v, want %v", got, want)
	}
}

package report

import (
	"math"
	"testing"

	"cctrack/server/internal/pricing"
	"cctrack/server/internal/store"
)

func tbl() *pricing.Table {
	return pricing.NewTable([]pricing.Price{
		{Model: "claude-opus-4-5", Base: pricing.Rates{Input: 5, Output: 25}},
	})
}

func TestBucketAddsCostAndUnpriced(t *testing.T) {
	var b Bucket
	b.Add(tbl(), store.AggRow{Model: "claude-opus-4-5-20251101", Speed: "standard",
		Counts: pricing.Counts{Input: 1_000_000, Output: 100_000}})
	b.Add(tbl(), store.AggRow{Model: "mystery-model", Speed: "standard",
		Counts: pricing.Counts{Input: 7, Output: 3}})
	tot := b.Totals()
	if math.Abs(tot.CostUsd-7.5) > 1e-9 {
		t.Fatalf("cost = %v, want 7.5", tot.CostUsd)
	}
	if tot.UnpricedTokens != 10 || tot.TotalTokens != 1_100_010 || tot.Input != 1_000_007 {
		t.Fatalf("totals = %+v", tot)
	}
}

func TestGroupKeepsInsertionOrderAndSorts(t *testing.T) {
	g := NewGroup[string]()
	g.Add("b", tbl(), store.AggRow{Model: "claude-opus-4-5", Counts: pricing.Counts{Input: 1_000_000}})
	g.Add("a", tbl(), store.AggRow{Model: "claude-opus-4-5", Counts: pricing.Counts{Input: 2_000_000}})
	g.Add("b", tbl(), store.AggRow{Model: "claude-opus-4-5", Counts: pricing.Counts{Input: 2_000_000}})
	if k := g.Keys(); len(k) != 2 || k[0] != "b" {
		t.Fatalf("Keys = %v", k)
	}
	if g.Get("b").Counts.Input != 3_000_000 {
		t.Fatalf("b = %+v", g.Get("b"))
	}
	byCost := g.SortedKeys(func(x, y string) bool { return g.Get(x).Cost > g.Get(y).Cost })
	if byCost[0] != "b" {
		t.Fatalf("sorted = %v", byCost)
	}
	if g.Get("zzz").Counts.Total() != 0 {
		t.Fatal("missing key should be zero")
	}
}

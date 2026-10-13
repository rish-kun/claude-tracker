package pricing

import (
	"math"
	"testing"
)

func TestNormalize(t *testing.T) {
	cases := map[string]string{
		"claude-opus-4-5-20251101":                     "claude-opus-4-5",
		"claude-sonnet-4-5-20250929[1m]":               "claude-sonnet-4-5",
		"us.anthropic.claude-sonnet-4-5-20250929-v1:0": "claude-sonnet-4-5",
		"anthropic.claude-3-5-haiku-20241022-v1:0":     "claude-3-5-haiku",
		"claude-opus-4-1@20250805":                     "claude-opus-4-1",
		"claude-opus-5-5":                              "claude-opus-5-5",
		" Claude-Haiku-4-5 ":                           "claude-haiku-4-5",
		"global.anthropic.claude-opus-4-6-v1":          "claude-opus-4-6",
	}
	for in, want := range cases {
		if got := Normalize(in); got != want {
			t.Errorf("Normalize(%q) = %q, want %q", in, got, want)
		}
	}
}

func f(v float64) *float64 { return &v }

func table() *Table {
	return NewTable([]Price{
		{Model: "claude-opus-4-5", Base: Rates{Input: 5, CacheWrite5m: 6.25, CacheWrite1h: 10, CacheRead: 0.5, Output: 25}},
		{Model: "claude-opus-5-5", Base: Rates{Input: 4, CacheWrite5m: 5, CacheWrite1h: 8, CacheRead: 0.2, Output: 20},
			FastInput: f(8), FastOutput: f(40)},
		{Model: "claude-haiku-5-5", Base: Rates{Input: 0.1, CacheWrite5m: 0.125, CacheWrite1h: 0.2, CacheRead: 0.01, Output: 0.5},
			Long: &Rates{Input: 0.5, CacheWrite5m: 0.625, CacheWrite1h: 1, CacheRead: 0.05, Output: 2.5}},
	})
}

func near(a, b float64) bool { return math.Abs(a-b) < 1e-9 }

func TestCostStandard(t *testing.T) {
	c := Counts{Input: 1_000_000, Output: 200_000, CacheWrite5m: 100_000, CacheWrite1h: 50_000, CacheRead: 2_000_000}
	usd, ok := table().Cost("claude-opus-4-5-20251101", "standard", false, c)
	// 5 + 0.2*25 + 0.1*6.25 + 0.05*10 + 2*0.5 = 5 + 5 + 0.625 + 0.5 + 1 = 12.125
	if !ok || !near(usd, 12.125) {
		t.Fatalf("got %v %v, want 12.125 true", usd, ok)
	}
}

func TestCostFastScalesInputAndCacheAndUsesFastOutput(t *testing.T) {
	c := Counts{Input: 1_000_000, Output: 1_000_000, CacheRead: 1_000_000}
	usd, ok := table().Cost("claude-opus-5-5", "fast", false, c)
	// input 8 + output 40 + cache read 0.2*2 = 48.4
	if !ok || !near(usd, 48.4) {
		t.Fatalf("got %v, want 48.4", usd)
	}
}

func TestCostFastWithoutFastRatesUsesBase(t *testing.T) {
	usd, _ := table().Cost("claude-opus-4-5", "fast", false, Counts{Input: 1_000_000})
	if !near(usd, 5) {
		t.Fatalf("got %v, want 5", usd)
	}
}

func TestCostLongContextTier(t *testing.T) {
	c := Counts{Input: 1_000_000, Output: 1_000_000}
	short, _ := table().Cost("claude-haiku-5-5", "standard", false, c)
	long, _ := table().Cost("claude-haiku-5-5", "standard", true, c)
	if !near(short, 0.6) || !near(long, 3.0) {
		t.Fatalf("short=%v long=%v, want 0.6 and 3.0", short, long)
	}
	// Long flag on a model without long rates falls back to base.
	base, _ := table().Cost("claude-opus-4-5", "standard", true, Counts{Input: 1_000_000})
	if !near(base, 5) {
		t.Fatalf("got %v, want 5", base)
	}
}

func TestCostUnknownModel(t *testing.T) {
	usd, ok := table().Cost("gpt-9", "standard", false, Counts{Input: 10})
	if ok || usd != 0 {
		t.Fatalf("got %v %v, want 0 false", usd, ok)
	}
}

func TestCounts(t *testing.T) {
	a := Counts{Input: 1, Output: 2, CacheWrite5m: 3, CacheWrite1h: 4, CacheRead: 5}
	a.Add(a)
	if a.Total() != 30 {
		t.Fatalf("Total = %d, want 30", a.Total())
	}
}

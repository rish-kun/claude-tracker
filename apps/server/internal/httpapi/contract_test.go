package httpapi

import (
	"net/http"
	"testing"

	"cctrack/server/internal/api"
	"cctrack/server/internal/pricing"
	"cctrack/server/internal/report"
	"cctrack/server/internal/store"
)

// TestSeriesCarriesUnpricedTokens runs without a database: charts need to
// know which points include tokens that cost_usd leaves out.
func TestSeriesCarriesUnpricedTokens(t *testing.T) {
	table := pricing.NewTable([]pricing.Price{{Model: "claude-opus-4-5",
		Base: pricing.Rates{Input: 5, CacheWrite5m: 6.25, CacheWrite1h: 10, CacheRead: 0.5, Output: 25}}})
	g := report.NewGroup[dayKey]()
	priced := store.AggRow{Model: "claude-opus-4-5", Speed: "standard", Counts: pricing.Counts{Input: 1_000_000}}
	unpriced := store.AggRow{Model: "mystery-model", Speed: "standard", Counts: pricing.Counts{Input: 40, Output: 2}}
	g.Add(dayKey{"2026-10-01", "a"}, table, priced)
	g.Add(dayKey{"2026-10-01", "a"}, table, unpriced)
	g.Add(dayKey{"2026-10-02", "a"}, table, priced)

	got := series(g, func(k string) string { return k })
	if len(got) != 2 {
		t.Fatalf("points = %+v", got)
	}
	if p := got[0]; p.UnpricedTokens != 42 || p.TotalTokens != 1_000_042 || p.CostUsd != 5 {
		t.Fatalf("mixed point = %+v, want unpriced 42 of 1000042 tokens, cost 5", p)
	}
	if p := got[1]; p.UnpricedTokens != 0 || p.CostUsd != 5 {
		t.Fatalf("priced point = %+v", p)
	}
}

// TestAdminPutPriceCreateConflict: ?create=true must never overwrite an
// existing (normalised) model's price.
func TestAdminPutPriceCreateConflict(t *testing.T) {
	h := newHarness(t)
	admin := h.loginWeb("admin@example.com")
	rates := map[string]float64{"input": 1, "cache_write_5m": 2, "cache_write_1h": 3, "cache_read": 4, "output": 5}

	// claude-opus-4-5 is seeded; a dated alias normalises onto it.
	rec := h.do("PUT", "/api/admin/prices/claude-opus-4-5-20251101?create=true", rates, withCookie(admin), withCSRF())
	if rec.Code != http.StatusConflict {
		t.Fatalf("create existing: %d %s, want 409", rec.Code, rec.Body)
	}
	if msg := decode[map[string]string](t, rec)["error"]; msg != "model already priced: claude-opus-4-5" {
		t.Fatalf("error = %q", msg)
	}
	for _, p := range adminGet[[]api.ModelPrice](h, "/api/admin/prices", admin) {
		if p.Model == "claude-opus-4-5" && p.Rates.Input == 1 {
			t.Fatal("conflicting create overwrote the existing price")
		}
	}

	rec = h.do("PUT", "/api/admin/prices/brand-new-model?create=true", rates, withCookie(admin), withCSRF())
	if rec.Code != http.StatusOK {
		t.Fatalf("create new: %d %s", rec.Code, rec.Body)
	}
	if p := decode[api.ModelPrice](t, rec); p.Model != "brand-new-model" || p.Rates.Output != 5 {
		t.Fatalf("created %+v", p)
	}
	if rec := h.do("PUT", "/api/admin/prices/brand-new-model?create=true", rates, withCookie(admin), withCSRF()); rec.Code != http.StatusConflict {
		t.Fatalf("second create: %d, want 409", rec.Code)
	}
	// Without create (or create=false) the PUT still upserts.
	rates["output"] = 9
	for _, path := range []string{"/api/admin/prices/brand-new-model", "/api/admin/prices/brand-new-model?create=false"} {
		rec := h.do("PUT", path, rates, withCookie(admin), withCSRF())
		if rec.Code != http.StatusOK || decode[api.ModelPrice](t, rec).Rates.Output != 9 {
			t.Fatalf("PUT %s: %d %s", path, rec.Code, rec.Body)
		}
	}
}

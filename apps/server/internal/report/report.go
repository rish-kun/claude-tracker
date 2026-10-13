// Package report folds aggregated token rows into priced totals.
package report

import (
	"sort"

	"cctrack/server/internal/api"
	"cctrack/server/internal/pricing"
	"cctrack/server/internal/store"
)

type Bucket struct {
	Counts   pricing.Counts
	Cost     float64
	Unpriced int64
}

func (b *Bucket) Add(t *pricing.Table, r store.AggRow) {
	b.Counts.Add(r.Counts)
	if usd, ok := t.Cost(r.Model, r.Speed, r.LongCtx, r.Counts); ok {
		b.Cost += usd
	} else {
		b.Unpriced += r.Counts.Total()
	}
}

func (b Bucket) Totals() api.Totals {
	return api.Totals{
		Input:          b.Counts.Input,
		Output:         b.Counts.Output,
		CacheWrite5m:   b.Counts.CacheWrite5m,
		CacheWrite1h:   b.Counts.CacheWrite1h,
		CacheRead:      b.Counts.CacheRead,
		TotalTokens:    b.Counts.Total(),
		CostUsd:        b.Cost,
		UnpricedTokens: b.Unpriced,
	}
}

// Group accumulates buckets by key, remembering first-seen order.
type Group[K comparable] struct {
	m     map[K]*Bucket
	order []K
}

func NewGroup[K comparable]() *Group[K] { return &Group[K]{m: map[K]*Bucket{}} }

func (g *Group[K]) Add(k K, t *pricing.Table, r store.AggRow) {
	b, ok := g.m[k]
	if !ok {
		b = &Bucket{}
		g.m[k] = b
		g.order = append(g.order, k)
	}
	b.Add(t, r)
}

func (g *Group[K]) Get(k K) Bucket {
	if b, ok := g.m[k]; ok {
		return *b
	}
	return Bucket{}
}

func (g *Group[K]) Keys() []K { return append([]K(nil), g.order...) }

func (g *Group[K]) SortedKeys(less func(a, b K) bool) []K {
	keys := g.Keys()
	sort.SliceStable(keys, func(i, j int) bool { return less(keys[i], keys[j]) })
	return keys
}

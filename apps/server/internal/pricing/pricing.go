// Package pricing estimates API-equivalent cost from token counts.
package pricing

import "time"

// LongContextThreshold is the prompt size (input + cache tokens) above which
// long-context rates apply, for models that define them.
const LongContextThreshold = 100_000

const perMTok = 1_000_000.0

type Counts struct {
	Input, Output, CacheWrite5m, CacheWrite1h, CacheRead int64
}

func (c Counts) Total() int64 {
	return c.Input + c.Output + c.CacheWrite5m + c.CacheWrite1h + c.CacheRead
}

func (c *Counts) Add(o Counts) {
	c.Input += o.Input
	c.Output += o.Output
	c.CacheWrite5m += o.CacheWrite5m
	c.CacheWrite1h += o.CacheWrite1h
	c.CacheRead += o.CacheRead
}

// Rates are USD per million tokens.
type Rates struct {
	Input, CacheWrite5m, CacheWrite1h, CacheRead, Output float64
}

type Price struct {
	Model      string
	Base       Rates
	FastInput  *float64
	FastOutput *float64
	Long       *Rates
	UpdatedAt  time.Time
}

type Table struct {
	prices map[string]Price
}

func NewTable(prices []Price) *Table {
	t := &Table{prices: make(map[string]Price, len(prices))}
	for _, p := range prices {
		t.prices[Normalize(p.Model)] = p
	}
	return t
}

// Cost returns the estimated USD cost. ok is false when the model has no price.
// Fast mode scales input and cache rates by FastInput/Base.Input and uses
// FastOutput for output tokens.
func (t *Table) Cost(model, speed string, longCtx bool, c Counts) (float64, bool) {
	p, ok := t.prices[Normalize(model)]
	if !ok {
		return 0, false
	}
	r := p.Base
	if longCtx && p.Long != nil {
		r = *p.Long
	}
	if speed == "fast" && p.FastInput != nil && p.FastOutput != nil && p.Base.Input > 0 {
		m := *p.FastInput / p.Base.Input
		r = Rates{
			Input:        r.Input * m,
			CacheWrite5m: r.CacheWrite5m * m,
			CacheWrite1h: r.CacheWrite1h * m,
			CacheRead:    r.CacheRead * m,
			Output:       *p.FastOutput,
		}
	}
	usd := float64(c.Input)*r.Input +
		float64(c.CacheWrite5m)*r.CacheWrite5m +
		float64(c.CacheWrite1h)*r.CacheWrite1h +
		float64(c.CacheRead)*r.CacheRead +
		float64(c.Output)*r.Output
	return usd / perMTok, true
}

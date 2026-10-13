import { describe, expect, it } from 'vitest'
import { markUnpriced, pivot } from './pivot'

const p = (date: string, key: string, cost: number) => ({ date, key, label: key.toUpperCase(), total_tokens: cost * 10, cost_usd: cost })

describe('pivot', () => {
  it('fills missing cells with zero and orders keys by total', () => {
    const out = pivot([p('2026-10-01', 'a', 1), p('2026-10-02', 'b', 5), p('2026-10-02', 'a', 1)], ['2026-10-01', '2026-10-02', '2026-10-03'], 'cost_usd')
    expect(out.keys).toEqual([{ key: 'b', label: 'B' }, { key: 'a', label: 'A' }])
    expect(out.rows).toEqual([
      { date: '2026-10-01', b: 0, a: 1 },
      { date: '2026-10-02', b: 5, a: 1 },
      { date: '2026-10-03', b: 0, a: 0 },
    ])
  })
  it('can pivot tokens and includes dates outside the list', () => {
    const out = pivot([p('2026-09-30', 'a', 2)], ['2026-10-01'], 'total_tokens')
    expect(out.rows).toEqual([{ date: '2026-09-30', a: 20 }, { date: '2026-10-01', a: 0 }])
  })
  it('relabels fully-unpriced series and leaves the rest alone', () => {
    const base = pivot([p('2026-10-01', 'a', 1), p('2026-10-01', 'x', 0)], ['2026-10-01'], 'cost_usd')
    const row = (key: string, cost_usd: number, unpriced_tokens: number) => ({
      key,
      label: key,
      totals: { input: 0, output: 0, cache_write_5m: 0, cache_write_1h: 0, cache_read: 0, total_tokens: 100, cost_usd, unpriced_tokens },
    })
    const out = markUnpriced(base, [row('a', 1, 50), row('x', 0, 100)])
    expect(out.keys).toEqual([{ key: 'a', label: 'A' }, { key: 'x', label: 'X (unpriced)' }])
    expect(out.rows).toBe(base.rows)
    expect(markUnpriced(base, [row('a', 1, 0)])).toBe(base)
  })
})

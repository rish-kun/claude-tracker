import { describe, expect, it } from 'vitest'
import { chartSummary, foldOther, markUnpriced, OTHER_KEY, pivot, unpricedAt } from './pivot'

const p = (date: string, key: string, cost: number, unpriced_tokens = 0) => ({
  date,
  key,
  label: key.toUpperCase(),
  total_tokens: cost * 10 + unpriced_tokens,
  cost_usd: cost,
  unpriced_tokens,
})

describe('pivot', () => {
  it('fills missing cells with zero and orders keys by total', () => {
    const out = pivot([p('2026-10-01', 'a', 1), p('2026-10-02', 'b', 5), p('2026-10-02', 'a', 1)], ['2026-10-01', '2026-10-02', '2026-10-03'], 'cost_usd')
    expect(out.keys).toEqual([{ key: 'b', label: 'B' }, { key: 'a', label: 'A' }])
    expect(out.rows).toEqual([
      { date: '2026-10-01', b: 0, a: 1 },
      { date: '2026-10-02', b: 5, a: 1 },
      { date: '2026-10-03', b: 0, a: 0 },
    ])
    expect(out.unpriced).toEqual({})
  })
  it('can pivot tokens and includes dates outside the list', () => {
    const out = pivot([p('2026-09-30', 'a', 2)], ['2026-10-01'], 'total_tokens')
    expect(out.rows).toEqual([{ date: '2026-09-30', a: 20 }, { date: '2026-10-01', a: 0 }])
  })
  it('carries per-cell unpriced tokens for cost pivots, summing duplicate points', () => {
    const pts = [p('2026-10-01', 'a', 1, 500), p('2026-10-01', 'a', 0, 250), p('2026-10-01', 'x', 0, 100), p('2026-10-02', 'a', 2)]
    const out = pivot(pts, ['2026-10-01', '2026-10-02'], 'cost_usd')
    expect(out.unpriced).toEqual({ '2026-10-01': { a: 750, x: 100 } })
    expect(out.rows[0]).toEqual({ date: '2026-10-01', a: 1, x: 0 })
    expect(unpricedAt(out, '2026-10-01', 'a')).toBe(750)
    expect(unpricedAt(out, '2026-10-02', 'a')).toBe(0)
    expect(unpricedAt(out, '2026-10-03', 'nope')).toBe(0)
    // Token pivots already count unpriced tokens in the plotted value.
    expect(pivot(pts, [], 'total_tokens').unpriced).toEqual({})
  })
})

describe('markUnpriced', () => {
  it('relabels only series that are unpriced across the whole range', () => {
    const base = pivot(
      [p('2026-10-01', 'a', 1, 50), p('2026-10-01', 'x', 0, 100), p('2026-10-02', 'x', 0, 10), p('2026-10-01', 'z', 0)],
      ['2026-10-01', '2026-10-02'],
      'cost_usd',
    )
    const out = markUnpriced(base)
    expect(out.keys).toEqual([
      { key: 'a', label: 'A' },
      { key: 'x', label: 'X (unpriced)' },
      { key: 'z', label: 'Z' },
    ])
    expect(out.rows).toBe(base.rows)
    expect(out.unpriced).toBe(base.unpriced)
  })
  it('leaves a partly-priced series and fully-priced pivots alone', () => {
    const partly = pivot([p('2026-10-01', 'x', 0, 100), p('2026-10-02', 'x', 3)], [], 'cost_usd')
    expect(markUnpriced(partly)).toBe(partly)
    const priced = pivot([p('2026-10-01', 'a', 1)], [], 'cost_usd')
    expect(markUnpriced(priced)).toBe(priced)
  })
})

describe('foldOther (top 5 + Other)', () => {
  const pts = ['a', 'b', 'c', 'd', 'e', 'f', 'g'].flatMap((k, i) => [p('2026-10-01', k, 7 - i), p('2026-10-02', k, 1, k === 'g' ? 50 : 0)])
  const base = pivot(pts, ['2026-10-01', '2026-10-02'], 'cost_usd')

  it('keeps the five biggest series and sums the rest into Other', () => {
    const f = foldOther(base)
    expect(f.keys.map((k) => k.key)).toEqual(['a', 'b', 'c', 'd', 'e', OTHER_KEY])
    expect(f.keys[5].label).toBe('Other')
    expect(f.rows[0][OTHER_KEY]).toBe(2 + 1)
    expect(f.rows[1][OTHER_KEY]).toBe(2)
    expect(unpricedAt(f, '2026-10-02', OTHER_KEY)).toBe(50)
  })
  it('adds no Other when five or fewer series', () => {
    const small = pivot(pts.filter((x) => x.key < 'f'), ['2026-10-01'], 'cost_usd')
    expect(foldOther(small).keys.map((k) => k.key)).toEqual(['a', 'b', 'c', 'd', 'e'])
  })
  it('keeps a caller-chosen set in the caller order', () => {
    const f = foldOther(base, ['g', 'a', 'zz'])
    expect(f.keys.map((k) => k.key)).toEqual(['g', 'a', OTHER_KEY])
    expect(unpricedAt(f, '2026-10-02', 'g')).toBe(50)
  })
  it('summarises a chart for screen readers', () => {
    const s = chartSummary('API-equivalent cost per day by model', base, (n) => `$${n}`, (d) => d.slice(5))
    expect(s).toBe('API-equivalent cost per day by model, 10-01 to 10-02. Highest day 10-01, $28.')
  })
})

import type { SeriesPoint } from '../api/client'

export interface PivotKey {
  key: string
  label: string
}

export interface Pivot {
  rows: Array<Record<string, string | number>>
  keys: PivotKey[]
  /**
   * Unpriced tokens per cell, as date -> key -> tokens (cells with none are absent).
   * Kept beside `rows` rather than in them so chart data holds only plottable series.
   * Only filled for cost pivots: there it says how much of a cell the cost leaves out.
   */
  unpriced: Record<string, Record<string, number>>
}

/** Turns long-form series points into one row per date with a column per key. */
export function pivot(points: SeriesPoint[], dates: string[], value: 'cost_usd' | 'total_tokens'): Pivot {
  const totals = new Map<string, { label: string; total: number }>()
  const cells = new Map<string, number>()
  const unpriced: Pivot['unpriced'] = {}
  for (const p of points) {
    const t = totals.get(p.key) ?? { label: p.label, total: 0 }
    t.total += p[value]
    totals.set(p.key, t)
    const cell = `${p.date}\u0000${p.key}`
    cells.set(cell, (cells.get(cell) ?? 0) + p[value])
    if (value === 'cost_usd' && p.unpriced_tokens > 0) {
      const day = (unpriced[p.date] ??= {})
      day[p.key] = (day[p.key] ?? 0) + p.unpriced_tokens
    }
  }
  const keys = [...totals.entries()]
    .sort((a, b) => b[1].total - a[1].total)
    .map(([key, t]) => ({ key, label: t.label }))
  const allDates = [...new Set([...dates, ...points.map((p) => p.date)])].sort()
  const rows = allDates.map((date) => {
    const row: Record<string, string | number> = { date }
    for (const k of keys) row[k.key] = cells.get(`${date}\u0000${k.key}`) ?? 0
    return row
  })
  return { rows, keys, unpriced }
}

/** Unpriced tokens in one cell of a cost pivot (0 when fully priced). */
export function unpricedAt(p: Pivot, date: string, key: string): number {
  return p.unpriced[date]?.[key] ?? 0
}

/**
 * Relabels series that are unpriced for the whole range (cost 0 everywhere, some unpriced
 * tokens) as "<label> (unpriced)". Such a series draws with zero height, so its legend entry
 * is the only place it shows on the chart; the suffix says why it is flat rather than free.
 * Per-day detail (including partly-priced days) is in the tooltip via `unpricedAt`.
 */
export function markUnpriced(p: Pivot): Pivot {
  const hasUnpriced = new Set<string>()
  for (const day of Object.values(p.unpriced)) for (const key of Object.keys(day)) hasUnpriced.add(key)
  const unpriced = new Set([...hasUnpriced].filter((key) => p.rows.every((r) => r[key] === 0)))
  if (unpriced.size === 0) return p
  return { ...p, keys: p.keys.map((k) => (unpriced.has(k.key) ? { ...k, label: `${k.label} (unpriced)` } : k)) }
}

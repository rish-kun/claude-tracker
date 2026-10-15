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

/** Key of the folded "Other" series. */
export const OTHER_KEY = '__other'

/** Most series a chart draws on their own; the rest fold into "Other". */
export const MAX_SERIES = 5

/**
 * Keeps the series in `keep` (in that order) and folds every other series into one "Other"
 * series, summing its cells and unpriced tokens. Without `keep`, the top 5 by total stay
 * (keys are already sorted by total). With nothing to fold, no "Other" series is added.
 */
export function foldOther(p: Pivot, keep?: string[]): Pivot {
  const kept = keep ? keep.filter((k) => p.keys.some((x) => x.key === k)) : p.keys.slice(0, MAX_SERIES).map((k) => k.key)
  const keepSet = new Set(kept)
  const folded = p.keys.filter((k) => !keepSet.has(k.key))
  if (folded.length === 0) {
    return { ...p, keys: kept.map((k) => p.keys.find((x) => x.key === k)!) }
  }
  const rows = p.rows.map((r) => {
    const out: Record<string, string | number> = { date: r.date }
    for (const k of kept) out[k] = r[k]
    out[OTHER_KEY] = folded.reduce((n, k) => n + Number(r[k.key] ?? 0), 0)
    return out
  })
  const unpriced: Pivot['unpriced'] = {}
  for (const [date, day] of Object.entries(p.unpriced)) {
    const out: Record<string, number> = {}
    for (const [key, n] of Object.entries(day)) {
      const k = keepSet.has(key) ? key : OTHER_KEY
      out[k] = (out[k] ?? 0) + n
    }
    unpriced[date] = out
  }
  const keys = [...kept.map((k) => p.keys.find((x) => x.key === k)!), { key: OTHER_KEY, label: 'Other' }]
  return { rows, keys, unpriced }
}

/** Per-date total across all series. */
export function rowTotal(p: Pivot, row: Record<string, string | number>): number {
  return p.keys.reduce((n, k) => n + Number(row[k.key] ?? 0), 0)
}

/** Screen-reader summary of a daily chart: "{title}, 1 Oct to 15 Oct. Highest day 9 Oct, $6.20." */
export function chartSummary(title: string, p: Pivot, format: (n: number) => string, day: (ymd: string) => string): string {
  if (p.rows.length === 0) return `${title}. No data.`
  const first = String(p.rows[0].date)
  const last = String(p.rows[p.rows.length - 1].date)
  let best = p.rows[0]
  for (const r of p.rows) if (rowTotal(p, r) > rowTotal(p, best)) best = r
  const span = first === last ? day(first) : `${day(first)} to ${day(last)}`
  const top = rowTotal(p, best)
  return top > 0 ? `${title}, ${span}. Highest day ${day(String(best.date))}, ${format(top)}.` : `${title}, ${span}. Nothing in this range.`
}

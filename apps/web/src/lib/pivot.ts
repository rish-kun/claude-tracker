import type { BreakdownRow, SeriesPoint } from '../api/client'

export interface PivotKey {
  key: string
  label: string
}

export interface Pivot {
  rows: Array<Record<string, string | number>>
  keys: PivotKey[]
}

/** Turns long-form series points into one row per date with a column per key. */
export function pivot(points: SeriesPoint[], dates: string[], value: 'cost_usd' | 'total_tokens'): Pivot {
  const totals = new Map<string, { label: string; total: number }>()
  const cells = new Map<string, number>()
  for (const p of points) {
    const t = totals.get(p.key) ?? { label: p.label, total: 0 }
    t.total += p[value]
    totals.set(p.key, t)
    const cell = `${p.date}\u0000${p.key}`
    cells.set(cell, (cells.get(cell) ?? 0) + p[value])
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
  return { rows, keys }
}

/**
 * Relabels series whose breakdown row is entirely unpriced (cost 0, unpriced tokens > 0)
 * as "<label> (unpriced)", so a cost chart doesn't present them as genuinely $0.
 */
export function markUnpriced(p: Pivot, breakdown: BreakdownRow[]): Pivot {
  const unpriced = new Set(
    breakdown.filter((r) => r.totals.cost_usd === 0 && r.totals.unpriced_tokens > 0).map((r) => r.key),
  )
  if (unpriced.size === 0) return p
  return { ...p, keys: p.keys.map((k) => (unpriced.has(k.key) ? { ...k, label: `${k.label} (unpriced)` } : k)) }
}

import { type ReactNode, useEffect, useId, useMemo, useState } from 'react'
import { Bar, BarChart, CartesianGrid, Line, LineChart, ReferenceArea, ReferenceLine, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'
import type { LimitSnapshot, Plan, Totals } from '../api/client'
import { cellCost, formatDateTime, formatDayLong, formatDayTick, formatMoneyTick, formatPct, formatTime, formatTokens, formatUSD, moneyTicks, PLAN_LABELS } from '../lib/format'
import { formatTimeTick, limitTimeline, timeTicks } from '../lib/limits'
import { chartSummary, OTHER_KEY, type Pivot, rowTotal, unpricedAt } from '../lib/pivot'
import { readTokens } from '../lib/tokens'
import { CostFigure } from './figures'

/* ───────── Colours ───────── */

const SERIES_VARS = ['--series-1', '--series-2', '--series-3', '--series-4', '--series-5', '--series-6'] as const
const CHROME_VARS = ['--bg', '--grid', '--text-3', '--surface-2', '--line-strong', '--meter-crit', '--series-other'] as const
type ColorVar = (typeof SERIES_VARS)[number] | (typeof CHROME_VARS)[number]

function readColors(): Record<ColorVar, string> {
  const cs = getComputedStyle(document.documentElement)
  const out = {} as Record<ColorVar, string>
  for (const v of [...SERIES_VARS, ...CHROME_VARS]) out[v] = cs.getPropertyValue(v).trim()
  return out
}

/** Resolved token colours for recharts props, re-read when the theme changes. */
export function useChartColors(): Record<ColorVar, string> {
  const [colors, setColors] = useState(readColors)
  useEffect(() => {
    const update = () => setColors(readColors())
    // Re-read once mounted: on a cold load the first render can run before the stylesheet
    // applies, leaving every colour empty (an invisible chart).
    update()
    const observer = new MutationObserver(update)
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] })
    return () => observer.disconnect()
  }, [])
  return colors
}

/** Series colour variable for a chart key: slots 1–5 by index, "Other" in its own neutral. */
export function seriesVar(key: string, index: number): ColorVar {
  if (key === OTHER_KEY || index >= 5) return '--series-other'
  return SERIES_VARS[index]
}

/* ───────── Legend and tooltip ───────── */

export interface LegendItem {
  label: string
  color: string
  dashed?: boolean
}

export function ChartLegend({ items }: { items: LegendItem[] }) {
  return (
    <ul className="chart-legend" role="list">
      {/* Keyed by position: two accounts can share a label. */}
      {items.map((it, i) => (
        <li key={i} className="legend-item">
          {it.dashed ? (
            <span className="swatch" data-dashed="" style={{ color: it.color }} aria-hidden="true" />
          ) : (
            <span className="swatch" style={{ background: it.color }} aria-hidden="true" />
          )}
          {it.label}
        </li>
      ))}
    </ul>
  )
}

export interface TooltipRow {
  label: string
  color?: string
  value: string
}

export function ChartTooltip({ title, rows, total, note }: { title: string; rows: TooltipRow[]; total?: string; note?: string[] }) {
  return (
    <div className="chart-tooltip">
      <div className="chart-tooltip-head">{title}</div>
      {rows.map((r, i) => (
        <div key={i} className="chart-tooltip-row">
          {r.color ? <span className="swatch" style={{ background: r.color }} /> : <span />}
          <span className="label">{r.label}</span>
          <span className="value">{r.value}</span>
        </div>
      ))}
      {total !== undefined && (
        <div className="chart-tooltip-row" data-total="">
          <span />
          <span className="label">Total</span>
          <span className="value">{total}</span>
        </div>
      )}
      {note?.map((n) => (
        <div key={n} className="chart-tooltip-note">
          {n}
        </div>
      ))}
    </div>
  )
}

/** Recharts passes these to a custom tooltip `content`; typed loosely across versions. */
interface TooltipProps {
  active?: boolean
  payload?: ReadonlyArray<{ payload?: unknown }>
  label?: unknown
}

const ymdTime = (ymd: string) => {
  const [y, m, d] = ymd.split('-').map(Number)
  return new Date(y, m - 1, d).getTime()
}
const dayLong = (ymd: string) => formatDayLong(ymdTime(ymd))

/* ───────── Table fallback toggle ───────── */

function TableToggle({ open, onToggle, controls }: { open: boolean; onToggle: () => void; controls: string }) {
  return (
    <button type="button" className="btn-text" aria-expanded={open} aria-controls={controls} onClick={onToggle}>
      {open ? 'Hide table' : 'Show as table'}
    </button>
  )
}

/* ───────── StackedDailyBars ───────── */

export function StackedDailyBars({
  pivot,
  unit = 'usd',
  format = unit === 'tokens' ? formatTokens : formatUSD,
  colorFor = seriesVar,
  title,
  footnote,
}: {
  pivot: Pivot
  /** What the bars measure: API-equivalent dollars, or token counts. */
  unit?: 'usd' | 'tokens'
  format?: (n: number) => string
  colorFor?: (key: string, index: number) => ColorVar
  /** What the chart shows, e.g. "API-equivalent cost per day by model"; used for the accessible summary. */
  title: string
  footnote?: ReactNode
}) {
  const c = useChartColors()
  const [table, setTable] = useState(false)
  const tableId = useId()
  const yTicks = useMemo(() => {
    const max = Math.max(0, ...pivot.rows.map((r) => rowTotal(pivot, r)))
    // Token counts are whole numbers: keep tick steps at 1 or more.
    return moneyTicks(unit === 'tokens' ? Math.max(max, 5) : max)
  }, [pivot, unit])
  const yMax = yTicks[yTicks.length - 1]
  const ariaLabel = useMemo(() => chartSummary(title, pivot, format, (d) => formatDayTick(d)), [title, pivot, format])
  const colorOf = (key: string, i: number) => c[colorFor(key, i)]

  const content = ({ active, payload }: TooltipProps) => {
    const row = payload?.[0]?.payload as Record<string, string | number> | undefined
    if (!active || !row) return null
    const date = String(row.date)
    let totalUnpriced = 0
    const rows: TooltipRow[] = []
    pivot.keys.forEach((k, i) => {
      const v = Number(row[k.key] ?? 0)
      const u = unpricedAt(pivot, date, k.key)
      totalUnpriced += u
      if (v === 0 && u === 0) return
      rows.push({ label: k.label, color: colorOf(k.key, i), value: cellCost(v, u, format) })
    })
    return <ChartTooltip title={dayLong(date)} rows={rows.reverse()} total={cellCost(rowTotal(pivot, row), totalUnpriced, format)} />
  }

  return (
    <div className="chart">
      <ChartLegend items={pivot.keys.map((k, i) => ({ label: k.label, color: `var(${colorFor(k.key, i)})` }))} />
      <div className="chart-frame" role="img" aria-label={ariaLabel}>
        <ResponsiveContainer width="100%" height="100%">
          <BarChart data={pivot.rows} barCategoryGap="20%" accessibilityLayer={false} margin={{ top: 10, right: 0, bottom: 0, left: 0 }}>
            <CartesianGrid vertical={false} stroke={c['--grid']} />
            <XAxis
              dataKey="date"
              tickFormatter={(d) => formatDayTick(String(d))}
              axisLine={false}
              tickLine={false}
              minTickGap={24}
              tick={{ fill: c['--text-3'], fontSize: 12, fontFamily: 'Public Sans' }}
            />
            <YAxis
              width={52}
              axisLine={false}
              tickLine={false}
              ticks={yTicks}
              domain={[0, yMax]}
              interval={0}
              tickFormatter={(v: number) => (unit === 'tokens' ? formatTokens(v) : formatMoneyTick(v, yMax))}
              tick={{ fill: c['--text-3'], fontSize: 12, fontFamily: 'Public Sans' }}
            />
            <Tooltip content={content} cursor={{ fill: c['--surface-2'] }} isAnimationActive={false} />
            {pivot.keys.map((k, i) => (
              <Bar
                key={k.key}
                dataKey={k.key}
                name={k.label}
                stackId="a"
                fill={colorOf(k.key, i)}
                stroke={c['--bg']}
                strokeWidth={1}
                maxBarSize={28}
                isAnimationActive={false}
              />
            ))}
          </BarChart>
        </ResponsiveContainer>
      </div>
      <div className="chart-foot">
        {footnote && <span>{footnote}</span>}
        <TableToggle open={table} onToggle={() => setTable((t) => !t)} controls={tableId} />
      </div>
      <div id={tableId} hidden={!table} className="chart-table table-scroll">
        {table && (
          <table className="table table-compact">
            <caption className="sr-only">{title}</caption>
            <thead>
              <tr>
                <th scope="col">Day</th>
                {pivot.keys.map((k) => (
                  <th key={k.key} scope="col" className="num">
                    {k.label}
                  </th>
                ))}
                <th scope="col" className="num">
                  Total
                </th>
              </tr>
            </thead>
            <tbody>
              {pivot.rows.map((r) => {
                const date = String(r.date)
                const tu = pivot.keys.reduce((n, k) => n + unpricedAt(pivot, date, k.key), 0)
                return (
                  <tr key={date}>
                    <th scope="row" style={{ font: 'var(--t-body)', color: 'var(--text)' }}>
                      {dayLong(date)}
                    </th>
                    {pivot.keys.map((k) => (
                      <td key={k.key} className="num">
                        {cellCost(Number(r[k.key] ?? 0), unpricedAt(pivot, date, k.key), format)}
                      </td>
                    ))}
                    <td className="num">{cellCost(rowTotal(pivot, r), tu, format)}</td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        )}
      </div>
    </div>
  )
}

/* ───────── LimitTimeline ───────── */

const RESET_NEAR_MS = 15 * 60_000

export function LimitTimeline({ limits }: { limits: LimitSnapshot[] }) {
  const c = useChartColors()
  const [table, setTable] = useState(false)
  const tableId = useId()
  const data = useMemo(() => limitTimeline(limits), [limits])
  if (data.points.length < 2) {
    return <p className="meta mt-3">Not enough plan-limit readings in this range to draw a timeline.</p>
  }
  const { points, domain, resets } = data
  const span = domain[1] - domain[0]
  const last = points[points.length - 1]
  const lastOf = (k: 'five' | 'seven') => [...points].reverse().find((p) => p[k] !== null)?.[k]
  const latestFive = lastOf('five')
  const latestSeven = lastOf('seven')
  const ariaLabel =
    `Plan limits over time, ${formatDateTime(new Date(domain[0]).toISOString())} to ${formatDateTime(new Date(last.t).toISOString())}.` +
    (latestFive != null ? ` Latest 5-hour window ${Math.round(latestFive)}%.` : '') +
    (latestSeven != null ? ` Latest 7-day window ${Math.round(latestSeven)}%.` : '')

  const content = ({ active, label }: TooltipProps) => {
    if (!active || label === undefined) return null
    const t = Number(label)
    const p = points.find((x) => x.t === t)
    if (!p) return null
    const rows: TooltipRow[] = []
    if (p.five !== null) rows.push({ label: '5-hour window', color: c['--series-1'], value: `${Math.round(p.five)}%` })
    if (p.seven !== null) rows.push({ label: '7-day window', color: c['--series-3'], value: `${Math.round(p.seven)}%` })
    const near = resets
      .filter((r) => Math.abs(r.t - t) <= RESET_NEAR_MS)
      .map((r) => `${r.label === '5h reset' ? '5-hour' : '7-day'} window reset at ${formatTime(r.t)}`)
    return <ChartTooltip title={formatDateTime(new Date(t).toISOString())} rows={rows} note={near} />
  }

  return (
    <div className="chart">
      <ChartLegend
        items={[
          { label: '5-hour window', color: 'var(--series-1)' },
          { label: '7-day window', color: 'var(--series-3)' },
          { label: 'Window reset', color: 'var(--text-3)', dashed: true },
        ]}
      />
      <div className="chart-frame" data-kind="timeline" role="img" aria-label={ariaLabel}>
        <ResponsiveContainer width="100%" height="100%">
          <LineChart data={points} accessibilityLayer={false} margin={{ top: 10, right: 8, bottom: 0, left: 0 }}>
            <CartesianGrid vertical={false} stroke={c['--grid']} />
            <ReferenceArea y1={90} y2={100} fill={c['--meter-crit']} fillOpacity={0.08} ifOverflow="hidden" />
            <ReferenceLine y={70} stroke={c['--line-strong']} strokeDasharray="2 4" />
            <XAxis
              dataKey="t"
              type="number"
              scale="time"
              domain={['dataMin', 'dataMax']}
              ticks={timeTicks(domain)}
              tickFormatter={(t) => formatTimeTick(Number(t), span)}
              axisLine={false}
              tickLine={false}
              minTickGap={40}
              tick={{ fill: c['--text-3'], fontSize: 12, fontFamily: 'Public Sans' }}
            />
            <YAxis
              domain={[0, 100]}
              ticks={[0, 50, 70, 90, 100]}
              tickFormatter={(v) => `${v}%`}
              width={52}
              axisLine={false}
              tickLine={false}
              tick={{ fill: c['--text-3'], fontSize: 12, fontFamily: 'Public Sans' }}
            />
            <Tooltip content={content} cursor={{ stroke: c['--line-strong'] }} isAnimationActive={false} />
            {resets.map((r) => (
              <ReferenceLine
                key={`${r.label}-${r.t}`}
                x={r.t}
                stroke={r.label === '5h reset' ? c['--series-1'] : c['--series-3']}
                strokeOpacity={0.5}
                strokeDasharray="2 4"
              />
            ))}
            <Line type="stepAfter" dataKey="five" name="5-hour window" stroke={c['--series-1']} strokeWidth={2} dot={false} activeDot={{ r: 4 }} connectNulls isAnimationActive={false} />
            <Line type="stepAfter" dataKey="seven" name="7-day window" stroke={c['--series-3']} strokeWidth={2} dot={false} activeDot={{ r: 4 }} connectNulls isAnimationActive={false} />
          </LineChart>
        </ResponsiveContainer>
      </div>
      <div className="chart-foot">
        <TableToggle open={table} onToggle={() => setTable((t) => !t)} controls={tableId} />
      </div>
      <div id={tableId} hidden={!table} className="chart-table table-scroll">
        {table && (
          <table className="table table-compact">
            <caption className="sr-only">Plan limits over time</caption>
            <thead>
              <tr>
                <th scope="col">Reported</th>
                <th scope="col" className="num">
                  5-hour window
                </th>
                <th scope="col" className="num">
                  7-day window
                </th>
              </tr>
            </thead>
            <tbody>
              {points.map((p) => (
                <tr key={p.t}>
                  <td>{formatDateTime(new Date(p.t).toISOString())}</td>
                  <td className="num">{p.five === null ? '–' : `${Math.round(p.five)}%`}</td>
                  <td className="num">{p.seven === null ? '–' : `${Math.round(p.seven)}%`}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  )
}

/* ───────── HTML bars ───────── */

/** A thin neutral bar showing `value` relative to `max`, anchored right (ledger rows). */
export function WeightBar({ value, max }: { value: number; max: number }) {
  const pct = max > 0 ? Math.min(100, (value / max) * 100) : 0
  return (
    <span className="weight-bar" aria-hidden="true">
      {value > 0 && <span style={{ width: `max(2px, ${pct}%)` }} />}
    </span>
  )
}

export interface ShareRow {
  key: string | number
  label: string
  share: number
  totals: Totals
}

/** Labelled horizontal share bars (who used an account). */
export function ShareBars({ rows }: { rows: ShareRow[] }) {
  const sorted = [...rows].sort((a, b) => b.share - a.share)
  return (
    <ul className="share-bars" role="list">
      {sorted.map((r) => (
        <li key={r.key} className="share-row">
          <span className="who" title={r.label}>
            {r.label}
          </span>
          <span className="share-track" aria-hidden="true">
            <span className="share-fill" style={{ width: `${Math.min(100, r.share * 100)}%` }} />
          </span>
          <span className="pct">{formatPct(r.share)}</span>
          <span className="cost">
            <CostFigure totals={r.totals} />
          </span>
        </li>
      ))}
    </ul>
  )
}

export interface SplitPart {
  key: string
  label: string
  plan: Plan
  totals: Totals
  color: string
}

/**
 * One person's API-equivalent cost split by account: total length is `scale` (0–1) of the
 * row width, segments sized by cost (by tokens read when nothing is priced).
 */
export function AccountSplitBar({ parts, scale }: { parts: SplitPart[]; scale: number }) {
  const [active, setActive] = useState<string | null>(null)
  const priced = parts.some((p) => p.totals.cost_usd > 0)
  const weight = (p: SplitPart) => (priced ? p.totals.cost_usd : readTokens(p.totals))
  const shown = parts.filter((p) => weight(p) > 0)
  return (
    <div className="split-bar-wrap">
      <div className="split-bar" style={{ width: `${Math.max(1, Math.min(100, scale * 100))}%` }}>
        {shown.map((p) => {
          const text = `${p.label} (${PLAN_LABELS[p.plan]}) — ${costText(p.totals)}, ${formatTokens(p.totals.output)} written, ${formatTokens(readTokens(p.totals))} read`
          return (
            <span
              key={p.key}
              className="split-seg"
              style={{ flexGrow: weight(p), flexBasis: 0, background: p.color }}
              tabIndex={0}
              role="img"
              aria-label={text}
              onMouseEnter={() => setActive(p.key)}
              onMouseLeave={() => setActive((a) => (a === p.key ? null : a))}
              onFocus={() => setActive(p.key)}
              onBlur={() => setActive((a) => (a === p.key ? null : a))}
            >
              {active === p.key && (
                <ChartTooltip
                  title={`${p.label} (${PLAN_LABELS[p.plan]})`}
                  rows={[
                    { label: 'API-equivalent', value: costText(p.totals) },
                    { label: 'Written', value: formatTokens(p.totals.output) },
                    { label: 'Read', value: formatTokens(readTokens(p.totals)) },
                  ]}
                />
              )}
            </span>
          )
        })}
      </div>
    </div>
  )
}

function costText(t: Totals): string {
  return t.cost_usd === 0 && t.unpriced_tokens > 0 ? 'not priced' : cellCost(t.cost_usd, t.unpriced_tokens)
}

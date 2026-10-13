import type { ReactNode } from 'react'
import {
  Area, AreaChart, Bar, BarChart, CartesianGrid, Cell, Legend, Line, LineChart, Pie, PieChart,
  ReferenceLine, ResponsiveContainer, Tooltip, XAxis, YAxis,
} from 'recharts'
import type { LimitSnapshot } from '../api/client'
import { cellCost, formatDateTime } from '../lib/format'
import { formatTimeTick, limitTimeline } from '../lib/limits'
import { type Pivot, type PivotKey, unpricedAt } from '../lib/pivot'

export const COLORS = ['#6366f1', '#f59e0b', '#10b981', '#ef4444', '#06b6d4', '#a855f7', '#84cc16', '#f97316', '#ec4899', '#14b8a6']
const color = (i: number) => COLORS[i % COLORS.length]

export function Empty({ children = 'No usage in this range.' }: { children?: ReactNode }) {
  return <div className="empty">{children}</div>
}

/**
 * Tooltip formatter for a cost cell: `category` names the row field identifying the cell's
 * row (e.g. "date"), and `unpriced(row, key)` gives that cell's unpriced tokens.
 */
function cellFormatter(format: (n: number) => string, category: string, unpriced: (row: string, key: string) => number) {
  return (v: unknown, _name: unknown, item: { dataKey?: unknown; payload?: unknown }) => {
    const row = String((item.payload as Record<string, unknown> | undefined)?.[category] ?? '')
    return cellCost(Number(v), unpriced(row, String(item.dataKey ?? '')), format)
  }
}

export function StackedDaily({ pivot, format, kind = 'area' }: { pivot: Pivot; format: (n: number) => string; kind?: 'area' | 'bar' }) {
  if (pivot.keys.length === 0) return <Empty />
  // Each tooltip line is one series on one date; for a cost pivot, say how much of it is
  // unpriced instead of letting a cost of 0 read as "$0.00".
  const cell = cellFormatter(format, 'date', (date, key) => unpricedAt(pivot, date, key))
  const common = (
    <>
      <CartesianGrid strokeDasharray="3 3" vertical={false} />
      <XAxis dataKey="date" fontSize={12} />
      <YAxis tickFormatter={format} width={70} fontSize={12} />
      <Tooltip formatter={cell} />
      <Legend />
    </>
  )
  return (
    <ResponsiveContainer width="100%" height={280}>
      {kind === 'area' ? (
        <AreaChart data={pivot.rows}>
          {common}
          {pivot.keys.map((k, i) => (
            <Area key={k.key} type="monotone" dataKey={k.key} name={k.label} stackId="1" stroke={color(i)} fill={color(i)} fillOpacity={0.45} />
          ))}
        </AreaChart>
      ) : (
        <BarChart data={pivot.rows}>
          {common}
          {pivot.keys.map((k, i) => (
            <Bar key={k.key} dataKey={k.key} name={k.label} stackId="1" fill={color(i)} />
          ))}
        </BarChart>
      )}
    </ResponsiveContainer>
  )
}

export function ShareDonut({ data, format }: { data: { name: string; value: number }[]; format: (n: number) => string }) {
  if (data.every((d) => d.value === 0)) return <Empty />
  return (
    <ResponsiveContainer width="100%" height={220}>
      <PieChart>
        <Pie data={data} dataKey="value" nameKey="name" innerRadius={50} outerRadius={85} paddingAngle={2}>
          {data.map((d, i) => (
            <Cell key={d.name} fill={color(i)} />
          ))}
        </Pie>
        <Tooltip formatter={(v) => format(Number(v))} />
        <Legend />
      </PieChart>
    </ResponsiveContainer>
  )
}

/**
 * Horizontal stacked bars: one bar per row (e.g. user), one segment per key (e.g. account).
 * `unpriced` (row name -> key -> tokens) makes cost tooltips flag unpriced usage.
 */
export function UserAccountBars({
  rows,
  keys,
  format,
  unpriced = {},
}: {
  rows: Array<Record<string, string | number>>
  keys: PivotKey[]
  format: (n: number) => string
  unpriced?: Record<string, Record<string, number>>
}) {
  if (rows.length === 0) return <Empty />
  const cell = cellFormatter(format, 'name', (name, key) => unpriced[name]?.[key] ?? 0)
  return (
    <ResponsiveContainer width="100%" height={Math.max(120, rows.length * 44 + 60)}>
      <BarChart data={rows} layout="vertical" margin={{ left: 20 }}>
        <CartesianGrid strokeDasharray="3 3" horizontal={false} />
        <XAxis type="number" tickFormatter={format} fontSize={12} />
        <YAxis type="category" dataKey="name" width={180} fontSize={12} />
        <Tooltip formatter={cell} />
        <Legend />
        {keys.map((k, i) => (
          <Bar key={k.key} dataKey={k.key} name={k.label} stackId="1" fill={color(i)} />
        ))}
      </BarChart>
    </ResponsiveContainer>
  )
}

export function LimitTimeline({ limits }: { limits: LimitSnapshot[] }) {
  if (limits.length < 2) return <Empty>Not enough limit snapshots in this range.</Empty>
  const { points, domain, resets } = limitTimeline(limits)
  const span = domain[1] - domain[0]
  return (
    <ResponsiveContainer width="100%" height={220}>
      <LineChart data={points}>
        <CartesianGrid strokeDasharray="3 3" vertical={false} />
        {/* A numeric time axis keeps the spacing between snapshots proportional to the time between them. */}
        <XAxis
          dataKey="t"
          type="number"
          scale="time"
          domain={['dataMin', 'dataMax']}
          tickFormatter={(t) => formatTimeTick(Number(t), span)}
          fontSize={11}
          minTickGap={40}
        />
        <YAxis domain={[0, 100]} tickFormatter={(v) => `${v}%`} width={45} fontSize={12} />
        <Tooltip labelFormatter={(t) => formatDateTime(new Date(Number(t)).toISOString())} formatter={(v) => `${Number(v).toFixed(0)}%`} />
        <Legend />
        {resets.map((r) => (
          <ReferenceLine
            key={`${r.label}-${r.t}`}
            x={r.t}
            stroke={r.label === '5h reset' ? color(0) : color(1)}
            strokeDasharray="4 4"
            strokeOpacity={0.7}
            label={{ value: r.label, position: 'insideTopRight', fontSize: 10, fill: 'currentColor' }}
          />
        ))}
        <Line type="stepAfter" dataKey="five" name="5-hour window" stroke={color(0)} dot={false} connectNulls isAnimationActive={false} />
        <Line type="stepAfter" dataKey="seven" name="7-day window" stroke={color(1)} dot={false} connectNulls isAnimationActive={false} />
      </LineChart>
    </ResponsiveContainer>
  )
}

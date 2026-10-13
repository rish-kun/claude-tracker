import type { ReactNode } from 'react'
import {
  Area, AreaChart, Bar, BarChart, CartesianGrid, Cell, Legend, Line, LineChart, Pie, PieChart,
  ResponsiveContainer, Tooltip, XAxis, YAxis,
} from 'recharts'
import type { LimitSnapshot } from '../api/client'
import { formatDateTime } from '../lib/format'
import type { Pivot, PivotKey } from '../lib/pivot'

export const COLORS = ['#6366f1', '#f59e0b', '#10b981', '#ef4444', '#06b6d4', '#a855f7', '#84cc16', '#f97316', '#ec4899', '#14b8a6']
const color = (i: number) => COLORS[i % COLORS.length]

export function Empty({ children = 'No usage in this range.' }: { children?: ReactNode }) {
  return <div className="empty">{children}</div>
}

export function StackedDaily({ pivot, format, kind = 'area' }: { pivot: Pivot; format: (n: number) => string; kind?: 'area' | 'bar' }) {
  if (pivot.keys.length === 0) return <Empty />
  const common = (
    <>
      <CartesianGrid strokeDasharray="3 3" vertical={false} />
      <XAxis dataKey="date" fontSize={12} />
      <YAxis tickFormatter={format} width={70} fontSize={12} />
      <Tooltip formatter={(v) => format(Number(v))} />
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

/** Horizontal stacked bars: one bar per row (e.g. user), one segment per key (e.g. account). */
export function UserAccountBars({ rows, keys, format }: { rows: Array<Record<string, string | number>>; keys: PivotKey[]; format: (n: number) => string }) {
  if (rows.length === 0) return <Empty />
  return (
    <ResponsiveContainer width="100%" height={Math.max(120, rows.length * 44 + 60)}>
      <BarChart data={rows} layout="vertical" margin={{ left: 20 }}>
        <CartesianGrid strokeDasharray="3 3" horizontal={false} />
        <XAxis type="number" tickFormatter={format} fontSize={12} />
        <YAxis type="category" dataKey="name" width={180} fontSize={12} />
        <Tooltip formatter={(v) => format(Number(v))} />
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
  const data = limits.map((l) => ({
    t: formatDateTime(l.captured_at),
    five: l.five_hour_pct ?? null,
    seven: l.seven_day_pct ?? null,
  }))
  return (
    <ResponsiveContainer width="100%" height={220}>
      <LineChart data={data}>
        <CartesianGrid strokeDasharray="3 3" vertical={false} />
        <XAxis dataKey="t" fontSize={11} minTickGap={40} />
        <YAxis domain={[0, 100]} tickFormatter={(v) => `${v}%`} width={45} fontSize={12} />
        <Tooltip formatter={(v) => `${Number(v).toFixed(0)}%`} />
        <Legend />
        <Line type="stepAfter" dataKey="five" name="5-hour window" stroke={color(0)} dot={false} connectNulls />
        <Line type="stepAfter" dataKey="seven" name="7-day window" stroke={color(1)} dot={false} connectNulls />
      </LineChart>
    </ResponsiveContainer>
  )
}

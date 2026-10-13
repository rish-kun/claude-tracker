import type { LimitSnapshot } from '../api/client'
import { formatDateTime } from '../lib/format'

function Gauge({ title, pct, resetsAt }: { title: string; pct?: number; resetsAt?: string }) {
  if (pct === undefined) return null
  const reset = resetsAt ? new Date(resetsAt) : undefined
  const stale = reset !== undefined && reset.getTime() < Date.now()
  const level = pct >= 90 ? 'hot' : pct >= 70 ? 'warm' : ''
  return (
    <div>
      <div>
        {title}: <strong>{stale ? 'reset' : `${Math.round(pct)}%`}</strong>
      </div>
      <div className="gauge">
        <div className={level} style={{ width: `${stale ? 0 : Math.min(100, pct)}%` }} />
      </div>
      <div className="muted">
        {stale ? `Window reset ${formatDateTime(resetsAt!)} (was ${Math.round(pct)}%)` : reset ? `Resets ${formatDateTime(resetsAt!)}` : ''}
      </div>
    </div>
  )
}

/** Latest plan-limit utilisation reported by Claude Code for an account. */
export function LimitGauges({ limit, label }: { limit: LimitSnapshot; label: string }) {
  return (
    <div className="card">
      <h2>
        {label} <span className="muted">· as of {formatDateTime(limit.captured_at)}</span>
      </h2>
      <div className="gauges">
        <Gauge title="5-hour window" pct={limit.five_hour_pct} resetsAt={limit.five_hour_resets_at} />
        <Gauge title="7-day window" pct={limit.seven_day_pct} resetsAt={limit.seven_day_resets_at} />
      </div>
    </div>
  )
}

import type { LimitSnapshot } from '../api/client'

const MINUTE_MS = 60_000
const DAY_MS = 86_400_000

export interface LimitPoint {
  /** captured_at, ms since epoch. */
  t: number
  five: number | null
  seven: number | null
}

export interface ResetMarker {
  /** Reset instant, ms since epoch. */
  t: number
  label: '5h reset' | '7d reset'
}

export interface LimitTimelineData {
  points: LimitPoint[]
  /** [first, last] capture time; markers are only drawn inside it. */
  domain: [number, number]
  resets: ResetMarker[]
}

/**
 * Chart data for the plan-limit timeline on a numeric time axis: points in capture order,
 * plus one marker per distinct window reset that falls within the captured span. Reset
 * times are rounded to the minute so second-level jitter between snapshots of the same
 * window doesn't draw duplicate lines. Markers are thinned by `selectResetMarkers`.
 */
export function limitTimeline(limits: LimitSnapshot[]): LimitTimelineData {
  const points = limits
    .map((l) => ({ t: Date.parse(l.captured_at), five: l.five_hour_pct ?? null, seven: l.seven_day_pct ?? null }))
    .filter((p) => Number.isFinite(p.t))
    .sort((a, b) => a.t - b.t)
  const domain: [number, number] = points.length > 0 ? [points[0].t, points[points.length - 1].t] : [0, 0]
  const resets: ResetMarker[] = []
  const add = (label: ResetMarker['label'], at: (l: LimitSnapshot) => string | undefined) => {
    const seen = new Set<number>()
    for (const l of limits) {
      const iso = at(l)
      if (!iso) continue
      const t = Math.round(Date.parse(iso) / MINUTE_MS) * MINUTE_MS
      if (!Number.isFinite(t) || t < domain[0] || t > domain[1] || seen.has(t)) continue
      seen.add(t)
      resets.push({ t, label })
    }
  }
  add('5h reset', (l) => l.five_hour_resets_at)
  add('7d reset', (l) => l.seven_day_resets_at)
  return { points, domain, resets: selectResetMarkers(resets, domain) }
}

/** 5-hour reset markers are only drawn when the visible domain spans at most this long. */
export const FIVE_HOUR_MARKER_MAX_SPAN_MS = 3 * DAY_MS
/** At most this many markers per series (the latest ones). */
export const MAX_MARKERS_PER_SERIES = 30

/**
 * Which reset markers to draw, so long ranges don't fill with hundreds of lines: 7-day
 * resets always; 5-hour resets only when the domain spans ≤ 3 days; and at most the
 * latest `max` markers per series. Returned in time order.
 */
export function selectResetMarkers(resets: ResetMarker[], domain: [number, number], max = MAX_MARKERS_PER_SERIES): ResetMarker[] {
  const showFive = domain[1] - domain[0] <= FIVE_HOUR_MARKER_MAX_SPAN_MS
  const latest = (label: ResetMarker['label']) =>
    resets
      .filter((r) => r.label === label)
      .sort((a, b) => a.t - b.t)
      .slice(-max)
  return [...(showFive ? latest('5h reset') : []), ...latest('7d reset')].sort((a, b) => a.t - b.t)
}

/** Axis tick text: time of day for spans up to two days, otherwise the date. */
export function formatTimeTick(t: number, spanMs: number, locale?: string, timeZone?: string): string {
  const opts: Intl.DateTimeFormatOptions =
    spanMs <= 2 * DAY_MS
      ? { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', timeZone }
      : { month: 'short', day: 'numeric', timeZone }
  return new Date(t).toLocaleString(locale, opts)
}

const HOUR_MS = 3_600_000
const HOUR_STEPS = [1, 2, 3, 6, 12]
const DAY_STEPS = [1, 2, 3, 7, 14, 30]

/**
 * Time-axis ticks for the limit timeline, aligned to local time so labels never repeat:
 * whole hours (1, 2, 3, 6 or 12 apart) for spans up to two days, local midnights
 * (1, 2, 3, 7, 14 or 30 days apart) beyond that. At most `maxTicks` ticks.
 */
export function timeTicks(domain: [number, number], maxTicks = 7): number[] {
  const [from, to] = domain
  if (!(to > from)) return [from]
  const span = to - from
  const out: number[] = []
  if (span <= 2 * DAY_MS) {
    const step = HOUR_STEPS.find((h) => span / (h * HOUR_MS) <= maxTicks) ?? 24
    const d = new Date(from)
    d.setMinutes(0, 0, 0)
    if (d.getTime() < from) d.setHours(d.getHours() + 1)
    // Align to a multiple of the step within the local day (00:00, 06:00, 12:00…).
    while (d.getHours() % step !== 0) d.setHours(d.getHours() + 1)
    for (; d.getTime() <= to; d.setHours(d.getHours() + step)) out.push(d.getTime())
    return out
  }
  const step = DAY_STEPS.find((n) => span / (n * DAY_MS) <= maxTicks) ?? Math.ceil(span / DAY_MS / maxTicks)
  const d = new Date(from)
  d.setHours(0, 0, 0, 0)
  if (d.getTime() < from) d.setDate(d.getDate() + 1)
  for (; d.getTime() <= to; d.setDate(d.getDate() + step)) out.push(d.getTime())
  return out
}

export type LimitLevel = 'ok' | 'high' | 'crit'

/** Meter colour band: below 70% ok, 70–89% getting full, 90% and up nearly full. */
export function limitLevel(pct: number): LimitLevel {
  if (pct >= 90) return 'crit'
  if (pct >= 70) return 'high'
  return 'ok'
}

/** Cells in a limit meter (each is 5%). */
export const METER_CELLS = 20

/** Filled cells for `pct`: any usage fills at least one, and 100% or more fills all of them. */
export function filledCells(pct: number): number {
  if (!(pct > 0)) return 0
  return Math.min(METER_CELLS, Math.max(1, Math.round(pct / 5)))
}

export type WindowState =
  | { kind: 'none' }
  | { kind: 'reset'; was: number; resetAt: string }
  | { kind: 'live'; pct: number; level: LimitLevel; filled: number; resetsAt?: string }

/**
 * What a limit meter shows: nothing without a reading; "reset" once the window's reset time has
 * passed (the reading is stale, new usage not reported yet); otherwise the live percentage.
 */
export function windowState(pct: number | undefined, resetsAt: string | undefined, now: number): WindowState {
  if (pct === undefined) return { kind: 'none' }
  if (resetsAt !== undefined && Date.parse(resetsAt) <= now) return { kind: 'reset', was: Math.round(pct), resetAt: resetsAt }
  return { kind: 'live', pct, level: limitLevel(pct), filled: filledCells(pct), resetsAt }
}

/** Live percentage of a window, counting a window whose reset has passed as 0. */
export function livePct(pct: number | undefined, resetsAt: string | undefined, now: number): number | undefined {
  const s = windowState(pct, resetsAt, now)
  return s.kind === 'live' ? s.pct : s.kind === 'reset' ? 0 : undefined
}

/** How close an account is to a limit right now: the fuller of its two windows; undefined without readings. */
export function limitPressure(l: LimitSnapshot | undefined, now: number): number | undefined {
  if (!l) return undefined
  const five = livePct(l.five_hour_pct, l.five_hour_resets_at, now)
  const seven = livePct(l.seven_day_pct, l.seven_day_resets_at, now)
  if (five === undefined && seven === undefined) return undefined
  return Math.max(five ?? 0, seven ?? 0)
}

/** Whether a reading still says something now: at least one of its windows has not reset yet. */
export function hasLiveWindow(l: LimitSnapshot, now: number): boolean {
  return (
    windowState(l.five_hour_pct, l.five_hour_resets_at, now).kind === 'live' ||
    windowState(l.seven_day_pct, l.seven_day_resets_at, now).kind === 'live'
  )
}

/** Readings older than this are flagged as possibly out of date. */
export const STALE_READING_MS = 3 * 60 * 60_000

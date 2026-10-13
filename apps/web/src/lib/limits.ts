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

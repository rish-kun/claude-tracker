export type Preset = 'today' | '7d' | '30d' | '90d'

export interface Range {
  from: Date
  to: Date
  tz: string
}

const DAY_MS = 86_400_000
const PRESET_DAYS: Record<Preset, number> = { today: 1, '7d': 7, '30d': 30, '90d': 90 }

export function browserTz(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
}

/** YYYY-MM-DD of `d` in `tz`. */
export function dayString(d: Date, tz: string): string {
  return new Intl.DateTimeFormat('en-CA', { timeZone: tz, year: 'numeric', month: '2-digit', day: '2-digit' }).format(d)
}

/** Offset of `tz` from UTC at instant `t` (ms since epoch), in ms: local wall time minus UTC. */
function tzOffset(t: number, tz: string): number {
  const parts = new Intl.DateTimeFormat('en-US', {
    timeZone: tz,
    hourCycle: 'h23',
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }).formatToParts(new Date(t))
  const get = (type: string) => Number(parts.find((p) => p.type === type)?.value ?? 0)
  const wall = Date.UTC(get('year'), get('month') - 1, get('day'), get('hour'), get('minute'), get('second'))
  return wall - Math.floor(t / 1000) * 1000
}

/** YYYY-MM-DD `n` calendar days after `ymd` (n may be negative). */
export function addDays(ymd: string, n: number): string {
  const [y, m, d] = ymd.split('-').map(Number)
  return new Date(Date.UTC(y, m - 1, d + n)).toISOString().slice(0, 10)
}

/**
 * The instant calendar day `ymd` begins in `tz` (local midnight). If a DST jump skips
 * midnight, this is the transition instant, i.e. the first moment of that day.
 */
export function zonedMidnight(ymd: string, tz: string): Date {
  const [y, m, d] = ymd.split('-').map(Number)
  const guess = Date.UTC(y, m - 1, d)
  const first = guess - tzOffset(guess, tz)
  const second = guess - tzOffset(first, tz)
  // When midnight doesn't exist, `second` falls on the previous day; `first` is the jump instant.
  return new Date(dayString(new Date(second), tz) === ymd ? second : first)
}

/** The instant at which the day containing `d` began in `tz`. */
export function startOfDay(d: Date, tz: string): Date {
  return zonedMidnight(dayString(d, tz), tz)
}

export function presetRange(p: Preset, now = new Date(), tz = browserTz()): Range {
  const days = PRESET_DAYS[p]
  const from = zonedMidnight(addDays(dayString(now, tz), -(days - 1)), tz)
  return { from, to: new Date(now.getTime() + 60_000), tz }
}

/** Every calendar day in `tz` touched by the range, in order. */
export function datesBetween(r: Range): string[] {
  const out = new Set<string>()
  for (let t = r.from.getTime(); t < r.to.getTime(); t += DAY_MS / 2) {
    out.add(dayString(new Date(t), r.tz))
  }
  out.add(dayString(new Date(r.to.getTime() - 1), r.tz))
  return [...out].sort()
}

export function rangeQuery(r: Range) {
  return { from: r.from.toISOString(), to: r.to.toISOString(), tz: r.tz }
}

export function rangeKey(r: Range): string {
  return `${r.from.toISOString()}|${r.to.toISOString()}|${r.tz}`
}

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

/** The instant at which the day containing `d` began in `tz`. */
export function startOfDay(d: Date, tz: string): Date {
  const parts = new Intl.DateTimeFormat('en-US', {
    timeZone: tz,
    hourCycle: 'h23',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }).formatToParts(d)
  const get = (type: string) => Number(parts.find((p) => p.type === type)?.value ?? 0)
  const elapsed = ((get('hour') * 60 + get('minute')) * 60 + get('second')) * 1000 + d.getMilliseconds()
  return new Date(d.getTime() - elapsed)
}

export function presetRange(p: Preset, now = new Date(), tz = browserTz()): Range {
  const days = PRESET_DAYS[p]
  const from = startOfDay(new Date(now.getTime() - (days - 1) * DAY_MS), tz)
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

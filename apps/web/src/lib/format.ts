import type { Plan } from '../api/client'
import { dayString } from './range'

function trim(n: number, digits: number): string {
  return n.toFixed(digits).replace(/\.?0+$/, '')
}

const TOKEN_UNITS = [
  { div: 1e9, suffix: 'B', digits: 2 },
  { div: 1e6, suffix: 'M', digits: 1 },
  { div: 1e3, suffix: 'k', digits: 1 },
] as const

/** 950, 1.5k, 2M, 1.23B. A value that rounds up to 1000 of a unit moves to the next one (999,950 is "1M", not "1000k"). */
export function formatTokens(n: number): string {
  const abs = Math.abs(n)
  for (let i = 0; i < TOKEN_UNITS.length; i++) {
    const u = TOKEN_UNITS[i]
    if (abs < u.div) continue
    const text = trim(n / u.div, u.digits)
    if (i > 0 && Math.abs(Number(text)) >= 1000) {
      const up = TOKEN_UNITS[i - 1]
      return `${trim(n / up.div, up.digits)}${up.suffix}`
    }
    return `${text}${u.suffix}`
  }
  return String(n)
}

export function formatUSD(n: number): string {
  if (n > 0 && n < 0.005) return '<$0.01'
  if (Math.abs(n) >= 100) return `$${Math.round(n).toLocaleString('en-US')}`
  return `$${n.toFixed(2)}`
}

/**
 * Cost text for a totals block. Unpriced tokens carry no cost, so a $0 total that
 * is entirely unpriced reads "unpriced" rather than implying the usage was free.
 */
export function costLabel(t: { cost_usd: number; unpriced_tokens: number }): string {
  if (t.cost_usd === 0 && t.unpriced_tokens > 0) return 'unpriced'
  return formatUSD(t.cost_usd)
}

/**
 * Cost text for one chart cell (a series on a date). Never shows unpriced usage as $0:
 * "$x.xx" when fully priced, "$x.xx + N unpriced tokens" when partly priced, and
 * "unpriced (N tokens)" when the cell's cost is 0 but it has unpriced tokens.
 */
export function cellCost(cost: number, unpricedTokens: number, format: (n: number) => string = formatUSD): string {
  if (unpricedTokens <= 0) return format(cost)
  if (cost === 0) return `unpriced (${formatTokens(unpricedTokens)} tokens)`
  return `${format(cost)} + ${formatTokens(unpricedTokens)} unpriced tokens`
}

export function unpricedTitle(unpricedTokens: number): string {
  return `Includes ${formatTokens(unpricedTokens)} unpriced tokens (models with no price in the price table; not in the cost)`
}

export function formatPct(share: number): string {
  return `${Math.round(share * 100)}%`
}

export function formatDateTime(iso: string): string {
  return new Date(iso).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
}

export const PLANS: Plan[] = ['pro', 'max_5x', 'max_20x', 'team', 'enterprise', 'api_key', 'unknown']

export const PLAN_LABELS: Record<Plan, string> = {
  pro: 'Pro',
  max_5x: 'Max 5x',
  max_20x: 'Max 20x',
  team: 'Team',
  enterprise: 'Enterprise',
  api_key: 'API key',
  unknown: 'Plan not set',
}


/** Locale and time zone for the date helpers; both default to the browser's. */
export interface DateOpts {
  locale?: string
  timeZone?: string
}

/** Share of `part` in `whole`: "0%" for nothing, "<1%" for a sliver, else a rounded percentage. */
export function formatShare(part: number, whole: number): string {
  if (part <= 0 || whole <= 0) return '0%'
  const pct = (part / whole) * 100
  if (pct < 0.5) return '<1%'
  return `${Math.round(pct)}%`
}

/** Counts such as API calls: 1,234. */
export function formatCount(n: number): string {
  return Math.round(n).toLocaleString('en-US')
}

/** Time of day in the user's locale: 14:05 or 2:05 pm. */
export function formatTime(iso: string | number, o: DateOpts = {}): string {
  return new Date(iso).toLocaleTimeString(o.locale, { hour: '2-digit', minute: '2-digit', timeZone: o.timeZone })
}

function year(t: number, timeZone?: string): string {
  return dayString(new Date(t), timeZone ?? Intl.DateTimeFormat().resolvedOptions().timeZone).slice(0, 4)
}

/** Weekday, day and month ("Mon 13 Oct"), plus the year when it isn't the current one. */
export function formatDayLong(iso: string | number, now: number = Date.now(), o: DateOpts = {}): string {
  const t = new Date(iso).getTime()
  const withYear = year(t, o.timeZone) !== year(now, o.timeZone)
  return new Date(t).toLocaleDateString(o.locale, {
    weekday: 'short',
    day: 'numeric',
    month: 'short',
    year: withYear ? 'numeric' : undefined,
    timeZone: o.timeZone,
  })
}

/** Ledger day heading: "Today", "Yesterday", else the long day. */
export function formatDayGroup(iso: string | number, now: number = Date.now(), o: DateOpts = {}): string {
  const tz = o.timeZone ?? Intl.DateTimeFormat().resolvedOptions().timeZone
  const day = dayString(new Date(iso), tz)
  const today = dayString(new Date(now), tz)
  if (day === today) return 'Today'
  const [y, m, d] = today.split('-').map(Number)
  const yesterday = new Date(Date.UTC(y, m - 1, d - 1)).toISOString().slice(0, 10)
  if (day === yesterday) return 'Yesterday'
  return formatDayLong(iso, now, o)
}

const MIN = 60_000
const HOUR = 60 * MIN
const DAY = 24 * HOUR

/** "under a minute", "12 min", "1 h 12 min", "3 h", "2 d 4 h". */
export function formatDuration(ms: number): string {
  if (!(ms >= MIN)) return 'under a minute'
  if (ms < HOUR) return `${Math.floor(ms / MIN)} min`
  if (ms < DAY) {
    const h = Math.floor(ms / HOUR)
    const m = Math.floor((ms % HOUR) / MIN)
    return m === 0 ? `${h} h` : `${h} h ${m} min`
  }
  const d = Math.floor(ms / DAY)
  const h = Math.floor((ms % DAY) / HOUR)
  return h === 0 ? `${d} d` : `${d} d ${h} h`
}

/** "just now", "12 min ago", "5 h ago", else the long day. */
export function formatRelative(iso: string, now: number = Date.now(), o: DateOpts = {}): string {
  const diff = now - Date.parse(iso)
  if (diff < MIN) return 'just now'
  if (diff < HOUR) return `${Math.floor(diff / MIN)} min ago`
  if (diff < DAY) return `${Math.floor(diff / HOUR)} h ago`
  return formatDayLong(iso, now, o)
}

/**
 * When a plan window resets: "Resets in 2 h 14 min" within a day, else "Resets Mon 20 Oct, 09:00".
 * A reset time in the past reads "Reset at 14:00" (LimitMeter words that case itself).
 */
export function formatReset(iso: string, now: number = Date.now(), o: DateOpts = {}): string {
  const diff = Date.parse(iso) - now
  if (diff < 0) return `Reset at ${formatTime(iso, o)}`
  if (diff < DAY) return `Resets in ${formatDuration(diff)}`
  return `Resets ${formatDayLong(iso, now, o)}, ${formatTime(iso, o)}`
}

/**
 * Y-axis ticks for a cost chart: 0 up to at least `max` in round steps (1, 2, 2.5 or 5 × 10ⁿ),
 * at most `maxIntervals` intervals. Below $10 every step is a multiple of $0.50 once the
 * axis reaches $1.
 */
export function moneyTicks(max: number, maxIntervals = 5): number[] {
  if (!(max > 0)) return [0, 1]
  const raw = max / maxIntervals
  const mag = 10 ** Math.floor(Math.log10(raw))
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw - 1e-12)!
  const n = Math.ceil(max / step - 1e-9)
  return Array.from({ length: n + 1 }, (_, i) => Math.round(i * step * 100) / 100)
}

/** Y-axis money label: whole dollars when the axis reaches $10, cents below that. */
export function formatMoneyTick(v: number, axisMax: number): string {
  return axisMax >= 10 ? `$${Math.round(v).toLocaleString('en-US')}` : `$${v.toFixed(2)}`
}

/** Chart axis day from a YYYY-MM-DD string, read as a local date (never shifted by UTC): "13 Oct". */
export function formatDayTick(ymd: string, locale?: string): string {
  const [y, m, d] = ymd.split('-').map(Number)
  return new Date(y, m - 1, d).toLocaleDateString(locale, { day: 'numeric', month: 'short' })
}

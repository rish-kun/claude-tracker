import type { Schemas, Session } from '../api/client'
import { type DateOpts, formatCount, formatDayGroup, formatPct, formatTokens, formatUSD } from './format'
import { readTokens } from './tokens'

export { readTokens } from './tokens'

type SessionDetail = Schemas['SessionDetail']
type SessionPart = Schemas['SessionPart']

const MAX_PATH = 44

/**
 * A working directory for display: the home directory becomes "~", and a path longer than
 * 44 characters keeps its head ("~" or the first segment) and its last two segments.
 */
export function shortenPath(cwd: string): string {
  const p = cwd.replace(/^\/(?:Users|home)\/[^/]+/, '~')
  if (p.length <= MAX_PATH) return p
  const absolute = p.startsWith('/')
  const segs = p.split('/').filter((s, i) => s !== '' || i === 0)
  // segs[0] is "~", a relative first segment, or "" for an absolute path.
  const head = absolute ? `/${segs[1]}` : segs[0]
  const rest = absolute ? segs.slice(2) : segs.slice(1)
  if (rest.length <= 2) return p
  return `${head}/…/${rest.slice(-2).join('/')}`
}

/** Last segment of a path ("/a/b/" → "b"). */
export function basename(path: string): string {
  const segs = path.split(/[/\\]/).filter(Boolean)
  return segs[segs.length - 1] ?? ''
}

export interface SessionLabel {
  primary: string
  secondary?: string
  /** The primary is a project or folder name, not something written about the work. */
  isFallback: boolean
}

const clean = (s?: string) => (s && s.trim() !== '' ? s.trim() : undefined)

/** How to name a session: its title, else its summary, else its project or folder. */
export function sessionLabel(s: Pick<Session, 'title' | 'summary' | 'project' | 'cwd'>): SessionLabel {
  const title = clean(s.title)
  const summary = clean(s.summary)
  if (title) return { primary: title, secondary: summary, isFallback: false }
  if (summary) return { primary: summary, isFallback: false }
  const fallback = clean(s.project) ?? (s.cwd ? clean(basename(s.cwd)) : undefined)
  return { primary: fallback ?? 'Untitled session', isFallback: true }
}

export interface DayBucket<T> {
  label: string
  items: T[]
}

/** Consecutive runs of sessions on the same day (input is newest first), labelled for the ledger. */
export function groupByDay<T extends { started_at: string }>(items: T[], now: number = Date.now(), o: DateOpts = {}): DayBucket<T>[] {
  const out: DayBucket<T>[] = []
  for (const it of items) {
    const label = formatDayGroup(it.started_at, now, o)
    const last = out[out.length - 1]
    if (last && last.label === label) last.items.push(it)
    else out.push({ label, items: [it] })
  }
  return out
}

/** Average tokens read per call; 0 when there were no calls. */
export function avgContext(p: SessionPart): number {
  return p.calls > 0 ? Math.round(readTokens(p.totals) / p.calls) : 0
}

/**
 * Up to three plain-language reasons a session cost what it did (§6.3). When nothing is
 * priced (cost 0), comparisons use tokens read instead of cost.
 */
export function costDrivers(d: SessionDetail): string[] {
  const t = d.session.totals
  const cost = t.cost_usd
  const priced = cost > 0
  const read = readTokens(t)
  const weight = (x: { totals: Schemas['Totals'] }) => (priced ? x.totals.cost_usd : readTokens(x.totals))
  const whole = priced ? cost : read
  const out: string[] = []

  // 1. Subagents did most of it.
  const sub = d.subagents
  if (sub.calls > 0 && whole > 0 && weight(sub) >= 0.5 * whole) {
    const amount = priced
      ? `${formatUSD(sub.totals.cost_usd)} of ${formatUSD(cost)}`
      : `${formatTokens(readTokens(sub.totals))} of ${formatTokens(read)} tokens read`
    out.push(`Subagents did most of the work: ${formatCount(sub.calls)} of ${formatCount(d.calls)} calls and ${amount}.`)
  }

  // 2. Many calls over a large context, for whichever part cost more.
  const bigger = sub.calls > 0 && weight(sub) > weight(d.main) ? sub : d.main
  const isSub = bigger === sub
  const avg = avgContext(bigger)
  const rule2 = bigger.calls > 0 && avg >= 150_000
  if (rule2) {
    out.push(
      `Each ${isSub ? 'subagent call' : 'call in the main conversation'} re-read about ${formatTokens(avg)} tokens of context, ` +
        `so many calls multiplied a large context: ${formatCount(bigger.calls)} × ${formatTokens(avg)} ≈ ${formatTokens(readTokens(bigger.totals))} tokens read.`,
    )
  }

  // 3. Simply a lot of calls.
  if (!rule2 && d.calls >= 200) {
    out.push(`${formatCount(d.calls)} API calls. Every tool use is a separate call that re-reads the conversation.`)
  }

  // 4. Heavy cache writes.
  const writes = t.cache_write_5m + t.cache_write_1h
  if (read > 0 && writes >= 0.15 * read) {
    out.push(
      `${formatTokens(writes)} tokens were written to cache. That happens when context changes or the cache expires after a pause (5 minutes or 1 hour).`,
    )
  }

  // 5. One model dominates.
  if (d.by_model.length === 1) {
    out.push(`All calls used ${d.by_model[0].model}.`)
  } else if (d.by_model.length > 1 && whole > 0) {
    const top = [...d.by_model].sort((a, b) => weight(b) - weight(a))[0]
    const share = weight(top) / whole
    if (share >= 0.8) {
      out.push(`${top.model} accounts for ${formatPct(share)} of the ${priced ? 'API-equivalent cost' : 'tokens read'}.`)
    }
  }

  // 6. Nothing stood out.
  if (out.length === 0) {
    if (d.calls === 0) return ['No API calls were recorded for this session.']
    out.push(`Nothing unusual: ${formatCount(d.calls)} calls at about ${formatTokens(Math.round(read / d.calls))} tokens of context each.`)
  }
  return out.slice(0, 3)
}

/**
 * The "from–to" a session pager shows for page `page` holding `count` rows: 1-based and never
 * from > to. `null` when the page has no rows (it was emptied by deletions, or is past the end).
 */
export function pageRange(page: number, pageSize: number, count: number): { from: number; to: number } | null {
  if (count <= 0) return null
  const from = page * pageSize + 1
  return { from, to: from + count - 1 }
}

/** The last page (0-based) that has rows for `total` rows; 0 when there are none. */
export function lastPage(total: number, pageSize: number): number {
  return Math.max(0, Math.ceil(total / pageSize) - 1)
}

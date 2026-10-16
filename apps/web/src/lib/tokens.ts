import type { Totals } from '../api/client'
import { formatShare } from './format'

/** Everything sent to Claude: new input plus cache writes and reads. */
export function readTokens(t: Pick<Totals, 'input' | 'cache_write_5m' | 'cache_write_1h' | 'cache_read'>): number {
  return t.input + t.cache_write_5m + t.cache_write_1h + t.cache_read
}

/**
 * Bar lengths (0–1) comparing each totals block with the largest. Cost is compared only when
 * every block with usage has a priced cost; otherwise read tokens are, so a heavy but unpriced
 * user isn't drawn as a sliver and all-unpriced usage doesn't draw every bar full width.
 */
export function barScales(ts: Totals[]): { by: 'cost' | 'read'; scales: number[] } {
  const used = ts.filter((t) => t.cost_usd > 0 || readTokens(t) + t.output > 0)
  const by = used.length > 0 && used.every((t) => t.cost_usd > 0) ? 'cost' : 'read'
  const weight = (t: Totals) => (by === 'cost' ? t.cost_usd : readTokens(t))
  const max = Math.max(0, ...ts.map(weight))
  return { by, scales: ts.map((t) => (max > 0 ? weight(t) / max : 0)) }
}

/** What Claude produced. */
export function writtenTokens(t: Pick<Totals, 'output'>): number {
  return t.output
}

export type TokenKind = 'written' | 'input' | 'cachewrite' | 'cacheread'

export interface MixPart {
  kind: TokenKind
  label: string
  value: number
  /** Formatted share of all tokens ("96%", "<1%"). */
  share: string
  /** Fraction of all tokens, 0–1, for the bar. */
  fraction: number
}

/** The four token kinds in display order (brightest to dimmest): written, new input, cache writes, cache reads. */
export function tokenMix(t: Totals): MixPart[] {
  const total = t.input + t.output + t.cache_write_5m + t.cache_write_1h + t.cache_read
  const parts: [TokenKind, string, number][] = [
    ['written', 'Written', t.output],
    ['input', 'New input', t.input],
    ['cachewrite', 'Cache writes', t.cache_write_5m + t.cache_write_1h],
    ['cacheread', 'Cache reads', t.cache_read],
  ]
  return parts.map(([kind, label, value]) => ({
    kind,
    label,
    value,
    share: formatShare(value, total),
    fraction: total > 0 ? value / total : 0,
  }))
}

import { describe, expect, it } from 'vitest'
import {
  cellCost,
  costLabel,
  formatDayGroup,
  formatDayLong,
  formatDayTick,
  formatDuration,
  formatPct,
  formatRelative,
  formatReset,
  formatShare,
  formatTime,
  formatTokens,
  formatUSD,
  PLAN_LABELS,
  unpricedTitle,
} from './format'

describe('format', () => {
  it('formats token counts compactly', () => {
    expect(formatTokens(950)).toBe('950')
    expect(formatTokens(1500)).toBe('1.5k')
    expect(formatTokens(2_000_000)).toBe('2M')
    expect(formatTokens(1_234_567_890)).toBe('1.23B')
  })
  it('formats dollars', () => {
    expect(formatUSD(0)).toBe('$0.00')
    expect(formatUSD(3.456)).toBe('$3.46')
    expect(formatUSD(1234.5)).toBe('$1,235')
  })
  it('formats shares and plans', () => {
    expect(formatPct(0.756)).toBe('76%')
    expect(PLAN_LABELS.max_20x).toBe('Max 20x')
  })
  it('labels fully-unpriced cost as unpriced, never $0', () => {
    expect(costLabel({ cost_usd: 0, unpriced_tokens: 1500 })).toBe('unpriced')
    expect(costLabel({ cost_usd: 0, unpriced_tokens: 0 })).toBe('$0.00')
    expect(costLabel({ cost_usd: 2.5, unpriced_tokens: 1500 })).toBe('$2.50')
    expect(unpricedTitle(1500)).toMatch(/^Includes 1\.5k unpriced tokens/)
  })
})

describe('cellCost', () => {
  it('shows a fully-priced cell as a plain cost', () => {
    expect(cellCost(1.5, 0)).toBe('$1.50')
    expect(cellCost(0, 0)).toBe('$0.00')
  })
  it('appends unpriced tokens to a partly-priced cell', () => {
    expect(cellCost(1.5, 1200)).toBe('$1.50 + 1.2k unpriced tokens')
    expect(cellCost(250, 3_000_000)).toBe('$250 + 3M unpriced tokens')
  })
  it('never shows an unpriced cell as $0.00', () => {
    expect(cellCost(0, 42)).toBe('unpriced (42 tokens)')
    expect(cellCost(0, 1_500_000)).toBe('unpriced (1.5M tokens)')
  })
  it('uses the given cost formatter', () => {
    expect(cellCost(2, 10, (n) => `${n} USD`)).toBe('2 USD + 10 unpriced tokens')
  })
})

describe('formatting rules (§4.7)', () => {
  const o = { locale: 'en-GB', timeZone: 'UTC' }
  const now = Date.parse('2026-10-15T12:00:00Z')

  it('shows tiny but non-zero cost as <$0.01', () => {
    expect(formatUSD(0.004)).toBe('<$0.01')
    expect(formatUSD(0.005)).toBe('$0.01')
    expect(formatUSD(0)).toBe('$0.00')
  })
  it('formats shares', () => {
    expect(formatShare(0, 100)).toBe('0%')
    expect(formatShare(5, 0)).toBe('0%')
    expect(formatShare(0.4, 100)).toBe('<1%')
    expect(formatShare(0.5, 100)).toBe('1%')
    expect(formatShare(96.4, 100)).toBe('96%')
  })
  it('formats durations', () => {
    expect(formatDuration(30_000)).toBe('under a minute')
    expect(formatDuration(-5)).toBe('under a minute')
    expect(formatDuration(12 * 60_000)).toBe('12 min')
    expect(formatDuration(72 * 60_000)).toBe('1 h 12 min')
    expect(formatDuration(3 * 3_600_000)).toBe('3 h')
    expect(formatDuration(2 * 86_400_000 + 4 * 3_600_000 + 60_000)).toBe('2 d 4 h')
    expect(formatDuration(86_400_000)).toBe('1 d')
  })
  it('formats days and times', () => {
    expect(formatTime('2026-10-13T14:05:00Z', o)).toBe('14:05')
    expect(formatDayLong('2026-10-13T14:05:00Z', now, o)).toBe('Tue 13 Oct')
    expect(formatDayLong('2025-10-13T14:05:00Z', now, o)).toBe('Mon, 13 Oct 2025')
    expect(formatDayGroup('2026-10-15T01:00:00Z', now, o)).toBe('Today')
    expect(formatDayGroup('2026-10-14T23:59:00Z', now, o)).toBe('Yesterday')
    expect(formatDayGroup('2026-10-13T10:00:00Z', now, o)).toBe('Tue 13 Oct')
    expect(formatDayTick('2026-10-13', 'en-GB')).toBe('13 Oct')
  })
  it('formats relative times', () => {
    expect(formatRelative('2026-10-15T11:59:30Z', now, o)).toBe('just now')
    expect(formatRelative('2026-10-15T11:48:00Z', now, o)).toBe('12 min ago')
    expect(formatRelative('2026-10-15T07:00:00Z', now, o)).toBe('5 h ago')
    expect(formatRelative('2026-10-13T07:00:00Z', now, o)).toBe('Tue 13 Oct')
  })
  it('formats reset times', () => {
    expect(formatReset('2026-10-15T14:14:00Z', now, o)).toBe('Resets in 2 h 14 min')
    expect(formatReset('2026-10-20T09:00:00Z', now, o)).toBe('Resets Tue 20 Oct, 09:00')
    expect(formatReset('2026-10-15T11:00:00Z', now, o)).toBe('Reset at 11:00')
  })
  it('labels an unknown plan as not set', () => {
    expect(PLAN_LABELS.unknown).toBe('Plan not set')
  })
})

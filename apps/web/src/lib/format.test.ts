import { describe, expect, it } from 'vitest'
import { formatPct, formatTokens, formatUSD, PLAN_LABELS } from './format'

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
})

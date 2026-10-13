import { describe, expect, it } from 'vitest'
import { cellCost, costLabel, formatPct, formatTokens, formatUSD, PLAN_LABELS, unpricedTitle } from './format'

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

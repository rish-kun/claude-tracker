import { describe, expect, it } from 'vitest'
import type { Totals } from '../api/client'
import { barScales, readTokens, tokenMix } from './tokens'
import { runsText, usageSentence } from './usage'

const totals: Totals = {
  input: 212_000,
  output: 1_400_000,
  cache_write_5m: 2_000_000,
  cache_write_1h: 1_100_000,
  cache_read: 109_500_000,
  total_tokens: 114_212_000,
  cost_usd: 40.17,
  unpriced_tokens: 0,
}

describe('token split', () => {
  it('reads everything sent to Claude and writes only output', () => {
    expect(readTokens(totals)).toBe(212_000 + 3_100_000 + 109_500_000)
  })
  it('orders the mix written → new input → cache writes → cache reads with shares of all tokens', () => {
    const mix = tokenMix(totals)
    expect(mix.map((m) => [m.label, m.value, m.share])).toEqual([
      ['Written', 1_400_000, '1%'],
      ['New input', 212_000, '<1%'],
      ['Cache writes', 3_100_000, '3%'],
      ['Cache reads', 109_500_000, '96%'],
    ])
    expect(mix.reduce((n, m) => n + m.fraction, 0)).toBeCloseTo(1)
  })
  it('handles no usage', () => {
    expect(tokenMix({ ...totals, input: 0, output: 0, cache_write_5m: 0, cache_write_1h: 0, cache_read: 0 }).every((m) => m.share === '0%')).toBe(true)
  })
})

describe('usageSentence', () => {
  it('reads read and written, accounts and cost', () => {
    const s = usageSentence('30d', totals, 2)
    expect(runsText(s.runs)).toBe(
      'In the last 30 days, Claude read 112.8M tokens and wrote 1.4M for you across 2 accounts. At pay-as-you-go API prices that would be $40.17.',
    )
    expect(runsText(s.note)).toBe('API-equivalent only: what this usage would cost at API list prices.')
  })
  it('says nobody pays only when every account is on a flat-rate plan', () => {
    expect(runsText(usageSentence('30d', totals, 2, ['max_20x', 'max_5x']).note)).toBe(
      "API-equivalent only. The team's Max plans are flat-rate, so nobody pays this.",
    )
    expect(runsText(usageSentence('30d', totals, 2, ['max_20x', 'api_key']).note)).toBe(
      'API-equivalent only: what this usage would cost at API list prices.',
    )
    expect(runsText(usageSentence('30d', totals, 0, []).note)).toBe('API-equivalent only: what this usage would cost at API list prices.')
  })
  it('notes unpriced tokens and handles fully unpriced usage', () => {
    const s = usageSentence('today', { ...totals, cost_usd: 0, unpriced_tokens: 5000 }, 1)
    expect(runsText(s.runs)).toMatch(/^Today, .* across 1 account\. None of it is priced yet, so there's no API-equivalent figure\.$/)
    expect(runsText(s.note)).toMatch(/Excludes 5k tokens from models without a price\.$/)
  })
  it('says so when there is no usage', () => {
    const none = { ...totals, input: 0, output: 0, cache_write_5m: 0, cache_write_1h: 0, cache_read: 0, total_tokens: 0, cost_usd: 0 }
    const s = usageSentence('7d', none, 0)
    expect(runsText(s.runs)).toBe("In the last 7 days, there's no Claude Code usage from you yet.")
    expect(s.note).toContainEqual({ cmd: 'cctrack status' })
  })
})

describe('barScales', () => {
  const t = (cost_usd: number, cache_read: number, unpriced_tokens = 0): Totals => ({
    input: 0,
    output: 0,
    cache_write_5m: 0,
    cache_write_1h: 0,
    cache_read,
    total_tokens: cache_read,
    cost_usd,
    unpriced_tokens,
  })
  it('compares cost when every user with usage is priced', () => {
    expect(barScales([t(10, 100), t(5, 900), t(0, 0)])).toEqual({ by: 'cost', scales: [1, 0.5, 0] })
  })
  it('falls back to read tokens when someone has no priced cost', () => {
    expect(barScales([t(10, 100), t(0, 400, 400)])).toEqual({ by: 'read', scales: [0.25, 1] })
  })
  it('falls back to read tokens when nothing is priced', () => {
    expect(barScales([t(0, 200, 200), t(0, 100, 100)])).toEqual({ by: 'read', scales: [1, 0.5] })
  })
  it('handles no usage', () => {
    expect(barScales([t(0, 0)])).toEqual({ by: 'read', scales: [0] })
    expect(barScales([])).toEqual({ by: 'read', scales: [] })
  })
})

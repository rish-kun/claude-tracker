import { describe, expect, it } from 'vitest'
import type { Schemas, Session, Totals } from '../api/client'
import { avgContext, basename, costDrivers, groupByDay, sessionLabel, shortenPath } from './sessions'

const zero: Totals = { input: 0, output: 0, cache_write_5m: 0, cache_write_1h: 0, cache_read: 0, total_tokens: 0, cost_usd: 0, unpriced_tokens: 0 }
const t = (o: Partial<Totals>): Totals => {
  const x = { ...zero, ...o }
  return { ...x, total_tokens: x.input + x.output + x.cache_write_5m + x.cache_write_1h + x.cache_read }
}
const session = (totals: Totals, over: Partial<Session> = {}): Session => ({
  session_id: 's1',
  user: { id: 1, email: 'a@x', name: 'A' },
  started_at: '2026-10-13T14:05:00Z',
  ended_at: '2026-10-13T15:17:00Z',
  totals,
  ...over,
})
type Detail = Schemas['SessionDetail']

describe('shortenPath', () => {
  it('abbreviates the home directory', () => {
    expect(shortenPath('/Users/rishit/Coding/swd-site')).toBe('~/Coding/swd-site')
    expect(shortenPath('/home/rk/x')).toBe('~/x')
    expect(shortenPath('/opt/app')).toBe('/opt/app')
  })
  it('keeps the head and last two segments of a long path', () => {
    expect(shortenPath('/Users/rishit/Coding/clients/bits-pilani/dvm/Coding/dvm-merch')).toBe('~/…/Coding/dvm-merch')
    expect(shortenPath('/srv/projects/very/deeply/nested/folder/structure/here/app')).toBe('/srv/…/here/app')
  })
  it('leaves a long path with too few segments alone', () => {
    const p = '~/a-really-long-directory-name-that-goes-on/forever-and-ever'
    expect(shortenPath(p)).toBe(p)
  })
  it('takes a basename', () => {
    expect(basename('/a/b/')).toBe('b')
  })
})

describe('sessionLabel', () => {
  it('prefers title, then summary', () => {
    expect(sessionLabel({ title: 'T', summary: 'S' })).toEqual({ primary: 'T', secondary: 'S', isFallback: false })
    expect(sessionLabel({ summary: 'S' })).toEqual({ primary: 'S', isFallback: false })
    expect(sessionLabel({ title: '  ', summary: 'S' })).toEqual({ primary: 'S', isFallback: false })
  })
  it('falls back to project, then folder, then "Untitled session"', () => {
    expect(sessionLabel({ project: 'swd', cwd: '/x/y' })).toEqual({ primary: 'swd', isFallback: true })
    expect(sessionLabel({ cwd: '/Users/r/Coding/dvm-merch' })).toEqual({ primary: 'dvm-merch', isFallback: true })
    expect(sessionLabel({})).toEqual({ primary: 'Untitled session', isFallback: true })
  })
})

describe('groupByDay', () => {
  it('groups consecutive sessions by local day', () => {
    const now = Date.parse('2026-10-15T12:00:00Z')
    const items = ['2026-10-15T10:00:00Z', '2026-10-15T09:00:00Z', '2026-10-14T20:00:00Z', '2026-10-12T08:00:00Z'].map((started_at) => ({ started_at }))
    const g = groupByDay(items, now, { locale: 'en-GB', timeZone: 'UTC' })
    expect(g.map((x) => [x.label, x.items.length])).toEqual([
      ['Today', 2],
      ['Yesterday', 1],
      ['Mon 12 Oct', 1],
    ])
  })
})

describe('costDrivers', () => {
  // The spec's worked example: 333 calls, subagents 296 of them and $27.14 of $32.04.
  const main = { calls: 37, totals: t({ input: 1000, cache_read: 17_000_000, cache_write_5m: 999_000, output: 11_200, cost_usd: 4.9 }) }
  const sub = { calls: 296, totals: t({ input: 2000, cache_read: 100_000_000, cache_write_5m: 2_118_000, output: 37_000, cost_usd: 27.14 }) }
  const example: Detail = {
    session: session(t({ input: 3000, cache_read: 117_000_000, cache_write_5m: 3_117_000, output: 48_200, cost_usd: 32.04 })),
    calls: 333,
    peak_context_tokens: 412_000,
    main,
    subagents: sub,
    by_model: [
      { model: 'claude-opus-5-5', calls: 300, totals: t({ cost_usd: 29.2 }) },
      { model: 'claude-haiku-5', calls: 33, totals: t({ cost_usd: 2.84 }) },
    ],
  }

  it('explains the worked example', () => {
    expect(costDrivers(example)).toEqual([
      'Subagents did most of the work: 296 of 333 calls and $27.14 of $32.04.',
      'Each subagent call re-read about 345k tokens of context, so many calls multiplied a large context: 296 × 345k ≈ 102.1M tokens read.',
      'claude-opus-5-5 accounts for 91% of the API-equivalent cost.',
    ])
  })

  it('keeps at most three reasons', () => {
    const heavyWrites: Detail = {
      ...example,
      session: session(t({ input: 3000, cache_read: 60_000_000, cache_write_5m: 60_000_000, output: 48_200, cost_usd: 32.04 })),
    }
    expect(costDrivers(heavyWrites)).toHaveLength(3)
    expect(costDrivers(heavyWrites)[2]).toMatch(/tokens were written to cache/)
  })

  it('counts calls when the context is small', () => {
    const d: Detail = {
      session: session(t({ input: 100_000, cache_read: 2_000_000, output: 5000, cost_usd: 3 })),
      calls: 250,
      peak_context_tokens: 20_000,
      main: { calls: 250, totals: t({ input: 100_000, cache_read: 2_000_000, output: 5000, cost_usd: 3 }) },
      subagents: { calls: 0, totals: zero },
      by_model: [{ model: 'claude-sonnet-5', calls: 250, totals: t({ cost_usd: 3 }) }],
    }
    expect(costDrivers(d)).toEqual([
      '250 API calls. Every tool use is a separate call that re-reads the conversation.',
      'All calls used claude-sonnet-5.',
    ])
  })

  it('says nothing unusual when no rule fires', () => {
    const d: Detail = {
      session: session(t({ input: 50_000, cache_read: 450_000, output: 2000, cost_usd: 0.4 })),
      calls: 10,
      peak_context_tokens: 60_000,
      main: { calls: 10, totals: t({ input: 50_000, cache_read: 450_000, output: 2000, cost_usd: 0.4 }) },
      subagents: { calls: 0, totals: zero },
      by_model: [
        { model: 'a', calls: 6, totals: t({ cost_usd: 0.2 }) },
        { model: 'b', calls: 4, totals: t({ cost_usd: 0.2 }) },
      ],
    }
    expect(costDrivers(d)).toEqual(['Nothing unusual: 10 calls at about 50k tokens of context each.'])
  })

  it('compares tokens read when nothing is priced', () => {
    const unpriced = (o: Partial<Totals>) => t({ ...o, unpriced_tokens: 1 })
    const d: Detail = {
      session: session(unpriced({ cache_read: 1_000_000 })),
      calls: 20,
      peak_context_tokens: 90_000,
      main: { calls: 5, totals: unpriced({ cache_read: 200_000 }) },
      subagents: { calls: 15, totals: unpriced({ cache_read: 800_000 }) },
      by_model: [
        { model: 'x', calls: 19, totals: unpriced({ cache_read: 900_000 }) },
        { model: 'y', calls: 1, totals: unpriced({ cache_read: 100_000 }) },
      ],
    }
    expect(costDrivers(d)).toEqual([
      'Subagents did most of the work: 15 of 20 calls and 800k of 1M tokens read.',
      'x accounts for 90% of the tokens read.',
    ])
  })

  it('averages context per call', () => {
    expect(avgContext({ calls: 0, totals: zero })).toBe(0)
    expect(avgContext({ calls: 4, totals: t({ cache_read: 10 }) })).toBe(3)
  })
})

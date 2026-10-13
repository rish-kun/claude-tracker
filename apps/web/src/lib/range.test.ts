import { describe, expect, it } from 'vitest'
import { datesBetween, dayString, presetRange, rangeQuery, startOfDay } from './range'

describe('range', () => {
  it('computes day strings in a timezone', () => {
    const d = new Date('2026-10-13T20:00:00Z')
    expect(dayString(d, 'UTC')).toBe('2026-10-13')
    expect(dayString(d, 'Asia/Kolkata')).toBe('2026-10-14')
    expect(startOfDay(d, 'Asia/Kolkata').toISOString()).toBe('2026-10-13T18:30:00.000Z')
  })

  it('builds a 7-day preset ending now', () => {
    const now = new Date('2026-10-13T10:00:00Z')
    const r = presetRange('7d', now, 'UTC')
    expect(r.from.toISOString()).toBe('2026-10-07T00:00:00.000Z')
    expect(r.to.getTime()).toBeGreaterThan(now.getTime())
    const days = datesBetween(r)
    expect(days).toHaveLength(7)
    expect(days[0]).toBe('2026-10-07')
    expect(days[6]).toBe('2026-10-13')
    expect(rangeQuery(r)).toEqual({ from: r.from.toISOString(), to: r.to.toISOString(), tz: 'UTC' })
  })

  it('lists each day exactly once across a DST change', () => {
    const r = presetRange('7d', new Date('2026-11-03T12:00:00Z'), 'America/New_York') // DST ends Nov 1
    const days = datesBetween(r)
    expect(days).toEqual(['2026-10-28', '2026-10-29', '2026-10-30', '2026-10-31', '2026-11-01', '2026-11-02', '2026-11-03'])
  })
})

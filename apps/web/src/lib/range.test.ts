import { describe, expect, it } from 'vitest'
import { datesBetween, dayString, msUntilRangeTick, nextMinute, presetRange, rangeAdvanced, rangeKey, rangeQuery, startOfDay } from './range'

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

  describe('DST boundaries in America/New_York', () => {
    const NY = 'America/New_York'

    it("'today' on the spring-forward day starts at local midnight (EST)", () => {
      const r = presetRange('today', new Date('2026-03-08T16:00:00Z'), NY) // 12:00 EDT
      expect(r.from.toISOString()).toBe('2026-03-08T05:00:00.000Z')
      expect(datesBetween(r)).toEqual(['2026-03-08'])
    })

    it("'today' on the fall-back day starts at local midnight (EDT)", () => {
      const r = presetRange('today', new Date('2026-11-01T17:00:00Z'), NY) // 12:00 EST
      expect(r.from.toISOString()).toBe('2026-11-01T04:00:00.000Z')
      expect(datesBetween(r)).toEqual(['2026-11-01'])
    })

    it("'7d' just after midnight a week past spring-forward covers exactly 7 days", () => {
      const r = presetRange('7d', new Date('2026-03-14T04:30:00Z'), NY) // 00:30 EDT
      expect(r.from.toISOString()).toBe('2026-03-08T05:00:00.000Z')
      expect(datesBetween(r)).toEqual(['2026-03-08', '2026-03-09', '2026-03-10', '2026-03-11', '2026-03-12', '2026-03-13', '2026-03-14'])
    })

    it("'7d' late in the evening across fall-back covers exactly 7 days", () => {
      const r = presetRange('7d', new Date('2026-11-07T04:30:00Z'), NY) // 23:30 EST on Nov 6
      expect(r.from.toISOString()).toBe('2026-10-31T04:00:00.000Z')
      expect(datesBetween(r)).toEqual(['2026-10-31', '2026-11-01', '2026-11-02', '2026-11-03', '2026-11-04', '2026-11-05', '2026-11-06'])
    })

    it("'7d' at midday a week past spring-forward covers exactly 7 days", () => {
      const r = presetRange('7d', new Date('2026-03-14T16:00:00Z'), NY) // 12:00 EDT
      expect(r.from.toISOString()).toBe('2026-03-08T05:00:00.000Z')
      expect(datesBetween(r)).toHaveLength(7)
      expect(datesBetween(r)[0]).toBe('2026-03-08')
    })
  })

  it('starts the day at the transition when DST skips midnight (America/Santiago)', () => {
    // 2026-09-06 00:00 -04 jumps to 01:00 -03, so the day begins at 04:00Z.
    const tz = 'America/Santiago'
    expect(startOfDay(new Date('2026-09-06T15:00:00Z'), tz).toISOString()).toBe('2026-09-06T04:00:00.000Z')
    const r = presetRange('today', new Date('2026-09-06T15:00:00Z'), tz)
    expect(datesBetween(r)).toEqual(['2026-09-06'])
  })

  describe('advancing ranges', () => {
    it('rounds `to` up to the next whole minute', () => {
      expect(new Date(nextMinute(Date.parse('2026-10-13T10:00:00.000Z'))).toISOString()).toBe('2026-10-13T10:01:00.000Z')
      expect(new Date(nextMinute(Date.parse('2026-10-13T10:00:59.999Z'))).toISOString()).toBe('2026-10-13T10:01:00.000Z')
      const r = presetRange('30d', new Date('2026-10-13T10:00:12.345Z'), 'UTC')
      expect(r.to.toISOString()).toBe('2026-10-13T10:01:00.000Z')
    })

    it('keeps the same key within a minute and advances it on the next one', () => {
      const a = rangeKey(presetRange('7d', new Date('2026-10-13T10:00:01Z'), 'UTC'))
      const b = rangeKey(presetRange('7d', new Date('2026-10-13T10:00:58Z'), 'UTC'))
      const c = rangeKey(presetRange('7d', new Date('2026-10-13T10:01:00Z'), 'UTC'))
      expect(a).toBe(b)
      expect(c).not.toBe(b)
      expect(rangeAdvanced(b, c)).toBe(true)
    })

    it("moves 'today' to the new day as soon as local midnight passes", () => {
      const tz = 'Asia/Kolkata'
      const before = presetRange('today', new Date('2026-10-13T18:29:30Z'), tz) // 23:59:30 IST
      const after = presetRange('today', new Date('2026-10-13T18:30:00Z'), tz) // 00:00 IST
      expect(datesBetween(before)).toEqual(['2026-10-13'])
      expect(after.from.toISOString()).toBe('2026-10-13T18:30:00.000Z')
      expect(datesBetween(after)).toEqual(['2026-10-14'])
      expect(rangeAdvanced(rangeKey(before), rangeKey(after))).toBe(false)
    })

    it('gives the same range for any instant in the minute, including its last ms', () => {
      const tz = 'Asia/Kolkata'
      const t = Date.parse('2026-10-13T18:29:05Z') // 23:59:05 IST, last minute of the day
      const last = new Date(nextMinute(t) - 1)
      expect(rangeKey(presetRange('today', last, tz))).toBe(rangeKey(presetRange('today', new Date(t), tz)))
    })

    it('schedules the next tick on the minute boundary, which includes midnight', () => {
      expect(msUntilRangeTick(Date.parse('2026-10-13T18:29:30Z'))).toBe(30_000)
      expect(msUntilRangeTick(Date.parse('2026-10-13T18:30:00Z'))).toBe(60_000)
    })

    it('treats a different preset or timezone as a new range, not an advance', () => {
      const now = new Date('2026-10-13T10:00:00Z')
      const k7 = rangeKey(presetRange('7d', now, 'UTC'))
      expect(rangeAdvanced(k7, rangeKey(presetRange('30d', now, 'UTC')))).toBe(false)
      expect(rangeAdvanced(k7, rangeKey(presetRange('7d', now, 'Asia/Kolkata')))).toBe(false)
      expect(rangeAdvanced(k7, k7)).toBe(true)
    })
  })
})

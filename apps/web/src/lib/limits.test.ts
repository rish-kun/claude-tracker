import { describe, expect, it } from 'vitest'
import type { LimitSnapshot } from '../api/client'
import { filledCells, formatTimeTick, limitLevel, limitPressure, limitTimeline, type ResetMarker, selectResetMarkers, timeTicks, windowState } from './limits'

const snap = (captured_at: string, extra: Partial<LimitSnapshot> = {}): LimitSnapshot => ({ account_id: 1, captured_at, ...extra })

describe('limitTimeline', () => {
  it('puts points on a numeric time axis in capture order, keeping gaps proportional', () => {
    const d = limitTimeline([
      snap('2026-10-13T12:00:00Z', { five_hour_pct: 40 }),
      snap('2026-10-13T10:00:00Z', { five_hour_pct: 10, seven_day_pct: 5 }),
      snap('2026-10-13T10:30:00Z', { seven_day_pct: 6 }),
    ])
    expect(d.points.map((p) => p.t)).toEqual([
      Date.parse('2026-10-13T10:00:00Z'),
      Date.parse('2026-10-13T10:30:00Z'),
      Date.parse('2026-10-13T12:00:00Z'),
    ])
    expect(d.points[1]).toEqual({ t: Date.parse('2026-10-13T10:30:00Z'), five: null, seven: 6 })
    expect(d.domain).toEqual([Date.parse('2026-10-13T10:00:00Z'), Date.parse('2026-10-13T12:00:00Z')])
  })

  it('marks each distinct reset within the domain once, ignoring ones outside it', () => {
    const d = limitTimeline([
      snap('2026-10-13T08:00:00Z', { five_hour_resets_at: '2026-10-13T07:00:00Z', seven_day_resets_at: '2026-10-15T00:00:00Z' }), // before / after domain
      snap('2026-10-13T09:00:00Z', { five_hour_resets_at: '2026-10-13T10:00:00Z' }),
      snap('2026-10-13T09:30:00Z', { five_hour_resets_at: '2026-10-13T10:00:00.400Z' }), // same window, jittered
      snap('2026-10-13T11:00:00Z', { five_hour_resets_at: '2026-10-13T15:00:00Z', seven_day_resets_at: '2026-10-13T10:30:00Z' }),
      snap('2026-10-13T12:00:00Z'),
    ])
    expect(d.resets).toEqual([
      { t: Date.parse('2026-10-13T10:00:00Z'), label: '5h reset' },
      { t: Date.parse('2026-10-13T10:30:00Z'), label: '7d reset' },
    ])
  })

  it('has no markers or points for no snapshots', () => {
    expect(limitTimeline([])).toEqual({ points: [], domain: [0, 0], resets: [] })
  })
})

describe('formatTimeTick', () => {
  const t = Date.parse('2026-10-13T14:05:00Z')
  it('shows the time for short spans and only the date for long ones', () => {
    expect(formatTimeTick(t, 6 * 3_600_000, 'en-US', 'UTC')).toBe('Oct 13, 02:05 PM')
    expect(formatTimeTick(t, 30 * 86_400_000, 'en-US', 'UTC')).toBe('Oct 13')
  })
})

describe('selectResetMarkers', () => {
  const H = 3_600_000
  const D = 24 * H
  const t0 = Date.parse('2026-10-01T00:00:00Z')
  const five = (n: number, from = t0): ResetMarker[] => Array.from({ length: n }, (_, i) => ({ t: from + (i + 1) * 5 * H, label: '5h reset' }))
  const seven = (n: number): ResetMarker[] => Array.from({ length: n }, (_, i) => ({ t: t0 + (i + 1) * 7 * D, label: '7d reset' }))

  it('draws both series in time order when the domain spans at most 3 days', () => {
    const resets = [...seven(1), ...five(3)]
    const out = selectResetMarkers(resets, [t0, t0 + 3 * D])
    expect(out.map((r) => r.label)).toEqual(['5h reset', '5h reset', '5h reset', '7d reset'])
    expect(out.map((r) => r.t)).toEqual([...out.map((r) => r.t)].sort((a, b) => a - b))
  })

  it('drops 5-hour markers on longer domains but keeps 7-day ones', () => {
    const out = selectResetMarkers([...five(20), ...seven(2)], [t0, t0 + 3 * D + 1])
    expect(out).toEqual(seven(2))
  })

  it('keeps only the latest 30 markers per series', () => {
    const many = five(100, t0 - 100 * 5 * H)
    const out = selectResetMarkers([...many, ...seven(2)], [t0 - 2 * D, t0])
    const fives = out.filter((r) => r.label === '5h reset')
    expect(fives).toHaveLength(30)
    expect(fives).toEqual(many.slice(-30))
    expect(out.filter((r) => r.label === '7d reset')).toHaveLength(2)

    const sevens = seven(45)
    expect(selectResetMarkers(sevens, [t0, t0 + 400 * D])).toEqual(sevens.slice(-30))
  })

  it('is applied by limitTimeline', () => {
    const snaps = [snap('2026-10-01T00:00:00Z', { five_hour_resets_at: '2026-10-01T05:00:00Z', seven_day_resets_at: '2026-10-03T00:00:00Z' }), snap('2026-10-06T00:00:00Z')]
    expect(limitTimeline(snaps).resets).toEqual([{ t: Date.parse('2026-10-03T00:00:00Z'), label: '7d reset' }])
  })
})

describe('limit meter levels', () => {
  it('bands by percentage', () => {
    expect(limitLevel(0)).toBe('ok')
    expect(limitLevel(69.9)).toBe('ok')
    expect(limitLevel(70)).toBe('high')
    expect(limitLevel(89)).toBe('high')
    expect(limitLevel(90)).toBe('crit')
    expect(limitLevel(120)).toBe('crit')
  })
  it('fills one cell per 5%, at least one for any usage, never more than 20', () => {
    expect(filledCells(0)).toBe(0)
    expect(filledCells(0.4)).toBe(1)
    expect(filledCells(62)).toBe(12)
    expect(filledCells(74)).toBe(15)
    expect(filledCells(100)).toBe(20)
    expect(filledCells(140)).toBe(20)
  })
  it('treats a window whose reset has passed as reset', () => {
    const now = Date.parse('2026-10-15T12:00:00Z')
    expect(windowState(undefined, undefined, now)).toEqual({ kind: 'none' })
    expect(windowState(62.4, '2026-10-15T11:00:00Z', now)).toEqual({ kind: 'reset', was: 62, resetAt: '2026-10-15T11:00:00Z' })
    expect(windowState(74, '2026-10-20T09:00:00Z', now)).toMatchObject({ kind: 'live', level: 'high', filled: 15 })
  })
  it('measures pressure as the fuller live window', () => {
    const now = Date.parse('2026-10-15T12:00:00Z')
    expect(limitPressure(undefined, now)).toBeUndefined()
    expect(limitPressure(snap('2026-10-15T11:00:00Z'), now)).toBeUndefined()
    expect(
      limitPressure(snap('2026-10-15T11:00:00Z', { five_hour_pct: 95, five_hour_resets_at: '2026-10-15T11:30:00Z', seven_day_pct: 40 }), now),
    ).toBe(40)
  })
})

describe('timeTicks', () => {
  const H = 3_600_000
  const D = 24 * H

  it('puts one tick per local midnight (or fewer) on spans over two days, never repeating a day', () => {
    const from = new Date(2026, 9, 9, 14, 30).getTime()
    const to = new Date(2026, 9, 15, 22, 0).getTime()
    const ticks = timeTicks([from, to])
    expect(ticks.length).toBeGreaterThan(1)
    expect(ticks.length).toBeLessThanOrEqual(7)
    for (const t of ticks) {
      const d = new Date(t)
      expect([d.getHours(), d.getMinutes()]).toEqual([0, 0])
      expect(t).toBeGreaterThanOrEqual(from)
      expect(t).toBeLessThanOrEqual(to)
    }
    expect(new Set(ticks.map((t) => new Date(t).toDateString())).size).toBe(ticks.length)
    expect(new Date(ticks[0]).getDate()).toBe(10)
  })

  it('spaces days further apart on long spans', () => {
    const from = new Date(2026, 6, 1).getTime()
    const ticks = timeTicks([from, from + 90 * D])
    expect(ticks.length).toBeLessThanOrEqual(7)
    expect(ticks[1] - ticks[0]).toBeGreaterThanOrEqual(14 * D - 2 * H)
  })

  it('uses aligned whole hours on short spans', () => {
    const from = new Date(2026, 9, 15, 8, 20).getTime()
    const ticks = timeTicks([from, from + 30 * H])
    expect(ticks.length).toBeLessThanOrEqual(7)
    for (const t of ticks) {
      const d = new Date(t)
      expect(d.getMinutes()).toBe(0)
      expect(d.getHours() % 6).toBe(0)
      expect(t).toBeGreaterThanOrEqual(from)
    }
    expect(new Date(ticks[0]).getHours()).toBe(12)
  })

  it('handles an empty span', () => {
    expect(timeTicks([5, 5])).toEqual([5])
  })
})

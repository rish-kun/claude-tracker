import { type CSSProperties, type ReactNode, useEffect, useState } from 'react'
import type { LimitSnapshot, Plan } from '../api/client'
import { formatRelative, formatReset, formatTime } from '../lib/format'
import { explain } from '../lib/glossary'
import { METER_CELLS, STALE_READING_MS, windowState } from '../lib/limits'
import { PlanChip } from './figures'
import { Glyph } from './ui'

const INTRO_MS = METER_CELLS * 12 + 260

export type WindowTitle = '5-hour window' | '7-day window'

/**
 * One plan window as 20 cells of 5%, echoing Claude Code's /usage bars. Warm colour only
 * from 70%. A window whose reset time has passed shows empty dashed cells and the old reading.
 */
export function LimitMeter({
  title,
  pct,
  resetsAt,
  now,
  size = 'md',
}: {
  title: WindowTitle
  pct?: number
  resetsAt?: string
  now: number
  size?: 'md' | 'lg'
}) {
  // Cells fade in left to right on first render only; refetches don't replay it.
  const [intro, setIntro] = useState(true)
  useEffect(() => {
    const t = setTimeout(() => setIntro(false), INTRO_MS)
    return () => clearTimeout(t)
  }, [])

  const s = windowState(pct, resetsAt, now)
  if (s.kind === 'none') return null

  const label = title === '5-hour window' ? '5-hour' : '7-day'
  let figure: string
  let status: ReactNode
  let valueText: string
  if (s.kind === 'reset') {
    figure = '–'
    const text = `Reset at ${formatTime(s.resetAt)}. Was ${s.was}% before.`
    status = <span title={explain('windowReset')}>{text}</span>
    valueText = text
  } else {
    figure = `${Math.round(s.pct)}%`
    const reset = s.resetsAt ? formatReset(s.resetsAt, now) : ''
    const levelText = s.level === 'crit' ? 'Nearly full.' : s.level === 'high' ? 'Getting full.' : ''
    status = (
      <>
        {levelText && (
          <span className="level" data-level={s.level} title={explain(s.level === 'crit' ? 'nearlyFull' : 'gettingFull')}>
            <Glyph name="alert" />
            {levelText}
          </span>
        )}
        {reset}
      </>
    )
    valueText = [`${Math.round(s.pct)}% used`, levelText.replace(/\.$/, ''), reset].filter(Boolean).join(', ')
  }

  return (
    <div
      className="meter-row"
      data-size={size}
      role="meter"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={s.kind === 'live' ? Math.round(Math.min(100, s.pct)) : 0}
      aria-label={title}
      aria-valuetext={valueText}
    >
      <span className="meter-label" title={explain(title === '5-hour window' ? 'fiveHour' : 'sevenDay')}>
        {label}
      </span>
      <span className="meter" data-level={s.kind === 'live' ? s.level : undefined} data-reset={s.kind === 'reset' ? '' : undefined} data-intro={intro ? '' : undefined}>
        {Array.from({ length: METER_CELLS }, (_, i) => (
          <span
            key={i}
            className="meter-cell"
            data-on={s.kind === 'live' && i < s.filled ? '' : undefined}
            style={{ '--i': i } as CSSProperties}
          />
        ))}
      </span>
      <span className="meter-fig">{figure}</span>
      <span className="meter-status">{status}</span>
    </div>
  )
}

export interface LimitEntry {
  name?: string
  plan?: Plan
  accountId: number
  limit: LimitSnapshot
}

/** Latest plan-window readings, one block per account, in a raised panel. */
export function LimitStrip({ entries, now, size = 'md', showNames = true }: { entries: LimitEntry[]; now: number; size?: 'md' | 'lg'; showNames?: boolean }) {
  return (
    <div className="panel limit-strip">
      {entries.map((e) => {
        const stale = now - Date.parse(e.limit.captured_at) > STALE_READING_MS
        const reported = formatRelative(e.limit.captured_at, now)
        return (
          <div key={e.accountId} className="limit-block">
            <div className="limit-head">
              {showNames && <span className="limit-name">{e.name ?? `Account ${e.accountId}`}</span>}
              {showNames && e.plan && <PlanChip plan={e.plan} settingsLink={e.accountId} />}
              <span className="limit-reported" data-stale={stale ? '' : undefined} title={explain('reported')}>
                Reported {reported}
                {stale && ', may be out of date'}
              </span>
            </div>
            <div className="limit-meters">
              <LimitMeter title="5-hour window" pct={e.limit.five_hour_pct} resetsAt={e.limit.five_hour_resets_at} now={now} size={size} />
              <LimitMeter title="7-day window" pct={e.limit.seven_day_pct} resetsAt={e.limit.seven_day_resets_at} now={now} size={size} />
            </div>
          </div>
        )
      })}
    </div>
  )
}

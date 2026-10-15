import { Link } from '@tanstack/react-router'
import { type ReactNode, useId, useRef } from 'react'
import type { Session, SessionPage } from '../api/client'
import { PAGE_SIZE } from '../api/hooks'
import { costLabel, formatDayLong, formatDuration, formatTime, formatTokens } from '../lib/format'
import { useNow } from '../lib/rangeContext'
import { groupByDay, readTokens, sessionLabel, shortenPath } from '../lib/sessions'
import { WeightBar } from './charts'
import { CostFigure, Fact, FactList, PlanChip } from './figures'
import { Button, EmptyState, ErrorNotice, Skeleton } from './ui'

const BRANCH_MAX = 28

function truncate(s: string, n: number): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s
}

export function DayGroup({ label, children }: { label: string; children: ReactNode }) {
  const id = useId()
  return (
    <section className="day-group" aria-labelledby={id}>
      <h3 id={id} className="day-head">
        {label}
      </h3>
      {children}
    </section>
  )
}

/** One session as a ledger entry; the whole row links to its detail page. */
export function SessionRow({ s, showUser, maxCost, compact = false }: { s: Session; showUser: boolean; maxCost: number; compact?: boolean }) {
  const now = useNow()
  const label = sessionLabel(s)
  const time = formatTime(s.started_at)
  const duration = formatDuration(Date.parse(s.ended_at) - Date.parse(s.started_at))
  const cost = s.totals.cost_usd === 0 && s.totals.unpriced_tokens > 0 ? 'not priced' : costLabel(s.totals)
  const who = s.user.name || s.user.email
  return (
    <Link
      to="/sessions/$sessionId"
      params={{ sessionId: s.session_id }}
      className="entry"
      data-compact={compact ? '' : undefined}
      aria-label={`${label.primary}, ${formatDayLong(s.started_at, now)} ${time}, API-equivalent ${cost}`}
    >
      <span className="entry-time">
        <span>{time}</span>
        <span className="dur">{duration}</span>
      </span>
      <span className="entry-body">
        <span className="entry-title">{label.primary}</span>
        {label.secondary && <span className="entry-summary">{label.secondary}</span>}
        <FactList>
          {showUser && (
            <Fact glyph="person" title={s.user.email}>
              {who}
            </Fact>
          )}
          {s.cwd ? (
            <Fact glyph="folder" title={s.cwd} mono>
              {shortenPath(s.cwd)}
            </Fact>
          ) : (
            s.project && (
              <Fact glyph="folder" title={s.project}>
                {s.project}
              </Fact>
            )
          )}
          {s.git_branch && (
            <Fact glyph="branch" title={s.git_branch} mono>
              {truncate(s.git_branch, BRANCH_MAX)}
            </Fact>
          )}
          {!compact && s.device_hostname && (
            <Fact glyph="device" title={s.device_hostname}>
              {s.device_hostname}
            </Fact>
          )}
          {s.account && (
            <Fact glyph="account" title={s.account.label}>
              {s.account.label || s.account.email || `Account ${s.account.id}`} <PlanChip plan={s.account.plan} size="sm" />
            </Fact>
          )}
        </FactList>
      </span>
      <span className="entry-foot">
        <span className="entry-when-phone">
          {time}, {duration}
        </span>
        <span className="entry-nums">
          <span className="cost">
            <CostFigure totals={s.totals} />
          </span>
          <WeightBar value={s.totals.cost_usd} max={maxCost} />
          <span className="tokens-read">{formatTokens(readTokens(s.totals))} read</span>
          <span className="tokens-written">{formatTokens(s.totals.output)} written</span>
        </span>
      </span>
    </Link>
  )
}

export function Pager({ page, total, pageSize, onPage }: { page: number; total: number; pageSize: number; onPage: (p: number) => void }) {
  const from = total === 0 ? 0 : page * pageSize + 1
  const to = Math.min(total, (page + 1) * pageSize)
  return (
    <nav className="pager" aria-label="Session pages">
      <Button variant="secondary" disabled={page === 0} onClick={() => onPage(page - 1)}>
        Newer
      </Button>
      <span className="range">
        {from}–{to} of {total}
      </span>
      <Button variant="secondary" disabled={to >= total} onClick={() => onPage(page + 1)}>
        Older
      </Button>
    </nav>
  )
}

function LedgerSkeleton() {
  return (
    <div aria-busy="true" aria-label="Loading sessions" className="ledger-list">
      {Array.from({ length: 5 }, (_, i) => (
        <div key={i} style={{ padding: '14px 12px' }}>
          <Skeleton width="60%" height={16} />
          <Skeleton width="40%" height={12} />
          <Skeleton width="30%" height={10} />
        </div>
      ))}
    </div>
  )
}

/**
 * Sessions as a day-grouped logbook (§6.1): newest first, one linked entry per session,
 * with a weight bar scaled to the most expensive session on the page.
 */
export function SessionLedger({
  data,
  error,
  page,
  onPage,
  showUser = false,
  variant = 'full',
  onRetry,
  errorWhat = 'load your sessions',
  empty,
}: {
  data?: SessionPage
  error?: Error | null
  page: number
  onPage: (p: number) => void
  showUser?: boolean
  variant?: 'full' | 'compact'
  onRetry?: () => void
  errorWhat?: string
  empty?: ReactNode
}) {
  const now = useNow()
  const top = useRef<HTMLDivElement>(null)
  const err = error ? <ErrorNotice what={errorWhat} error={error} onRetry={onRetry} /> : null
  if (!data) return err ?? <LedgerSkeleton />
  if (data.items.length === 0 && page === 0) {
    return (
      err ??
      (empty ?? (
        <EmptyState title="No sessions yet.">
          Sessions appear here after Claude Code syncs through the cctrack plugin. Run <code>cctrack status</code> to check you're logged in.
        </EmptyState>
      ))
    )
  }
  const maxCost = Math.max(0, ...data.items.map((s) => s.totals.cost_usd))
  const go = (p: number) => {
    onPage(p)
    top.current?.scrollIntoView({ block: 'start' })
  }
  return (
    <div ref={top} className="ledger-list">
      {err}
      {groupByDay(data.items, now).map((g) => (
        <DayGroup key={`${g.label}-${g.items[0].session_id}`} label={g.label}>
          {g.items.map((s) => (
            <SessionRow key={s.session_id} s={s} showUser={showUser} maxCost={maxCost} compact={variant === 'compact'} />
          ))}
        </DayGroup>
      ))}
      <Pager page={page} total={data.total} pageSize={PAGE_SIZE} onPage={go} />
    </div>
  )
}

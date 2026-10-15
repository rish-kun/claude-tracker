import { useState } from 'react'
import type { BreakdownRow, Session, Usage } from '../api/client'
import { useMySessions, useMyUsage } from '../api/hooks'
import { StackedDailyBars } from '../components/charts'
import { CostFigure, TokenMix, UnpricedNote } from '../components/figures'
import { PageHeader } from '../components/Layout'
import { SessionLedger } from '../components/ledger'
import { type LimitEntry, LimitStrip } from '../components/limits'
import { Button, ErrorNotice, Glyph, Section, Skeleton, useDisclosure } from '../components/ui'
import { formatTokens } from '../lib/format'
import { explain, GLOSSARY } from '../lib/glossary'
import { foldOther, markUnpriced, pivot } from '../lib/pivot'
import { datesBetween } from '../lib/range'
import { useRange } from '../lib/rangeContext'
import { readTokens } from '../lib/tokens'
import { usageSentence } from '../lib/usage'

function PlanWindows({ usage, sessions, now }: { usage: Usage; sessions?: Session[]; now: number }) {
  const entries: LimitEntry[] = usage.limits.map((l) => ({
    accountId: l.account_id,
    limit: l,
    name: usage.by_account.find((r) => r.key === String(l.account_id))?.label,
    plan: sessions?.find((s) => s.account?.id === l.account_id)?.account?.plan,
  }))
  return (
    <Section title="Plan windows" actions={<p className="section-desc">{GLOSSARY.planWindows.text}</p>}>
      {entries.length === 0 ? (
        <div className="panel">
          <p className="meta">No plan-limit readings yet. Claude Code reports them during normal use; they appear after your next sync.</p>
        </div>
      ) : (
        <LimitStrip entries={entries} now={now} />
      )}
    </Section>
  )
}

function Sentence({ usage }: { usage: Usage }) {
  const { preset } = useRange()
  const s = usageSentence(preset, usage.totals, usage.by_account.length)
  return (
    <div>
      <p className="sentence">
        {s.runs.map((r, i) =>
          typeof r === 'string' ? (
            r
          ) : (
            <span key={i} className="fig">
              {r.fig}
            </span>
          ),
        )}
      </p>
      <p className="sentence-note">
        {s.note.map((n, i) =>
          typeof n === 'string' ? (
            n
          ) : (
            <code key={i} className="mono">
              {n.cmd}
            </code>
          ),
        )}
      </p>
    </div>
  )
}

/** Model or account breakdown for the rail: read, written and API-equivalent, costliest first. */
function BreakdownTable({ title, first, rows }: { title: string; first: string; rows: BreakdownRow[] }) {
  const sorted = [...rows].sort((a, b) => b.totals.cost_usd - a.totals.cost_usd || readTokens(b.totals) - readTokens(a.totals))
  return (
    <Section title={title}>
      {sorted.length === 0 ? (
        <p className="meta">No usage in this range.</p>
      ) : (
        <div className="table-scroll">
          <table className="table table-compact" data-hover="">
            <thead>
              <tr>
                <th scope="col">{first}</th>
                <th scope="col" className="num" title={explain('read')}>
                  Read
                </th>
                <th scope="col" className="num" title={explain('written')}>
                  Written
                </th>
                <th scope="col" className="num" title={explain('apiEquivalent')}>
                  API-equivalent
                </th>
              </tr>
            </thead>
            <tbody>
              {sorted.map((r) => (
                <tr key={r.key}>
                  <td className="id-cell">{r.label}</td>
                  <td className="num">{formatTokens(readTokens(r.totals))}</td>
                  <td className="num">{formatTokens(r.totals.output)}</td>
                  <td className="num">
                    <CostFigure totals={r.totals} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Section>
  )
}

function WhatTheseMean() {
  const d = useDisclosure()
  const keys = ['apiEquivalent', 'read', 'written', 'newInput', 'cacheWrites', 'cacheReads', 'apiCalls', 'planWindows', 'gettingFull', 'nearlyFull', 'unpriced'] as const
  return (
    <div className="area-foot">
      <Button variant="ghost" className="disclosure" {...d.buttonProps}>
        <Glyph name="chevron-right" />
        What these mean
      </Button>
      <dl className="glossary" {...d.regionProps}>
        {keys.map((k) => (
          <div key={k} style={{ display: 'contents' }}>
            <dt>{GLOSSARY[k].term}</dt>
            <dd>{GLOSSARY[k].text}</dd>
          </div>
        ))}
      </dl>
    </div>
  )
}

export function MyUsagePage() {
  const { range, preset, now } = useRange()
  const usage = useMyUsage(range)
  const [page, setPage] = useState(0)
  const sessions = useMySessions(page)
  const u = usage.data

  return (
    <>
      <PageHeader title="My usage" range />
      <div className="usage-grid">
        {usage.error && !u && (
          <div className="area-limits">
            <ErrorNotice what="load your usage" error={usage.error} onRetry={() => void usage.refetch()} />
          </div>
        )}
        {!u && !usage.error && (
          <div className="area-limits" aria-busy="true">
            <div className="panel">
              <Skeleton width="30%" height={14} />
              <Skeleton width="70%" height={10} />
              <Skeleton width="60%" height={10} />
            </div>
            <div className="mt-5">
              <Skeleton width="80%" height={22} />
              <Skeleton width="55%" height={22} />
            </div>
          </div>
        )}
        {u && (
          <>
            <div className="area-limits">
              <PlanWindows usage={u} sessions={sessions.data?.items} now={now} />
            </div>
            <div className="area-sentence">
              <Sentence usage={u} />
            </div>
            {readTokens(u.totals) + u.totals.output > 0 && (
              <div className="area-mix">
                <TokenMix totals={u.totals} />
              </div>
            )}
            {/* One bar (Today) or no bars says nothing, so the chart is left out. */}
            {preset !== 'today' && u.daily_by_model.length > 0 && (
              <Section title="API-equivalent cost per day, by model" className="area-chart">
                <StackedDailyBars
                  title="API-equivalent cost per day by model"
                  pivot={markUnpriced(foldOther(pivot(u.daily_by_model, datesBetween(range), 'cost_usd')))}
                  footnote={<UnpricedNote tokens={u.totals.unpriced_tokens} />}
                />
              </Section>
            )}
            <div className="area-rail">
              <BreakdownTable title="By model" first="Model" rows={u.by_model} />
              <BreakdownTable title="By account" first="Account" rows={u.by_account} />
            </div>
          </>
        )}
        <Section
          title="Latest sessions"
          description="All time, newest first. Select a session to see what drove its cost."
          className="area-ledger"
        >
          <SessionLedger page={page} data={sessions.data} error={sessions.error} onPage={setPage} onRetry={() => void sessions.refetch()} />
        </Section>
        <WhatTheseMean />
      </div>
    </>
  )
}

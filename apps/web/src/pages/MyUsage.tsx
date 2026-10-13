import { useState } from 'react'
import { useMySessions, useMyUsage } from '../api/hooks'
import { StackedDaily } from '../components/charts'
import { LimitGauges } from '../components/limits'
import { BreakdownTable, Kpis, SessionsTable } from '../components/tables'
import { formatTokens, formatUSD } from '../lib/format'
import { markUnpriced, pivot } from '../lib/pivot'
import { datesBetween } from '../lib/range'
import { useRange } from '../lib/rangeContext'

export function MyUsagePage() {
  const { range } = useRange()
  const usage = useMyUsage(range)
  const [page, setPage] = useState(0)
  const sessions = useMySessions(page)

  return (
    <>
      <h1>My usage</h1>
      {usage.error && <div className="card error">{usage.error.message}</div>}
      {usage.data && (
        <>
          <Kpis totals={usage.data.totals} sessions={sessions.data?.total} />
          <section className="card">
            <h2>Daily estimated cost by model</h2>
            <StackedDaily
              pivot={markUnpriced(pivot(usage.data.daily_by_model, datesBetween(range), 'cost_usd'))}
              format={formatUSD}
            />
            {usage.data.totals.unpriced_tokens > 0 && (
              <div className="muted">Excludes {formatTokens(usage.data.totals.unpriced_tokens)} tokens from unpriced models.</div>
            )}
          </section>
          <div className="grid2">
            <BreakdownTable title="By model" rows={usage.data.by_model} />
            <BreakdownTable title="By Claude account" rows={usage.data.by_account} />
          </div>
          {usage.data.limits.map((l) => (
            <LimitGauges
              key={l.account_id}
              limit={l}
              label={usage.data.by_account.find((a) => a.key === String(l.account_id))?.label ?? `Account ${l.account_id}`}
            />
          ))}
        </>
      )}
      <section className="card">
        <h2>Sessions</h2>
        <SessionsTable page={page} data={sessions.data} error={sessions.error} onPage={setPage} />
      </section>
    </>
  )
}

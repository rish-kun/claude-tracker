import type { AccountDetail } from '../api/client'
import { useByAccount } from '../api/hooks'
import { AdminOnly } from '../components/AdminOnly'
import { LimitTimeline, ShareDonut, StackedDaily } from '../components/charts'
import { LimitGauges } from '../components/limits'
import { Cost, UnpricedNote } from '../components/tables'
import { accountName } from '../lib/accounts'
import { formatPct, formatTokens, formatUSD, PLAN_LABELS } from '../lib/format'
import { markUnpriced, pivot } from '../lib/pivot'
import { datesBetween, type Range } from '../lib/range'
import { useRange } from '../lib/rangeContext'

function AccountCard({ d, range }: { d: AccountDetail; range: Range }) {
  const latest = d.limits.length > 0 ? d.limits[d.limits.length - 1] : undefined
  return (
    <section className="card">
      <h2>
        {accountName(d.account)} <span className="badge">{PLAN_LABELS[d.account.plan]}</span>{' '}
        <span className="muted">
          {d.account.email && <>{d.account.email} · </>}
          <Cost totals={d.totals} /> · {formatTokens(d.totals.total_tokens)} tokens
        </span>
      </h2>
      {latest && <LimitGauges limit={latest} label="Latest plan limits" />}
      <div className="grid2">
        <div>
          <h2>Share of this account</h2>
          <ShareDonut data={d.by_user.map((u) => ({ name: u.user.email, value: u.share }))} format={formatPct} />
          <UnpricedNote tokens={d.totals.unpriced_tokens} />
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>User</th>
                  <th className="num">Share</th>
                  <th className="num">Tokens</th>
                  <th className="num">Est. cost</th>
                </tr>
              </thead>
              <tbody>
                {d.by_user.map((u) => (
                  <tr key={u.user.id}>
                    <td>{u.user.email}</td>
                    <td className="num">{formatPct(u.share)}</td>
                    <td className="num">{formatTokens(u.totals.total_tokens)}</td>
                    <td className="num">
                      <Cost totals={u.totals} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
        <div>
          <h2>Daily cost per user</h2>
          <StackedDaily pivot={markUnpriced(pivot(d.daily_by_user, datesBetween(range), 'cost_usd'))} format={formatUSD} kind="bar" />
          <UnpricedNote tokens={d.totals.unpriced_tokens} />
          <h2>Plan-limit utilisation</h2>
          <LimitTimeline limits={d.limits} />
        </div>
      </div>
    </section>
  )
}

function Content() {
  const { range } = useRange()
  const q = useByAccount(range)
  if (q.error) return <div className="card error">{q.error.message}</div>
  if (!q.data) return <div className="muted">Loading…</div>
  if (q.data.accounts.length === 0) return <div className="card">No Claude accounts have reported usage yet.</div>
  return (
    <>
      {q.data.accounts.map((d) => (
        <AccountCard key={d.account.id} d={d} range={range} />
      ))}
    </>
  )
}

export function AdminAccountsPage() {
  return (
    <>
      <h1>Usage by Claude account</h1>
      <AdminOnly>
        <Content />
      </AdminOnly>
    </>
  )
}

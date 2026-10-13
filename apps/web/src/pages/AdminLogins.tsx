import { Fragment, useMemo, useState } from 'react'
import { useByLogin, useUserSessions } from '../api/hooks'
import { AdminOnly } from '../components/AdminOnly'
import { StackedDaily, UserAccountBars } from '../components/charts'
import { Cost, SessionsTable, UnpricedNote } from '../components/tables'
import type { Totals } from '../api/client'
import { accountColumns, groupSpan, hasSubtotal, planGroups, planSubtotal } from '../lib/accounts'
import { formatTokens, formatUSD, PLAN_LABELS } from '../lib/format'
import { pivot } from '../lib/pivot'
import { datesBetween } from '../lib/range'
import { useRange } from '../lib/rangeContext'

/** Cost over tokens; a dash when the login has no usage there. */
function TotalsCell({ totals }: { totals?: Totals }) {
  if (!totals) return <td className="num">—</td>
  return (
    <td className="num">
      <Cost totals={totals} />
      <div className="muted">{formatTokens(totals.total_tokens)}</div>
    </td>
  )
}

function UserSessions({ userId }: { userId: number }) {
  const [page, setPage] = useState(0)
  const sessions = useUserSessions(userId, page)
  return <SessionsTable page={page} data={sessions.data} error={sessions.error} onPage={setPage} />
}

function Content() {
  const { range } = useRange()
  const q = useByLogin(range)
  const [open, setOpen] = useState<number | null>(null)
  const columns = useMemo(() => (q.data ? accountColumns(q.data) : []), [q.data])

  if (q.error) return <div className="card error">{q.error.message}</div>
  if (!q.data) return <div className="muted">Loading…</div>
  const data = q.data

  const groups = planGroups(columns)
  const unpriced = data.users.reduce((n, u) => n + u.totals.unpriced_tokens, 0)
  const barRows = data.users.map((u) => {
    const row: Record<string, string | number> = { name: u.user.email }
    for (const a of u.by_account) row[String(a.account.id)] = a.totals.cost_usd
    return row
  })
  const barKeys = columns.map((c) => ({ key: String(c.id), label: `${c.label} (${PLAN_LABELS[c.plan]})` }))

  return (
    <>
      <section className="card">
        <h2>Estimated cost per login, split by account</h2>
        <UserAccountBars rows={barRows} keys={barKeys} format={formatUSD} />
        <UnpricedNote tokens={unpriced} />
      </section>
      <section className="card">
        <h2>Usage per login, per plan and account</h2>
        <table>
          <thead>
            <tr>
              <th rowSpan={2}>Login</th>
              <th rowSpan={2} className="num">Total</th>
              {groups.map((g) => (
                <th key={g.plan} colSpan={groupSpan(g)}>{PLAN_LABELS[g.plan]}</th>
              ))}
            </tr>
            <tr>
              {groups.map((g) => (
                <Fragment key={g.plan}>
                  {g.accounts.map((c) => (
                    <th key={c.id} className="num">{c.label}</th>
                  ))}
                  {hasSubtotal(g) && <th className="num">All {PLAN_LABELS[g.plan]}</th>}
                </Fragment>
              ))}
            </tr>
          </thead>
          <tbody>
            {data.users.map((u) => (
              <Fragment key={u.user.id}>
                <tr className="clickable" onClick={() => setOpen(open === u.user.id ? null : u.user.id)}>
                  <td>
                    {u.user.name || u.user.email}
                    <div className="muted">{u.user.email}</div>
                  </td>
                  <TotalsCell totals={u.totals} />
                  {groups.map((g) => (
                    <Fragment key={g.plan}>
                      {g.accounts.map((c) => (
                        <TotalsCell key={c.id} totals={u.by_account.find((a) => a.account.id === c.id)?.totals} />
                      ))}
                      {hasSubtotal(g) && <TotalsCell totals={planSubtotal(u.by_account, g.plan)} />}
                    </Fragment>
                  ))}
                </tr>
                {open === u.user.id && (
                  <tr>
                    <td colSpan={2 + groups.reduce((n, g) => n + groupSpan(g), 0)}>
                      <UserSessions userId={u.user.id} />
                    </td>
                  </tr>
                )}
              </Fragment>
            ))}
          </tbody>
        </table>
      </section>
      <section className="card">
        <h2>Daily estimated cost per login</h2>
        <StackedDaily pivot={pivot(data.daily_by_user, datesBetween(range), 'cost_usd')} format={formatUSD} />
        <UnpricedNote tokens={unpriced} />
      </section>
    </>
  )
}

export function AdminLoginsPage() {
  return (
    <>
      <h1>Usage by login</h1>
      <AdminOnly>
        <Content />
      </AdminOnly>
    </>
  )
}

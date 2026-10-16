import { useSearch } from '@tanstack/react-router'
import { Fragment, useEffect, useMemo, useRef, useState } from 'react'
import type { AccountRef, ByLogin, Schemas, Totals } from '../api/client'
import { useByLogin, useUserSessions } from '../api/hooks'
import { AdminOnly } from '../components/AdminOnly'
import { AccountSplitBar, ChartLegend, seriesVar, StackedDailyBars } from '../components/charts'
import { CostFigure, PlanChip, UnpricedNote } from '../components/figures'
import { PageHeader } from '../components/Layout'
import { SessionLedger } from '../components/ledger'
import { EmptyState, ErrorNotice, Glyph, Section, SkeletonRows } from '../components/ui'
import { accountColumns, groupSpan, hasSubtotal, planGroups, planSubtotal } from '../lib/accounts'
import { formatTokens, PLAN_LABELS } from '../lib/format'
import { foldOther, markUnpriced, pivot, relabel } from '../lib/pivot'
import { datesBetween } from '../lib/range'
import { useRange } from '../lib/rangeContext'
import { barScales, readTokens } from '../lib/tokens'

type LoginUsage = Schemas['LoginUsage']

const personId = (id: number) => `person-${id}`

/** Account colour: slots 1–5 by position in the account columns, the rest share "Other". */
function accountColor(columns: AccountRef[], id: number): string {
  const i = columns.findIndex((c) => c.id === id)
  return `var(${seriesVar(String(id), i < 0 ? 5 : i)})`
}

function UserSessions({ userId, name }: { userId: number; name: string }) {
  const [page, setPage] = useState(0)
  const sessions = useUserSessions(userId, page)
  return (
    <SessionLedger
      variant="compact"
      page={page}
      data={sessions.data}
      stale={sessions.isPlaceholderData}
      error={sessions.error}
      onPage={setPage}
      onRetry={() => void sessions.refetch()}
      errorWhat={`load ${name}'s sessions`}
      empty={<EmptyState title="No sessions yet.">{name} has no synced sessions.</EmptyState>}
    />
  )
}

function PersonRow({
  u,
  columns,
  scale,
  open,
  onToggle,
}: {
  u: LoginUsage
  columns: AccountRef[]
  /** Bar length, 0–1, relative to the heaviest person (see `barScales`). */
  scale: number
  open: boolean
  onToggle: () => void
}) {
  const name = u.user.name || u.user.email
  const regionId = `person-sessions-${u.user.id}`
  return (
    <li className="person" id={personId(u.user.id)}>
      <div className="person-row">
        <button type="button" className="person-toggle" aria-expanded={open} aria-controls={regionId} onClick={onToggle}>
          <Glyph name="chevron-right" />
          <span className="person-name">{name}</span>
          {u.user.name && <span className="person-email">{u.user.email}</span>}
        </button>
        <AccountSplitBar
          scale={scale}
          parts={u.by_account.map((a) => ({
            key: String(a.account.id),
            label: a.account.label || a.account.email || `Account ${a.account.id}`,
            plan: a.account.plan,
            totals: a.totals,
            color: accountColor(columns, a.account.id),
          }))}
        />
        <span className="person-cost">
          <CostFigure totals={u.totals} size="fig" />
        </span>
        <span className="person-written">{formatTokens(u.totals.output)} written</span>
      </div>
      <div className="person-drawer" data-open={open ? '' : undefined} id={regionId} role="region" aria-label={`${name}'s sessions`} inert={!open}>
        <div>
          {open && (
            <div className="person-drawer-inner">
              <p className="meta">Latest sessions, all time</p>
              <UserSessions userId={u.user.id} name={name} />
            </div>
          )}
        </div>
      </div>
    </li>
  )
}

function TotalsCell({ totals, groupStart }: { totals?: Totals; groupStart?: boolean }) {
  const cls = groupStart ? 'num group-start' : 'num'
  if (!totals) {
    return (
      <td className={cls}>
        <span className="dash">–</span>
      </td>
    )
  }
  return (
    <td className={cls}>
      <CostFigure totals={totals} />
      <div className="sub">{formatTokens(totals.output)} written</div>
    </td>
  )
}

function Matrix({ data, columns, onOpen }: { data: ByLogin; columns: AccountRef[]; onOpen: (id: number) => void }) {
  const groups = planGroups(columns)
  const users = [...data.users].sort((a, b) => b.totals.cost_usd - a.totals.cost_usd)
  return (
    <div className="table-scroll">
      <table className="table matrix" data-hover="">
        <thead>
          <tr className="group-row">
            <th rowSpan={2} scope="col" className="sticky-col">
              Login
            </th>
            <th rowSpan={2} scope="col" className="num">
              Total
            </th>
            {groups.map((g) => (
              <th key={g.plan} colSpan={groupSpan(g)} scope="colgroup">
                <PlanChip plan={g.plan} size="sm" />
              </th>
            ))}
          </tr>
          <tr>
            {groups.map((g) => (
              <Fragment key={g.plan}>
                {g.accounts.map((c, i) => (
                  <th key={c.id} scope="col" className={i === 0 ? 'num group-start' : 'num'}>
                    {c.label || c.email || `Account ${c.id}`}
                  </th>
                ))}
                {hasSubtotal(g) && (
                  <th scope="col" className="num">
                    All {PLAN_LABELS[g.plan]}
                  </th>
                )}
              </Fragment>
            ))}
          </tr>
        </thead>
        <tbody>
          {users.map((u) => (
            <tr key={u.user.id}>
              <th scope="row" className="sticky-col" style={{ font: 'var(--t-body)', color: 'var(--text)' }}>
                <button type="button" className="btn-text" style={{ font: 'inherit' }} onClick={() => onOpen(u.user.id)}>
                  {u.user.name || u.user.email}
                </button>
              </th>
              <TotalsCell totals={u.totals} />
              {groups.map((g) => (
                <Fragment key={g.plan}>
                  {g.accounts.map((c, i) => (
                    <TotalsCell key={c.id} groupStart={i === 0} totals={u.by_account.find((a) => a.account.id === c.id)?.totals} />
                  ))}
                  {hasSubtotal(g) && <TotalsCell totals={planSubtotal(u.by_account, g.plan)} />}
                </Fragment>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function Content() {
  const { range } = useRange()
  const search = useSearch({ strict: false }) as { open?: number }
  const q = useByLogin(range)
  // `?open=` (from Settings → People) is read once, on arrival.
  const [open, setOpen] = useState<number | null>(() => search.open ?? null)
  const scrolledTo = useRef(false)
  const columns = useMemo(() => (q.data ? accountColumns(q.data) : []), [q.data])

  useEffect(() => {
    if (scrolledTo.current || !q.data || open === null) return
    scrolledTo.current = true
    document.getElementById(personId(open))?.scrollIntoView({ block: 'start' })
  }, [q.data, open])

  if (q.error && !q.data) return <ErrorNotice what="load usage by login" error={q.error} onRetry={() => void q.refetch()} />
  if (!q.data) return <SkeletonRows rows={5} />
  const data = q.data
  const users = [...data.users].sort((a, b) => b.totals.cost_usd - a.totals.cost_usd || readTokens(b.totals) - readTokens(a.totals))
  if (users.length === 0) {
    return (
      <EmptyState title="No usage in this range.">
        Pick a longer range, or check that people have logged in with <code>cctrack login</code>.
      </EmptyState>
    )
  }
  const bars = barScales(users.map((u) => u.totals))
  const names = new Map(users.map((u) => [String(u.user.id), u.user.name || u.user.email]))
  const unpriced = users.reduce((n, u) => n + u.totals.unpriced_tokens, 0)
  const legend = columns.slice(0, 5).map((c, i) => ({ label: `${c.label || c.email || `Account ${c.id}`} (${PLAN_LABELS[c.plan]})`, color: `var(${seriesVar(String(c.id), i)})` }))
  if (columns.length > 5) legend.push({ label: 'Other accounts', color: 'var(--series-other)' })
  const openPerson = (id: number) => {
    setOpen(id)
    document.getElementById(personId(id))?.scrollIntoView({ block: 'start' })
  }

  return (
    <>
      <Section
        title="People"
        description={
          bars.by === 'cost'
            ? 'API-equivalent cost in the range, split by the Claude account it ran on. Open a person to see their sessions.'
            : "Usage in the range, split by the Claude account it ran on. Bars compare tokens read, since some usage isn't priced. Open a person to see their sessions."
        }
      >
        <ChartLegend items={legend} />
        <ul className="people" role="list">
          {users.map((u, i) => (
            <PersonRow
              key={u.user.id}
              u={u}
              columns={columns}
              scale={bars.scales[i]}
              open={open === u.user.id}
              onToggle={() => setOpen(open === u.user.id ? null : u.user.id)}
            />
          ))}
        </ul>
        {unpriced > 0 && (
          <p className="meta mt-3">
            <UnpricedNote tokens={unpriced} />
          </p>
        )}
      </Section>
      <Section title="Per account and plan" description="Each login's API-equivalent cost and output on every account, grouped by plan.">
        <Matrix data={data} columns={columns} onOpen={openPerson} />
      </Section>
      <Section title="API-equivalent cost per day, by login">
        <StackedDailyBars
          title="API-equivalent cost per day by login"
          pivot={markUnpriced(relabel(foldOther(pivot(data.daily_by_user, datesBetween(range), 'cost_usd')), (k) => names.get(k)))}
          footnote={<UnpricedNote tokens={unpriced} />}
        />
      </Section>
    </>
  )
}

export function AdminLoginsPage() {
  return (
    <>
      <PageHeader title="Usage by login" range />
      <AdminOnly>
        <Content />
      </AdminOnly>
    </>
  )
}

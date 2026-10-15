import { useMemo } from 'react'
import type { AccountDetail, ByAccount } from '../api/client'
import { useByAccount } from '../api/hooks'
import { AdminOnly } from '../components/AdminOnly'
import { LimitTimeline, ShareBars, StackedDailyBars } from '../components/charts'
import { CostFigure, PlanChip, UnpricedNote } from '../components/figures'
import { PageHeader } from '../components/Layout'
import { LimitStrip } from '../components/limits'
import { EmptyState, ErrorNotice, Section, SkeletonRows } from '../components/ui'
import { accountName, sortByPressure } from '../lib/accounts'
import { formatTokens } from '../lib/format'
import { foldOther, markUnpriced, MAX_SERIES, OTHER_KEY, pivot, relabel } from '../lib/pivot'
import { datesBetween, type Range } from '../lib/range'
import { useRange } from '../lib/rangeContext'
import { readTokens } from '../lib/tokens'

const SLOTS = ['--series-1', '--series-2', '--series-3', '--series-4', '--series-5'] as const

/**
 * People ranked by API-equivalent cost across every account on the page, so each person keeps
 * the same colour in every account's chart. Also maps user id → display name.
 */
function usePeople(data: ByAccount | undefined) {
  return useMemo(() => {
    const cost = new Map<string, number>()
    const names = new Map<string, string>()
    for (const a of data?.accounts ?? []) {
      for (const u of a.by_user) {
        const k = String(u.user.id)
        cost.set(k, (cost.get(k) ?? 0) + u.totals.cost_usd + readTokens(u.totals) * 1e-12)
        names.set(k, u.user.name || u.user.email)
      }
    }
    const ranked = [...cost.entries()].sort((a, b) => b[1] - a[1]).map(([k]) => k)
    return { ranked, names }
  }, [data])
}

function AccountSection({ d, range, now, people }: { d: AccountDetail; range: Range; now: number; people: ReturnType<typeof usePeople> }) {
  const latest = d.limits.length > 0 ? d.limits[d.limits.length - 1] : undefined
  const a = d.account
  const keep = people.ranked.slice(0, MAX_SERIES)
  const colorFor = (key: string) => {
    const i = key === OTHER_KEY ? -1 : people.ranked.indexOf(key)
    return i >= 0 && i < MAX_SERIES ? SLOTS[i] : ('--series-other' as const)
  }
  const daily = markUnpriced(relabel(foldOther(pivot(d.daily_by_user, datesBetween(range), 'cost_usd'), keep), (k) => people.names.get(k)))
  return (
    <section className="account-section" id={`account-${a.id}`} aria-labelledby={`account-${a.id}-name`}>
      <div className="account-head">
        <h2 id={`account-${a.id}-name`}>{accountName(a)}</h2>
        <PlanChip plan={a.plan} settingsLink={a.id} />
        {a.email && a.email !== accountName(a) && <span className="email">{a.email}</span>}
        <span className="totals">
          <CostFigure totals={d.totals} size="fig" />
          <span className="num">{formatTokens(d.totals.output)} written</span>
          <span className="num">{formatTokens(readTokens(d.totals))} read</span>
        </span>
      </div>
      {latest ? (
        <LimitStrip entries={[{ accountId: a.id, limit: latest, plan: a.plan }]} now={now} showNames={false} />
      ) : (
        <div className="panel">
          <p className="meta">No plan-limit readings for this account yet.</p>
        </div>
      )}
      <div className="two-col section">
        <Section title="Plan limits over time" level={3}>
          <LimitTimeline limits={d.limits} />
        </Section>
        <Section title="Who used it" level={3} description="Share of this account's API-equivalent cost in the range (token share when nothing is priced)">
          {d.by_user.length === 0 ? (
            <p className="meta">No usage in this range.</p>
          ) : (
            <ShareBars rows={d.by_user.map((u) => ({ key: u.user.id, label: u.user.name || u.user.email, share: u.share, totals: u.totals }))} />
          )}
        </Section>
      </div>
      {d.daily_by_user.length > 0 && (
        <Section title="API-equivalent cost per day, by person" level={3}>
          <StackedDailyBars
            title={`API-equivalent cost per day by person on ${accountName(a)}`}
            pivot={daily}
            colorFor={colorFor}
            footnote={<UnpricedNote tokens={d.totals.unpriced_tokens} />}
          />
        </Section>
      )}
    </section>
  )
}

function Content() {
  const { range, now } = useRange()
  const q = useByAccount(range)
  const people = usePeople(q.data)
  if (q.error && !q.data) return <ErrorNotice what="load usage by account" error={q.error} onRetry={() => void q.refetch()} />
  if (!q.data) return <SkeletonRows rows={5} />
  if (q.data.accounts.length === 0) {
    return <EmptyState title="No Claude accounts have reported usage yet.">Accounts appear automatically after someone's Claude Code syncs.</EmptyState>
  }
  const accounts = sortByPressure(q.data.accounts, now)
  return (
    <>
      {accounts.length > 3 && (
        <nav className="jump" aria-label="Jump to account">
          <span>Jump to</span>
          {accounts.map((d) => (
            <a key={d.account.id} href={`#account-${d.account.id}`}>
              {accountName(d.account)}
            </a>
          ))}
        </nav>
      )}
      {accounts.map((d) => (
        <AccountSection key={d.account.id} d={d} range={range} now={now} people={people} />
      ))}
    </>
  )
}

export function AdminAccountsPage() {
  return (
    <>
      <PageHeader title="Usage by Claude account" range />
      <AdminOnly>
        <Content />
      </AdminOnly>
    </>
  )
}

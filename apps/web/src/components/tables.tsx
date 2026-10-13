import type { BreakdownRow, Session, SessionPage, Totals } from '../api/client'
import { PAGE_SIZE } from '../api/hooks'
import { costLabel, formatDateTime, formatTokens, PLAN_LABELS, unpricedTitle } from '../lib/format'
import { Empty } from './charts'

/** Cost cell that never shows a bare $0 for unpriced usage; the marker's title explains it. */
function Cost({ totals }: { totals: Totals }) {
  const label = costLabel(totals)
  if (totals.unpriced_tokens === 0) return <>{label}</>
  const title = unpricedTitle(totals.unpriced_tokens)
  if (label === 'unpriced') {
    return (
      <span className="muted" title={title}>
        unpriced
      </span>
    )
  }
  return (
    <>
      {label}
      <span className="muted" title={title}>
        {' '}*
      </span>
    </>
  )
}

export function Kpis({ totals, sessions }: { totals: Totals; sessions?: number }) {
  const cacheShare = totals.total_tokens > 0 ? totals.cache_read / totals.total_tokens : 0
  return (
    <div className="kpis">
      <div className="card kpi">
        <div className="muted">Estimated API cost</div>
        <div className="value">{costLabel(totals)}</div>
        {totals.unpriced_tokens > 0 && (
          <div className="muted" title="Tokens from models with no price in the price table">
            + {formatTokens(totals.unpriced_tokens)} unpriced tokens
          </div>
        )}
      </div>
      <div className="card kpi">
        <div className="muted">Total tokens</div>
        <div className="value">{formatTokens(totals.total_tokens)}</div>
        <div className="muted">{Math.round(cacheShare * 100)}% cache reads</div>
      </div>
      <div className="card kpi">
        <div className="muted">Output tokens</div>
        <div className="value">{formatTokens(totals.output)}</div>
      </div>
      {sessions !== undefined && (
        <div className="card kpi">
          <div className="muted">Sessions (all time)</div>
          <div className="value">{sessions}</div>
        </div>
      )}
    </div>
  )
}

export function BreakdownTable({ title, rows }: { title: string; rows: BreakdownRow[] }) {
  return (
    <section className="card">
      <h2>{title}</h2>
      {rows.length === 0 ? (
        <Empty />
      ) : (
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th className="num">Tokens</th>
              <th className="num">Est. cost</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.key}>
                <td>{r.label}</td>
                <td className="num">{formatTokens(r.totals.total_tokens)}</td>
                <td className="num">
                  <Cost totals={r.totals} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  )
}

function SessionRow({ s, showUser }: { s: Session; showUser: boolean }) {
  return (
    <tr>
      <td>{formatDateTime(s.started_at)}</td>
      {showUser && <td>{s.user.email}</td>}
      <td>
        {s.project ?? '—'}
        {s.git_branch && <span className="muted"> · {s.git_branch}</span>}
        <div className="muted">{s.device_hostname}</div>
      </td>
      <td>
        {s.account ? s.account.label : '—'}
        {s.account && <div className="muted">{PLAN_LABELS[s.account.plan]}</div>}
      </td>
      <td>{s.summary ?? <span className="muted">No summary</span>}</td>
      <td className="num">{formatTokens(s.totals.total_tokens)}</td>
      <td className="num">
        <Cost totals={s.totals} />
      </td>
    </tr>
  )
}

export function SessionsTable({
  page,
  data,
  error,
  onPage,
  showUser = false,
}: {
  page: number
  data?: SessionPage
  error?: Error | null
  onPage: (p: number) => void
  showUser?: boolean
}) {
  const err = error ? <div className="error">{error.message}</div> : null
  if (!data) return err ?? <div className="muted">Loading…</div>
  if (data.items.length === 0) return err ?? <Empty>No sessions yet.</Empty>
  const pages = Math.max(1, Math.ceil(data.total / PAGE_SIZE))
  return (
    <>
      {err}
      <table>
        <thead>
          <tr>
            <th>Started</th>
            {showUser && <th>User</th>}
            <th>Project</th>
            <th>Account</th>
            <th>Summary</th>
            <th className="num">Tokens</th>
            <th className="num">Est. cost</th>
          </tr>
        </thead>
        <tbody>
          {data.items.map((s) => (
            <SessionRow key={s.session_id} s={s} showUser={showUser} />
          ))}
        </tbody>
      </table>
      <div className="pager">
        <button disabled={page === 0} onClick={() => onPage(page - 1)}>Previous</button>
        <span className="muted">Page {page + 1} of {pages}</span>
        <button disabled={page + 1 >= pages} onClick={() => onPage(page + 1)}>Next</button>
      </div>
    </>
  )
}

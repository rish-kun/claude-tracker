import type { BreakdownRow, Session, SessionPage, Totals } from '../api/client'
import { PAGE_SIZE } from '../api/hooks'
import { formatDateTime, formatTokens, formatUSD, PLAN_LABELS } from '../lib/format'
import { Empty } from './charts'

export function Kpis({ totals, sessions }: { totals: Totals; sessions?: number }) {
  const cacheShare = totals.total_tokens > 0 ? totals.cache_read / totals.total_tokens : 0
  return (
    <div className="kpis">
      <div className="card kpi">
        <div className="muted">Estimated API cost</div>
        <div className="value">{formatUSD(totals.cost_usd)}</div>
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
                  {formatUSD(r.totals.cost_usd)}
                  {r.totals.unpriced_tokens > 0 && <span className="muted"> *</span>}
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
      <td className="num">{formatUSD(s.totals.cost_usd)}</td>
    </tr>
  )
}

export function SessionsTable({ page, data, onPage, showUser = false }: { page: number; data?: SessionPage; onPage: (p: number) => void; showUser?: boolean }) {
  if (!data) return <div className="muted">Loading…</div>
  if (data.items.length === 0) return <Empty>No sessions yet.</Empty>
  const pages = Math.max(1, Math.ceil(data.total / PAGE_SIZE))
  return (
    <>
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

import { Link, useNavigate, useParams } from '@tanstack/react-router'
import { type ReactNode, useEffect } from 'react'
import { ApiError, type SessionDetail, type SessionPart } from '../api/client'
import { useMe, useSession } from '../api/hooks'
import { isUnauthorized, LOGIN_PATH } from '../api/unauthorized'
import { AccountName, CostFigure, Fact, FactList, PlanChip, TokenMix } from '../components/figures'
import { CopyButton, EmptyState, ErrorNotice, Glyph, Section, Skeleton } from '../components/ui'
import { formatCount, formatDayLong, formatDuration, formatShare, formatTime, formatTokens } from '../lib/format'
import { explain, explainApiEquivalent } from '../lib/glossary'
import { useNow } from '../lib/rangeContext'
import { avgContext, costDrivers, readTokens, sessionLabel, shortenPath } from '../lib/sessions'

function BackLink({ toMine }: { toMine: boolean }) {
  return toMine ? (
    <Link to="/" className="back-link">
      <Glyph name="chevron-left" />
      My usage
    </Link>
  ) : (
    <Link to="/admin/logins" className="back-link">
      <Glyph name="chevron-left" />
      By login
    </Link>
  )
}

/** The API-equivalent tooltip for this session's account (it says nobody pays only on a flat-rate plan). */
function apiEquivalentTitle(d: SessionDetail): string {
  return explainApiEquivalent(d.session.account ? [d.session.account.plan] : undefined)
}

function HeadlineFigures({ d }: { d: SessionDetail }) {
  const t = d.session.totals
  return (
    <dl className="headline-figs">
      <div className="headline-fig" title={apiEquivalentTitle(d)}>
        <dt>API-equivalent</dt>
        <dd>
          <CostFigure totals={t} size="fig-lg" />
        </dd>
      </div>
      <div className="headline-fig" title={explain('read')}>
        <dt>read</dt>
        <dd>{formatTokens(readTokens(t))}</dd>
      </div>
      <div className="headline-fig" title={explain('written')}>
        <dt>written</dt>
        <dd>{formatTokens(t.output)}</dd>
      </div>
      <div className="headline-fig" title={explain('apiCalls')}>
        <dt>API calls</dt>
        <dd>{formatCount(d.calls)}</dd>
      </div>
      <div className="headline-fig" title="The biggest single call: input plus cache reads and writes.">
        <dt>Largest context</dt>
        <dd>{formatTokens(d.peak_context_tokens)}</dd>
      </div>
    </dl>
  )
}

/** Where the cost went: main conversation vs subagents, side by side, then the reasons in sentences. */
function SessionAnatomy({ d }: { d: SessionDetail }) {
  const hasSub = d.subagents.calls > 0
  const priced = d.main.totals.cost_usd + d.subagents.totals.cost_usd > 0
  const weight = (p: SessionPart) => (priced ? p.totals.cost_usd : readTokens(p.totals))
  const whole = weight(d.main) + weight(d.subagents)
  const parts: { key: string; title: string; part: SessionPart }[] = [{ key: 'main', title: 'Main conversation', part: d.main }]
  if (hasSub) parts.push({ key: 'sub', title: 'Subagents', part: d.subagents })
  const rows: { label: string; title: string; cell: (p: SessionPart) => ReactNode }[] = [
    {
      label: 'API-equivalent',
      title: apiEquivalentTitle(d),
      cell: (p) => (
        <>
          <CostFigure totals={p.totals} />
          {hasSub && <span className="share">({formatShare(weight(p), whole)})</span>}
        </>
      ),
    },
    { label: 'API calls', title: explain('apiCalls'), cell: (p) => formatCount(p.calls) },
    { label: 'Avg context per call', title: explain('contextPerCall'), cell: (p) => (p.calls > 0 ? `~${formatTokens(avgContext(p))}` : '–') },
    { label: 'Read', title: explain('read'), cell: (p) => formatTokens(readTokens(p.totals)) },
    { label: 'Written', title: explain('written'), cell: (p) => formatTokens(p.totals.output) },
  ]
  const drivers = costDrivers(d)
  return (
    <Section title="Why it cost this much">
      {hasSub && whole > 0 && (
        <div className="cost-split" aria-hidden="true">
          <div className="cost-split-labels">
            <span style={{ flexGrow: weight(d.main), flexBasis: 0 }} title={explain('main')}>
              Main conversation
            </span>
            <span style={{ flexGrow: weight(d.subagents), flexBasis: 0 }} title={explain('subagents')}>
              Subagents
            </span>
          </div>
          <div className="cost-split-bar">
            <span className="seg-main" style={{ flexGrow: weight(d.main), flexBasis: 0 }} />
            <span className="seg-sub" style={{ flexGrow: weight(d.subagents), flexBasis: 0 }} />
          </div>
        </div>
      )}
      <div className="table-scroll">
        <table className="table compare">
          <thead>
            <tr>
              <td />
              {parts.map((p) => (
                <th key={p.key} scope="col" className="num" title={explain(p.key === 'main' ? 'main' : 'subagents')}>
                  {p.title}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.label}>
                <th scope="row" title={r.title}>
                  {r.label}
                </th>
                {parts.map((p) => (
                  <td key={p.key} className="num">
                    {r.cell(p.part)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <ul className="drivers">
        {drivers.map((s) => (
          <li key={s}>{s}</li>
        ))}
      </ul>
    </Section>
  )
}

function ByModel({ d }: { d: SessionDetail }) {
  const rows = [...d.by_model].sort((a, b) => b.totals.cost_usd - a.totals.cost_usd || readTokens(b.totals) - readTokens(a.totals))
  const priced = d.session.totals.cost_usd > 0
  const whole = rows.reduce((n, r) => n + (priced ? r.totals.cost_usd : readTokens(r.totals)), 0)
  return (
    <Section title="By model">
      {rows.length === 0 ? (
        <p className="meta">No model usage recorded.</p>
      ) : (
        <div className="table-scroll">
          <table className="table table-compact" data-hover="">
            <thead>
              <tr>
                <th scope="col">Model</th>
                <th scope="col" className="num">
                  Calls
                </th>
                <th scope="col" className="num">
                  Read
                </th>
                <th scope="col" className="num">
                  Written
                </th>
                <th scope="col" className="num">
                  API-equivalent
                </th>
                <th scope="col">
                  <span className="sr-only">Share</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => {
                const share = whole > 0 ? (priced ? r.totals.cost_usd : readTokens(r.totals)) / whole : 0
                return (
                  <tr key={r.model}>
                    <td className="id-cell">{r.model}</td>
                    <td className="num">{formatCount(r.calls)}</td>
                    <td className="num">{formatTokens(readTokens(r.totals))}</td>
                    <td className="num">{formatTokens(r.totals.output)}</td>
                    <td className="num">
                      <CostFigure totals={r.totals} />
                    </td>
                    <td style={{ verticalAlign: 'middle' }}>
                      <span className="share-inline" title={formatShare(share, 1)}>
                        <span className="share-fill" style={{ width: `${share * 100}%` }} />
                      </span>
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      )}
    </Section>
  )
}

function Detail({ d, isAdmin, mine }: { d: SessionDetail; isAdmin: boolean; mine: boolean }) {
  const now = useNow()
  const s = d.session
  const label = sessionLabel(s)
  const started = Date.parse(s.started_at)
  return (
    <article>
      <BackLink toMine={mine} />
      <h1 className="detail-title">{label.primary}</h1>
      {label.secondary && <p className="detail-summary">{label.secondary}</p>}
      {label.isFallback && <p className="no-title">No title recorded for this session.</p>}

      <FactList className="detail-facts">
        {isAdmin && (
          <Fact glyph="person" title={s.user.email}>
            {s.user.name || s.user.email}
          </Fact>
        )}
        {s.account && (
          <span className="fact">
            <Glyph name="account" />
            <span>
              <AccountName account={s.account} />
            </span>
            <PlanChip plan={s.account.plan} settingsLink={s.account.id} />
          </span>
        )}
        {s.device_hostname && <Fact glyph="device">{s.device_hostname}</Fact>}
        {s.cwd && (
          <span className="fact copy-wrap" title={s.cwd}>
            <Glyph name="folder" />
            <span className="fact-text mono">{shortenPath(s.cwd)}</span>
            <CopyButton value={s.cwd} label="folder path" />
          </span>
        )}
        {s.git_branch && (
          <Fact glyph="branch" mono title={s.git_branch}>
            {s.git_branch}
          </Fact>
        )}
        <span className="fact">
          {formatDayLong(started, now)}, {formatTime(started)} for {formatDuration(Date.parse(s.ended_at) - started)}
        </span>
      </FactList>

      <HeadlineFigures d={d} />
      <SessionAnatomy d={d} />
      <div className="two-col section">
        <ByModel d={d} />
        <Section title="Token mix">
          <TokenMix totals={s.totals} />
        </Section>
      </div>

      <footer className="detail-foot">
        <span>Session id</span>
        <span className="copy-wrap">
          <code className="mono" style={{ overflowWrap: 'anywhere' }}>
            {s.session_id}
          </code>
          <CopyButton value={s.session_id} label="session id" />
        </span>
      </footer>
    </article>
  )
}

export function SessionDetailPage() {
  const { sessionId } = useParams({ from: '/app/sessions/$sessionId' })
  const me = useMe()
  const q = useSession(sessionId)
  const isAdmin = me.data?.is_admin === true
  const navigate = useNavigate()
  // A 401 means the sign-in has gone: go to /login, as the app shell does for /me,
  // and keep showing the loading state rather than an error until it navigates.
  const unauthorized = isUnauthorized(q.error)
  useEffect(() => {
    if (unauthorized) void navigate({ to: LOGIN_PATH })
  }, [unauthorized, navigate])

  if (q.data) return <Detail d={q.data} isAdmin={isAdmin} mine={q.data.session.user.id === me.data?.id} />

  if (q.error instanceof ApiError && q.error.status === 404) {
    return (
      <>
        <BackLink toMine />
        <EmptyState
          title="Session not found."
          action={
            <Link to="/" className="btn btn-secondary">
              Go to my usage
            </Link>
          }
        >
          It may belong to someone else, or it hasn't synced yet.
        </EmptyState>
      </>
    )
  }
  if (q.error && !unauthorized) {
    return (
      <>
        <BackLink toMine />
        <ErrorNotice what="load this session" error={q.error} onRetry={() => void q.refetch()} />
      </>
    )
  }
  return (
    <div aria-busy="true" aria-label="Loading session">
      <Skeleton width={90} height={12} />
      <div className="mt-4">
        <Skeleton width="65%" height={30} />
        <Skeleton width="45%" height={30} />
      </div>
      <div className="mt-5">
        <Skeleton width="55%" height={12} />
      </div>
      <div className="headline-figs">
        {Array.from({ length: 5 }, (_, i) => (
          <div key={i}>
            <Skeleton width={90} height={28} />
            <Skeleton width={70} height={10} />
          </div>
        ))}
      </div>
    </div>
  )
}

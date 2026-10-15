import { Link } from '@tanstack/react-router'
import type { ReactNode } from 'react'
import type { Account, AccountRef, Plan, Totals } from '../api/client'
import { useMe } from '../api/hooks'
import { accountName } from '../lib/accounts'
import { costLabel, formatTokens, PLAN_LABELS, unpricedTitle } from '../lib/format'
import { explain, GLOSSARY, type GlossaryKey } from '../lib/glossary'
import { readTokens, tokenMix } from '../lib/tokens'
import { Glyph, type GlyphName } from './ui'

/**
 * API-equivalent cost. Partly unpriced usage gets a "+" marker explaining what's left out;
 * fully unpriced usage reads "Not priced", never "$0.00".
 */
export function CostFigure({ totals, size = 'body' }: { totals: Pick<Totals, 'cost_usd' | 'unpriced_tokens'>; size?: 'fig' | 'fig-lg' | 'body' }) {
  const cls = size === 'fig' ? 'fig-md' : size === 'fig-lg' ? 'fig-lg' : 'fig'
  if (totals.cost_usd === 0 && totals.unpriced_tokens > 0) {
    return (
      <span className={`${cls} not-priced`} title={explain('unpriced')}>
        Not priced
      </span>
    )
  }
  const label = costLabel(totals)
  if (totals.unpriced_tokens > 0) {
    return (
      <span className={cls}>
        {label}
        <span className="sr-only">, excludes {formatTokens(totals.unpriced_tokens)} unpriced tokens</span>
        <span className="unpriced-mark" title={unpricedTitle(totals.unpriced_tokens)} aria-hidden="true">
          +
        </span>
      </span>
    )
  }
  return <span className={cls}>{label}</span>
}

/** "{read} read" and "{written} written". */
export function Tokens({ totals, layout = 'inline' }: { totals: Totals; layout?: 'stacked' | 'inline' }) {
  const read = <span title={explain('read')}>{formatTokens(readTokens(totals))} read</span>
  const written = <span title={explain('written')}>{formatTokens(totals.output)} written</span>
  if (layout === 'stacked') {
    return (
      <>
        <span>{read}</span>
        <span>{written}</span>
      </>
    )
  }
  return (
    <span className="num">
      {read}, {written}
    </span>
  )
}

const MIX_GLOSSARY: Record<string, GlossaryKey> = {
  written: 'written',
  input: 'newInput',
  cachewrite: 'cacheWrites',
  cacheread: 'cacheReads',
}

/** Written / new input / cache writes / cache reads as figures, with a thin proportional bar. */
export function TokenMix({ totals }: { totals: Totals }) {
  const mix = tokenMix(totals)
  return (
    <div className="token-mix">
      <dl className="mix-figs">
        {mix.map((m) => (
          <div key={m.kind} className="mix-fig" title={explain(MIX_GLOSSARY[m.kind])}>
            <dt>
              <span className={`swatch kind-${m.kind}`} aria-hidden="true" />
              {m.label}
            </dt>
            <dd>
              <span className="fig-md">{formatTokens(m.value)}</span>
              <span className="share">{m.share}</span>
            </dd>
          </div>
        ))}
      </dl>
      <div className="mix-bar" aria-hidden="true">
        {mix
          .filter((m) => m.value > 0)
          .map((m) => (
            <span key={m.kind} className={`mix-seg kind-${m.kind}`} style={{ flexGrow: m.fraction, flexBasis: 0 }} />
          ))}
      </div>
    </div>
  )
}

/** The account's plan. An unset plan is flagged, with a "Set plan" link for admins when `settingsLink` is given. */
export function PlanChip({ plan, size = 'md', settingsLink }: { plan: Plan; size?: 'sm' | 'md'; settingsLink?: number }) {
  const me = useMe()
  const unknown = plan === 'unknown'
  const chip = (
    <span className={['chip', unknown && 'chip-warn', size === 'sm' && 'chip-sm'].filter(Boolean).join(' ')} title={unknown ? GLOSSARY.planNotSet.text : undefined}>
      {PLAN_LABELS[plan]}
    </span>
  )
  if (!unknown || settingsLink === undefined || !me.data?.is_admin) return chip
  return (
    <span className="chip-wrap">
      {chip}
      <Link to="/admin/settings" hash={`account-${settingsLink}`}>
        Set plan
      </Link>
    </span>
  )
}

export function AccountName({ account }: { account: AccountRef | Account }) {
  const name = 'account_uuid' in account ? accountName(account) : account.label || account.email || `Account ${account.id}`
  return <>{name}</>
}

export function Fact({ glyph, children, title, mono }: { glyph: GlyphName; children: ReactNode; title?: string; mono?: boolean }) {
  return (
    <span className="fact" title={title}>
      <Glyph name={glyph} />
      <span className={mono ? 'fact-text mono' : 'fact-text'}>{children}</span>
    </span>
  )
}

export function FactList({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={className ? `fact-list ${className}` : 'fact-list'}>{children}</span>
}

/** Caption under a cost chart when some usage has no price. */
export function UnpricedNote({ tokens }: { tokens: number }) {
  if (tokens <= 0) return null
  return <span>Excludes {formatTokens(tokens)} tokens from models without a price.</span>
}

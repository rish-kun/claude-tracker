import type { Plan } from '../api/client'

function trim(n: number, digits: number): string {
  return n.toFixed(digits).replace(/\.?0+$/, '')
}

export function formatTokens(n: number): string {
  const abs = Math.abs(n)
  if (abs >= 1e9) return `${trim(n / 1e9, 2)}B`
  if (abs >= 1e6) return `${trim(n / 1e6, 1)}M`
  if (abs >= 1e3) return `${trim(n / 1e3, 1)}k`
  return String(n)
}

export function formatUSD(n: number): string {
  if (Math.abs(n) >= 100) return `$${Math.round(n).toLocaleString('en-US')}`
  return `$${n.toFixed(2)}`
}

/**
 * Cost text for a totals block. Unpriced tokens carry no cost, so a $0 total that
 * is entirely unpriced reads "unpriced" rather than implying the usage was free.
 */
export function costLabel(t: { cost_usd: number; unpriced_tokens: number }): string {
  if (t.cost_usd === 0 && t.unpriced_tokens > 0) return 'unpriced'
  return formatUSD(t.cost_usd)
}

/**
 * Cost text for one chart cell (a series on a date). Never shows unpriced usage as $0:
 * "$x.xx" when fully priced, "$x.xx + N unpriced tokens" when partly priced, and
 * "unpriced (N tokens)" when the cell's cost is 0 but it has unpriced tokens.
 */
export function cellCost(cost: number, unpricedTokens: number, format: (n: number) => string = formatUSD): string {
  if (unpricedTokens <= 0) return format(cost)
  if (cost === 0) return `unpriced (${formatTokens(unpricedTokens)} tokens)`
  return `${format(cost)} + ${formatTokens(unpricedTokens)} unpriced tokens`
}

export function unpricedTitle(unpricedTokens: number): string {
  return `Includes ${formatTokens(unpricedTokens)} unpriced tokens (models with no price in the price table; not in the cost)`
}

export function formatPct(share: number): string {
  return `${Math.round(share * 100)}%`
}

export function formatDateTime(iso: string): string {
  return new Date(iso).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
}

export const PLANS: Plan[] = ['pro', 'max_5x', 'max_20x', 'team', 'enterprise', 'api_key', 'unknown']

export const PLAN_LABELS: Record<Plan, string> = {
  pro: 'Pro',
  max_5x: 'Max 5x',
  max_20x: 'Max 20x',
  team: 'Team',
  enterprise: 'Enterprise',
  api_key: 'API key',
  unknown: 'Unknown plan',
}

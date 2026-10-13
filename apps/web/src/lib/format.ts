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

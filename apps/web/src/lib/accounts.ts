import type { AccountRef, ByLogin, Plan, Schemas, Totals } from '../api/client'
import { PLANS } from './format'

type AccountUsage = Schemas['AccountUsage']

/** Accounts seen in the data, grouped by plan (plan order, then label). */
export function accountColumns(data: ByLogin): AccountRef[] {
  const byId = new Map<number, AccountRef>()
  for (const u of data.users) for (const a of u.by_account) byId.set(a.account.id, a.account)
  return [...byId.values()].sort((a, b) => PLANS.indexOf(a.plan) - PLANS.indexOf(b.plan) || a.label.localeCompare(b.label))
}

export interface PlanGroup {
  plan: Plan
  accounts: AccountRef[]
}

/** Runs of consecutive columns sharing a plan, for a grouped table header. */
export function planGroups(columns: AccountRef[]): PlanGroup[] {
  const groups: PlanGroup[] = []
  for (const c of columns) {
    const last = groups[groups.length - 1]
    if (last && last.plan === c.plan) last.accounts.push(c)
    else groups.push({ plan: c.plan, accounts: [c] })
  }
  return groups
}

/** A group with several accounts gets an extra per-plan subtotal column. */
export function hasSubtotal(g: PlanGroup): boolean {
  return g.accounts.length > 1
}

export function groupSpan(g: PlanGroup): number {
  return g.accounts.length + (hasSubtotal(g) ? 1 : 0)
}

/** Field-wise sum of totals blocks (cost and unpriced tokens included). */
export function sumTotals(ts: Totals[]): Totals {
  const out: Totals = {
    input: 0,
    output: 0,
    cache_write_5m: 0,
    cache_write_1h: 0,
    cache_read: 0,
    total_tokens: 0,
    cost_usd: 0,
    unpriced_tokens: 0,
  }
  for (const t of ts) {
    out.input += t.input
    out.output += t.output
    out.cache_write_5m += t.cache_write_5m
    out.cache_write_1h += t.cache_write_1h
    out.cache_read += t.cache_read
    out.total_tokens += t.total_tokens
    out.cost_usd += t.cost_usd
    out.unpriced_tokens += t.unpriced_tokens
  }
  return out
}

/** One login's usage across all its accounts on a plan; undefined when it used none of them. */
export function planSubtotal(byAccount: AccountUsage[], plan: Plan): Totals | undefined {
  const rows = byAccount.filter((a) => a.account.plan === plan)
  return rows.length === 0 ? undefined : sumTotals(rows.map((a) => a.totals))
}

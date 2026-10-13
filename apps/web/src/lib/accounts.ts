import type { AccountRef, ByLogin, Plan } from '../api/client'
import { PLANS } from './format'

/** Accounts seen in the data, grouped by plan (plan order, then label). */
export function accountColumns(data: ByLogin): AccountRef[] {
  const byId = new Map<number, AccountRef>()
  for (const u of data.users) for (const a of u.by_account) byId.set(a.account.id, a.account)
  return [...byId.values()].sort((a, b) => PLANS.indexOf(a.plan) - PLANS.indexOf(b.plan) || a.label.localeCompare(b.label))
}

/** Runs of consecutive columns sharing a plan, for a grouped table header. */
export function planGroups(columns: AccountRef[]): { plan: Plan; count: number }[] {
  const groups: { plan: Plan; count: number }[] = []
  for (const c of columns) {
    const last = groups[groups.length - 1]
    if (last && last.plan === c.plan) last.count++
    else groups.push({ plan: c.plan, count: 1 })
  }
  return groups
}

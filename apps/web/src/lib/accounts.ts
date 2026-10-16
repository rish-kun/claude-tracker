import type { Account, AccountRef, ByLogin, LimitSnapshot, Plan, Schemas, Totals } from '../api/client'
import { type Draft, draftConflict, draftDirty, type DraftOps, startDraft, syncDraft } from './drafts'
import { PLANS } from './format'
import { limitPressure } from './limits'

type AccountUsage = Schemas['AccountUsage']

/** How to name a full Account: its admin label (raw, may be empty), else email, else uuid. */
export function accountName(a: Pick<Account, 'label' | 'email' | 'account_uuid'>): string {
  return a.label || a.email || a.account_uuid
}

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

/** The editable fields of an account row. */
export interface AccountFields {
  label: string
  plan: Plan
}

/** An editable account row: the typed label/plan and the server values it was started from. */
export type AccountDraft = Draft<AccountFields, AccountFields>

const accountOps: DraftOps<AccountFields, AccountFields> = {
  fromServer: (a) => ({ label: a.label, plan: a.plan }),
  sameServer: (a, b) => a.label === b.label && a.plan === b.plan,
  matches: (d, a) => d.label === a.label && d.plan === a.plan,
}

export function accountDraft(a: AccountFields): AccountDraft {
  return startDraft(accountOps, { label: a.label, plan: a.plan })
}

export function accountDirty(s: AccountDraft): boolean {
  return draftDirty(accountOps, s)
}

/** Follows new server label/plan while pristine; keeps a dirty draft (see `syncDraft`). */
export function syncAccountDraft(s: AccountDraft, server: AccountFields): AccountDraft {
  return syncDraft(accountOps, s, server)
}

/** The server label/plan changed underneath an edited draft. */
export function accountConflict(s: AccountDraft, server: AccountFields): boolean {
  return draftConflict(accountOps, s, server)
}

/** A plan guessed from Claude's reported rate-limit tier, offered to admins as a suggestion. */
export function suggestPlan(tier?: string): Plan | undefined {
  if (!tier) return undefined
  if (/max_20x|20x/i.test(tier)) return 'max_20x'
  if (/max_5x|5x/i.test(tier)) return 'max_5x'
  if (/pro/i.test(tier)) return 'pro'
  return undefined
}

/**
 * Accounts ordered by current limit pressure (fullest window first; a window whose reset
 * has passed counts as 0), accounts without readings last, ties by API-equivalent cost.
 */
export function sortByPressure<T extends { limits: LimitSnapshot[]; totals: Totals }>(accounts: T[], now: number): T[] {
  const pressure = (a: T) => limitPressure(a.limits[a.limits.length - 1], now)
  return [...accounts].sort((a, b) => {
    const pa = pressure(a)
    const pb = pressure(b)
    if (pa === undefined && pb !== undefined) return 1
    if (pb === undefined && pa !== undefined) return -1
    if (pa !== undefined && pb !== undefined && pa !== pb) return pb - pa
    return b.totals.cost_usd - a.totals.cost_usd
  })
}

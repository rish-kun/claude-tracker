import { describe, expect, it } from 'vitest'
import type { AccountRef, ByLogin, Totals } from '../api/client'
import { accountColumns, accountConflict, accountDirty, accountDraft, accountName, groupSpan, planGroups, planSubtotal, sumTotals, syncAccountDraft } from './accounts'

const zero: Totals = {
  input: 0,
  output: 0,
  cache_write_5m: 0,
  cache_write_1h: 0,
  cache_read: 0,
  total_tokens: 0,
  cost_usd: 0,
  unpriced_tokens: 0,
}
const t = (over: Partial<Totals>): Totals => ({ ...zero, ...over })
const acct = (id: number, label: string, plan: AccountRef['plan']): AccountRef => ({ id, label, plan })
const user = (id: number, accounts: AccountRef[]) => ({
  user: { id, email: `u${id}@x`, name: '' },
  totals: zero,
  by_account: accounts.map((account) => ({ account, totals: zero })),
})

describe('accountColumns', () => {
  it('dedupes accounts across users and orders by plan, then label', () => {
    const data: ByLogin = {
      users: [
        user(1, [acct(3, 'zeta', 'pro'), acct(1, 'beta', 'max_20x')]),
        user(2, [acct(2, 'alpha', 'max_20x'), acct(3, 'zeta', 'pro'), acct(4, 'key', 'api_key'), acct(5, 'alpha', 'pro')]),
      ],
      daily_by_user: [],
    }
    expect(accountColumns(data).map((a) => a.id)).toEqual([5, 3, 2, 1, 4])
  })
  it('returns nothing for no users', () => {
    expect(accountColumns({ users: [], daily_by_user: [] })).toEqual([])
  })
})

describe('planGroups', () => {
  it('groups consecutive columns per plan and adds a subtotal column only to multi-account groups', () => {
    const cols = [acct(1, 'a', 'pro'), acct(2, 'b', 'pro'), acct(3, 'c', 'max_5x'), acct(4, 'd', 'unknown')]
    const groups = planGroups(cols)
    expect(groups.map((g) => [g.plan, g.accounts.map((a) => a.id)])).toEqual([
      ['pro', [1, 2]],
      ['max_5x', [3]],
      ['unknown', [4]],
    ])
    expect(groups.map(groupSpan)).toEqual([3, 1, 1])
  })
  it('is empty for no columns', () => {
    expect(planGroups([])).toEqual([])
  })
})

describe('sumTotals', () => {
  it('sums every field, including unpriced tokens', () => {
    expect(
      sumTotals([
        t({ input: 1, output: 2, cache_write_5m: 3, cache_write_1h: 4, cache_read: 5, total_tokens: 15, cost_usd: 1.5 }),
        t({ input: 10, total_tokens: 10, unpriced_tokens: 10 }),
      ]),
    ).toEqual({ input: 11, output: 2, cache_write_5m: 3, cache_write_1h: 4, cache_read: 5, total_tokens: 25, cost_usd: 1.5, unpriced_tokens: 10 })
  })
  it('is all zero for nothing', () => {
    expect(sumTotals([])).toEqual(zero)
  })
})

describe('planSubtotal', () => {
  const byAccount = [
    { account: acct(1, 'a', 'max_20x'), totals: t({ total_tokens: 100, cost_usd: 2 }) },
    { account: acct(2, 'b', 'max_20x'), totals: t({ total_tokens: 50, unpriced_tokens: 50 }) },
    { account: acct(3, 'c', 'pro'), totals: t({ total_tokens: 7, cost_usd: 9 }) },
  ]
  it("sums only the plan's accounts and keeps partly-unpriced usage visible", () => {
    expect(planSubtotal(byAccount, 'max_20x')).toEqual(t({ total_tokens: 150, cost_usd: 2, unpriced_tokens: 50 }))
  })
  it('is fully unpriced when every account on the plan is unpriced', () => {
    const only = [byAccount[1], { account: acct(4, 'd', 'max_20x'), totals: t({ total_tokens: 5, unpriced_tokens: 5 }) }]
    expect(planSubtotal(only, 'max_20x')).toEqual(t({ total_tokens: 55, unpriced_tokens: 55 }))
  })
  it('is undefined when the login used no account on the plan', () => {
    expect(planSubtotal(byAccount, 'team')).toBeUndefined()
  })
})

describe('accountName', () => {
  it('prefers the raw label, then email, then uuid', () => {
    expect(accountName({ label: 'Team Max', email: 'a@x.com', account_uuid: 'u1' })).toBe('Team Max')
    expect(accountName({ label: '', email: 'a@x.com', account_uuid: 'u1' })).toBe('a@x.com')
    expect(accountName({ label: '', account_uuid: 'u1' })).toBe('u1')
  })
})

describe('account row drafts', () => {
  const server = { label: 'Work', plan: 'max_5x' as const }
  const newer = { label: 'Work (shared)', plan: 'max_20x' as const }

  it('leaves a row alone while the server label/plan are unchanged', () => {
    const s = accountDraft(server)
    expect(syncAccountDraft(s, { ...server })).toBe(s)
    const edited = { ...s, draft: { ...s.draft, label: 'Mine' } }
    expect(accountDirty(edited)).toBe(true)
    const fullAccount = { ...server, id: 1, email: 'x@y' } // other Account fields don't matter
    expect(syncAccountDraft(edited, fullAccount)).toBe(edited)
    expect(accountConflict(edited, server)).toBe(false)
  })

  it('resets a pristine draft when the server label or plan changes', () => {
    const s = syncAccountDraft(accountDraft(server), newer)
    expect(s.draft).toEqual(newer)
    expect(accountDirty(s)).toBe(false)
    expect(accountConflict(s, newer)).toBe(false)
  })

  it('keeps a dirty draft and flags the conflict until the row is reloaded', () => {
    const edited = { ...accountDraft(server), draft: { label: 'Mine', plan: 'max_5x' as const } }
    const s = syncAccountDraft(edited, newer)
    expect(s).toBe(edited)
    expect(s.draft.label).toBe('Mine')
    expect(accountConflict(s, newer)).toBe(true)
    const reloaded = accountDraft(newer)
    expect(accountConflict(reloaded, newer)).toBe(false)
    expect(accountDirty(reloaded)).toBe(false)
  })

  it('treats a plan-only server change under a label edit as a conflict too', () => {
    const edited = { ...accountDraft(server), draft: { ...server, label: 'Mine' } }
    expect(accountConflict(syncAccountDraft(edited, { ...server, plan: 'team' }), { ...server, plan: 'team' })).toBe(true)
  })

  it('adopts a server change the dirty draft already matches (e.g. its own save)', () => {
    const edited = { ...accountDraft(server), draft: { ...newer } }
    const s = syncAccountDraft(edited, newer)
    expect(s.base).toEqual(newer)
    expect(accountConflict(s, newer)).toBe(false)
    expect(accountDirty(s)).toBe(false)
  })
})

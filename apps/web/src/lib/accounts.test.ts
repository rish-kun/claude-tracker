import { describe, expect, it } from 'vitest'
import type { AccountRef, ByLogin, Totals } from '../api/client'
import { accountColumns, planGroups } from './accounts'

const totals: Totals = {
  input: 0,
  output: 0,
  cache_write_5m: 0,
  cache_write_1h: 0,
  cache_read: 0,
  total_tokens: 0,
  cost_usd: 0,
  unpriced_tokens: 0,
}
const acct = (id: number, label: string, plan: AccountRef['plan']): AccountRef => ({ id, label, plan })
const user = (id: number, accounts: AccountRef[]) => ({
  user: { id, email: `u${id}@x`, name: '' },
  totals,
  by_account: accounts.map((account) => ({ account, totals })),
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
  it('counts consecutive columns per plan', () => {
    const cols = [acct(1, 'a', 'pro'), acct(2, 'b', 'pro'), acct(3, 'c', 'max_5x'), acct(4, 'd', 'unknown')]
    expect(planGroups(cols)).toEqual([
      { plan: 'pro', count: 2 },
      { plan: 'max_5x', count: 1 },
      { plan: 'unknown', count: 1 },
    ])
  })
  it('is empty for no columns', () => {
    expect(planGroups([])).toEqual([])
  })
})

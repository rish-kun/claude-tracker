import { describe, expect, it } from 'vitest'
import type { ProxyKey } from '../api/client'
import { groupProxyKeys, proxyKeyOwner, revokePrompt } from './proxyKeys'

const k = (id: number, status: ProxyKey['status'], created_at: string, extra: Partial<ProxyKey> = {}): ProxyKey => ({
  id,
  user: { id: 1, email: 'a@example.com', name: 'A' },
  device_name: 'mac',
  status,
  created_at,
  linked: true,
  ...extra,
})

describe('groupProxyKeys', () => {
  it('queues pending oldest first, approved and revoked most recent first', () => {
    const g = groupProxyKeys([
      k(1, 'pending', '2026-11-03T00:00:00Z'),
      k(2, 'pending', '2026-11-01T00:00:00Z'),
      k(3, 'approved', '2026-11-01T00:00:00Z', { approved_at: '2026-11-05T00:00:00Z' }),
      k(4, 'approved', '2026-11-02T00:00:00Z', { approved_at: '2026-11-04T00:00:00Z' }),
      k(5, 'revoked', '2026-11-02T00:00:00Z', { revoked_at: '2026-11-04T00:00:00Z' }),
      k(6, 'revoked', '2026-11-01T00:00:00Z', { revoked_at: '2026-11-06T00:00:00Z', approved_at: '2026-11-02T00:00:00Z' }),
    ])
    expect(g.pending.map((x) => x.id)).toEqual([2, 1])
    expect(g.approved.map((x) => x.id)).toEqual([3, 4])
    expect(g.revoked.map((x) => x.id)).toEqual([6, 5])
  })

  it('breaks ties by id and handles an empty list', () => {
    const same = '2026-11-01T00:00:00Z'
    expect(groupProxyKeys([k(9, 'pending', same), k(8, 'pending', same)]).pending.map((x) => x.id)).toEqual([8, 9])
    expect(groupProxyKeys([])).toEqual({ pending: [], approved: [], revoked: [] })
  })
})

describe('owner and prompts', () => {
  it('names the person by name and email, or email alone', () => {
    expect(proxyKeyOwner(k(1, 'pending', '2026-11-01T00:00:00Z'))).toBe('A (a@example.com)')
    const anon = k(1, 'pending', '2026-11-01T00:00:00Z', { user: { id: 2, email: 'b@example.com', name: '' } })
    expect(proxyKeyOwner(anon)).toBe('b@example.com')
    expect(revokePrompt({ ...anon, device_name: 'b-laptop' })).toBe(
      'Revoke the request for b@example.com on b-laptop? The proxy stops accepting the key within a minute.',
    )
    expect(revokePrompt(k(1, 'approved', '2026-11-01T00:00:00Z'))).toMatch(/^Revoke access for A \(a@example.com\) on mac\?/)
  })
})

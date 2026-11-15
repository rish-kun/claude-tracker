import type { ProxyKey } from '../api/client'

export type ProxyKeyGroups = { pending: ProxyKey[]; approved: ProxyKey[]; revoked: ProxyKey[] }

const t = (iso: string) => Date.parse(iso)

/**
 * Pending requests oldest first (the approval queue), approved keys most
 * recently approved first, revoked keys most recently revoked first.
 */
export function groupProxyKeys(keys: ProxyKey[]): ProxyKeyGroups {
  const by = (s: ProxyKey['status']) => keys.filter((k) => k.status === s)
  return {
    pending: by('pending').sort((a, b) => t(a.created_at) - t(b.created_at) || a.id - b.id),
    approved: by('approved').sort((a, b) => t(b.approved_at ?? b.created_at) - t(a.approved_at ?? a.created_at) || b.id - a.id),
    revoked: by('revoked').sort((a, b) => t(b.revoked_at ?? b.created_at) - t(a.revoked_at ?? a.created_at) || b.id - a.id),
  }
}

/** "Name (email)" or just the email, for prompts. */
export function proxyKeyOwner(k: ProxyKey): string {
  return k.user.name ? `${k.user.name} (${k.user.email})` : k.user.email
}

/** What the admin confirms before revoking a key. */
export function revokePrompt(k: ProxyKey): string {
  const what = k.status === 'pending' ? 'the request' : 'access'
  return `Revoke ${what} for ${proxyKeyOwner(k)} on ${k.device_name}? The proxy stops accepting the key within a minute.`
}

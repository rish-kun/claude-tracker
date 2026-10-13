import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { api, unwrap, type Plan, type PriceRates } from './client'
import { type Range, rangeKey, rangeQuery } from '../lib/range'

export const PAGE_SIZE = 20

export function useMe() {
  return useQuery({ queryKey: ['me'], queryFn: () => unwrap(api.GET('/me')), retry: false })
}

/** Throws ApiError on a non-2xx response; the caller navigates away and then clears the cache. */
export function useLogout() {
  return useMutation({
    mutationFn: () => unwrap(api.POST('/auth/logout')),
  })
}

export function useMyUsage(r: Range) {
  return useQuery({
    queryKey: ['me', 'usage', rangeKey(r)],
    queryFn: () => unwrap(api.GET('/me/usage', { params: { query: rangeQuery(r) } })),
  })
}

export function useMySessions(page: number) {
  return useQuery({
    queryKey: ['me', 'sessions', page],
    queryFn: () => unwrap(api.GET('/me/sessions', { params: { query: { limit: PAGE_SIZE, offset: page * PAGE_SIZE } } })),
    placeholderData: (prev) => prev,
  })
}

export function useByLogin(r: Range) {
  return useQuery({
    queryKey: ['admin', 'by-login', rangeKey(r)],
    queryFn: () => unwrap(api.GET('/admin/by-login', { params: { query: rangeQuery(r) } })),
  })
}

export function useByAccount(r: Range) {
  return useQuery({
    queryKey: ['admin', 'by-account', rangeKey(r)],
    queryFn: () => unwrap(api.GET('/admin/by-account', { params: { query: rangeQuery(r) } })),
  })
}

export function useUserSessions(userId: number, page: number) {
  return useQuery({
    queryKey: ['admin', 'user-sessions', userId, page],
    queryFn: () =>
      unwrap(api.GET('/admin/users/{id}/sessions', {
        params: { path: { id: userId }, query: { limit: PAGE_SIZE, offset: page * PAGE_SIZE } },
      })),
    placeholderData: (prev) => prev,
  })
}

export function useAccounts() {
  return useQuery({ queryKey: ['admin', 'accounts'], queryFn: () => unwrap(api.GET('/admin/accounts')) })
}

export function useUsers() {
  return useQuery({ queryKey: ['admin', 'users'], queryFn: () => unwrap(api.GET('/admin/users')) })
}

export function useTokens() {
  return useQuery({ queryKey: ['admin', 'tokens'], queryFn: () => unwrap(api.GET('/admin/tokens')) })
}

export function usePrices() {
  return useQuery({ queryKey: ['admin', 'prices'], queryFn: () => unwrap(api.GET('/admin/prices')) })
}

export function useUpdateAccount() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (v: { id: number; label: string; plan: Plan }) =>
      unwrap(api.PATCH('/admin/accounts/{id}', { params: { path: { id: v.id } }, body: { label: v.label, plan: v.plan } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['admin'] }),
  })
}

/** Throws ApiError on a non-2xx response so a failed revoke is surfaced, not silently treated as success. */
export function useRevokeToken() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => unwrap(api.DELETE('/admin/tokens/{id}', { params: { path: { id } } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['admin', 'tokens'] }),
  })
}

export function usePutPrice() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (v: { model: string; rates: PriceRates }) =>
      unwrap(api.PUT('/admin/prices/{model}', { params: { path: { model: v.model } }, body: v.rates })),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['admin'] }),
  })
}

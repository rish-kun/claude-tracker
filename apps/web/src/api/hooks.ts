import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { api, unwrap } from './client'
import { type Range, rangeKey, rangeQuery } from '../lib/range'

export const PAGE_SIZE = 20

export function useMe() {
  return useQuery({ queryKey: ['me'], queryFn: () => unwrap(api.GET('/me')), retry: false })
}

export function useLogout() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: () => api.POST('/auth/logout'),
    onSuccess: () => qc.clear(),
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

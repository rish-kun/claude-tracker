import { MutationCache, QueryCache, QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider } from '@tanstack/react-router'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { createUnauthorizedHandler, LOGIN_PATH, shouldRetry } from './api/unauthorized'
import { initTheme } from './lib/theme'
import { router } from './router'
import './styles.css'

initTheme()

// Any 401, from any query or mutation, means the session is gone: go to /login and drop the cache.
const onError = createUnauthorizedHandler({
  pathname: () => router.state.location.pathname,
  toLogin: () => router.navigate({ to: LOGIN_PATH }),
  clear: () => queryClient.clear(),
})

const queryClient = new QueryClient({
  queryCache: new QueryCache({ onError }),
  mutationCache: new MutationCache({ onError }),
  // Window-focus refetching stays off: the range clock (AppLayout) already advances the range,
  // and so refetches usage, when the tab becomes visible, and usage queries poll every 60 s.
  defaultOptions: { queries: { staleTime: 30_000, refetchOnWindowFocus: false, retry: shouldRetry } },
})

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
)

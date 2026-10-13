import { ApiError } from './client'

export const LOGIN_PATH = '/login'

export function isUnauthorized(err: unknown): boolean {
  return err instanceof ApiError && err.status === 401
}

/** Retry transient failures up to 3 times, but never a 401: the session is gone. */
export function shouldRetry(failureCount: number, err: unknown): boolean {
  return !isUnauthorized(err) && failureCount < 3
}

export interface UnauthorizedDeps {
  /** The current router pathname. */
  pathname: () => string
  /** Navigate to the login page. */
  toLogin: () => Promise<unknown>
  /** Drop all cached data (another user may sign in next). */
  clear: () => void
}

/**
 * The QueryCache / MutationCache onError handler: on any 401, go to /login and then clear
 * the cache. Navigating first unmounts the app's observers, so clearing doesn't make them
 * refetch (and 401) again. Concurrent 401s, and 401s while already on /login, do nothing.
 */
export function createUnauthorizedHandler(deps: UnauthorizedDeps): (err: unknown) => void {
  let redirecting = false
  return (err) => {
    if (!isUnauthorized(err) || redirecting || deps.pathname() === LOGIN_PATH) return
    redirecting = true
    void deps
      .toLogin()
      .catch(() => {})
      .finally(() => {
        deps.clear()
        redirecting = false
      })
  }
}

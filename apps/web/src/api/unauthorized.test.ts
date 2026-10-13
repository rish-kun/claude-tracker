import { describe, expect, it, vi } from 'vitest'
import { ApiError } from './client'
import { createUnauthorizedHandler, isUnauthorized, shouldRetry } from './unauthorized'

function setup(path = '/') {
  let pathname = path
  let resolve!: () => void
  const order: string[] = []
  const toLogin = vi.fn(
    () =>
      new Promise<void>((r) => {
        order.push('navigate')
        resolve = () => {
          pathname = '/login'
          r()
        }
      }),
  )
  const clear = vi.fn(() => order.push('clear'))
  const handle = createUnauthorizedHandler({ pathname: () => pathname, toLogin, clear })
  return { handle, toLogin, clear, order, finish: () => resolve() }
}

describe('isUnauthorized / shouldRetry', () => {
  it('recognises only 401 ApiErrors', () => {
    expect(isUnauthorized(new ApiError(401, 'unauthorized'))).toBe(true)
    expect(isUnauthorized(new ApiError(403, 'forbidden'))).toBe(false)
    expect(isUnauthorized(new Error('network'))).toBe(false)
  })

  it('never retries a 401 but retries other failures up to 3 times', () => {
    expect(shouldRetry(0, new ApiError(401, 'x'))).toBe(false)
    expect(shouldRetry(0, new ApiError(500, 'x'))).toBe(true)
    expect(shouldRetry(2, new Error('network'))).toBe(true)
    expect(shouldRetry(3, new Error('network'))).toBe(false)
  })
})

describe('createUnauthorizedHandler', () => {
  it('navigates to /login and then clears the cache on a 401', async () => {
    const s = setup('/admin/settings')
    s.handle(new ApiError(401, 'unauthorized'))
    expect(s.toLogin).toHaveBeenCalledTimes(1)
    expect(s.clear).not.toHaveBeenCalled()
    s.finish()
    await vi.waitFor(() => expect(s.clear).toHaveBeenCalledTimes(1))
    expect(s.order).toEqual(['navigate', 'clear'])
  })

  it('ignores other errors', () => {
    const s = setup()
    s.handle(new ApiError(500, 'boom'))
    s.handle(new ApiError(403, 'forbidden'))
    s.handle(new Error('network'))
    expect(s.toLogin).not.toHaveBeenCalled()
    expect(s.clear).not.toHaveBeenCalled()
  })

  it('redirects once for a burst of concurrent 401s', async () => {
    const s = setup()
    s.handle(new ApiError(401, 'a'))
    s.handle(new ApiError(401, 'b'))
    s.handle(new ApiError(401, 'c'))
    expect(s.toLogin).toHaveBeenCalledTimes(1)
    s.finish()
    await vi.waitFor(() => expect(s.clear).toHaveBeenCalledTimes(1))
    // Already on /login now: a late 401 must not loop.
    s.handle(new ApiError(401, 'late'))
    expect(s.toLogin).toHaveBeenCalledTimes(1)
  })

  it('does nothing on /login itself', () => {
    const s = setup('/login')
    s.handle(new ApiError(401, 'unauthorized'))
    expect(s.toLogin).not.toHaveBeenCalled()
    expect(s.clear).not.toHaveBeenCalled()
  })
})

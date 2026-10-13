import { useQueryClient } from '@tanstack/react-query'
import { Link, Outlet, useNavigate } from '@tanstack/react-router'
import { useEffect, useMemo, useState } from 'react'
import { ApiError } from '../api/client'
import { useLogout, useMe } from '../api/hooks'
import { msUntilRangeTick, nextMinute, type Preset, presetRange } from '../lib/range'
import { RangeContext } from '../lib/rangeContext'

const PRESETS: { value: Preset; label: string }[] = [
  { value: 'today', label: 'Today' },
  { value: '7d', label: '7 days' },
  { value: '30d', label: '30 days' },
  { value: '90d', label: '90 days' },
]

/**
 * The current time, re-read on every whole minute (which includes local midnight) and
 * whenever the tab becomes visible again, since background tabs throttle timers.
 */
function useMinuteClock(): [number, () => void] {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout>
    const schedule = () => {
      // A few ms past the boundary so the new minute is definitely current.
      timer = setTimeout(() => {
        setNow(Date.now())
        schedule()
      }, msUntilRangeTick(Date.now()) + 50)
    }
    const onVisible = () => {
      if (document.visibilityState !== 'visible') return
      clearTimeout(timer)
      setNow(Date.now())
      schedule()
    }
    schedule()
    document.addEventListener('visibilitychange', onVisible)
    return () => {
      clearTimeout(timer)
      document.removeEventListener('visibilitychange', onVisible)
    }
  }, [])
  return [now, () => setNow(Date.now())]
}

export function AppLayout() {
  const me = useMe()
  const logout = useLogout()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [preset, setPreset] = useState<Preset>('30d')
  const [now, tick] = useMinuteClock()
  // The range only changes per minute; deriving it from the minute keeps its identity stable.
  // `minute - 1` is the last instant of the current minute, so it is on the same day as `now`.
  const minute = nextMinute(now)
  const range = useMemo(() => presetRange(preset, new Date(minute - 1)), [preset, minute])
  const choosePreset = (p: Preset) => {
    if (p !== preset) {
      setPreset(p)
      tick()
    } else if (nextMinute(Date.now()) !== minute) {
      tick() // The range advances to a new key, which fetches it.
    } else {
      void queryClient.invalidateQueries({ type: 'active' }) // Same key: refetch what's on screen.
    }
  }
  const unauthorized = me.error instanceof ApiError && me.error.status === 401

  useEffect(() => {
    if (unauthorized) void navigate({ to: '/login' })
  }, [unauthorized, navigate])

  if (me.isPending || unauthorized) return <div className="page muted">Loading…</div>
  if (me.error) return <div className="page error">Could not load your account: {me.error.message}</div>

  return (
    <RangeContext.Provider value={{ preset, setPreset: choosePreset, range }}>
      <header className="topbar">
        <span className="brand">cctrack</span>
        <nav>
          <Link to="/" activeOptions={{ exact: true }}>My usage</Link>
          {me.data.is_admin && (
            <>
              <Link to="/admin/logins">By login</Link>
              <Link to="/admin/accounts">By account</Link>
              <Link to="/admin/settings">Settings</Link>
            </>
          )}
        </nav>
        <div className="spacer" />
        <div className="presets" role="group" aria-label="Date range">
          {PRESETS.map((p) => (
            <button key={p.value} className={p.value === preset ? 'active' : ''} onClick={() => choosePreset(p.value)}>
              {p.label}
            </button>
          ))}
        </div>
        <span className="muted">{me.data.email}</span>
        {logout.error && <span className="error">Sign out failed: {logout.error.message}</span>}
        <button
          className="link"
          disabled={logout.isPending}
          onClick={() =>
            logout.mutate(undefined, {
              onSuccess: async () => {
                await navigate({ to: '/login' })
                queryClient.clear()
              },
            })
          }
        >
          Sign out
        </button>
      </header>
      <main className="page">
        <Outlet />
      </main>
    </RangeContext.Provider>
  )
}

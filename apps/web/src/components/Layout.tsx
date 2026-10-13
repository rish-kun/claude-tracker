import { Link, Outlet, useNavigate } from '@tanstack/react-router'
import { useEffect, useMemo, useState } from 'react'
import { ApiError } from '../api/client'
import { useLogout, useMe } from '../api/hooks'
import { type Preset, presetRange } from '../lib/range'
import { RangeContext } from '../lib/rangeContext'

const PRESETS: { value: Preset; label: string }[] = [
  { value: 'today', label: 'Today' },
  { value: '7d', label: '7 days' },
  { value: '30d', label: '30 days' },
  { value: '90d', label: '90 days' },
]

export function AppLayout() {
  const me = useMe()
  const logout = useLogout()
  const navigate = useNavigate()
  const [preset, setPreset] = useState<Preset>('30d')
  const range = useMemo(() => presetRange(preset), [preset])
  const unauthorized = me.error instanceof ApiError && me.error.status === 401

  useEffect(() => {
    if (unauthorized) void navigate({ to: '/login' })
  }, [unauthorized, navigate])

  if (me.isPending || unauthorized) return <div className="page muted">Loading…</div>
  if (me.error) return <div className="page error">Could not load your account: {me.error.message}</div>

  return (
    <RangeContext.Provider value={{ preset, setPreset, range }}>
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
            <button key={p.value} className={p.value === preset ? 'active' : ''} onClick={() => setPreset(p.value)}>
              {p.label}
            </button>
          ))}
        </div>
        <span className="muted">{me.data.email}</span>
        <button
          className="link"
          onClick={() => logout.mutate(undefined, { onSuccess: () => void navigate({ to: '/login' }) })}
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

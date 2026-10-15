import { useQueryClient } from '@tanstack/react-query'
import { Link, Outlet, useNavigate, useRouterState } from '@tanstack/react-router'
import { type KeyboardEvent, type ReactNode, type RefObject, useEffect, useMemo, useRef, useState } from 'react'
import { ApiError, type Me } from '../api/client'
import { useAccounts, useLogout, useMe } from '../api/hooks'
import { msUntilRangeTick, nextMinute, type Preset, PRESETS, presetRange, rangeLabel } from '../lib/range'
import { RangeContext, useRange } from '../lib/rangeContext'
import { Button, ErrorNotice, Glyph } from './ui'

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

function Nav({ me, needsPlan }: { me: Me; needsPlan: boolean }) {
  return (
    <nav className="nav" aria-label="Main">
      <Link to="/" activeOptions={{ exact: true }}>
        My usage
      </Link>
      {me.is_admin && (
        <>
          <Link to="/admin/logins">By login</Link>
          <Link to="/admin/accounts">By account</Link>
          <Link to="/admin/settings">
            Settings
            {needsPlan && (
              <>
                <span className="nav-dot" aria-hidden="true" />
                <span className="sr-only">(an account needs a plan)</span>
              </>
            )}
          </Link>
        </>
      )}
    </nav>
  )
}

function UserMenu({ me }: { me: Me }) {
  const logout = useLogout()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [open, setOpen] = useState(false)
  const wrap = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  const display = me.name || me.email
  const first = (me.name || me.email.split('@')[0]).split(/\s+/)[0]

  const close = (refocus: boolean) => {
    setOpen(false)
    if (refocus) trigger.current?.focus()
  }

  useEffect(() => {
    if (!open) return
    const onDown = (e: PointerEvent) => {
      if (!wrap.current?.contains(e.target as Node)) close(false)
    }
    document.addEventListener('pointerdown', onDown)
    return () => document.removeEventListener('pointerdown', onDown)
  }, [open])

  return (
    <div
      className="user-menu"
      ref={wrap}
      onKeyDown={(e) => {
        if (e.key === 'Escape' && open) {
          e.stopPropagation()
          close(true)
        }
      }}
      onBlur={(e) => {
        if (open && !wrap.current?.contains(e.relatedTarget as Node | null) && e.relatedTarget !== null) close(false)
      }}
    >
      <button
        ref={trigger}
        type="button"
        className="user-trigger"
        aria-haspopup="true"
        aria-expanded={open}
        aria-controls="user-menu-pop"
        aria-label={`Account menu for ${display}`}
        onClick={() => setOpen((o) => !o)}
      >
        <span className="avatar" aria-hidden="true">
          {display.trim().charAt(0).toUpperCase()}
        </span>
        <span className="user-first">{first}</span>
        <Glyph name="chevron-down" />
      </button>
      {open && (
        <div className="menu-pop" id="user-menu-pop">
          <div style={{ fontWeight: 600 }}>{me.name || me.email}</div>
          {me.name && <div className="menu-email">{me.email}</div>}
          <hr />
          <Button
            variant="secondary"
            style={{ width: '100%' }}
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
            {logout.isPending ? 'Signing out…' : 'Sign out'}
          </Button>
          {logout.error && (
            <p className="field-error mt-3" role="alert">
              Couldn't sign out: {logout.error.message}
            </p>
          )}
        </div>
      )}
    </div>
  )
}

/** Keeps the active tab visible in the phone tab row after navigation. */
function useActiveTabIntoView(ref: RefObject<HTMLDivElement | null>) {
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  useEffect(() => {
    const el = ref.current?.querySelector<HTMLElement>("[data-status='active']")
    el?.scrollIntoView({ block: 'nearest', inline: 'nearest' })
  }, [pathname, ref])
}

export function AppLayout() {
  const me = useMe()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const [preset, setPreset] = useState<Preset>('30d')
  const [now, tick] = useMinuteClock()
  const accounts = useAccounts(me.data?.is_admin === true)
  const tabrow = useRef<HTMLDivElement>(null)
  useActiveTabIntoView(tabrow)
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

  if (me.isPending || unauthorized) {
    return (
      <div className="center-screen" aria-busy="true">
        <span className="wordmark">cctrack</span>
        <span className="loading-shell">Loading your account…</span>
      </div>
    )
  }
  if (me.error) {
    return (
      <div className="center-screen">
        <span className="wordmark">cctrack</span>
        <ErrorNotice what="load your account" error={me.error} onRetry={() => void me.refetch()} />
      </div>
    )
  }

  const needsPlan = accounts.data?.some((a) => a.plan === 'unknown') ?? false

  return (
    <RangeContext.Provider value={{ preset, setPreset: choosePreset, range, now }}>
      <header className="app-header">
        <div className="header-row">
          <Link to="/" className="wordmark">
            cctrack
          </Link>
          <Nav me={me.data} needsPlan={needsPlan} />
          <div className="header-end">
            <UserMenu me={me.data} />
          </div>
        </div>
        <div className="tabrow" ref={tabrow}>
          <Nav me={me.data} needsPlan={needsPlan} />
        </div>
      </header>
      <main className="page">
        <Outlet />
      </main>
    </RangeContext.Provider>
  )
}

/** Date range presets as a radio group: arrow keys move and select, one tab stop. */
export function RangeControl() {
  const { preset, setPreset } = useRange()
  const refs = useRef<(HTMLButtonElement | null)[]>([])
  const onKey = (e: KeyboardEvent<HTMLButtonElement>, i: number) => {
    let next = -1
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') next = (i + 1) % PRESETS.length
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') next = (i - 1 + PRESETS.length) % PRESETS.length
    else if (e.key === 'Home') next = 0
    else if (e.key === 'End') next = PRESETS.length - 1
    if (next < 0) return
    e.preventDefault()
    setPreset(PRESETS[next].value)
    refs.current[next]?.focus()
  }
  return (
    <div className="range-control" role="radiogroup" aria-label="Date range">
      {PRESETS.map((p, i) => {
        const checked = p.value === preset
        return (
          <button
            key={p.value}
            ref={(el) => {
              refs.current[i] = el
            }}
            type="button"
            role="radio"
            aria-checked={checked}
            tabIndex={checked ? 0 : -1}
            onClick={() => setPreset(p.value)}
            onKeyDown={(e) => onKey(e, i)}
          >
            {p.label}
          </button>
        )
      })}
    </div>
  )
}

export function PageHeader({ title, range = false, children }: { title: string; range?: boolean; children?: ReactNode }) {
  const r = useRange()
  return (
    <div className="page-header">
      <h1>{title}</h1>
      {range && (
        <div className="page-header-range">
          <RangeControl />
          <span className="range-meta">{rangeLabel(r.preset, r.range)}</span>
        </div>
      )}
      {children}
    </div>
  )
}

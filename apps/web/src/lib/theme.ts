import { useSyncExternalStore } from 'react'

/**
 * Light/dark theme. The choice is stored per browser; "system" follows the OS setting.
 * The resolved theme is `data-theme` on <html>, which the stylesheet's dark tokens key off.
 * index.html sets it before first paint with the same rules as `resolveTheme`.
 */
export type ThemePref = 'system' | 'light' | 'dark'
export type Theme = 'light' | 'dark'

export const THEME_KEY = 'cctrack-theme'

export const THEME_OPTIONS: { value: ThemePref; label: string }[] = [
  { value: 'system', label: 'System' },
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
]

export function parseThemePref(stored: string | null): ThemePref {
  return stored === 'light' || stored === 'dark' ? stored : 'system'
}

export function resolveTheme(pref: ThemePref, systemDark: boolean): Theme {
  if (pref === 'system') return systemDark ? 'dark' : 'light'
  return pref
}

function readStored(): ThemePref {
  try {
    return parseThemePref(localStorage.getItem(THEME_KEY))
  } catch {
    return 'system' // Storage can be blocked (e.g. some private modes).
  }
}

const systemQuery = () => window.matchMedia('(prefers-color-scheme: dark)')

let pref: ThemePref = 'system'
const listeners = new Set<() => void>()

function apply() {
  document.documentElement.dataset.theme = resolveTheme(pref, systemQuery().matches)
}

/** Applies the stored theme and keeps it in step with the OS setting and other tabs. */
export function initTheme() {
  pref = readStored()
  apply()
  systemQuery().addEventListener('change', () => {
    if (pref === 'system') apply()
  })
  window.addEventListener('storage', (e) => {
    if (e.key !== THEME_KEY && e.key !== null) return
    pref = readStored()
    apply()
    for (const l of listeners) l()
  })
}

export function setThemePref(next: ThemePref) {
  pref = next
  try {
    if (next === 'system') localStorage.removeItem(THEME_KEY)
    else localStorage.setItem(THEME_KEY, next)
  } catch {
    // Not persisted; the choice still holds for this page.
  }
  apply()
  for (const l of listeners) l()
}

function subscribe(l: () => void) {
  listeners.add(l)
  return () => {
    listeners.delete(l)
  }
}

export function useThemePref(): ThemePref {
  return useSyncExternalStore(subscribe, () => pref)
}

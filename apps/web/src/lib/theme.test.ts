import { describe, expect, it } from 'vitest'
import { parseThemePref, resolveTheme } from './theme'

describe('parseThemePref', () => {
  it('keeps an explicit light or dark choice', () => {
    expect(parseThemePref('light')).toBe('light')
    expect(parseThemePref('dark')).toBe('dark')
  })

  it('falls back to system for anything else', () => {
    expect(parseThemePref(null)).toBe('system')
    expect(parseThemePref('system')).toBe('system')
    expect(parseThemePref('blue')).toBe('system')
  })
})

describe('resolveTheme', () => {
  it('follows the OS setting for system', () => {
    expect(resolveTheme('system', true)).toBe('dark')
    expect(resolveTheme('system', false)).toBe('light')
  })

  it('ignores the OS setting for an explicit choice', () => {
    expect(resolveTheme('light', true)).toBe('light')
    expect(resolveTheme('dark', false)).toBe('dark')
  })
})

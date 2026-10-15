import { createContext, useContext } from 'react'
import type { Preset, Range } from './range'

export interface RangeState {
  preset: Preset
  setPreset: (p: Preset) => void
  range: Range
  /** The app's minute clock (ms since epoch), so countdowns update without their own timers. */
  now: number
}

export const RangeContext = createContext<RangeState | null>(null)

export function useRange(): RangeState {
  const v = useContext(RangeContext)
  if (!v) throw new Error('useRange outside RangeContext')
  return v
}

/** The current minute, from the app clock. */
export function useNow(): number {
  return useRange().now
}

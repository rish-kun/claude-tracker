import { createContext, useContext } from 'react'
import type { Preset, Range } from './range'

export interface RangeState {
  preset: Preset
  setPreset: (p: Preset) => void
  range: Range
}

export const RangeContext = createContext<RangeState | null>(null)

export function useRange(): RangeState {
  const v = useContext(RangeContext)
  if (!v) throw new Error('useRange outside RangeContext')
  return v
}

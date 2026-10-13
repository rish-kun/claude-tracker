import { describe, expect, it } from 'vitest'
import type { PriceRates } from '../api/client'
import {
  addPriceError,
  baseRatesEqual,
  draftChanged,
  draftFromRates,
  EMPTY_DRAFT,
  modelError,
  parseDraft,
  parseRate,
  rowConflict,
  rowDirty,
  rowDraft,
  syncRowDraft,
} from './prices'

const rates: PriceRates = { input: 2, cache_write_5m: 2.5, cache_write_1h: 4, cache_read: 0.2, output: 10, fast_input: 4, fast_output: 20 }

describe('parseRate', () => {
  it('accepts plain non-negative decimals', () => {
    expect(parseRate('0')).toBe(0)
    expect(parseRate(' 2.5 ')).toBe(2.5)
    expect(parseRate('0.125')).toBe(0.125)
    expect(parseRate('.5')).toBe(0.5)
    expect(parseRate('10.')).toBe(10)
  })
  it('rejects empty, negative and non-numeric input instead of coercing to 0', () => {
    expect(parseRate('')).toBe('is required')
    expect(parseRate('   ')).toBe('is required')
    expect(parseRate('-1')).toBe('must not be negative')
    expect(parseRate('abc')).toBe('must be a number')
    expect(parseRate('1,5')).toBe('must be a number')
    expect(parseRate('0x10')).toBe('must be a number')
    expect(parseRate('1e3')).toBe('must be a number')
    expect(parseRate('Infinity')).toBe('must be a number')
  })
})

describe('parseDraft', () => {
  it('returns the five base rates as numbers', () => {
    expect(parseDraft(draftFromRates(rates))).toEqual({
      rates: { input: 2, cache_write_5m: 2.5, cache_write_1h: 4, cache_read: 0.2, output: 10 },
    })
  })
  it('names the first invalid field', () => {
    expect(parseDraft({ ...draftFromRates(rates), cache_read: '' })).toEqual({ error: 'Cache read is required.' })
    expect(parseDraft({ ...draftFromRates(rates), output: '-3' })).toEqual({ error: 'Output must not be negative.' })
    expect(parseDraft(EMPTY_DRAFT)).toEqual({ error: 'Input is required.' })
  })
})

describe('draftChanged', () => {
  it('compares numerically and treats invalid drafts as changed', () => {
    const d = draftFromRates(rates)
    expect(draftChanged(d, rates)).toBe(false)
    expect(draftChanged({ ...d, cache_read: '0.20' }, rates)).toBe(false)
    expect(draftChanged({ ...d, cache_read: '0.3' }, rates)).toBe(true)
    expect(draftChanged({ ...d, input: '' }, rates)).toBe(true)
  })
})

describe('modelError', () => {
  it('requires a non-empty id without spaces', () => {
    expect(modelError('claude-opus-6')).toBeUndefined()
    expect(modelError('  ')).toBe('Model id is required.')
    expect(modelError('claude opus')).toBe('Model id must not contain spaces.')
    expect(modelError('x'.repeat(201))).toBe('Model id must be at most 200 characters.')
  })
})

describe('price row drafts', () => {
  const newer: PriceRates = { ...rates, input: 3, output: 15 }

  it('compares only the five base rates', () => {
    expect(baseRatesEqual(rates, { ...rates, fast_input: 99 })).toBe(true)
    expect(baseRatesEqual(rates, newer)).toBe(false)
  })

  it('leaves a row alone while the server rates are unchanged', () => {
    const s = rowDraft(rates)
    expect(syncRowDraft(s, { ...rates })).toBe(s)
    const edited = { ...s, draft: { ...s.draft, input: '9' } }
    expect(syncRowDraft(edited, { ...rates })).toBe(edited)
    expect(rowConflict(edited, rates)).toBe(false)
  })

  it('resets a pristine draft to new server rates', () => {
    const s = syncRowDraft(rowDraft(rates), newer)
    expect(s.base).toBe(newer)
    expect(s.draft).toEqual(draftFromRates(newer))
    expect(rowDirty(s)).toBe(false)
    expect(rowConflict(s, newer)).toBe(false)
  })

  it('treats a draft typed back to the old values (e.g. "2.0") as pristine', () => {
    const s = { ...rowDraft(rates), draft: { ...draftFromRates(rates), input: '2.0' } }
    expect(rowDirty(s)).toBe(false)
    expect(syncRowDraft(s, newer).draft).toEqual(draftFromRates(newer))
  })

  it('keeps a dirty draft and flags the conflict instead of overwriting the newer value', () => {
    const edited = { ...rowDraft(rates), draft: { ...draftFromRates(rates), cache_read: '0.3' } }
    const s = syncRowDraft(edited, newer)
    expect(s).toBe(edited)
    expect(s.draft.cache_read).toBe('0.3')
    expect(rowConflict(s, newer)).toBe(true)
    // "Reload row" replaces it with the server's rates.
    const reloaded = rowDraft(newer)
    expect(rowConflict(reloaded, newer)).toBe(false)
    expect(rowDirty(reloaded)).toBe(false)
  })

  it('adopts new server rates that a dirty draft already matches, without a conflict', () => {
    const edited = { ...rowDraft(rates), draft: draftFromRates(newer) }
    const s = syncRowDraft(edited, newer)
    expect(s.base).toBe(newer)
    expect(rowConflict(s, newer)).toBe(false)
  })

  it('syncs to the saved response so the follow-up refetch is a no-op', () => {
    const saved: PriceRates = { ...rates, cache_read: 0.3 }
    const s = rowDraft(saved)
    expect(syncRowDraft(s, { ...saved })).toBe(s)
    expect(rowDirty(s)).toBe(false)
  })
})

describe('addPriceError', () => {
  it('explains a 409 with the normalised model and points to its row', () => {
    expect(addPriceError({ status: 409, message: 'model already priced: claude-opus-6' })).toBe(
      'claude-opus-6 is already priced — edit its row instead.',
    )
    expect(addPriceError({ status: 409, message: 'Conflict' })).toBe('already priced — edit its row instead.')
  })
  it('passes other errors through', () => {
    expect(addPriceError({ status: 400, message: 'bad rates' })).toBe('bad rates')
    expect(addPriceError(new Error('network down'))).toBe('network down')
  })
})

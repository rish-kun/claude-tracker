import { describe, expect, it } from 'vitest'
import type { PriceRates } from '../api/client'
import { draftChanged, draftFromRates, EMPTY_DRAFT, modelError, parseDraft, parseRate } from './prices'

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

import { describe, expect, it } from 'vitest'
import { putPriceParams } from './hooks'

describe('putPriceParams', () => {
  it('sends ?create=true only for the Add row', () => {
    expect(putPriceParams({ model: 'claude-opus-6', create: true })).toEqual({ path: { model: 'claude-opus-6' }, query: { create: true } })
    expect(putPriceParams({ model: 'claude-opus-6' })).toEqual({ path: { model: 'claude-opus-6' }, query: undefined })
    expect(putPriceParams({ model: 'claude-opus-6', create: false }).query).toBeUndefined()
  })
})

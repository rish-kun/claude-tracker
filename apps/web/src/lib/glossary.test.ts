import { describe, expect, it } from 'vitest'
import { allFlatRate, explainApiEquivalent, GLOSSARY } from './glossary'

describe('allFlatRate / explainApiEquivalent', () => {
  it('is flat-rate only when every known plan is not billed per token', () => {
    expect(allFlatRate(['max_20x', 'pro'])).toBe(true)
    expect(allFlatRate(['max_20x', 'api_key'])).toBe(false)
    expect(allFlatRate([])).toBe(false)
    expect(allFlatRate(undefined)).toBe(false)
  })
  it('claims nobody pays only for flat-rate plans', () => {
    expect(explainApiEquivalent(['max_5x'])).toMatch(/nobody pays this/)
    expect(explainApiEquivalent(['api_key'])).toBe(GLOSSARY.apiEquivalent.text)
    expect(explainApiEquivalent()).toBe(GLOSSARY.apiEquivalent.text)
    expect(GLOSSARY.apiEquivalent.text).not.toMatch(/nobody pays/)
  })
})

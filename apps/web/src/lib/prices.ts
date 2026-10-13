import type { PriceRates } from '../api/client'
import { type Draft, draftConflict, draftDirty, type DraftOps, startDraft, syncDraft } from './drafts'

export type BaseRateKey = 'input' | 'cache_write_5m' | 'cache_write_1h' | 'cache_read' | 'output'

export const BASE_RATE_FIELDS: { key: BaseRateKey; label: string }[] = [
  { key: 'input', label: 'Input' },
  { key: 'cache_write_5m', label: 'Cache write 5m' },
  { key: 'cache_write_1h', label: 'Cache write 1h' },
  { key: 'cache_read', label: 'Cache read' },
  { key: 'output', label: 'Output' },
]

/** Form state for the five base rates, kept as typed so nothing is silently coerced. */
export type RateDraft = Record<BaseRateKey, string>

export const EMPTY_DRAFT: RateDraft = { input: '', cache_write_5m: '', cache_write_1h: '', cache_read: '', output: '' }

export function draftFromRates(r: PriceRates): RateDraft {
  const d = { ...EMPTY_DRAFT }
  for (const f of BASE_RATE_FIELDS) d[f.key] = String(r[f.key])
  return d
}

/** Whether the draft differs from the saved rates (compared as numbers when both parse). */
export function draftChanged(d: RateDraft, r: PriceRates): boolean {
  return BASE_RATE_FIELDS.some((f) => {
    const v = parseRate(d[f.key])
    return typeof v !== 'number' || v !== r[f.key]
  })
}

/** A plain non-negative decimal (USD per MTok), or an error message. */
export function parseRate(s: string): number | string {
  const t = s.trim()
  if (t === '') return 'is required'
  if (t.startsWith('-')) return 'must not be negative'
  if (!/^(\d+\.?\d*|\.\d+)$/.test(t)) return 'must be a number'
  const v = Number(t)
  return Number.isFinite(v) ? v : 'must be a number'
}

export type ParsedRates = Pick<PriceRates, BaseRateKey>

/** All five base rates as numbers, or the first problem as a sentence. */
export function parseDraft(d: RateDraft): { rates: ParsedRates } | { error: string } {
  const rates = {} as ParsedRates
  for (const f of BASE_RATE_FIELDS) {
    const v = parseRate(d[f.key])
    if (typeof v === 'string') return { error: `${f.label} ${v}.` }
    rates[f.key] = v
  }
  return { rates }
}

/** Validates a new model id; the server normalises it (e.g. strips dates and provider prefixes). */
export function modelError(model: string): string | undefined {
  const t = model.trim()
  if (t === '') return 'Model id is required.'
  if (t.length > 200) return 'Model id must be at most 200 characters.'
  if (/\s/.test(t)) return 'Model id must not contain spaces.'
  return undefined
}

/** Whether two rate sets agree on the five base rates (the ones a row edits). */
export function baseRatesEqual(a: PriceRates, b: PriceRates): boolean {
  return BASE_RATE_FIELDS.every((f) => a[f.key] === b[f.key])
}

/** An editable price row: the typed draft and the server rates it was started from. */
export type RowDraft = Draft<RateDraft, PriceRates>

const priceOps: DraftOps<RateDraft, PriceRates> = {
  fromServer: draftFromRates,
  sameServer: baseRatesEqual,
  matches: (d, r) => !draftChanged(d, r),
}

export function rowDraft(r: PriceRates): RowDraft {
  return startDraft(priceOps, r)
}

/** Whether the user has edited the draft away from the rates it was started from. */
export function rowDirty(s: RowDraft): boolean {
  return draftDirty(priceOps, s)
}

/** Reconciles a row with the latest server rates; see `syncDraft`. */
export function syncRowDraft(s: RowDraft, server: PriceRates): RowDraft {
  return syncDraft(priceOps, s, server)
}

/** The server's base rates changed underneath an edited draft. */
export function rowConflict(s: RowDraft, server: PriceRates): boolean {
  return draftConflict(priceOps, s, server)
}

/**
 * Error text for the Add-price row. A 409 means the normalised model is already priced
 * (server message "model already priced: <normalised>"); nothing was written.
 */
export function addPriceError(err: { status?: number; message: string }): string {
  if (err.status !== 409) return err.message
  const model = /already priced:\s*(.+)$/.exec(err.message)?.[1]?.trim()
  return `${model ? `${model} is ` : ''}already priced — edit its row instead.`
}

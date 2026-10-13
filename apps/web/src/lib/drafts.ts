/**
 * Editable-row drafts that notice when the server value changes underneath them.
 * `D` is the form state, `S` the server value the draft was started from.
 */
export interface Draft<D, S> {
  draft: D
  base: S
}

export interface DraftOps<D, S> {
  /** Form state for a server value. */
  fromServer(s: S): D
  /** Whether two server values agree on the fields the row edits. */
  sameServer(a: S, b: S): boolean
  /** Whether the form state already equals a server value. */
  matches(d: D, s: S): boolean
}

export function startDraft<D, S>(ops: DraftOps<D, S>, s: S): Draft<D, S> {
  return { draft: ops.fromServer(s), base: s }
}

/** Whether the user has edited the draft away from the value it was started from. */
export function draftDirty<D, S>(ops: DraftOps<D, S>, d: Draft<D, S>): boolean {
  return !ops.matches(d.draft, d.base)
}

/**
 * Reconciles a draft with the latest server value. A pristine draft (or one already typed
 * to exactly the new value) follows the server; any other dirty draft is kept as typed
 * (returned unchanged) so `draftConflict` can flag it instead of a save silently
 * overwriting the newer value. Returns `d` itself when nothing changes.
 */
export function syncDraft<D, S>(ops: DraftOps<D, S>, d: Draft<D, S>, server: S): Draft<D, S> {
  if (ops.sameServer(d.base, server)) return d
  return draftDirty(ops, d) && !ops.matches(d.draft, server) ? d : startDraft(ops, server)
}

/** The server value changed underneath an edited draft (after `syncDraft`). */
export function draftConflict<D, S>(ops: DraftOps<D, S>, d: Draft<D, S>, server: S): boolean {
  return !ops.sameServer(d.base, server)
}

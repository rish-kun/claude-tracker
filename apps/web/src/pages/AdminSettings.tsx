import { Link } from '@tanstack/react-router'
import { useEffect, useId, useState } from 'react'
import type { Account, ApiToken, ModelPrice, Plan } from '../api/client'
import { useAccounts, usePrices, usePutPrice, useRevokeToken, useTokens, useUpdateAccount, useUsers } from '../api/hooks'
import { AdminOnly } from '../components/AdminOnly'
import { PageHeader } from '../components/Layout'
import { Button, ConfirmButton, ErrorNotice, Glyph, Notice, Section, SkeletonRows, useDisclosure } from '../components/ui'
import { accountConflict, accountDirty, accountDraft, type AccountDraft, type AccountFields, suggestPlan, syncAccountDraft } from '../lib/accounts'
import { formatDayLong, formatRelative, PLAN_LABELS, PLANS } from '../lib/format'
import {
  addPriceError,
  BASE_RATE_FIELDS,
  EMPTY_DRAFT,
  modelError,
  parseDraft,
  parseRate,
  type RateDraft,
  rowConflict,
  rowDirty,
  rowDraft,
  type RowDraft,
  syncRowDraft,
} from '../lib/prices'
import { useNow } from '../lib/rangeContext'

const SAVED_MS = 2000
const WEEK_MS = 7 * 86_400_000

/** Shows "Saved" for two seconds after `mark()`. */
function useSavedFlash(): [boolean, () => void] {
  const [at, setAt] = useState(0)
  useEffect(() => {
    if (at === 0) return
    const t = setTimeout(() => setAt(0), SAVED_MS)
    return () => clearTimeout(t)
  }, [at])
  return [at !== 0, () => setAt(Date.now())]
}

function Saved() {
  return (
    <span className="saved" role="status">
      <Glyph name="check" />
      Saved
    </span>
  )
}

/** Relative within a week ("3 h ago"), otherwise the day. */
function seen(iso: string | undefined, now: number): string {
  if (!iso) return '–'
  return now - Date.parse(iso) < WEEK_MS ? formatRelative(iso, now) : formatDayLong(iso, now)
}

const planOptionLabel = (p: Plan) => (p === 'unknown' ? 'Not set' : PLAN_LABELS[p])

/* ───────── Claude accounts ───────── */

function AccountForm({ a }: { a: Account }) {
  const update = useUpdateAccount()
  const [saved, flashSaved] = useSavedFlash()
  const [row, setRow] = useState<AccountDraft>(() => accountDraft(a))
  // Follow refetched server label/plan while the draft is untouched (adjusting state during render).
  const synced = syncAccountDraft(row, a)
  if (synced !== row) setRow(synced)
  const dirty = accountDirty(synced)
  const conflict = accountConflict(synced, a)
  const edit = (d: Partial<AccountFields>) => setRow({ ...synced, draft: { ...synced.draft, ...d } })
  const save = () =>
    update.mutate(
      { id: a.id, ...synced.draft },
      {
        onSuccess: (s) => {
          setRow(accountDraft(s))
          flashSaved()
        },
      },
    )
  const identity = a.email ?? a.account_uuid
  const suggested = synced.draft.plan === 'unknown' ? suggestPlan(a.rate_limit_tier) : undefined
  const labelId = `label-${a.id}`
  const planId = `plan-${a.id}`
  return (
    <div className="account-form" id={`account-${a.id}`} data-unknown={a.plan === 'unknown' ? '' : undefined}>
      <div>
        <label htmlFor={labelId}>Label</label>
        <input id={labelId} value={synced.draft.label} onChange={(e) => edit({ label: e.target.value })} maxLength={100} placeholder={identity} />
        <div className="identity">{identity}</div>
      </div>
      <div>
        <label htmlFor={planId}>Plan</label>
        <div className="plan-line">
          <select id={planId} value={synced.draft.plan} onChange={(e) => edit({ plan: e.target.value as Plan })}>
            {PLANS.map((p) => (
              <option key={p} value={p}>
                {planOptionLabel(p)}
              </option>
            ))}
          </select>
          {suggested && (
            <span className="suggest">
              Suggested: {PLAN_LABELS[suggested]}
              <Button size="sm" variant="secondary" onClick={() => edit({ plan: suggested })}>
                Use it
              </Button>
            </span>
          )}
        </div>
        {a.rate_limit_tier && (
          <p className="plan-meta">
            Claude reports tier: <code>{a.rate_limit_tier}</code>
            {a.org_type && `, ${a.org_type} org`}
          </p>
        )}
      </div>
      <div className="form-actions">
        <Button variant="secondary" disabled={!dirty || conflict || update.isPending} onClick={save}>
          {update.isPending ? 'Saving…' : 'Save'}
        </Button>
        {saved && !dirty && <Saved />}
        {update.error && <p className="field-error">Couldn't save: {update.error.message}</p>}
      </div>
      {conflict && (
        <div style={{ gridColumn: '1 / -1' }}>
          <Notice
            tone="warning"
            action={
              <Button size="sm" variant="secondary" onClick={() => setRow(accountDraft(a))}>
                Load their version
              </Button>
            }
          >
            Someone else changed this account.
          </Notice>
        </div>
      )}
    </div>
  )
}

function Accounts() {
  const q = useAccounts()
  // Arriving from "Set plan" (By account): bring that row into view once it has loaded.
  const loaded = q.data !== undefined
  useEffect(() => {
    if (!loaded) return
    const hash = window.location.hash.slice(1)
    if (!hash.startsWith('account-')) return
    const el = document.getElementById(hash)
    el?.scrollIntoView({ block: 'center' })
    el?.querySelector<HTMLSelectElement>('select')?.focus({ preventScroll: true })
  }, [loaded])
  return (
    <Section id="accounts" title="Claude accounts" description="Accounts appear automatically when someone's Claude Code reports usage.">
      {q.error && !q.data && <ErrorNotice what="load accounts" error={q.error} onRetry={() => void q.refetch()} />}
      {!q.data && !q.error && <SkeletonRows />}
      {q.data && q.data.length === 0 && <p className="meta">No accounts yet.</p>}
      {q.data && q.data.length > 0 && (
        <div className="form-list">
          {q.data.map((a) => (
            <AccountForm key={a.id} a={a} />
          ))}
        </div>
      )}
    </Section>
  )
}

/* ───────── CLI tokens ───────── */

function TokenRows({ tokens, now }: { tokens: ApiToken[]; now: number }) {
  const revoke = useRevokeToken()
  return (
    <>
      {tokens.map((t) => (
        <tr key={t.id}>
          <td>
            {t.user.name || t.user.email}
            {t.user.name && <div className="sub">{t.user.email}</div>}
          </td>
          <td>{t.device_name}</td>
          <td>{formatDayLong(t.created_at, now)}</td>
          <td>{t.last_used_at ? seen(t.last_used_at, now) : <span className="dash">Never</span>}</td>
          <td className="num">
            {t.revoked_at ? (
              <span className="sub">Revoked {formatDayLong(t.revoked_at, now)}</span>
            ) : (
              <ConfirmButton
                label="Revoke"
                confirmLabel="Revoke token"
                prompt={`Revoke ${t.device_name}? Their Claude Code stops working until they log in again.`}
                pending={revoke.isPending && revoke.variables === t.id}
                onConfirm={() => revoke.mutate(t.id)}
              />
            )}
            {revoke.error && revoke.variables === t.id && <p className="field-error">Couldn't revoke: {revoke.error.message}</p>}
          </td>
        </tr>
      ))}
    </>
  )
}

function Tokens() {
  const q = useTokens()
  const now = useNow()
  const revoked = useDisclosure()
  const used = (t: ApiToken) => Date.parse(t.last_used_at ?? t.created_at)
  const active = (q.data ?? []).filter((t) => !t.revoked_at).sort((a, b) => used(b) - used(a))
  const gone = (q.data ?? []).filter((t) => t.revoked_at).sort((a, b) => Date.parse(b.revoked_at!) - Date.parse(a.revoked_at!))
  const head = (
    <thead>
      <tr>
        <th scope="col">User</th>
        <th scope="col">Device</th>
        <th scope="col">Created</th>
        <th scope="col">Last used</th>
        <th scope="col">
          <span className="sr-only">Action</span>
        </th>
      </tr>
    </thead>
  )
  return (
    <Section id="tokens" title="CLI tokens" description="One per device someone has logged in from with cctrack login.">
      {q.error && !q.data && <ErrorNotice what="load CLI tokens" error={q.error} onRetry={() => void q.refetch()} />}
      {!q.data && !q.error && <SkeletonRows />}
      {q.data && (
        <>
          {active.length === 0 ? (
            <p className="meta">No active tokens.</p>
          ) : (
            <div className="table-scroll">
              <table className="table" data-hover="">
                {head}
                <tbody>
                  <TokenRows tokens={active} now={now} />
                </tbody>
              </table>
            </div>
          )}
          {gone.length > 0 && (
            <>
              <Button variant="ghost" className="disclosure" {...revoked.buttonProps}>
                <Glyph name="chevron-right" />
                {revoked.open ? 'Hide' : 'Show'} {gone.length} revoked {gone.length === 1 ? 'token' : 'tokens'}
              </Button>
              <div className="table-scroll mt-3" {...revoked.regionProps}>
                <table className="table">
                  {head}
                  <tbody>
                    <TokenRows tokens={gone} now={now} />
                  </tbody>
                </table>
              </div>
            </>
          )}
        </>
      )}
    </Section>
  )
}

/* ───────── People ───────── */

function People() {
  const q = useUsers()
  const now = useNow()
  return (
    <Section id="people" title="People" description="Everyone who has signed in. Select a name to see their usage and sessions.">
      {q.error && !q.data && <ErrorNotice what="load people" error={q.error} onRetry={() => void q.refetch()} />}
      {!q.data && !q.error && <SkeletonRows />}
      {q.data && (
        <div className="table-scroll">
          <table className="table" data-hover="">
            <thead>
              <tr>
                <th scope="col">Person</th>
                <th scope="col">Joined</th>
                <th scope="col">Last seen</th>
              </tr>
            </thead>
            <tbody>
              {q.data.map((u) => (
                <tr key={u.id}>
                  <td>
                    <span className="chip-wrap">
                      <Link to="/admin/logins" search={{ open: u.id }}>
                        {u.name || u.email}
                      </Link>
                      {u.is_admin && <span className="chip">Admin</span>}
                    </span>
                    {u.name && <div className="sub">{u.email}</div>}
                  </td>
                  <td>{formatDayLong(u.created_at, now)}</td>
                  <td>{seen(u.last_seen_at, now)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Section>
  )
}

/* ───────── Model prices ───────── */

function RateInputs({
  draft,
  onChange,
  disabled = false,
  showInvalid,
  errorId,
  idPrefix,
}: {
  draft: RateDraft
  onChange: (d: RateDraft) => void
  disabled?: boolean
  showInvalid: boolean
  errorId: string
  idPrefix: string
}) {
  return (
    <>
      {BASE_RATE_FIELDS.map((f) => {
        const bad = showInvalid && typeof parseRate(draft[f.key]) === 'string'
        return (
          <td key={f.key} className="num">
            <input
              id={`${idPrefix}-${f.key}`}
              className="rate"
              type="text"
              inputMode="decimal"
              aria-label={f.label}
              aria-invalid={bad || undefined}
              aria-describedby={bad ? errorId : undefined}
              value={draft[f.key]}
              disabled={disabled}
              onChange={(e) => onChange({ ...draft, [f.key]: e.target.value })}
            />
          </td>
        )
      })}
    </>
  )
}

function tierNote(p: ModelPrice): string | undefined {
  const fast = p.rates.fast_input !== undefined
  const long = p.rates.long_input !== undefined
  if (fast && long) return 'Also has fast-mode and long-context rates (kept as is).'
  if (fast) return 'Also has fast-mode rates (kept as is).'
  if (long) return 'Also has long-context rates (kept as is).'
  return undefined
}

function PriceRow({ p }: { p: ModelPrice }) {
  const put = usePutPrice()
  const errorId = useId()
  const [saved, flashSaved] = useSavedFlash()
  const [row, setRow] = useState<RowDraft>(() => rowDraft(p.rates))
  // Follow refetched server rates while the draft is untouched (adjusting state during render).
  const synced = syncRowDraft(row, p.rates)
  if (synced !== row) setRow(synced)
  const dirty = rowDirty(synced)
  const conflict = rowConflict(synced, p.rates)
  const parsed = parseDraft(synced.draft)
  const invalid = 'error' in parsed ? parsed.error : undefined
  const note = tierNote(p)
  const save = () => {
    if (!('rates' in parsed)) return
    put.mutate(
      // Spread the saved rates first so fast_* / long_* groups are sent back unchanged.
      { model: p.model, rates: { ...p.rates, ...parsed.rates } },
      {
        onSuccess: (s) => {
          setRow(rowDraft(s.rates))
          flashSaved()
        },
      },
    )
  }
  return (
    <tr>
      <td>
        <span className="model-id">{p.model}</span>
        {note && <div className="tier-note">{note}</div>}
      </td>
      <RateInputs
        draft={synced.draft}
        onChange={(draft) => setRow({ ...synced, draft })}
        showInvalid={dirty}
        errorId={errorId}
        idPrefix={`price-${p.model}`}
      />
      <td className="num">
        <div className="form-actions" style={{ paddingTop: 0 }}>
          <Button variant="secondary" disabled={!dirty || conflict || invalid !== undefined || put.isPending} onClick={save}>
            {put.isPending ? 'Saving…' : 'Save'}
          </Button>
          {saved && !dirty && <Saved />}
          {conflict && (
            <p className="field-error" style={{ color: 'var(--warn-text)' }}>
              Someone else changed this price.{' '}
              <button type="button" className="btn-text" onClick={() => setRow(rowDraft(p.rates))}>
                Load their version
              </button>
            </p>
          )}
          {dirty && invalid && (
            <p className="field-error" id={errorId}>
              {invalid}
            </p>
          )}
          {put.error && <p className="field-error">Couldn't save: {put.error.message}</p>}
        </div>
      </td>
    </tr>
  )
}

function AddPriceRow() {
  const put = usePutPrice()
  const errorId = useId()
  const [model, setModel] = useState('')
  const [draft, setDraft] = useState<RateDraft>(EMPTY_DRAFT)
  const [saved, setSaved] = useState<string>()
  const touched = model !== '' || Object.values(draft).some((v) => v !== '')
  const parsed = parseDraft(draft)
  const mErr = modelError(model)
  const invalid = mErr ?? ('error' in parsed ? parsed.error : undefined)
  const save = () => {
    if (invalid !== undefined || !('rates' in parsed)) return
    setSaved(undefined)
    put.mutate(
      // create=true: never replace an existing model's rates (incl. fast/long) from here.
      { model: model.trim(), rates: parsed.rates, create: true },
      {
        onSuccess: (p) => {
          setSaved(p.model)
          setModel('')
          setDraft(EMPTY_DRAFT)
        },
      },
    )
  }
  return (
    <tr>
      <td>
        <input
          value={model}
          onChange={(e) => {
            setModel(e.target.value)
            setSaved(undefined)
            // A 409 for the previous id no longer applies. Never reset an
            // in-flight Add: its "Saved"/409 feedback would be detached.
            if (!put.isPending) put.reset()
          }}
          // Locked while an Add is in flight so its result matches what is shown.
          disabled={put.isPending}
          placeholder="New model id, e.g. claude-opus-6"
          aria-label="New model id"
          aria-invalid={(touched && mErr !== undefined) || undefined}
          aria-describedby={touched && invalid ? errorId : undefined}
          maxLength={200}
          className="mono"
          style={{ width: '16rem', maxWidth: '100%' }}
        />
      </td>
      <RateInputs
        draft={draft}
        disabled={put.isPending}
        onChange={(d) => {
          setDraft(d)
          setSaved(undefined)
        }}
        showInvalid={touched}
        errorId={errorId}
        idPrefix="price-new"
      />
      <td className="num">
        <div className="form-actions" style={{ paddingTop: 0 }}>
          <Button variant="secondary" disabled={!touched || invalid !== undefined || put.isPending} onClick={save}>
            {put.isPending ? 'Saving…' : 'Add model'}
          </Button>
          {touched && invalid && (
            <p className="field-error" id={errorId}>
              {invalid}
            </p>
          )}
          {put.error && <p className="field-error">Couldn't add model: {addPriceError(put.error)}</p>}
          {saved && <span className="saved">Saved as {saved}</span>}
        </div>
      </td>
    </tr>
  )
}

function Prices() {
  const q = usePrices()
  return (
    <Section
      id="prices"
      title="Model prices"
      description="USD per million tokens. Used only for the API-equivalent figures, and changes apply to all past usage. Add a model that shows as unpriced."
    >
      {q.error && !q.data && <ErrorNotice what="load model prices" error={q.error} onRetry={() => void q.refetch()} />}
      {!q.data && !q.error && <SkeletonRows />}
      {q.data && (
        <div className="table-scroll">
          <table className="table">
            <thead>
              <tr>
                <th scope="col">Model</th>
                {BASE_RATE_FIELDS.map((f) => (
                  <th key={f.key} scope="col" className="num">
                    {f.label}
                  </th>
                ))}
                <th scope="col">
                  <span className="sr-only">Save</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {q.data.map((p) => (
                <PriceRow key={p.model} p={p} />
              ))}
            </tbody>
            <tfoot>
              <AddPriceRow />
            </tfoot>
          </table>
        </div>
      )}
    </Section>
  )
}

/* ───────── Page ───────── */

function UnknownPlanNotice() {
  const q = useAccounts()
  const unknown = q.data?.filter((a) => a.plan === 'unknown') ?? []
  if (unknown.length === 0) return null
  const n = unknown.length
  const show = () => {
    const row = document.getElementById(`account-${unknown[0].id}`)
    row?.scrollIntoView({ block: 'center' })
    document.getElementById(`plan-${unknown[0].id}`)?.focus({ preventScroll: true })
  }
  return (
    <div className="settings-notice">
      <Notice
        tone="warning"
        action={
          <Button size="sm" variant="secondary" onClick={show}>
            Show accounts
          </Button>
        }
      >
        {n} {n === 1 ? 'account has' : 'accounts have'} no plan set. Set the plan so everyone can tell which pool account is which tier.
      </Notice>
    </div>
  )
}

export function AdminSettingsPage() {
  return (
    <>
      <PageHeader title="Settings" />
      <AdminOnly>
        <UnknownPlanNotice />
        <div className="settings-layout">
          <nav className="settings-nav" aria-label="Settings sections">
            <a href="#accounts">Accounts</a>
            <a href="#tokens">CLI tokens</a>
            <a href="#people">People</a>
            <a href="#prices">Model prices</a>
          </nav>
          <div className="settings-main">
            <Accounts />
            <Tokens />
            <People />
            <Prices />
          </div>
        </div>
      </AdminOnly>
    </>
  )
}

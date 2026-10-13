import { useState } from 'react'
import type { Account, ModelPrice, Plan } from '../api/client'
import { useAccounts, usePrices, usePutPrice, useRevokeToken, useTokens, useUpdateAccount, useUsers } from '../api/hooks'
import { AdminOnly } from '../components/AdminOnly'
import { formatDateTime, PLAN_LABELS, PLANS } from '../lib/format'
import {
  BASE_RATE_FIELDS,
  EMPTY_DRAFT,
  modelError,
  parseDraft,
  type RateDraft,
  rowConflict,
  rowDirty,
  rowDraft,
  type RowDraft,
  syncRowDraft,
} from '../lib/prices'

function AccountRow({ a }: { a: Account }) {
  const update = useUpdateAccount()
  const [label, setLabel] = useState(a.label)
  const [plan, setPlan] = useState<Plan>(a.plan)
  const dirty = label !== a.label || plan !== a.plan
  return (
    <tr>
      <td>
        <input
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          maxLength={100}
          placeholder={a.email ?? a.account_uuid}
          aria-label="Label"
        />
        <div className="muted">{a.email ?? a.account_uuid}</div>
      </td>
      <td>
        <select value={plan} onChange={(e) => setPlan(e.target.value as Plan)}>
          {PLANS.map((p) => (
            <option key={p} value={p}>{PLAN_LABELS[p]}</option>
          ))}
        </select>
        <div className="muted">{[a.org_type, a.rate_limit_tier].filter(Boolean).join(' / ')}</div>
      </td>
      <td>
        <button disabled={!dirty || update.isPending} onClick={() => update.mutate({ id: a.id, label, plan })}>
          {update.isPending ? 'Saving…' : 'Save'}
        </button>
        {update.error && <div className="error">{update.error.message}</div>}
      </td>
    </tr>
  )
}

function Accounts() {
  const q = useAccounts()
  return (
    <section className="card">
      <h2>Claude accounts</h2>
      <p className="muted">Accounts appear automatically when a user's Claude Code reports usage. Set a recognisable label and the plan.</p>
      {q.error && <div className="error">{q.error.message}</div>}
      <table>
        <thead>
          <tr><th>Label</th><th>Plan</th><th /></tr>
        </thead>
        <tbody>{q.data?.map((a) => <AccountRow key={a.id} a={a} />)}</tbody>
      </table>
    </section>
  )
}

function Users() {
  const q = useUsers()
  return (
    <section className="card">
      <h2>Users</h2>
      {q.error && <div className="error">{q.error.message}</div>}
      <table>
        <thead>
          <tr><th>User</th><th>Joined</th><th>Last seen</th></tr>
        </thead>
        <tbody>
          {q.data?.map((u) => (
            <tr key={u.id}>
              <td>
                {u.name || u.email} {u.is_admin && <span className="badge">admin</span>}
                <div className="muted">{u.email}</div>
              </td>
              <td>{formatDateTime(u.created_at)}</td>
              <td>{u.last_seen_at ? formatDateTime(u.last_seen_at) : '—'}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  )
}

function Tokens() {
  const q = useTokens()
  const revoke = useRevokeToken()
  return (
    <section className="card">
      <h2>CLI tokens</h2>
      {q.error && <div className="error">{q.error.message}</div>}
      {revoke.error && <div className="error">Revoke failed: {revoke.error.message}</div>}
      <table>
        <thead>
          <tr><th>User</th><th>Device</th><th>Created</th><th>Last used</th><th /></tr>
        </thead>
        <tbody>
          {q.data?.map((t) => (
            <tr key={t.id}>
              <td>{t.user.email}</td>
              <td>{t.device_name}</td>
              <td>{formatDateTime(t.created_at)}</td>
              <td>{t.last_used_at ? formatDateTime(t.last_used_at) : '—'}</td>
              <td>
                {t.revoked_at ? (
                  <span className="muted">Revoked {formatDateTime(t.revoked_at)}</span>
                ) : (
                  <button
                    disabled={revoke.isPending}
                    onClick={() => {
                      if (confirm(`Revoke ${t.user.email}'s token on ${t.device_name}? Their Claude Code will stop working until they log in again.`)) revoke.mutate(t.id)
                    }}
                  >
                    Revoke
                  </button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  )
}

function RateInputs({ draft, onChange }: { draft: RateDraft; onChange: (d: RateDraft) => void }) {
  return (
    <>
      {BASE_RATE_FIELDS.map((f) => (
        <td key={f.key} className="num">
          <input
            className="rate"
            type="text"
            inputMode="decimal"
            aria-label={f.label}
            value={draft[f.key]}
            onChange={(e) => onChange({ ...draft, [f.key]: e.target.value })}
          />
        </td>
      ))}
    </>
  )
}

function PriceRow({ p }: { p: ModelPrice }) {
  const put = usePutPrice()
  const [row, setRow] = useState<RowDraft>(() => rowDraft(p.rates))
  // Follow refetched server rates while the draft is untouched (adjusting state during render).
  const synced = syncRowDraft(row, p.rates)
  if (synced !== row) setRow(synced)
  const dirty = rowDirty(synced)
  const conflict = rowConflict(synced, p.rates)
  const parsed = parseDraft(synced.draft)
  const invalid = 'error' in parsed ? parsed.error : undefined
  const save = () => {
    if (!('rates' in parsed)) return
    put.mutate(
      // Spread the saved rates first so fast_* / long_* groups are sent back unchanged.
      { model: p.model, rates: { ...p.rates, ...parsed.rates } },
      { onSuccess: (saved) => setRow(rowDraft(saved.rates)) },
    )
  }
  return (
    <tr>
      <td>
        {p.model}
        {(p.rates.fast_input !== undefined || p.rates.long_input !== undefined) && (
          <div className="muted">{[p.rates.fast_input !== undefined && 'fast mode', p.rates.long_input !== undefined && '>100k tier'].filter(Boolean).join(', ')}</div>
        )}
      </td>
      <RateInputs draft={synced.draft} onChange={(draft) => setRow({ ...synced, draft })} />
      <td>
        <button disabled={!dirty || conflict || invalid !== undefined || put.isPending} onClick={save}>
          {put.isPending ? 'Saving…' : 'Save'}
        </button>
        {conflict && (
          <div className="error">
            Server value changed —{' '}
            <button className="link" onClick={() => setRow(rowDraft(p.rates))}>
              reload row
            </button>
          </div>
        )}
        {dirty && invalid && <div className="error">{invalid}</div>}
        {put.error && <div className="error">{put.error.message}</div>}
      </td>
    </tr>
  )
}

function AddPriceRow() {
  const put = usePutPrice()
  const [model, setModel] = useState('')
  const [draft, setDraft] = useState<RateDraft>(EMPTY_DRAFT)
  const [saved, setSaved] = useState<string>()
  const touched = model !== '' || Object.values(draft).some((v) => v !== '')
  const parsed = parseDraft(draft)
  const invalid = modelError(model) ?? ('error' in parsed ? parsed.error : undefined)
  const save = () => {
    if (invalid !== undefined || !('rates' in parsed)) return
    setSaved(undefined)
    put.mutate(
      { model: model.trim(), rates: parsed.rates },
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
          }}
          placeholder="New model id, e.g. claude-opus-6"
          aria-label="New model id"
          maxLength={200}
        />
      </td>
      <RateInputs
        draft={draft}
        onChange={(d) => {
          setDraft(d)
          setSaved(undefined)
        }}
      />
      <td>
        <button disabled={!touched || invalid !== undefined || put.isPending} onClick={save}>
          {put.isPending ? 'Saving…' : 'Add'}
        </button>
        {touched && invalid && <div className="error">{invalid}</div>}
        {put.error && <div className="error">{put.error.message}</div>}
        {saved && <div className="muted">Saved as {saved}</div>}
      </td>
    </tr>
  )
}

function Prices() {
  const q = usePrices()
  return (
    <section className="card">
      <h2>Model prices (USD per million tokens)</h2>
      <p className="muted">
        Used to estimate API-equivalent cost. Changes apply to all past usage. Add a row for a model that shows as
        unpriced; the id is normalised (dates and provider prefixes removed), and an existing model's price is replaced.
      </p>
      {q.error && <div className="error">{q.error.message}</div>}
      <table>
        <thead>
          <tr>
            <th>Model</th>
            {BASE_RATE_FIELDS.map((f) => (
              <th key={f.key} className="num">{f.label}</th>
            ))}
            <th />
          </tr>
        </thead>
        <tbody>{q.data?.map((p) => <PriceRow key={p.model} p={p} />)}</tbody>
        <tfoot>
          <AddPriceRow />
        </tfoot>
      </table>
    </section>
  )
}

export function AdminSettingsPage() {
  return (
    <>
      <h1>Settings</h1>
      <AdminOnly>
        <Accounts />
        <Tokens />
        <Users />
        <Prices />
      </AdminOnly>
    </>
  )
}

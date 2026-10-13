import { useState } from 'react'
import type { Account, ModelPrice, Plan, PriceRates } from '../api/client'
import { useAccounts, usePrices, usePutPrice, useRevokeToken, useTokens, useUpdateAccount, useUsers } from '../api/hooks'
import { AdminOnly } from '../components/AdminOnly'
import { formatDateTime, PLAN_LABELS, PLANS } from '../lib/format'

function AccountRow({ a }: { a: Account }) {
  const update = useUpdateAccount()
  const [label, setLabel] = useState(a.label)
  const [plan, setPlan] = useState<Plan>(a.plan)
  const dirty = label !== a.label || plan !== a.plan
  return (
    <tr>
      <td>
        <input value={label} onChange={(e) => setLabel(e.target.value)} maxLength={100} />
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

const RATE_FIELDS: { key: keyof PriceRates; label: string }[] = [
  { key: 'input', label: 'Input' },
  { key: 'cache_write_5m', label: 'Cache write 5m' },
  { key: 'cache_write_1h', label: 'Cache write 1h' },
  { key: 'cache_read', label: 'Cache read' },
  { key: 'output', label: 'Output' },
]

function PriceRow({ p }: { p: ModelPrice }) {
  const put = usePutPrice()
  // The draft starts from the full rates, so fast_* / long_* groups are sent back unchanged on save.
  const [draft, setDraft] = useState<PriceRates>(p.rates)
  const dirty = RATE_FIELDS.some((f) => draft[f.key] !== p.rates[f.key])
  return (
    <tr>
      <td>
        {p.model}
        {(p.rates.fast_input !== undefined || p.rates.long_input !== undefined) && (
          <div className="muted">{[p.rates.fast_input !== undefined && 'fast mode', p.rates.long_input !== undefined && '>100k tier'].filter(Boolean).join(', ')}</div>
        )}
      </td>
      {RATE_FIELDS.map((f) => (
        <td key={f.key} className="num">
          <input
            className="rate"
            type="number"
            min={0}
            step="0.01"
            value={draft[f.key] ?? 0}
            onChange={(e) => setDraft({ ...draft, [f.key]: Number(e.target.value) })}
          />
        </td>
      ))}
      <td>
        <button disabled={!dirty || put.isPending} onClick={() => put.mutate({ model: p.model, rates: draft })}>
          {put.isPending ? 'Saving…' : 'Save'}
        </button>
        {put.error && <div className="error">{put.error.message}</div>}
      </td>
    </tr>
  )
}

function Prices() {
  const q = usePrices()
  return (
    <section className="card">
      <h2>Model prices (USD per million tokens)</h2>
      <p className="muted">Used to estimate API-equivalent cost. Changes apply to all past usage.</p>
      {q.error && <div className="error">{q.error.message}</div>}
      <table>
        <thead>
          <tr>
            <th>Model</th>
            {RATE_FIELDS.map((f) => (
              <th key={f.key} className="num">{f.label}</th>
            ))}
            <th />
          </tr>
        </thead>
        <tbody>{q.data?.map((p) => <PriceRow key={p.model} p={p} />)}</tbody>
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

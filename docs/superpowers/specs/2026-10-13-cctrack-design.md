# cctrack — Claude Code usage tracking for shared subscriptions

**Date:** 2026-10-13
**Status:** Approved design, pending implementation plan

## 1. Problem

A team shares a small number of Claude subscriptions across more people and devices (e.g. 2 Claude Code accounts used from 3 devices by different people). Nobody can see who consumed how much of which account's plan limits. We want:

1. A **token + API-cost estimator** running on each user's machine.
2. A **Rust CLI** (`cctrack`) with **Google OAuth** login that identifies the user.
3. A **web app** (Go backend, Vite frontend, hosted on Vercel) with a **user dashboard** (own usage) and an **admin dashboard** (everyone's usage).
4. A **Claude Code plugin** (skill + hooks) that makes the agent upload usage via authenticated requests, and **refuses to work when the user is not logged in to the CLI**.
5. An optional **1–2 line summary** of the session's work attached to uploads, written by a Haiku subagent.

Required admin views:
- **By login** — how much usage each login (person) consumed, per account/plan.
- **By account** — how much of each Claude account was consumed by each user.

### Non-goals

- Backfilling historical transcripts from before a user's first `cctrack login`.
- Exact plan-limit accounting (Anthropic publishes no token budgets). We show token counts, estimated API-equivalent cost, and best-effort limit-utilisation snapshots.
- Server-side LLM calls (no Anthropic API key on the server).
- Tamper-proofing against a user who deliberately disables the plugin; we document a managed-settings lockdown instead.

## 2. Architecture

```
 developer machine                                   Vercel
┌──────────────────────────────────────┐        ┌────────────────────────────┐
│ Claude Code                          │        │  /api/*  → Go server       │
│  └─ cctrack plugin                   │ HTTPS  │            (net/http, pgx) │
│      ├─ hooks ──► cctrack hook <evt> │──────► │  /*      → Vite SPA        │
│      └─ skill ──► cctrack upload     │ Bearer │                            │
│ cctrack CLI (Rust)                   │        └─────────────┬──────────────┘
│  ├─ parses ~/.claude/projects/*.jsonl│                      │ pgx (pooled)
│  ├─ reads ~/.claude.json (account,   │                      ▼
│  │   limit utilisation snapshot)     │               Neon Postgres
│  └─ ~/.config/cctrack/credentials    │
└──────────────────────────────────────┘
```

- The CLI is the only component that reads local Claude Code files. It extracts **raw token counts** only — never prompt/response content (only cwd, project dir name, git branch, model, timestamps, token counts, and the agent-written summary leave the machine).
- The server stores raw tokens and computes cost at query time from a price table, so price corrections apply retroactively.

## 3. Monorepo layout

```
claude-tracker/
├── api/openapi.yaml                 # OpenAPI 3.0.x contract (source of truth)
├── apps/
│   ├── cli/                         # Rust: clap, reqwest(rustls), tokio, serde, directories, open
│   ├── server/                      # Go 1.25: net/http ServeMux, pgx/v5, goose, oauth2, go-oidc
│   │   ├── cmd/server/main.go
│   │   ├── cmd/migrate/main.go
│   │   ├── internal/...             # api (oapi-codegen), auth, db, ingest, pricing, query
│   │   └── migrations/*.sql         # embedded via embed.FS
│   └── web/                         # Vite + React 19 + TS, TanStack Router/Query, Recharts, openapi-fetch
├── plugin/cctrack/
│   ├── .claude-plugin/plugin.json
│   ├── hooks/hooks.json
│   ├── scripts/run                  # wrapper: exits 2 if cctrack missing
│   └── skills/cctrack/SKILL.md
├── .claude-plugin/marketplace.json  # makes the repo a plugin marketplace
├── docs/                            # setup guide, specs, plans
├── .github/workflows/ci.yml
├── Makefile                         # `just` is not installed; make is
└── vercel.json
```

Code generation: Go server interfaces/types via **oapi-codegen v2** (std `net/http` strict server); TS types via **openapi-typescript 7** consumed by **openapi-fetch**; Rust uses hand-written serde types mirroring the spec (only the ingest/auth/summary/me endpoints).

## 4. Data model (Postgres)

| Table | Columns (key ones) | Notes |
|---|---|---|
| `users` | id, google_sub (unique), email, name, created_at | created on first allowed login |
| `api_tokens` | id, user_id, token_hash (sha256, unique), device_name, created_at, last_used_at, revoked_at | CLI bearer tokens |
| `auth_requests` | id, state, cli_port, code_challenge, mode (`cli`/`cli_paste`/`web`), created_at, expires_at | pending Google flows, 5 min TTL |
| `cli_auth_codes` | code_hash, user_id, code_challenge, expires_at, used_at | one-time, 60 s TTL |
| `claude_accounts` | id, account_uuid (unique), org_uuid, email, org_type, rate_limit_tier, label, plan, created_at | `plan` ∈ Pro / Max 5x / Max 20x / Team / Enterprise / API key / Unknown; label+plan admin-editable |
| `devices` | id, user_id, hostname, os, first_seen, last_seen | unique (user_id, hostname) |
| `sessions` | session_id (PK, Claude's UUID), user_id, device_id, account_id, cwd, project, git_branch, started_at, ended_at, summary, summary_at | |
| `usage_entries` | id, session_id, user_id, account_id, message_id, request_id (`''` if absent), model, ts, input, output, cache_write_5m, cache_write_1h, cache_read, speed, is_subagent | **unique (message_id)** — API message ids are globally unique, `requestId` is missing on most lines, and resumed sessions re-copy earlier lines into a new session file; indexes on (user_id, ts), (account_id, ts) |
| `model_prices` | model (normalised id), input, cache_write_5m, cache_write_1h, cache_read, output, fast_input, fast_output, long_input, long_cache_write_5m, long_cache_write_1h, long_cache_read, long_output, updated_at | seeded by migration, admin-editable; `long_*` apply when the prompt exceeds 100k tokens |
| `limit_snapshots` | id, account_id, user_id, captured_at, five_hour_pct, five_hour_resets_at, seven_day_pct, seven_day_resets_at, source | unique (account_id, captured_at) |

`user_id` and `account_id` are denormalised onto `usage_entries` so the two admin views are single-table aggregates.

## 5. CLI (`apps/cli`, binary `cctrack`)

### 5.1 Commands

| Command | Behaviour |
|---|---|
| `cctrack login [--server URL] [--paste]` | Google login via server-brokered OAuth (§5.2). Saves server URL + token + user + `logged_in_at`. |
| `cctrack logout` | Calls `DELETE /api/auth/token`, deletes credentials. |
| `cctrack whoami` | Calls `GET /api/me`; prints user, server, admin flag. |
| `cctrack status [--json]` | **Local only, fast** (no network): logged in?, user, server, credentials valid flag, current Claude account from `~/.claude.json`, pending unsynced bytes. Exit 0 if logged in, 1 if not. |
| `cctrack sync [--transcript PATH] [--quiet]` | Parse new transcript bytes and upload (§5.4). Without `--transcript` it sweeps all transcripts modified since login. `--quiet` logs errors to `sync.log` (used by hooks, which spawn it detached). |
| `cctrack upload --summary "…" [--session ID]` | Run a sync for the session, then `POST /api/sessions/{id}/summary`. Session defaults to the last session recorded by the SessionStart hook (`last_session`). |
| `cctrack hook <event>` | Reads hook JSON on stdin, implements the gate/sync logic (§5.3). |

Files:
- `~/.config/cctrack/credentials.json` (mode 0600): `{server, token, user:{id,email,name}, logged_in_at, invalid:false}`. Plain file, not the OS keychain — the gate runs on every prompt/tool call and must be fast and never trigger a keychain prompt.
- `~/.local/state/cctrack/`: `offsets.json` (per-transcript byte offset), `sync.lock`, `accounts-<key>.json` (account timeline, §5.4) and `last_snapshot-<key>.json` — one each per Claude config dir, `<key>` being a short sha256 of its `.claude.json` path, so concurrent sessions under the default dir and `CLAUDE_CONFIG_DIR` don't mix accounts — `quarantine.json` (sessions the server deterministically rejected), `last_session.json`, `sync.log`.
- Respects `CLAUDE_CONFIG_DIR` (default `~/.claude`) for transcripts and `~/.claude.json`.

### 5.2 Login flow (server-brokered OAuth + PKCE)

1. CLI binds `127.0.0.1:0` (random port), generates `state` and a PKCE `code_verifier`, opens the browser at `SERVER/api/auth/cli/start?port=P&state=S&code_challenge=C`.
2. Server records an `auth_requests` row and redirects to Google (its own OAuth client, web flow, `openid email profile`).
3. Google → `GET /api/auth/google/callback`. Server exchanges the code, verifies the ID token (go-oidc against Google's JWKS), checks the allowlist (`hd` ∈ `ALLOWED_DOMAINS` or email ∈ `ALLOWED_EMAILS`), upserts the user, mints a one-time code (60 s) bound to the code_challenge, and redirects to `http://127.0.0.1:P/callback?code=X&state=S`.
4. CLI checks `state`, calls `POST /api/auth/cli/exchange {code, code_verifier, device_name}` → `{token, user}`. Server verifies `sha256(verifier)` matches the challenge, marks the code used, stores only `sha256(token)`.
5. `--paste` mode (headless/SSH): no loopback; the server shows the one-time code on a page and the CLI prompts for it.

Token format: `cct_` + 32 random bytes base64url.

### 5.3 The gate (plugin hooks → `cctrack hook`)

Hooks call `${CLAUDE_PLUGIN_ROOT}/scripts/run hook <event>`. The wrapper exits 2 with a "cctrack is not installed — install it with …" message if the binary is missing, so a missing CLI also blocks.

| Event | Behaviour |
|---|---|
| `SessionStart` | Cannot block. Injects `additionalContext`: whether the user is logged in, the `session_id`, and (if not logged in) the instruction that no work may begin until `cctrack login` succeeds. Records `last_session`. Spawns `cctrack sync --detach` (sweep of all new transcript bytes). |
| `UserPromptSubmit` | If not logged in or credentials marked invalid → `{"decision":"block","reason":"cctrack: not logged in. Run `cctrack login` in a terminal, then retry."}`. Otherwise allow silently. |
| `PreToolUse` (matcher `*`) | If not logged in → `permissionDecision:"deny"` for every tool except `Bash` whose command is exactly `cctrack login…`, `cctrack status…`, or `cctrack whoami…`. |
| `Stop`, `SessionEnd` | Spawn a detached `cctrack sync --quiet --transcript <path>` and exit immediately (plugin SessionEnd hooks get ~1.5 s; Stop does not fire on user interrupt, the next SessionStart sweep catches it). |

Hook settings: short timeouts (10 s; SessionEnd 5 s). Gate hooks fail closed: the wrapper exits 2 (blocking) if the binary is missing, crashes, or exits non-zero. The PreToolUse allowlist rejects any command containing shell chaining or redirection (`; & | \` $( > <` or newlines). The gate only checks local state (credentials file + `invalid` flag), so it never depends on the network. When a sync receives `401`, the CLI sets `invalid:true`, so a server-side revocation blocks the next prompt.

Enforcement hardening (documented, admin-applied): managed settings that force-enable the plugin via `enabledPlugins` and set `allowManagedHooksOnly`, so users cannot disable the hooks.

### 5.4 Transcript parsing and sync

Sources: `$CLAUDE_CONFIG_DIR/projects/*/*.jsonl` and `$CLAUDE_CONFIG_DIR/projects/*/<session>/subagents/**/agent-*.jsonl` (subagent entries attributed to the parent session, `is_subagent=true`).

For each file, read from the stored byte offset to the last complete line:
- Keep only `type:"assistant"` lines with `message.usage`.
- Skip `message.model == "<synthetic>"` and `isApiErrorMessage == true`.
- Skip entries with `timestamp < logged_in_at` (no backfill).
- Extract: `message.id`, `requestId` (optional), `sessionId`, `timestamp`, `message.model`, `cwd`, `gitBranch`, `isSidechain`; usage `input_tokens`, `output_tokens`, `cache_read_input_tokens`, `cache_creation.ephemeral_5m_input_tokens`, `cache_creation.ephemeral_1h_input_tokens` (fallback: all `cache_creation_input_tokens` → 5m), `speed` (default `standard`).
- Dedupe on `message.id` — one API response is written as several lines with identical usage (verified on real transcripts). The server's unique key is the backstop.

Batching: entries grouped into requests < 1 MB (Vercel body cap is 4.5 MB). Offsets for a file advance **only after** the server accepts the batch containing its last entry. A lock file prevents concurrent syncs (a second sync exits quietly). Sync failures are logged to `sync.log`, never surfaced as hook errors.

Account attribution: every SessionStart/UserPromptSubmit/Stop hook (and every sync) records the current `oauthAccount` from `~/.claude.json` (`accountUuid`, `organizationUuid`, `emailAddress`, `organizationType`, `organizationRateLimitTier`) into an account timeline whenever it changes. Each entry is attributed to the account active at its timestamp, so switching accounts before a sync does not misattribute earlier usage. If no `oauthAccount` is present (API-key auth), entries use `account_uuid: "api_key"`, which the server maps to a synthetic "API key" account per user+device.

Limit snapshot: if `~/.claude.json` `cachedUsageUtilization` (undocumented) has `accountUuid` matching the current account and `fetchedAtMs` newer than `last_snapshot.json`, attach `{five_hour:{pct,resets_at}, seven_day:{pct,resets_at}, captured_at}`. Parsed defensively — any shape mismatch means "no snapshot", never an error.

## 6. Plugin (`plugin/cctrack`)

- `.claude-plugin/plugin.json`: name `cctrack`, version, description.
- `hooks/hooks.json`: `{"hooks":{…}}` wiring §5.3 events to `${CLAUDE_PLUGIN_ROOT}/scripts/run hook <event>`.
- `skills/cctrack/SKILL.md` instructs the agent:
  - If any cctrack gate message appears (SessionStart context, blocked prompt, denied tool): **do not start or continue the task**; tell the user to run `cctrack login` and stop.
  - After completing a meaningful unit of work (and before ending a session where work was done): dispatch a **Haiku subagent** to write a ≤2-line summary of what was accomplished (no secrets, no code), then run `cctrack upload --summary "<summary>" --session <session_id>` using the session id from the SessionStart context.
  - How to check `cctrack status` / `cctrack whoami`.
- Root `.claude-plugin/marketplace.json` lists the plugin so teams install with `/plugin marketplace add <repo>` → `/plugin install cctrack@cctrack`.
- CLI install: `cargo install --path apps/cli` (prebuilt release binaries are a later addition).

## 7. Server API (`/api`, Go)

### 7.1 Auth
| Endpoint | Purpose |
|---|---|
| `GET /auth/cli/start?port&state&code_challenge[&mode=paste]` | Create `auth_requests` row, redirect to Google |
| `GET /auth/web/start` | Web login, redirect to Google |
| `GET /auth/google/callback` | Verify ID token + allowlist, upsert user; CLI → one-time code redirect/paste page; web → session cookie, redirect `/` |
| `POST /auth/cli/exchange` | `{code, code_verifier, device_name}` → `{token, user}` |
| `DELETE /auth/token` | Revoke the calling bearer token |
| `POST /auth/logout` | Clear web session cookie |
| `GET /me` | `{id, email, name, is_admin}` (cookie or bearer) |

### 7.2 Ingest (bearer)
- `POST /ingest` — `{device:{hostname,os}, account:{account_uuid,org_uuid,email,org_type,rate_limit_tier}|null, sessions:[{session_id,cwd,project,git_branch,started_at,ended_at}], entries:[…], limit_snapshot?}`. Single transaction: upsert device/account/sessions (sessions are only ever associated with the calling user — a session id owned by another user is rejected), insert entries `ON CONFLICT DO NOTHING`, insert snapshot `ON CONFLICT DO NOTHING`. Returns `{accepted, duplicates}`. Idempotent.
- `POST /sessions/{id}/summary` — `{summary}` (≤500 chars, trimmed); 404 unless the session belongs to the caller.

### 7.3 Queries (all take `from`, `to`, `tz`; daily buckets in `tz`)
- `GET /me/usage` — totals, daily series by model, breakdown by account, latest limit snapshot per account the user used.
- `GET /me/sessions?cursor` — paginated sessions with tokens, cost, summary.
- Admin (`ADMIN_EMAILS`, checked per request):
  - `GET /admin/by-login` — per user → per account (with plan) → per model totals + daily series.
  - `GET /admin/by-account` — per account → per user totals/share + daily per-user series + limit snapshot timeline.
  - `GET /admin/accounts`, `PATCH /admin/accounts/{id}` (`label`, `plan`).
  - `GET /admin/users`, `GET /admin/users/{id}/sessions`.
  - `GET /admin/tokens`, `DELETE /admin/tokens/{id}`.
  - `GET /admin/prices`, `PUT /admin/prices/{model}` (body: rates; `model` is normalised).

### 7.4 Cost computation
SQL aggregates token sums grouped by requested dimensions plus `model`, `speed`, and a long-context flag (`input + cache_write_5m + cache_write_1h + cache_read > 100000`). Go normalises model ids and multiplies by the price table (loaded per request):

- Normalisation: strip date suffixes (`-20251001`), `[1m]`, Bedrock `us.`/`eu.`/`anthropic.` prefixes and `-v1:0`, Vertex `@date`.
- Fast mode uses `fast_output` for output and scales input and cache rates by `fast_input / input`, where set (Opus 5.5 $8/$40; Opus 5 and 4.8 $10/$50).
- Long-context rates (`long_*`) apply per request when the long-context flag is set (Haiku 5.5).
- Models with no price contribute to `unpriced_tokens`, never $0.

Seed prices (USD per MTok — input / 5m write / 1h write / cache read / output):

| Model | In | 5m | 1h | Read | Out |
|---|---|---|---|---|---|
| claude-fable-5-1 | 10 | 12.5 | 20 | 0.25 | 50 |
| claude-fable-5 | 10 | 12.5 | 20 | 1.0 | 50 |
| claude-opus-5-5 | 4 | 5 | 8 | 0.20 | 20 |
| claude-opus-5 / 4-8 / 4-7 / 4-6 / 4-5 | 5 | 6.25 | 10 | 0.5 | 25 |
| claude-opus-4-1 / 4 | 15 | 18.75 | 30 | 1.5 | 75 |
| claude-sonnet-5-5 | 2 | 2.5 | 4 | 0.20 | 10 |
| claude-sonnet-5 | 2 | 2.5 | 4 | 0.2 | 10 |
| claude-sonnet-4-6 / 4-5 / 4 | 3 | 3.75 | 6 | 0.3 | 15 |
| claude-haiku-5-5 (≤100k prompt) | 0.10 | 0.125 | 0.20 | 0.01 | 0.50 |
| claude-haiku-5-5 (>100k prompt) | 0.5 | 0.625 | 1.0 | 0.05 | 2.5 |
| claude-haiku-4-5 | 1 | 1.25 | 2 | 0.1 | 5 |
| claude-3-5-haiku | 0.8 | 1 | 1.6 | 0.08 | 4 |
| claude-3-7-sonnet | 3 | 3.75 | — | 0.3 | 15 |

### 7.5 Security
- Web session: stateless HMAC-signed cookie (`SESSION_SECRET`), HttpOnly, Secure, SameSite=Lax, 30-day expiry.
- Cookie-authenticated non-GET requests require the header `X-CCTrack: 1`; browsers cannot send a custom header cross-site without a CORS preflight, which the server never grants. Bearer-authenticated requests are exempt.
- Bearer tokens looked up by sha256; `last_used_at` updated on each use.
- Request body limit 1 MB; OAuth `state` and one-time codes single-use with TTLs.
- No transcript content is ever accepted by the API.

## 8. Web app (`apps/web`)

- `/login` — Sign in with Google.
- `/` **My usage** — KPI cards (est. API cost with unpriced tokens, total tokens, output tokens, session count) for the selected range (presets: today / 7 d / 30 d / 90 d), stacked area chart of daily cost by model, by-account table, current 5h/7d limit gauges for accounts used, sessions list (project, branch, tokens, cost, summary).
- `/admin/logins` — **By login**: table users × accounts grouped by plan (tokens, cost, sessions); stacked bar per user split by account; drill-down to a user's sessions and summaries.
- `/admin/accounts` — **By account**: card per account with label/plan, limit gauges, donut of per-user share, daily stacked bar per user, line chart of 5h/7d limit % over time with reset markers.
- `/admin/settings` — edit account label/plan, list users, revoke tokens, edit prices.
- Shared date-range picker; browser timezone sent as `tz`. Admin nav visible only when `is_admin`.

## 9. Deployment

- **Vercel** with Services in `vercel.json`: `/api/(.*)` → Go service rooted at `apps/server` (`cmd/server/main.go`, listens on `$PORT`); everything else → Vite build of `apps/web` with SPA fallback.
- **Neon Postgres** via pooled connection string; small `pgxpool` (max ~4 conns).
- Env vars: `DATABASE_URL`, `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`, `SESSION_SECRET`, `PUBLIC_URL`, `ALLOWED_DOMAINS`, `ALLOWED_EMAILS`, `ADMIN_EMAILS`, optional `AUTO_MIGRATE`.
- Migrations: goose, embedded. `cmd/migrate` run explicitly; `AUTO_MIGRATE=true` runs them at startup under a Postgres advisory lock.
- The Go server is plain `net/http` + env config, so it can also run as a container elsewhere.
- Local dev: `make dev-server` + `make dev-web` (Vite proxies `/api`), requires `DATABASE_URL` (a Neon dev branch works — no local Postgres install required). `make gen` regenerates OpenAPI code. `make test` runs every suite.
- `docs/setup.md`: Google OAuth client setup (authorised redirect `PUBLIC_URL/api/auth/google/callback`), Vercel + Neon steps, plugin install, managed-settings snippet.

## 10. Testing

- **Plugin**: `scripts/test-run.sh` checks the wrapper fails closed for gate events.
- **Rust**: parser unit tests with fixture JSONL (dedupe across multi-line responses, missing requestId, `<synthetic>`/error skip, subagent files, partial trailing line, offset resume, login-time cutoff); `~/.claude.json` parsing including malformed/missing fields; hook gate tests (stdin JSON → exit code/stdout JSON); sync test against a local mock HTTP server (offsets advance only on success, 401 marks credentials invalid).
- **Go**: unit tests for pricing/normalisation, allowlist, cookie signing, PKCE; `httptest` handler tests; DB integration tests against `TEST_DATABASE_URL` (skipped when unset) for ingest idempotency and aggregate queries.
- **Web**: `tsc --noEmit` and `vite build`.
- **CI**: GitHub Actions running all three.

## 11. Risks and open points

- `cachedUsageUtilization` and transcript formats are undocumented and may change; parsers are defensive and the dashboard degrades to "no limit data".
- Transcripts are deleted after `cleanupPeriodDays` (default 30); an offline machine that never syncs within that window loses data.
- Hooks can be bypassed by a user who disables the plugin unless managed settings are applied.
- Vercel Services/Go runtime specifics are verified during implementation; the server stays portable if they differ.

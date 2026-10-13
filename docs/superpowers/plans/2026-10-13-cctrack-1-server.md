# cctrack Server Implementation Plan (Plan 1 of 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Go API server (auth, ingest, user + admin queries) and the shared OpenAPI contract for cctrack.

**Architecture:** Go 1.25 `net/http` ServeMux with handlers generated from `api/openapi.yaml` by oapi-codegen (`std-http-server` + models). Postgres via pgx/v5, migrations embedded and run by goose. Google sign-in through `golang.org/x/oauth2` + `go-oidc`. Raw tokens stored; cost computed at query time in Go from a `model_prices` table.

**Tech Stack:** Go 1.25, pgx/v5, goose/v3, oapi-codegen v2 (as a `go tool`), oapi-codegen/runtime, golang.org/x/oauth2, github.com/coreos/go-oidc/v3.

**Spec:** `docs/superpowers/specs/2026-10-13-cctrack-design.md`

**Plan series:** Plan 1 (this) → Plan 2 `2026-10-13-cctrack-2-cli-plugin.md` → Plan 3 `2026-10-13-cctrack-3-web-deploy.md`. Plans 2 and 3 depend only on `api/openapi.yaml` from Task 1 here.

## Global Constraints

- **Never install, upgrade, or uninstall software without the user's explicit permission** (brew, pip, npm/pnpm, cargo install, go modules, docker images…). Adding a dependency to `go.mod` counts. The user approves the dependency list before execution starts; anything not on that list needs a new ask. Pass this rule to every subagent.
- Go version: `go 1.25` in `go.mod`. Do not upgrade the toolchain.
- Module path: `cctrack/server`, rooted at `apps/server/` (Vercel requires `go.mod` at the service root and detects `cmd/server/main.go`).
- Server listens on `$PORT` (default `8080`). All routes live under `/api` (Vercel forwards the full original path).
- Env vars, verbatim: `DATABASE_URL`, `DATABASE_URL_UNPOOLED` (optional, used for migrations), `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`, `SESSION_SECRET` (≥32 bytes), `PUBLIC_URL`, `ALLOWED_DOMAINS`, `ALLOWED_EMAILS`, `ADMIN_EMAILS`, `AUTO_MIGRATE`, `PORT`.
- Never store plaintext API tokens or one-time codes — sha256 only.
- Never accept or store transcript content. Ingest carries token counts and metadata only.
- Models without a price are reported in `unpriced_tokens`, never as $0 cost.
- `usage_entries` dedupe key is `message_id` alone (globally unique API response id; `requestId` is missing on most transcript lines and resumed sessions re-copy lines).
- Request body limit 1 MB. Ingest limits: ≤5000 entries, ≤500 sessions, ≤20 accounts per request.
- Plans: `pro`, `max_5x`, `max_20x`, `team`, `enterprise`, `api_key`, `unknown`.
- Cookie-authenticated non-GET requests must carry header `X-CCTrack: 1` (CSRF); bearer-authenticated requests are exempt.
- Commit after each task with messages ending in `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- DB-backed tests read `TEST_DATABASE_URL` (a direct, non-pooled Postgres URL) and `t.Skip` when unset. Each test package gets its own throwaway schema.

## Review Focus

1. **Session id owned by another user** — ingest must reject the whole batch (403) and insert nothing; a user must never add usage to someone else's session. Test in Task 6.
2. **Day bucketing in a non-UTC timezone** — an entry at 23:30Z must land on the next day for `tz=Asia/Kolkata`; invalid tz names must be 400, not 500. Test in Task 7.
3. **Re-ingesting an account after an admin edited its label/plan** — upserts must not overwrite admin edits. Test in Task 6.
4. **One-time CLI code reuse or wrong PKCE verifier** — second exchange and wrong verifier both 400; code is burned on first attempt. Test in Task 5.
5. **Revoked token** — any request with it returns 401 (the CLI relies on 401 to lock the gate). Test in Tasks 5 and 8.

---

## File Structure

```
.gitignore
Makefile
README.md
api/openapi.yaml                         # contract (Task 1)
apps/server/
  go.mod, go.sum
  oapi-codegen.yaml
  .env.example
  cmd/server/main.go                     # Task 5
  cmd/migrate/main.go                    # Task 2
  migrations/
    embed.go                             # embed.FS of *.sql
    00001_init.sql
    00002_seed_prices.sql
  internal/
    api/generate.go, api.gen.go          # generated (Task 1)
    config/config.go, config_test.go     # env config (Task 1)
    db/db.go, db_test.go                 # pool + goose migrate (Task 2)
    testdb/testdb.go                     # per-test schema harness (Task 2)
    pricing/normalize.go, pricing.go, pricing_test.go   # (Task 3)
    auth/allowlist.go, tokens.go, pkce.go, session.go, identity.go, google.go, auth_test.go  # (Task 4)
    store/store.go, users.go, ingest.go, query.go, admin.go, plan.go, plan_test.go  # (Tasks 2,5–8)
    report/report.go, report_test.go     # cost folding (Task 7)
    httpapi/
      server.go, respond.go, middleware.go, stubs.go, html.go   # (Task 5)
      auth_handlers.go                   # (Task 5)
      ingest.go, validate.go             # (Task 6)
      me.go, convert.go, rangeparams.go  # (Task 7)
      admin.go                           # (Task 8)
      helpers_test.go, auth_test.go, ingest_test.go, validate_test.go, me_test.go, admin_test.go
```

---

### Task 1: Repo scaffold, OpenAPI contract, config, codegen

**Files:**
- Create: `.gitignore`, `Makefile`, `README.md`, `api/openapi.yaml`
- Create: `apps/server/go.mod`, `apps/server/oapi-codegen.yaml`, `apps/server/.env.example`
- Create: `apps/server/internal/api/generate.go` (+ generated `api.gen.go`)
- Create: `apps/server/internal/config/config.go`, `apps/server/internal/config/config_test.go`

**Interfaces:**
- Produces: `config.Config` (fields below), `config.Load(getenv func(string) string) (config.Config, error)`, `Config.SecureCookies() bool`, `Config.MigrateURL() string`; package `api` with generated types (`api.IngestRequest`, `api.Totals`, …), `api.ServerInterface`, `api.HandlerWithOptions`, `api.StdHTTPServerOptions`.

- [ ] **Step 1: Root files**

`.gitignore`:
```gitignore
# Go
apps/server/.env
apps/server/bin/
# Rust
apps/cli/target/
# Web
apps/web/node_modules/
apps/web/dist/
# OS / editors
.DS_Store
.idea/
.vscode/
.vercel
```

`Makefile` (recipe lines are indented with a TAB):
```make
.PHONY: gen gen-go gen-ts test test-go test-cli test-web test-plugin dev-server dev-web migrate db-up db-down

gen: gen-go gen-ts

gen-go:
	cd apps/server && go generate ./...

gen-ts:
	cd apps/web && pnpm exec openapi-typescript ../../api/openapi.yaml -o src/api/schema.d.ts

test: test-go test-cli test-web test-plugin

test-go:
	cd apps/server && go vet ./... && go test ./...

test-cli:
	cd apps/cli && cargo test

test-web:
	cd apps/web && pnpm exec tsc --noEmit && pnpm exec vitest run

test-plugin:
	sh plugin/cctrack/scripts/test-run.sh

dev-server:
	cd apps/server && set -a && { [ ! -f .env ] || . ./.env; } && set +a && go run ./cmd/server

dev-web:
	cd apps/web && pnpm dev

migrate:
	cd apps/server && set -a && { [ ! -f .env ] || . ./.env; } && set +a && go run ./cmd/migrate

# Optional local Postgres (requires Docker/OrbStack running and pulling postgres:17-alpine — ask first)
db-up:
	docker run -d --name cctrack-pg -e POSTGRES_PASSWORD=postgres -p 54329:5432 postgres:17-alpine

db-down:
	docker rm -f cctrack-pg
```

`README.md`:
```markdown
# cctrack

Track Claude Code plan usage across a team that shares Claude subscriptions.

- `apps/cli` — Rust CLI (`cctrack`): Google login, transcript parsing, upload.
- `apps/server` — Go API (`/api/*`), Postgres.
- `apps/web` — Vite + React dashboards.
- `plugin/cctrack` — Claude Code plugin (hooks + skill) that gates work on login and uploads usage.
- `api/openapi.yaml` — the API contract.

See `docs/setup.md` for deployment and installation.
```

- [ ] **Step 2: Write `api/openapi.yaml`**

```yaml
openapi: 3.0.3
info:
  title: cctrack API
  version: 0.1.0
servers:
  - url: /api
paths:
  /healthz:
    get:
      operationId: getHealth
      responses:
        "200":
          description: OK
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Health" }

  /auth/cli/start:
    get:
      operationId: cliStart
      description: Begin a CLI login. Redirects to Google.
      parameters:
        - { name: port, in: query, required: false, schema: { type: integer } }
        - { name: state, in: query, required: true, schema: { type: string } }
        - { name: code_challenge, in: query, required: true, schema: { type: string } }
        - { name: mode, in: query, required: false, schema: { type: string, enum: [loopback, paste] } }
      responses:
        "302": { description: Redirect to Google }
        "400": { $ref: "#/components/responses/Error" }

  /auth/web/start:
    get:
      operationId: webStart
      responses:
        "302": { description: Redirect to Google }

  /auth/google/callback:
    get:
      operationId: googleCallback
      parameters:
        - { name: code, in: query, required: false, schema: { type: string } }
        - { name: state, in: query, required: false, schema: { type: string } }
        - { name: error, in: query, required: false, schema: { type: string } }
      responses:
        "200": { description: Paste-code page, content: { text/html: { schema: { type: string } } } }
        "302": { description: Redirect to CLI loopback or web app }
        "400": { description: Bad or expired request, content: { text/html: { schema: { type: string } } } }
        "403": { description: Account not allowed, content: { text/html: { schema: { type: string } } } }

  /auth/cli/exchange:
    post:
      operationId: cliExchange
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/CliExchangeRequest" }
      responses:
        "200":
          description: Token issued
          content:
            application/json:
              schema: { $ref: "#/components/schemas/CliExchangeResponse" }
        "400": { $ref: "#/components/responses/Error" }

  /auth/token:
    delete:
      operationId: revokeOwnToken
      security: [{ bearer: [] }]
      responses:
        "204": { description: Revoked }
        "401": { $ref: "#/components/responses/Error" }

  /auth/logout:
    post:
      operationId: webLogout
      responses:
        "204": { description: Logged out }

  /me:
    get:
      operationId: getMe
      security: [{ bearer: [] }, { cookie: [] }]
      responses:
        "200":
          description: Current user
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Me" }
        "401": { $ref: "#/components/responses/Error" }

  /ingest:
    post:
      operationId: ingest
      security: [{ bearer: [] }]
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/IngestRequest" }
      responses:
        "200":
          description: Accepted
          content:
            application/json:
              schema: { $ref: "#/components/schemas/IngestResponse" }
        "400": { $ref: "#/components/responses/Error" }
        "401": { $ref: "#/components/responses/Error" }
        "403": { $ref: "#/components/responses/Error" }

  /sessions/{sessionId}/summary:
    post:
      operationId: setSessionSummary
      security: [{ bearer: [] }]
      parameters:
        - { name: sessionId, in: path, required: true, schema: { type: string } }
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/SummaryRequest" }
      responses:
        "204": { description: Saved }
        "400": { $ref: "#/components/responses/Error" }
        "401": { $ref: "#/components/responses/Error" }
        "404": { $ref: "#/components/responses/Error" }

  /me/usage:
    get:
      operationId: getMyUsage
      security: [{ bearer: [] }, { cookie: [] }]
      parameters:
        - $ref: "#/components/parameters/From"
        - $ref: "#/components/parameters/To"
        - $ref: "#/components/parameters/Tz"
      responses:
        "200":
          description: Usage for the current user
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Usage" }
        "400": { $ref: "#/components/responses/Error" }
        "401": { $ref: "#/components/responses/Error" }

  /me/sessions:
    get:
      operationId: listMySessions
      security: [{ bearer: [] }, { cookie: [] }]
      parameters:
        - $ref: "#/components/parameters/Limit"
        - $ref: "#/components/parameters/Offset"
      responses:
        "200":
          description: Sessions, newest first
          content:
            application/json:
              schema: { $ref: "#/components/schemas/SessionPage" }
        "401": { $ref: "#/components/responses/Error" }

  /admin/by-login:
    get:
      operationId: adminByLogin
      security: [{ cookie: [] }]
      parameters:
        - $ref: "#/components/parameters/From"
        - $ref: "#/components/parameters/To"
        - $ref: "#/components/parameters/Tz"
      responses:
        "200":
          description: Usage per login, broken down by account
          content:
            application/json:
              schema: { $ref: "#/components/schemas/ByLogin" }
        "400": { $ref: "#/components/responses/Error" }
        "403": { $ref: "#/components/responses/Error" }

  /admin/by-account:
    get:
      operationId: adminByAccount
      security: [{ cookie: [] }]
      parameters:
        - $ref: "#/components/parameters/From"
        - $ref: "#/components/parameters/To"
        - $ref: "#/components/parameters/Tz"
      responses:
        "200":
          description: Usage per Claude account, broken down by user
          content:
            application/json:
              schema: { $ref: "#/components/schemas/ByAccount" }
        "400": { $ref: "#/components/responses/Error" }
        "403": { $ref: "#/components/responses/Error" }

  /admin/accounts:
    get:
      operationId: adminListAccounts
      security: [{ cookie: [] }]
      responses:
        "200":
          description: All Claude accounts
          content:
            application/json:
              schema: { type: array, items: { $ref: "#/components/schemas/Account" } }
        "403": { $ref: "#/components/responses/Error" }

  /admin/accounts/{id}:
    patch:
      operationId: adminUpdateAccount
      security: [{ cookie: [] }]
      parameters:
        - $ref: "#/components/parameters/Id"
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/AccountPatch" }
      responses:
        "200":
          description: Updated
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Account" }
        "400": { $ref: "#/components/responses/Error" }
        "403": { $ref: "#/components/responses/Error" }
        "404": { $ref: "#/components/responses/Error" }

  /admin/users:
    get:
      operationId: adminListUsers
      security: [{ cookie: [] }]
      responses:
        "200":
          description: All users
          content:
            application/json:
              schema: { type: array, items: { $ref: "#/components/schemas/AdminUser" } }
        "403": { $ref: "#/components/responses/Error" }

  /admin/users/{id}/sessions:
    get:
      operationId: adminListUserSessions
      security: [{ cookie: [] }]
      parameters:
        - $ref: "#/components/parameters/Id"
        - $ref: "#/components/parameters/Limit"
        - $ref: "#/components/parameters/Offset"
      responses:
        "200":
          description: The user's sessions, newest first
          content:
            application/json:
              schema: { $ref: "#/components/schemas/SessionPage" }
        "403": { $ref: "#/components/responses/Error" }

  /admin/tokens:
    get:
      operationId: adminListTokens
      security: [{ cookie: [] }]
      responses:
        "200":
          description: All CLI tokens
          content:
            application/json:
              schema: { type: array, items: { $ref: "#/components/schemas/ApiToken" } }
        "403": { $ref: "#/components/responses/Error" }

  /admin/tokens/{id}:
    delete:
      operationId: adminRevokeToken
      security: [{ cookie: [] }]
      parameters:
        - $ref: "#/components/parameters/Id"
      responses:
        "204": { description: Revoked }
        "403": { $ref: "#/components/responses/Error" }
        "404": { $ref: "#/components/responses/Error" }

  /admin/prices:
    get:
      operationId: adminListPrices
      security: [{ cookie: [] }]
      responses:
        "200":
          description: Model price table (USD per million tokens)
          content:
            application/json:
              schema: { type: array, items: { $ref: "#/components/schemas/ModelPrice" } }
        "403": { $ref: "#/components/responses/Error" }

  /admin/prices/{model}:
    put:
      operationId: adminPutPrice
      security: [{ cookie: [] }]
      parameters:
        - { name: model, in: path, required: true, schema: { type: string } }
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/PriceRates" }
      responses:
        "200":
          description: Saved
          content:
            application/json:
              schema: { $ref: "#/components/schemas/ModelPrice" }
        "400": { $ref: "#/components/responses/Error" }
        "403": { $ref: "#/components/responses/Error" }

components:
  securitySchemes:
    bearer: { type: http, scheme: bearer }
    cookie: { type: apiKey, in: cookie, name: cctrack_session }

  parameters:
    From: { name: from, in: query, required: true, schema: { type: string, format: date-time } }
    To: { name: to, in: query, required: true, schema: { type: string, format: date-time } }
    Tz: { name: tz, in: query, required: false, schema: { type: string } }
    Limit: { name: limit, in: query, required: false, schema: { type: integer } }
    Offset: { name: offset, in: query, required: false, schema: { type: integer } }
    Id: { name: id, in: path, required: true, schema: { type: integer, format: int64 } }

  responses:
    Error:
      description: Error
      content:
        application/json:
          schema: { $ref: "#/components/schemas/Error" }

  schemas:
    Health:
      type: object
      required: [status]
      properties: { status: { type: string } }

    Error:
      type: object
      required: [error]
      properties: { error: { type: string } }

    User:
      type: object
      required: [id, email, name]
      properties:
        id: { type: integer, format: int64 }
        email: { type: string }
        name: { type: string }

    UserRef:
      type: object
      required: [id, email, name]
      properties:
        id: { type: integer, format: int64 }
        email: { type: string }
        name: { type: string }

    Me:
      type: object
      required: [id, email, name, is_admin]
      properties:
        id: { type: integer, format: int64 }
        email: { type: string }
        name: { type: string }
        is_admin: { type: boolean }

    CliExchangeRequest:
      type: object
      required: [code, code_verifier, device_name]
      properties:
        code: { type: string }
        code_verifier: { type: string }
        device_name: { type: string }

    CliExchangeResponse:
      type: object
      required: [token, user]
      properties:
        token: { type: string }
        user: { $ref: "#/components/schemas/User" }

    Device:
      type: object
      required: [hostname, os]
      properties:
        hostname: { type: string }
        os: { type: string }

    IngestAccount:
      type: object
      description: >
        A Claude account seen on the device. account_uuid "api_key" means API-key auth;
        the server maps it to a per-user, per-device synthetic account.
      required: [account_uuid]
      properties:
        account_uuid: { type: string }
        org_uuid: { type: string }
        email: { type: string }
        org_type: { type: string }
        rate_limit_tier: { type: string }

    IngestSession:
      type: object
      required: [session_id, started_at, ended_at]
      properties:
        session_id: { type: string }
        cwd: { type: string }
        git_branch: { type: string }
        started_at: { type: string, format: date-time }
        ended_at: { type: string, format: date-time }

    IngestEntry:
      type: object
      required: [session_id, account_uuid, message_id, model, ts, input, output, cache_write_5m, cache_write_1h, cache_read, is_subagent]
      properties:
        session_id: { type: string }
        account_uuid: { type: string }
        message_id: { type: string }
        request_id: { type: string }
        model: { type: string }
        ts: { type: string, format: date-time }
        input: { type: integer, format: int64 }
        output: { type: integer, format: int64 }
        cache_write_5m: { type: integer, format: int64 }
        cache_write_1h: { type: integer, format: int64 }
        cache_read: { type: integer, format: int64 }
        speed: { type: string }
        is_subagent: { type: boolean }

    LimitSnapshotIn:
      type: object
      required: [account_uuid, captured_at]
      properties:
        account_uuid: { type: string }
        captured_at: { type: string, format: date-time }
        five_hour_pct: { type: number, format: double }
        five_hour_resets_at: { type: string, format: date-time }
        seven_day_pct: { type: number, format: double }
        seven_day_resets_at: { type: string, format: date-time }

    IngestRequest:
      type: object
      required: [device, accounts, sessions, entries]
      properties:
        device: { $ref: "#/components/schemas/Device" }
        accounts: { type: array, items: { $ref: "#/components/schemas/IngestAccount" } }
        sessions: { type: array, items: { $ref: "#/components/schemas/IngestSession" } }
        entries: { type: array, items: { $ref: "#/components/schemas/IngestEntry" } }
        limit_snapshot: { $ref: "#/components/schemas/LimitSnapshotIn" }

    IngestResponse:
      type: object
      required: [accepted, duplicates]
      properties:
        accepted: { type: integer }
        duplicates: { type: integer }

    SummaryRequest:
      type: object
      required: [summary]
      properties:
        summary: { type: string }

    Totals:
      type: object
      required: [input, output, cache_write_5m, cache_write_1h, cache_read, total_tokens, cost_usd, unpriced_tokens]
      properties:
        input: { type: integer, format: int64 }
        output: { type: integer, format: int64 }
        cache_write_5m: { type: integer, format: int64 }
        cache_write_1h: { type: integer, format: int64 }
        cache_read: { type: integer, format: int64 }
        total_tokens: { type: integer, format: int64 }
        cost_usd: { type: number, format: double }
        unpriced_tokens: { type: integer, format: int64 }

    BreakdownRow:
      type: object
      required: [key, label, totals]
      properties:
        key: { type: string }
        label: { type: string }
        totals: { $ref: "#/components/schemas/Totals" }

    SeriesPoint:
      type: object
      required: [date, key, label, total_tokens, cost_usd]
      properties:
        date: { type: string, description: "YYYY-MM-DD in the requested tz" }
        key: { type: string }
        label: { type: string }
        total_tokens: { type: integer, format: int64 }
        cost_usd: { type: number, format: double }

    Plan:
      type: string
      enum: [pro, max_5x, max_20x, team, enterprise, api_key, unknown]

    AccountRef:
      type: object
      required: [id, label, plan]
      properties:
        id: { type: integer, format: int64 }
        label: { type: string }
        plan: { $ref: "#/components/schemas/Plan" }
        email: { type: string }

    Account:
      type: object
      required: [id, account_uuid, label, plan, created_at]
      properties:
        id: { type: integer, format: int64 }
        account_uuid: { type: string }
        org_uuid: { type: string }
        email: { type: string }
        org_type: { type: string }
        rate_limit_tier: { type: string }
        label: { type: string }
        plan: { $ref: "#/components/schemas/Plan" }
        created_at: { type: string, format: date-time }

    AccountPatch:
      type: object
      properties:
        label: { type: string }
        plan: { $ref: "#/components/schemas/Plan" }

    LimitSnapshot:
      type: object
      required: [account_id, captured_at]
      properties:
        account_id: { type: integer, format: int64 }
        captured_at: { type: string, format: date-time }
        five_hour_pct: { type: number, format: double }
        five_hour_resets_at: { type: string, format: date-time }
        seven_day_pct: { type: number, format: double }
        seven_day_resets_at: { type: string, format: date-time }

    Usage:
      type: object
      required: [totals, by_model, by_account, daily_by_model, limits]
      properties:
        totals: { $ref: "#/components/schemas/Totals" }
        by_model: { type: array, items: { $ref: "#/components/schemas/BreakdownRow" } }
        by_account: { type: array, items: { $ref: "#/components/schemas/BreakdownRow" } }
        daily_by_model: { type: array, items: { $ref: "#/components/schemas/SeriesPoint" } }
        limits: { type: array, items: { $ref: "#/components/schemas/LimitSnapshot" } }

    Session:
      type: object
      required: [session_id, user, started_at, ended_at, totals]
      properties:
        session_id: { type: string }
        user: { $ref: "#/components/schemas/UserRef" }
        account: { $ref: "#/components/schemas/AccountRef" }
        device_hostname: { type: string }
        project: { type: string }
        git_branch: { type: string }
        started_at: { type: string, format: date-time }
        ended_at: { type: string, format: date-time }
        summary: { type: string }
        totals: { $ref: "#/components/schemas/Totals" }

    SessionPage:
      type: object
      required: [items, total]
      properties:
        items: { type: array, items: { $ref: "#/components/schemas/Session" } }
        total: { type: integer }

    AccountUsage:
      type: object
      required: [account, totals]
      properties:
        account: { $ref: "#/components/schemas/AccountRef" }
        totals: { $ref: "#/components/schemas/Totals" }

    LoginUsage:
      type: object
      required: [user, totals, by_account]
      properties:
        user: { $ref: "#/components/schemas/UserRef" }
        totals: { $ref: "#/components/schemas/Totals" }
        by_account: { type: array, items: { $ref: "#/components/schemas/AccountUsage" } }

    ByLogin:
      type: object
      required: [users, daily_by_user]
      properties:
        users: { type: array, items: { $ref: "#/components/schemas/LoginUsage" } }
        daily_by_user: { type: array, items: { $ref: "#/components/schemas/SeriesPoint" } }

    UserShare:
      type: object
      required: [user, totals, share]
      properties:
        user: { $ref: "#/components/schemas/UserRef" }
        totals: { $ref: "#/components/schemas/Totals" }
        share: { type: number, format: double, description: "0..1 share of the account's cost (token share when cost is 0)" }

    AccountDetail:
      type: object
      required: [account, totals, by_user, daily_by_user, limits]
      properties:
        account: { $ref: "#/components/schemas/Account" }
        totals: { $ref: "#/components/schemas/Totals" }
        by_user: { type: array, items: { $ref: "#/components/schemas/UserShare" } }
        daily_by_user: { type: array, items: { $ref: "#/components/schemas/SeriesPoint" } }
        limits: { type: array, items: { $ref: "#/components/schemas/LimitSnapshot" } }

    ByAccount:
      type: object
      required: [accounts]
      properties:
        accounts: { type: array, items: { $ref: "#/components/schemas/AccountDetail" } }

    AdminUser:
      type: object
      required: [id, email, name, is_admin, created_at]
      properties:
        id: { type: integer, format: int64 }
        email: { type: string }
        name: { type: string }
        is_admin: { type: boolean }
        created_at: { type: string, format: date-time }
        last_seen_at: { type: string, format: date-time }

    ApiToken:
      type: object
      required: [id, user, device_name, created_at]
      properties:
        id: { type: integer, format: int64 }
        user: { $ref: "#/components/schemas/UserRef" }
        device_name: { type: string }
        created_at: { type: string, format: date-time }
        last_used_at: { type: string, format: date-time }
        revoked_at: { type: string, format: date-time }

    PriceRates:
      type: object
      description: USD per million tokens. fast_* apply to speed=fast; long_* apply when prompt > 100k tokens.
      required: [input, cache_write_5m, cache_write_1h, cache_read, output]
      properties:
        input: { type: number, format: double }
        cache_write_5m: { type: number, format: double }
        cache_write_1h: { type: number, format: double }
        cache_read: { type: number, format: double }
        output: { type: number, format: double }
        fast_input: { type: number, format: double }
        fast_output: { type: number, format: double }
        long_input: { type: number, format: double }
        long_cache_write_5m: { type: number, format: double }
        long_cache_write_1h: { type: number, format: double }
        long_cache_read: { type: number, format: double }
        long_output: { type: number, format: double }

    ModelPrice:
      type: object
      required: [model, rates, updated_at]
      properties:
        model: { type: string }
        rates: { $ref: "#/components/schemas/PriceRates" }
        updated_at: { type: string, format: date-time }
```

- [ ] **Step 3: Go module and codegen config**

```bash
mkdir -p apps/server/internal/api apps/server/internal/config
cd apps/server
go mod init cctrack/server
go mod edit -go=1.25
go get -tool github.com/oapi-codegen/oapi-codegen/v2/cmd/oapi-codegen@v2.8.0
go get github.com/oapi-codegen/runtime
```

`apps/server/oapi-codegen.yaml`:
```yaml
package: api
output: internal/api/api.gen.go
generate:
  models: true
  std-http-server: true
```

`apps/server/internal/api/generate.go`:
```go
// Package api contains types and routing generated from api/openapi.yaml.
package api

//go:generate go tool oapi-codegen -config ../../oapi-codegen.yaml ../../../../api/openapi.yaml
```

`go generate` runs in the package dir, so the `output:` path in the config is resolved relative to it. If the file lands at `internal/api/internal/api/api.gen.go`, change `output:` to `api.gen.go`.

- [ ] **Step 4: Generate and build**

Run: `cd apps/server && go generate ./... && go mod tidy && go build ./...`
Expected: `internal/api/api.gen.go` exists, build succeeds. Open it and note the exact `ServerInterface` method signatures — Task 5 implements them.

- [ ] **Step 5: Write failing config tests**

`apps/server/internal/config/config_test.go`:
```go
package config

import (
	"strings"
	"testing"
)

func env(m map[string]string) func(string) string {
	return func(k string) string { return m[k] }
}

func valid() map[string]string {
	return map[string]string{
		"DATABASE_URL":         "postgres://x",
		"GOOGLE_CLIENT_ID":     "id",
		"GOOGLE_CLIENT_SECRET": "secret",
		"SESSION_SECRET":       strings.Repeat("s", 32),
		"PUBLIC_URL":           "https://cctrack.example.com/",
		"ALLOWED_DOMAINS":      " Example.com , ,acme.io",
		"ADMIN_EMAILS":         "Boss@Example.com",
	}
}

func TestLoadValid(t *testing.T) {
	c, err := Load(env(valid()))
	if err != nil {
		t.Fatal(err)
	}
	if c.PublicURL != "https://cctrack.example.com" {
		t.Errorf("PublicURL = %q", c.PublicURL)
	}
	if strings.Join(c.AllowedDomains, ",") != "example.com,acme.io" {
		t.Errorf("AllowedDomains = %v", c.AllowedDomains)
	}
	if c.AdminEmails[0] != "boss@example.com" {
		t.Errorf("AdminEmails = %v", c.AdminEmails)
	}
	if c.Port != "8080" || !c.SecureCookies() || c.AutoMigrate {
		t.Errorf("defaults wrong: %+v", c)
	}
	if c.MigrateURL() != "postgres://x" {
		t.Errorf("MigrateURL = %q", c.MigrateURL())
	}
}

func TestLoadUnpooledMigrateURL(t *testing.T) {
	m := valid()
	m["DATABASE_URL_UNPOOLED"] = "postgres://direct"
	c, err := Load(env(m))
	if err != nil {
		t.Fatal(err)
	}
	if c.MigrateURL() != "postgres://direct" {
		t.Errorf("MigrateURL = %q", c.MigrateURL())
	}
}

func TestLoadMissing(t *testing.T) {
	_, err := Load(env(map[string]string{}))
	if err == nil {
		t.Fatal("expected error")
	}
	for _, name := range []string{"DATABASE_URL", "GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET", "SESSION_SECRET", "PUBLIC_URL"} {
		if !strings.Contains(err.Error(), name) {
			t.Errorf("error %q does not mention %s", err, name)
		}
	}
}

func TestLoadShortSecret(t *testing.T) {
	m := valid()
	m["SESSION_SECRET"] = "short"
	if _, err := Load(env(m)); err == nil || !strings.Contains(err.Error(), "SESSION_SECRET") {
		t.Fatalf("expected SESSION_SECRET error, got %v", err)
	}
}

func TestLoadRequiresAllowlist(t *testing.T) {
	m := valid()
	delete(m, "ALLOWED_DOMAINS")
	if _, err := Load(env(m)); err == nil {
		t.Fatal("expected error when no allowlist is configured")
	}
	m["ALLOWED_EMAILS"] = "solo@gmail.com"
	if _, err := Load(env(m)); err != nil {
		t.Fatalf("ALLOWED_EMAILS alone should be enough: %v", err)
	}
}

func TestLoadBadPublicURL(t *testing.T) {
	m := valid()
	m["PUBLIC_URL"] = "cctrack.example.com"
	if _, err := Load(env(m)); err == nil {
		t.Fatal("expected error for PUBLIC_URL without scheme")
	}
}
```

- [ ] **Step 6: Run to verify failure**

Run: `cd apps/server && go test ./internal/config/`
Expected: FAIL — `undefined: Load`.

- [ ] **Step 7: Implement config**

`apps/server/internal/config/config.go`:
```go
// Package config loads server configuration from environment variables.
package config

import (
	"errors"
	"fmt"
	"strings"
)

type Config struct {
	DatabaseURL         string
	DatabaseURLUnpooled string
	GoogleClientID      string
	GoogleClientSecret  string
	SessionSecret       []byte
	PublicURL           string // no trailing slash
	AllowedDomains      []string
	AllowedEmails       []string
	AdminEmails         []string
	AutoMigrate         bool
	Port                string
}

// SecureCookies reports whether cookies should carry the Secure attribute.
func (c Config) SecureCookies() bool { return strings.HasPrefix(c.PublicURL, "https://") }

// MigrateURL is the connection string used for migrations. Session-level
// advisory locks do not work through a transaction pooler, so prefer the
// direct URL when one is configured.
func (c Config) MigrateURL() string {
	if c.DatabaseURLUnpooled != "" {
		return c.DatabaseURLUnpooled
	}
	return c.DatabaseURL
}

func Load(getenv func(string) string) (Config, error) {
	c := Config{
		DatabaseURL:         strings.TrimSpace(getenv("DATABASE_URL")),
		DatabaseURLUnpooled: strings.TrimSpace(getenv("DATABASE_URL_UNPOOLED")),
		GoogleClientID:      strings.TrimSpace(getenv("GOOGLE_CLIENT_ID")),
		GoogleClientSecret:  strings.TrimSpace(getenv("GOOGLE_CLIENT_SECRET")),
		SessionSecret:       []byte(getenv("SESSION_SECRET")),
		PublicURL:           strings.TrimRight(strings.TrimSpace(getenv("PUBLIC_URL")), "/"),
		AllowedDomains:      list(getenv("ALLOWED_DOMAINS")),
		AllowedEmails:       list(getenv("ALLOWED_EMAILS")),
		AdminEmails:         list(getenv("ADMIN_EMAILS")),
		AutoMigrate:         strings.TrimSpace(getenv("AUTO_MIGRATE")) == "true",
		Port:                strings.TrimSpace(getenv("PORT")),
	}
	if c.Port == "" {
		c.Port = "8080"
	}

	var missing []string
	for _, kv := range [][2]string{
		{"DATABASE_URL", c.DatabaseURL},
		{"GOOGLE_CLIENT_ID", c.GoogleClientID},
		{"GOOGLE_CLIENT_SECRET", c.GoogleClientSecret},
		{"SESSION_SECRET", string(c.SessionSecret)},
		{"PUBLIC_URL", c.PublicURL},
	} {
		if kv[1] == "" {
			missing = append(missing, kv[0])
		}
	}
	if len(missing) > 0 {
		return Config{}, fmt.Errorf("missing required env vars: %s", strings.Join(missing, ", "))
	}

	var errs []error
	if len(c.SessionSecret) < 32 {
		errs = append(errs, errors.New("SESSION_SECRET must be at least 32 bytes"))
	}
	if !strings.HasPrefix(c.PublicURL, "https://") && !strings.HasPrefix(c.PublicURL, "http://") {
		errs = append(errs, errors.New("PUBLIC_URL must start with http:// or https://"))
	}
	if len(c.AllowedDomains) == 0 && len(c.AllowedEmails) == 0 {
		errs = append(errs, errors.New("set ALLOWED_DOMAINS and/or ALLOWED_EMAILS; refusing to allow every Google account"))
	}
	if err := errors.Join(errs...); err != nil {
		return Config{}, err
	}
	return c, nil
}

// list splits a comma-separated env var into trimmed, lowercased, non-empty items.
func list(s string) []string {
	var out []string
	for _, part := range strings.Split(s, ",") {
		part = strings.ToLower(strings.TrimSpace(part))
		if part != "" {
			out = append(out, part)
		}
	}
	return out
}
```

`apps/server/.env.example`:
```bash
DATABASE_URL=postgres://postgres:postgres@localhost:54329/postgres?sslmode=disable
# DATABASE_URL_UNPOOLED=
GOOGLE_CLIENT_ID=
GOOGLE_CLIENT_SECRET=
SESSION_SECRET=change-me-to-32-or-more-random-bytes-please
PUBLIC_URL=http://localhost:5173
ALLOWED_DOMAINS=example.com
ALLOWED_EMAILS=
ADMIN_EMAILS=you@example.com
AUTO_MIGRATE=true
PORT=8080
```

- [ ] **Step 8: Run tests**

Run: `cd apps/server && go test ./internal/config/ && go vet ./...`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add .gitignore Makefile README.md api apps/server
git commit -m "feat(server): scaffold monorepo, OpenAPI contract, config loading

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Migrations, DB pool, test harness, migrate command

**Files:**
- Create: `apps/server/migrations/embed.go`, `00001_init.sql`, `00002_seed_prices.sql`
- Create: `apps/server/internal/db/db.go`, `apps/server/internal/db/db_test.go`
- Create: `apps/server/internal/testdb/testdb.go`
- Create: `apps/server/internal/store/store.go`
- Create: `apps/server/cmd/migrate/main.go`

**Interfaces:**
- Produces: `db.Open(ctx, url string) (*pgxpool.Pool, error)`, `db.Migrate(ctx, pool *pgxpool.Pool) error`, `db.MigrateURL(ctx, url string) error`; `testdb.New(t *testing.T) *pgxpool.Pool`; `store.Store`, `store.New(pool) *store.Store`, `store.ErrNotFound`, `store.ErrForbidden`.

- [ ] **Step 1: Add dependencies**

```bash
cd apps/server
go get github.com/jackc/pgx/v5 github.com/pressly/goose/v3
```

- [ ] **Step 2: Write the schema migration**

`apps/server/migrations/embed.go`:
```go
// Package migrations embeds the goose SQL migrations.
package migrations

import "embed"

//go:embed *.sql
var FS embed.FS
```

`apps/server/migrations/00001_init.sql`:
```sql
-- +goose Up
CREATE TABLE users (
  id          bigserial PRIMARY KEY,
  google_sub  text NOT NULL UNIQUE,
  email       text NOT NULL,
  name        text NOT NULL DEFAULT '',
  created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE api_tokens (
  id           bigserial PRIMARY KEY,
  user_id      bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  token_hash   bytea NOT NULL UNIQUE,
  device_name  text NOT NULL DEFAULT '',
  created_at   timestamptz NOT NULL DEFAULT now(),
  last_used_at timestamptz,
  revoked_at   timestamptz
);

CREATE TABLE auth_requests (
  state          text PRIMARY KEY,
  mode           text NOT NULL CHECK (mode IN ('loopback', 'paste', 'web')),
  cli_state      text NOT NULL DEFAULT '',
  cli_port       integer NOT NULL DEFAULT 0,
  code_challenge text NOT NULL DEFAULT '',
  expires_at     timestamptz NOT NULL
);

CREATE TABLE cli_auth_codes (
  code_hash      bytea PRIMARY KEY,
  user_id        bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  code_challenge text NOT NULL,
  expires_at     timestamptz NOT NULL,
  used_at        timestamptz
);

CREATE TABLE claude_accounts (
  id              bigserial PRIMARY KEY,
  account_uuid    text NOT NULL UNIQUE,
  org_uuid        text,
  email           text,
  org_type        text,
  rate_limit_tier text,
  label           text NOT NULL DEFAULT '',
  plan            text NOT NULL DEFAULT 'unknown'
                  CHECK (plan IN ('pro', 'max_5x', 'max_20x', 'team', 'enterprise', 'api_key', 'unknown')),
  created_at      timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE devices (
  id         bigserial PRIMARY KEY,
  user_id    bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  hostname   text NOT NULL,
  os         text NOT NULL DEFAULT '',
  first_seen timestamptz NOT NULL DEFAULT now(),
  last_seen  timestamptz NOT NULL DEFAULT now(),
  UNIQUE (user_id, hostname)
);

CREATE TABLE sessions (
  session_id text PRIMARY KEY,
  user_id    bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  device_id  bigint REFERENCES devices(id) ON DELETE SET NULL,
  account_id bigint REFERENCES claude_accounts(id) ON DELETE SET NULL,
  cwd        text,
  project    text,
  git_branch text,
  started_at timestamptz NOT NULL,
  ended_at   timestamptz NOT NULL,
  summary    text,
  summary_at timestamptz
);
CREATE INDEX sessions_user_started ON sessions (user_id, started_at DESC);

CREATE TABLE usage_entries (
  id             bigserial PRIMARY KEY,
  session_id     text NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
  user_id        bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  account_id     bigint NOT NULL REFERENCES claude_accounts(id),
  message_id     text NOT NULL UNIQUE,
  request_id     text NOT NULL DEFAULT '',
  model          text NOT NULL,
  ts             timestamptz NOT NULL,
  input          bigint NOT NULL DEFAULT 0,
  output         bigint NOT NULL DEFAULT 0,
  cache_write_5m bigint NOT NULL DEFAULT 0,
  cache_write_1h bigint NOT NULL DEFAULT 0,
  cache_read     bigint NOT NULL DEFAULT 0,
  speed          text NOT NULL DEFAULT 'standard',
  is_subagent    boolean NOT NULL DEFAULT false
);
CREATE INDEX usage_entries_ts ON usage_entries (ts);
CREATE INDEX usage_entries_user_ts ON usage_entries (user_id, ts);
CREATE INDEX usage_entries_account_ts ON usage_entries (account_id, ts);
CREATE INDEX usage_entries_session ON usage_entries (session_id);

CREATE TABLE model_prices (
  model               text PRIMARY KEY,
  input               double precision NOT NULL,
  cache_write_5m      double precision NOT NULL,
  cache_write_1h      double precision NOT NULL,
  cache_read          double precision NOT NULL,
  output              double precision NOT NULL,
  fast_input          double precision,
  fast_output         double precision,
  long_input          double precision,
  long_cache_write_5m double precision,
  long_cache_write_1h double precision,
  long_cache_read     double precision,
  long_output         double precision,
  updated_at          timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE limit_snapshots (
  id                  bigserial PRIMARY KEY,
  account_id          bigint NOT NULL REFERENCES claude_accounts(id) ON DELETE CASCADE,
  user_id             bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  captured_at         timestamptz NOT NULL,
  five_hour_pct       double precision,
  five_hour_resets_at timestamptz,
  seven_day_pct       double precision,
  seven_day_resets_at timestamptz,
  source              text NOT NULL DEFAULT 'claude_json',
  UNIQUE (account_id, captured_at)
);

-- +goose Down
DROP TABLE limit_snapshots, model_prices, usage_entries, sessions, devices,
  claude_accounts, cli_auth_codes, auth_requests, api_tokens, users;
```

`apps/server/migrations/00002_seed_prices.sql` (USD per MTok; model ids are already normalised):
```sql
-- +goose Up
INSERT INTO model_prices (model, input, cache_write_5m, cache_write_1h, cache_read, output,
                          fast_input, fast_output,
                          long_input, long_cache_write_5m, long_cache_write_1h, long_cache_read, long_output)
VALUES
  ('claude-fable-5-1',  10,   12.5,  20,   0.25, 50,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-fable-5',    10,   12.5,  20,   1.0,  50,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-5-5',   4,    5,     8,    0.20, 20,  8,    40,   NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-5',     5,    6.25,  10,   0.5,  25,  10,   50,   NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-4-8',   5,    6.25,  10,   0.5,  25,  10,   50,   NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-4-7',   5,    6.25,  10,   0.5,  25,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-4-6',   5,    6.25,  10,   0.5,  25,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-4-5',   5,    6.25,  10,   0.5,  25,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-4-1',   15,   18.75, 30,   1.5,  75,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-opus-4',     15,   18.75, 30,   1.5,  75,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-sonnet-5-5', 2,    2.5,   4,    0.10, 10,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-sonnet-5',   2,    2.5,   4,    0.2,  10,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-sonnet-4-6', 3,    3.75,  6,    0.3,  15,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-sonnet-4-5', 3,    3.75,  6,    0.3,  15,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-sonnet-4',   3,    3.75,  6,    0.3,  15,  NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-haiku-5-5',  0.10, 0.125, 0.20, 0.01, 0.50, NULL, NULL, 0.5, 0.625, 1.0, 0.05, 2.5),
  ('claude-haiku-4-5',  1,    1.25,  2,    0.1,  5,   NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-3-5-haiku',  0.8,  1,     1.6,  0.08, 4,   NULL, NULL, NULL, NULL, NULL, NULL, NULL),
  ('claude-3-7-sonnet', 3,    3.75,  6,    0.3,  15,  NULL, NULL, NULL, NULL, NULL, NULL, NULL);

-- +goose Down
DELETE FROM model_prices;
```

- [ ] **Step 3: Write the DB package and harness**

`apps/server/internal/db/db.go`:
```go
// Package db opens the Postgres pool and runs embedded migrations.
package db

import (
	"context"
	"fmt"

	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/jackc/pgx/v5/stdlib"
	"github.com/pressly/goose/v3"
	"github.com/pressly/goose/v3/database"
	"github.com/pressly/goose/v3/lock"

	"cctrack/server/migrations"
)

// Open creates a small pool suitable for serverless instances.
func Open(ctx context.Context, url string) (*pgxpool.Pool, error) {
	cfg, err := pgxpool.ParseConfig(url)
	if err != nil {
		return nil, fmt.Errorf("parse DATABASE_URL: %w", err)
	}
	cfg.MaxConns = 4
	return pgxpool.NewWithConfig(ctx, cfg)
}

// Migrate applies all pending migrations under a Postgres advisory lock.
func Migrate(ctx context.Context, pool *pgxpool.Pool) error {
	sqlDB := stdlib.OpenDBFromPool(pool)
	defer sqlDB.Close()
	locker, err := lock.NewPostgresSessionLocker()
	if err != nil {
		return err
	}
	provider, err := goose.NewProvider(database.DialectPostgres, sqlDB, migrations.FS,
		goose.WithSessionLocker(locker))
	if err != nil {
		return err
	}
	_, err = provider.Up(ctx)
	return err
}

// MigrateURL opens a temporary pool for url and migrates it.
func MigrateURL(ctx context.Context, url string) error {
	pool, err := Open(ctx, url)
	if err != nil {
		return err
	}
	defer pool.Close()
	return Migrate(ctx, pool)
}
```

`apps/server/internal/testdb/testdb.go`:
```go
// Package testdb gives each test an isolated, migrated Postgres schema.
package testdb

import (
	"context"
	"fmt"
	"math/rand/v2"
	"os"
	"testing"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"

	"cctrack/server/internal/db"
)

// New returns a pool whose search_path is a fresh schema with all migrations
// applied. It skips the test when TEST_DATABASE_URL is unset.
func New(t *testing.T) *pgxpool.Pool {
	t.Helper()
	url := os.Getenv("TEST_DATABASE_URL")
	if url == "" {
		t.Skip("TEST_DATABASE_URL not set")
	}
	ctx := context.Background()
	schema := fmt.Sprintf("t_%d_%d", time.Now().UnixNano(), rand.IntN(1_000_000))

	admin, err := pgx.Connect(ctx, url)
	if err != nil {
		t.Fatalf("connect: %v", err)
	}
	if _, err := admin.Exec(ctx, "CREATE SCHEMA "+schema); err != nil {
		t.Fatalf("create schema: %v", err)
	}

	cfg, err := pgxpool.ParseConfig(url)
	if err != nil {
		t.Fatal(err)
	}
	cfg.ConnConfig.RuntimeParams["search_path"] = schema
	cfg.MaxConns = 4
	pool, err := pgxpool.NewWithConfig(ctx, cfg)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		pool.Close()
		_, _ = admin.Exec(ctx, "DROP SCHEMA "+schema+" CASCADE")
		_ = admin.Close(ctx)
	})
	if err := db.Migrate(ctx, pool); err != nil {
		t.Fatalf("migrate: %v", err)
	}
	return pool
}
```

`apps/server/internal/store/store.go`:
```go
// Package store holds all SQL access.
package store

import (
	"errors"

	"github.com/jackc/pgx/v5/pgxpool"
)

var (
	ErrNotFound  = errors.New("not found")
	ErrForbidden = errors.New("forbidden")
)

type Store struct {
	pool *pgxpool.Pool
}

func New(pool *pgxpool.Pool) *Store { return &Store{pool: pool} }
```

`apps/server/cmd/migrate/main.go`:
```go
// Command migrate applies database migrations.
package main

import (
	"context"
	"log"
	"os"

	"cctrack/server/internal/db"
)

func main() {
	url := os.Getenv("DATABASE_URL_UNPOOLED")
	if url == "" {
		url = os.Getenv("DATABASE_URL")
	}
	if url == "" {
		log.Fatal("set DATABASE_URL (or DATABASE_URL_UNPOOLED)")
	}
	if err := db.MigrateURL(context.Background(), url); err != nil {
		log.Fatalf("migrate: %v", err)
	}
	log.Print("migrations applied")
}
```

- [ ] **Step 4: Write the migration test**

`apps/server/internal/db/db_test.go`:
```go
package db_test

import (
	"context"
	"testing"

	"cctrack/server/internal/db"
	"cctrack/server/internal/testdb"
)

func TestMigrateSeedsPricesAndIsIdempotent(t *testing.T) {
	pool := testdb.New(t)
	ctx := context.Background()
	var n int
	if err := pool.QueryRow(ctx, "SELECT count(*) FROM model_prices").Scan(&n); err != nil {
		t.Fatal(err)
	}
	if n != 19 {
		t.Fatalf("seeded %d prices, want 19", n)
	}
	if err := db.Migrate(ctx, pool); err != nil {
		t.Fatalf("second migrate: %v", err)
	}
}
```

- [ ] **Step 5: Run tests**

Run: `cd apps/server && go mod tidy && go vet ./... && go test ./internal/db/`
Expected: PASS when `TEST_DATABASE_URL` is set; `SKIP` otherwise (report which happened).

- [ ] **Step 6: Commit**

```bash
git add apps/server
git commit -m "feat(server): schema migrations, price seed, db pool and test harness

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Pricing (model normalisation + cost)

**Files:**
- Create: `apps/server/internal/pricing/normalize.go`, `pricing.go`, `pricing_test.go`

**Interfaces:**
- Produces:
  - `pricing.Normalize(model string) string`
  - `type Counts struct{ Input, Output, CacheWrite5m, CacheWrite1h, CacheRead int64 }` with `Total() int64`, `Add(Counts)`
  - `type Rates struct{ Input, CacheWrite5m, CacheWrite1h, CacheRead, Output float64 }`
  - `type Price struct{ Model string; Base Rates; FastInput, FastOutput *float64; Long *Rates; UpdatedAt time.Time }`
  - `pricing.NewTable([]Price) *Table`, `(*Table).Cost(model, speed string, longCtx bool, c Counts) (usd float64, ok bool)`
  - `const LongContextThreshold = 100_000`

- [ ] **Step 1: Write failing tests**

`apps/server/internal/pricing/pricing_test.go`:
```go
package pricing

import (
	"math"
	"testing"
)

func TestNormalize(t *testing.T) {
	cases := map[string]string{
		"claude-opus-4-5-20251101":                     "claude-opus-4-5",
		"claude-sonnet-4-5-20250929[1m]":               "claude-sonnet-4-5",
		"us.anthropic.claude-sonnet-4-5-20250929-v1:0": "claude-sonnet-4-5",
		"anthropic.claude-3-5-haiku-20241022-v1:0":     "claude-3-5-haiku",
		"claude-opus-4-1@20250805":                     "claude-opus-4-1",
		"claude-opus-5-5":                              "claude-opus-5-5",
		" Claude-Haiku-4-5 ":                           "claude-haiku-4-5",
		"global.anthropic.claude-opus-4-6-v1":          "claude-opus-4-6",
	}
	for in, want := range cases {
		if got := Normalize(in); got != want {
			t.Errorf("Normalize(%q) = %q, want %q", in, got, want)
		}
	}
}

func f(v float64) *float64 { return &v }

func table() *Table {
	return NewTable([]Price{
		{Model: "claude-opus-4-5", Base: Rates{Input: 5, CacheWrite5m: 6.25, CacheWrite1h: 10, CacheRead: 0.5, Output: 25}},
		{Model: "claude-opus-5-5", Base: Rates{Input: 4, CacheWrite5m: 5, CacheWrite1h: 8, CacheRead: 0.2, Output: 20},
			FastInput: f(8), FastOutput: f(40)},
		{Model: "claude-haiku-5-5", Base: Rates{Input: 0.1, CacheWrite5m: 0.125, CacheWrite1h: 0.2, CacheRead: 0.01, Output: 0.5},
			Long: &Rates{Input: 0.5, CacheWrite5m: 0.625, CacheWrite1h: 1, CacheRead: 0.05, Output: 2.5}},
	})
}

func near(a, b float64) bool { return math.Abs(a-b) < 1e-9 }

func TestCostStandard(t *testing.T) {
	c := Counts{Input: 1_000_000, Output: 200_000, CacheWrite5m: 100_000, CacheWrite1h: 50_000, CacheRead: 2_000_000}
	usd, ok := table().Cost("claude-opus-4-5-20251101", "standard", false, c)
	// 5 + 0.2*25 + 0.1*6.25 + 0.05*10 + 2*0.5 = 5 + 5 + 0.625 + 0.5 + 1 = 12.125
	if !ok || !near(usd, 12.125) {
		t.Fatalf("got %v %v, want 12.125 true", usd, ok)
	}
}

func TestCostFastScalesInputAndCacheAndUsesFastOutput(t *testing.T) {
	c := Counts{Input: 1_000_000, Output: 1_000_000, CacheRead: 1_000_000}
	usd, ok := table().Cost("claude-opus-5-5", "fast", false, c)
	// input 8 + output 40 + cache read 0.2*2 = 48.4
	if !ok || !near(usd, 48.4) {
		t.Fatalf("got %v, want 48.4", usd)
	}
}

func TestCostFastWithoutFastRatesUsesBase(t *testing.T) {
	usd, _ := table().Cost("claude-opus-4-5", "fast", false, Counts{Input: 1_000_000})
	if !near(usd, 5) {
		t.Fatalf("got %v, want 5", usd)
	}
}

func TestCostLongContextTier(t *testing.T) {
	c := Counts{Input: 1_000_000, Output: 1_000_000}
	short, _ := table().Cost("claude-haiku-5-5", "standard", false, c)
	long, _ := table().Cost("claude-haiku-5-5", "standard", true, c)
	if !near(short, 0.6) || !near(long, 3.0) {
		t.Fatalf("short=%v long=%v, want 0.6 and 3.0", short, long)
	}
	// Long flag on a model without long rates falls back to base.
	base, _ := table().Cost("claude-opus-4-5", "standard", true, Counts{Input: 1_000_000})
	if !near(base, 5) {
		t.Fatalf("got %v, want 5", base)
	}
}

func TestCostUnknownModel(t *testing.T) {
	usd, ok := table().Cost("gpt-9", "standard", false, Counts{Input: 10})
	if ok || usd != 0 {
		t.Fatalf("got %v %v, want 0 false", usd, ok)
	}
}

func TestCounts(t *testing.T) {
	a := Counts{Input: 1, Output: 2, CacheWrite5m: 3, CacheWrite1h: 4, CacheRead: 5}
	a.Add(a)
	if a.Total() != 30 {
		t.Fatalf("Total = %d, want 30", a.Total())
	}
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd apps/server && go test ./internal/pricing/`
Expected: FAIL — undefined symbols.

- [ ] **Step 3: Implement**

`apps/server/internal/pricing/normalize.go`:
```go
package pricing

import (
	"regexp"
	"strings"
)

var (
	bracketSuffix = regexp.MustCompile(`\[[^\]]*\]$`)
	regionPrefix  = regexp.MustCompile(`^(us|eu|apac|global|jp|au)\.`)
	bedrockVer    = regexp.MustCompile(`-v\d+(:\d+)?$`)
	dateSuffix    = regexp.MustCompile(`-\d{8}$`)
)

// Normalize maps provider-specific model ids (dated, Bedrock, Vertex, [1m])
// to the canonical id used as the price-table key.
func Normalize(model string) string {
	m := strings.ToLower(strings.TrimSpace(model))
	m = bracketSuffix.ReplaceAllString(m, "")
	if i := strings.IndexByte(m, '@'); i >= 0 {
		m = m[:i]
	}
	m = regionPrefix.ReplaceAllString(m, "")
	m = strings.TrimPrefix(m, "anthropic.")
	m = bedrockVer.ReplaceAllString(m, "")
	m = dateSuffix.ReplaceAllString(m, "")
	return m
}
```

`apps/server/internal/pricing/pricing.go`:
```go
// Package pricing estimates API-equivalent cost from token counts.
package pricing

import "time"

// LongContextThreshold is the prompt size (input + cache tokens) above which
// long-context rates apply, for models that define them.
const LongContextThreshold = 100_000

const perMTok = 1_000_000.0

type Counts struct {
	Input, Output, CacheWrite5m, CacheWrite1h, CacheRead int64
}

func (c Counts) Total() int64 {
	return c.Input + c.Output + c.CacheWrite5m + c.CacheWrite1h + c.CacheRead
}

func (c *Counts) Add(o Counts) {
	c.Input += o.Input
	c.Output += o.Output
	c.CacheWrite5m += o.CacheWrite5m
	c.CacheWrite1h += o.CacheWrite1h
	c.CacheRead += o.CacheRead
}

// Rates are USD per million tokens.
type Rates struct {
	Input, CacheWrite5m, CacheWrite1h, CacheRead, Output float64
}

type Price struct {
	Model      string
	Base       Rates
	FastInput  *float64
	FastOutput *float64
	Long       *Rates
	UpdatedAt  time.Time
}

type Table struct {
	prices map[string]Price
}

func NewTable(prices []Price) *Table {
	t := &Table{prices: make(map[string]Price, len(prices))}
	for _, p := range prices {
		t.prices[Normalize(p.Model)] = p
	}
	return t
}

// Cost returns the estimated USD cost. ok is false when the model has no price.
// Fast mode scales input and cache rates by FastInput/Base.Input and uses
// FastOutput for output tokens.
func (t *Table) Cost(model, speed string, longCtx bool, c Counts) (float64, bool) {
	p, ok := t.prices[Normalize(model)]
	if !ok {
		return 0, false
	}
	r := p.Base
	if longCtx && p.Long != nil {
		r = *p.Long
	}
	if speed == "fast" && p.FastInput != nil && p.FastOutput != nil && p.Base.Input > 0 {
		m := *p.FastInput / p.Base.Input
		r = Rates{
			Input:        r.Input * m,
			CacheWrite5m: r.CacheWrite5m * m,
			CacheWrite1h: r.CacheWrite1h * m,
			CacheRead:    r.CacheRead * m,
			Output:       *p.FastOutput,
		}
	}
	usd := float64(c.Input)*r.Input +
		float64(c.CacheWrite5m)*r.CacheWrite5m +
		float64(c.CacheWrite1h)*r.CacheWrite1h +
		float64(c.CacheRead)*r.CacheRead +
		float64(c.Output)*r.Output
	return usd / perMTok, true
}
```

- [ ] **Step 4: Run tests**

Run: `cd apps/server && go test ./internal/pricing/`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/server/internal/pricing
git commit -m "feat(server): model id normalisation and cost estimation

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Auth primitives (allowlist, tokens, PKCE, session cookie, Google)

**Files:**
- Create: `apps/server/internal/auth/identity.go`, `allowlist.go`, `tokens.go`, `pkce.go`, `session.go`, `google.go`, `auth_test.go`

**Interfaces:**
- Produces:
  - `type Identity struct{ Subject, Email, Name, HostedDomain string; EmailVerified bool }`
  - `type IdentityProvider interface { AuthCodeURL(state, redirectURL string) string; Exchange(ctx context.Context, code, redirectURL string) (Identity, error) }`
  - `auth.NewAllowlist(domains, emails []string) Allowlist`, `(Allowlist).Allows(Identity) bool`
  - `auth.TokenPrefix = "cct_"`, `auth.NewToken() (plain string, hash []byte)`, `auth.HashToken(string) []byte`, `auth.RandomString(nBytes int) string`
  - `auth.VerifyPKCE(verifier, challenge string) bool`
  - `auth.NewSessionCodec(secret []byte) SessionCodec`, `Encode(userID int64, exp time.Time) string`, `Decode(value string, now time.Time) (int64, error)`
  - `auth.NewGoogle(ctx, clientID, clientSecret string) *Google` (implements `IdentityProvider`)

- [ ] **Step 1: Add dependencies**

```bash
cd apps/server
go get golang.org/x/oauth2 github.com/coreos/go-oidc/v3
```

- [ ] **Step 2: Write failing tests**

`apps/server/internal/auth/auth_test.go`:
```go
package auth

import (
	"strings"
	"testing"
	"time"
)

func TestAllowlist(t *testing.T) {
	a := NewAllowlist([]string{"example.com"}, []string{"solo@gmail.com"})
	cases := []struct {
		id   Identity
		want bool
	}{
		{Identity{Email: "a@example.com", HostedDomain: "example.com", EmailVerified: true}, true},
		{Identity{Email: "a@example.com", HostedDomain: "", EmailVerified: true}, false}, // consumer account spoofing a domain
		{Identity{Email: "Solo@Gmail.com", EmailVerified: true}, true},
		{Identity{Email: "solo@gmail.com", EmailVerified: false}, false},
		{Identity{Email: "x@other.com", HostedDomain: "other.com", EmailVerified: true}, false},
		{Identity{Email: "a@EXAMPLE.com", HostedDomain: "Example.com", EmailVerified: true}, true},
	}
	for i, c := range cases {
		if got := a.Allows(c.id); got != c.want {
			t.Errorf("case %d: Allows(%+v) = %v, want %v", i, c.id, got, c.want)
		}
	}
}

func TestNewTokenAndHash(t *testing.T) {
	plain, hash := NewToken()
	if !strings.HasPrefix(plain, TokenPrefix) || len(plain) < 40 {
		t.Fatalf("bad token %q", plain)
	}
	if string(HashToken(plain)) != string(hash) || len(hash) != 32 {
		t.Fatal("hash mismatch")
	}
	other, _ := NewToken()
	if other == plain {
		t.Fatal("tokens must be random")
	}
}

func TestVerifyPKCE(t *testing.T) {
	// RFC 7636 Appendix B
	verifier := "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
	challenge := "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
	if !VerifyPKCE(verifier, challenge) {
		t.Fatal("RFC vector should verify")
	}
	if VerifyPKCE(verifier+"x", challenge) {
		t.Fatal("wrong verifier must fail")
	}
	if VerifyPKCE("short", challenge) {
		t.Fatal("too-short verifier must fail")
	}
}

func TestSessionCodec(t *testing.T) {
	c := NewSessionCodec([]byte(strings.Repeat("k", 32)))
	now := time.Unix(1_800_000_000, 0)
	v := c.Encode(42, now.Add(time.Hour))
	uid, err := c.Decode(v, now)
	if err != nil || uid != 42 {
		t.Fatalf("Decode = %d, %v", uid, err)
	}
	if _, err := c.Decode(v, now.Add(2*time.Hour)); err == nil {
		t.Fatal("expired cookie must fail")
	}
	if _, err := c.Decode(strings.Replace(v, "42.", "43.", 1), now); err == nil {
		t.Fatal("tampered cookie must fail")
	}
	other := NewSessionCodec([]byte(strings.Repeat("z", 32)))
	if _, err := other.Decode(v, now); err == nil {
		t.Fatal("cookie signed with another secret must fail")
	}
	if _, err := c.Decode("garbage", now); err == nil {
		t.Fatal("garbage must fail")
	}
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cd apps/server && go test ./internal/auth/`
Expected: FAIL — undefined symbols.

- [ ] **Step 4: Implement**

`apps/server/internal/auth/identity.go`:
```go
// Package auth contains authentication primitives: Google identity, allowlist,
// API tokens, PKCE and signed session cookies.
package auth

import "context"

type Identity struct {
	Subject       string
	Email         string
	Name          string
	HostedDomain  string // Google "hd" claim; empty for consumer accounts
	EmailVerified bool
}

type IdentityProvider interface {
	AuthCodeURL(state, redirectURL string) string
	Exchange(ctx context.Context, code, redirectURL string) (Identity, error)
}
```

`apps/server/internal/auth/allowlist.go`:
```go
package auth

import "strings"

type Allowlist struct {
	domains map[string]bool
	emails  map[string]bool
}

func NewAllowlist(domains, emails []string) Allowlist {
	a := Allowlist{domains: map[string]bool{}, emails: map[string]bool{}}
	for _, d := range domains {
		a.domains[strings.ToLower(strings.TrimPrefix(d, "@"))] = true
	}
	for _, e := range emails {
		a.emails[strings.ToLower(e)] = true
	}
	return a
}

// Allows accepts verified emails listed explicitly, or Google Workspace
// accounts whose hd claim is an allowed domain. The email's domain alone is
// not trusted: consumer accounts can use any address.
func (a Allowlist) Allows(id Identity) bool {
	if !id.EmailVerified {
		return false
	}
	if a.emails[strings.ToLower(id.Email)] {
		return true
	}
	hd := strings.ToLower(id.HostedDomain)
	return hd != "" && a.domains[hd]
}
```

`apps/server/internal/auth/tokens.go`:
```go
package auth

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
)

const TokenPrefix = "cct_"

// RandomString returns nBytes of crypto randomness, base64url-encoded.
func RandomString(nBytes int) string {
	b := make([]byte, nBytes)
	_, _ = rand.Read(b) // never fails on supported platforms
	return base64.RawURLEncoding.EncodeToString(b)
}

// NewToken returns a new API token and its sha256 hash for storage.
func NewToken() (plain string, hash []byte) {
	plain = TokenPrefix + RandomString(32)
	return plain, HashToken(plain)
}

func HashToken(s string) []byte {
	h := sha256.Sum256([]byte(s))
	return h[:]
}
```

`apps/server/internal/auth/pkce.go`:
```go
package auth

import (
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
)

// VerifyPKCE checks an S256 code_verifier against its code_challenge (RFC 7636).
func VerifyPKCE(verifier, challenge string) bool {
	if len(verifier) < 43 || len(verifier) > 128 {
		return false
	}
	sum := sha256.Sum256([]byte(verifier))
	got := base64.RawURLEncoding.EncodeToString(sum[:])
	return subtle.ConstantTimeCompare([]byte(got), []byte(challenge)) == 1
}
```

`apps/server/internal/auth/session.go`:
```go
package auth

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"time"
)

var ErrBadSession = errors.New("invalid session")

// SessionCodec signs stateless session cookies: "<userID>.<expUnix>.<hmac>".
type SessionCodec struct {
	secret []byte
}

func NewSessionCodec(secret []byte) SessionCodec { return SessionCodec{secret: secret} }

func (c SessionCodec) sign(payload string) string {
	m := hmac.New(sha256.New, c.secret)
	m.Write([]byte(payload))
	return base64.RawURLEncoding.EncodeToString(m.Sum(nil))
}

func (c SessionCodec) Encode(userID int64, exp time.Time) string {
	payload := fmt.Sprintf("%d.%d", userID, exp.Unix())
	return payload + "." + c.sign(payload)
}

func (c SessionCodec) Decode(value string, now time.Time) (int64, error) {
	parts := strings.Split(value, ".")
	if len(parts) != 3 {
		return 0, ErrBadSession
	}
	payload := parts[0] + "." + parts[1]
	if !hmac.Equal([]byte(c.sign(payload)), []byte(parts[2])) {
		return 0, ErrBadSession
	}
	uid, err1 := strconv.ParseInt(parts[0], 10, 64)
	exp, err2 := strconv.ParseInt(parts[1], 10, 64)
	if err1 != nil || err2 != nil || now.Unix() >= exp {
		return 0, ErrBadSession
	}
	return uid, nil
}
```

`apps/server/internal/auth/google.go`:
```go
package auth

import (
	"context"
	"errors"
	"fmt"

	"github.com/coreos/go-oidc/v3/oidc"
	"golang.org/x/oauth2"
)

const googleIssuer = "https://accounts.google.com"

// Google implements IdentityProvider with Google's OAuth 2.0 web flow.
type Google struct {
	clientID     string
	clientSecret string
	verifier     *oidc.IDTokenVerifier
}

// NewGoogle does no network I/O; signing keys are fetched lazily on first verify.
func NewGoogle(ctx context.Context, clientID, clientSecret string) *Google {
	keys := oidc.NewRemoteKeySet(ctx, "https://www.googleapis.com/oauth2/v3/certs")
	return &Google{
		clientID:     clientID,
		clientSecret: clientSecret,
		verifier:     oidc.NewVerifier(googleIssuer, keys, &oidc.Config{ClientID: clientID}),
	}
}

func (g *Google) config(redirectURL string) *oauth2.Config {
	return &oauth2.Config{
		ClientID:     g.clientID,
		ClientSecret: g.clientSecret,
		RedirectURL:  redirectURL,
		Scopes:       []string{"openid", "email", "profile"},
		Endpoint: oauth2.Endpoint{
			AuthURL:   "https://accounts.google.com/o/oauth2/v2/auth",
			TokenURL:  "https://oauth2.googleapis.com/token",
			AuthStyle: oauth2.AuthStyleInParams,
		},
	}
}

func (g *Google) AuthCodeURL(state, redirectURL string) string {
	return g.config(redirectURL).AuthCodeURL(state, oauth2.SetAuthURLParam("prompt", "select_account"))
}

func (g *Google) Exchange(ctx context.Context, code, redirectURL string) (Identity, error) {
	tok, err := g.config(redirectURL).Exchange(ctx, code)
	if err != nil {
		return Identity{}, fmt.Errorf("exchange code: %w", err)
	}
	raw, ok := tok.Extra("id_token").(string)
	if !ok || raw == "" {
		return Identity{}, errors.New("no id_token in token response")
	}
	idt, err := g.verifier.Verify(ctx, raw)
	if err != nil {
		return Identity{}, fmt.Errorf("verify id_token: %w", err)
	}
	var claims struct {
		Sub           string `json:"sub"`
		Email         string `json:"email"`
		EmailVerified bool   `json:"email_verified"`
		Name          string `json:"name"`
		HD            string `json:"hd"`
	}
	if err := idt.Claims(&claims); err != nil {
		return Identity{}, err
	}
	return Identity{
		Subject:       claims.Sub,
		Email:         claims.Email,
		Name:          claims.Name,
		HostedDomain:  claims.HD,
		EmailVerified: claims.EmailVerified,
	}, nil
}
```

- [ ] **Step 5: Run tests**

Run: `cd apps/server && go mod tidy && go vet ./... && go test ./internal/auth/`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/server
git commit -m "feat(server): auth primitives (allowlist, tokens, PKCE, session cookie, Google)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: HTTP server skeleton, auth flows, main

**Files:**
- Create: `apps/server/internal/store/users.go`
- Create: `apps/server/internal/httpapi/server.go`, `respond.go`, `middleware.go`, `stubs.go`, `html.go`, `auth_handlers.go`
- Create: `apps/server/internal/httpapi/helpers_test.go`, `auth_test.go`
- Create: `apps/server/cmd/server/main.go`

**Interfaces:**
- Consumes: `config.Config`, `store.Store`, `auth.*`, generated `api.ServerInterface`.
- Produces:
  - `store.User{ID int64; GoogleSub, Email, Name string; CreatedAt time.Time}`
  - `(*Store).UpsertUser(ctx, sub, email, name string) (User, error)`, `UserByID(ctx, id int64) (User, error)`, `UserByToken(ctx, hash []byte) (User, int64, error)`, `CreateToken(ctx, userID int64, hash []byte, device string) (int64, error)`, `RevokeToken(ctx, id int64) error`
  - `store.AuthRequest{State, Mode, CLIState string; CLIPort int; CodeChallenge string; ExpiresAt time.Time}`, `CreateAuthRequest(ctx, AuthRequest) error`, `TakeAuthRequest(ctx, state string) (AuthRequest, error)`, `CreateCLICode(ctx, hash []byte, userID int64, challenge string, exp time.Time) error`, `ConsumeCLICode(ctx, hash []byte) (userID int64, challenge string, err error)`, `PurgeExpired(ctx) error`
  - `httpapi.New(cfg config.Config, st *store.Store, idp auth.IdentityProvider) *Server`, `(*Server).Handler() http.Handler`
  - helpers used by later tasks: `writeJSON(w, status, v)`, `writeError(w, status, msg)`, `decodeJSON(w, r, dst) bool`, `(*Server).requireUser(w, r) (principal, bool)`, `(*Server).requireAdmin(w, r) (principal, bool)`, `(*Server).internal(w, err)`, `type principal struct{ User store.User; TokenID int64 }`, `(*Server).isAdmin(email string) bool`
  - test helpers (package `httpapi`): `newHarness(t) *harness`, `(*harness).do(method, path string, body any, mods ...func(*http.Request)) *httptest.ResponseRecorder`, `bearer(token string)`, `withCookie(c *http.Cookie)`, `withCSRF()`, `(*harness).loginCLI(email string) string`, `(*harness).loginWeb(email string) *http.Cookie`, `decode[T any](t, rec) T`

- [ ] **Step 1: Store — users, tokens, auth requests**

`apps/server/internal/store/users.go`:
```go
package store

import (
	"context"
	"errors"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
)

type User struct {
	ID        int64
	GoogleSub string
	Email     string
	Name      string
	CreatedAt time.Time
}

const userCols = "id, google_sub, email, name, created_at"

func scanUser(row pgx.Row) (User, error) {
	var u User
	err := row.Scan(&u.ID, &u.GoogleSub, &u.Email, &u.Name, &u.CreatedAt)
	if errors.Is(err, pgx.ErrNoRows) {
		return u, ErrNotFound
	}
	return u, err
}

func (s *Store) UpsertUser(ctx context.Context, sub, email, name string) (User, error) {
	return scanUser(s.pool.QueryRow(ctx, `
		INSERT INTO users (google_sub, email, name) VALUES ($1, $2, $3)
		ON CONFLICT (google_sub) DO UPDATE SET email = EXCLUDED.email, name = EXCLUDED.name
		RETURNING `+userCols, sub, strings.ToLower(email), name))
}

func (s *Store) UserByID(ctx context.Context, id int64) (User, error) {
	return scanUser(s.pool.QueryRow(ctx, `SELECT `+userCols+` FROM users WHERE id = $1`, id))
}

// UserByToken resolves a non-revoked token hash, touching last_used_at.
func (s *Store) UserByToken(ctx context.Context, hash []byte) (User, int64, error) {
	var u User
	var tokenID int64
	err := s.pool.QueryRow(ctx, `
		UPDATE api_tokens t SET last_used_at = now()
		FROM users u
		WHERE t.token_hash = $1 AND t.revoked_at IS NULL AND u.id = t.user_id
		RETURNING t.id, u.id, u.google_sub, u.email, u.name, u.created_at`, hash).
		Scan(&tokenID, &u.ID, &u.GoogleSub, &u.Email, &u.Name, &u.CreatedAt)
	if errors.Is(err, pgx.ErrNoRows) {
		return User{}, 0, ErrNotFound
	}
	return u, tokenID, err
}

func (s *Store) CreateToken(ctx context.Context, userID int64, hash []byte, device string) (int64, error) {
	var id int64
	err := s.pool.QueryRow(ctx,
		`INSERT INTO api_tokens (user_id, token_hash, device_name) VALUES ($1, $2, $3) RETURNING id`,
		userID, hash, device).Scan(&id)
	return id, err
}

func (s *Store) RevokeToken(ctx context.Context, id int64) error {
	tag, err := s.pool.Exec(ctx,
		`UPDATE api_tokens SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL`, id)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

type AuthRequest struct {
	State         string
	Mode          string // loopback | paste | web
	CLIState      string
	CLIPort       int
	CodeChallenge string
	ExpiresAt     time.Time
}

func (s *Store) CreateAuthRequest(ctx context.Context, r AuthRequest) error {
	_, err := s.pool.Exec(ctx, `
		INSERT INTO auth_requests (state, mode, cli_state, cli_port, code_challenge, expires_at)
		VALUES ($1, $2, $3, $4, $5, $6)`,
		r.State, r.Mode, r.CLIState, r.CLIPort, r.CodeChallenge, r.ExpiresAt)
	return err
}

// TakeAuthRequest deletes and returns an unexpired auth request (single use).
func (s *Store) TakeAuthRequest(ctx context.Context, state string) (AuthRequest, error) {
	var r AuthRequest
	err := s.pool.QueryRow(ctx, `
		DELETE FROM auth_requests WHERE state = $1 AND expires_at > now()
		RETURNING state, mode, cli_state, cli_port, code_challenge, expires_at`, state).
		Scan(&r.State, &r.Mode, &r.CLIState, &r.CLIPort, &r.CodeChallenge, &r.ExpiresAt)
	if errors.Is(err, pgx.ErrNoRows) {
		return r, ErrNotFound
	}
	return r, err
}

func (s *Store) CreateCLICode(ctx context.Context, hash []byte, userID int64, challenge string, exp time.Time) error {
	_, err := s.pool.Exec(ctx, `
		INSERT INTO cli_auth_codes (code_hash, user_id, code_challenge, expires_at)
		VALUES ($1, $2, $3, $4)`, hash, userID, challenge, exp)
	return err
}

// ConsumeCLICode marks an unexpired, unused code as used and returns its owner.
// The code is burned even if the caller's PKCE check later fails.
func (s *Store) ConsumeCLICode(ctx context.Context, hash []byte) (int64, string, error) {
	var userID int64
	var challenge string
	err := s.pool.QueryRow(ctx, `
		UPDATE cli_auth_codes SET used_at = now()
		WHERE code_hash = $1 AND used_at IS NULL AND expires_at > now()
		RETURNING user_id, code_challenge`, hash).Scan(&userID, &challenge)
	if errors.Is(err, pgx.ErrNoRows) {
		return 0, "", ErrNotFound
	}
	return userID, challenge, err
}

func (s *Store) PurgeExpired(ctx context.Context) error {
	_, err := s.pool.Exec(ctx, `
		DELETE FROM auth_requests WHERE expires_at < now();
		DELETE FROM cli_auth_codes WHERE expires_at < now() - interval '1 day';`)
	return err
}
```

Note: `PurgeExpired` sends two statements in one Exec; pgx uses the simple protocol for argument-less multi-statement Exec, which supports this. If it errors, split into two `Exec` calls.

- [ ] **Step 2: HTTP plumbing**

`apps/server/internal/httpapi/respond.go`:
```go
package httpapi

import (
	"encoding/json"
	"errors"
	"log"
	"net/http"
)

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func writeError(w http.ResponseWriter, status int, msg string) {
	writeJSON(w, status, map[string]string{"error": msg})
}

// decodeJSON decodes the request body into dst, writing a 400/413 on failure.
func decodeJSON(w http.ResponseWriter, r *http.Request, dst any) bool {
	if err := json.NewDecoder(r.Body).Decode(dst); err != nil {
		var tooBig *http.MaxBytesError
		if errors.As(err, &tooBig) {
			writeError(w, http.StatusRequestEntityTooLarge, "request body too large")
			return false
		}
		writeError(w, http.StatusBadRequest, "invalid JSON: "+err.Error())
		return false
	}
	return true
}

func (s *Server) internal(w http.ResponseWriter, err error) {
	log.Printf("internal error: %v", err)
	writeError(w, http.StatusInternalServerError, "internal error")
}
```

`apps/server/internal/httpapi/middleware.go`:
```go
package httpapi

import (
	"context"
	"errors"
	"net/http"
	"strings"

	"cctrack/server/internal/auth"
	"cctrack/server/internal/store"
)

const (
	sessionCookie = "cctrack_session"
	stateCookie   = "cctrack_oauth_state"
	csrfHeader    = "X-CCTrack"
)

type principal struct {
	User    store.User
	TokenID int64 // non-zero when authenticated by bearer token
}

type ctxKey struct{}

func principalFrom(r *http.Request) (principal, bool) {
	p, ok := r.Context().Value(ctxKey{}).(principal)
	return p, ok
}

func limitBody(next http.Handler, n int64) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		r.Body = http.MaxBytesReader(w, r.Body, n)
		next.ServeHTTP(w, r)
	})
}

// authenticate resolves a bearer token or session cookie into a principal.
// An invalid bearer token is rejected immediately with 401; an invalid cookie
// is ignored (treated as anonymous).
func (s *Server) authenticate(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if h := r.Header.Get("Authorization"); h != "" {
			tok, ok := strings.CutPrefix(h, "Bearer ")
			if !ok || !strings.HasPrefix(tok, auth.TokenPrefix) {
				writeError(w, http.StatusUnauthorized, "invalid token")
				return
			}
			u, tokenID, err := s.store.UserByToken(r.Context(), auth.HashToken(tok))
			if errors.Is(err, store.ErrNotFound) {
				writeError(w, http.StatusUnauthorized, "invalid or revoked token")
				return
			}
			if err != nil {
				s.internal(w, err)
				return
			}
			r = r.WithContext(context.WithValue(r.Context(), ctxKey{}, principal{User: u, TokenID: tokenID}))
		} else if c, err := r.Cookie(sessionCookie); err == nil {
			if uid, err := s.sessions.Decode(c.Value, s.now()); err == nil {
				if u, err := s.store.UserByID(r.Context(), uid); err == nil {
					r = r.WithContext(context.WithValue(r.Context(), ctxKey{}, principal{User: u}))
				}
			}
		}
		next.ServeHTTP(w, r)
	})
}

// csrf requires a custom header on cookie-authenticated state-changing
// requests; browsers cannot send it cross-site without a CORS preflight,
// which this server never grants.
func (s *Server) csrf(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodGet, http.MethodHead, http.MethodOptions:
		default:
			if _, err := r.Cookie(sessionCookie); err == nil && r.Header.Get("Authorization") == "" &&
				r.Header.Get(csrfHeader) != "1" {
				writeError(w, http.StatusForbidden, "missing "+csrfHeader+" header")
				return
			}
		}
		next.ServeHTTP(w, r)
	})
}

func (s *Server) requireUser(w http.ResponseWriter, r *http.Request) (principal, bool) {
	p, ok := principalFrom(r)
	if !ok {
		writeError(w, http.StatusUnauthorized, "not logged in")
	}
	return p, ok
}

func (s *Server) requireAdmin(w http.ResponseWriter, r *http.Request) (principal, bool) {
	p, ok := s.requireUser(w, r)
	if !ok {
		return p, false
	}
	if !s.isAdmin(p.User.Email) {
		writeError(w, http.StatusForbidden, "admin only")
		return p, false
	}
	return p, true
}
```

`apps/server/internal/httpapi/server.go`:
```go
// Package httpapi implements the cctrack HTTP API.
package httpapi

import (
	"net/http"
	"strings"
	"time"

	"cctrack/server/internal/api"
	"cctrack/server/internal/auth"
	"cctrack/server/internal/config"
	"cctrack/server/internal/store"
)

type Server struct {
	cfg      config.Config
	store    *store.Store
	idp      auth.IdentityProvider
	allow    auth.Allowlist
	sessions auth.SessionCodec
	admins   map[string]bool
	now      func() time.Time
}

var _ api.ServerInterface = (*Server)(nil)

func New(cfg config.Config, st *store.Store, idp auth.IdentityProvider) *Server {
	admins := map[string]bool{}
	for _, e := range cfg.AdminEmails {
		admins[strings.ToLower(e)] = true
	}
	return &Server{
		cfg:      cfg,
		store:    st,
		idp:      idp,
		allow:    auth.NewAllowlist(cfg.AllowedDomains, cfg.AllowedEmails),
		sessions: auth.NewSessionCodec(cfg.SessionSecret),
		admins:   admins,
		now:      time.Now,
	}
}

func (s *Server) isAdmin(email string) bool { return s.admins[strings.ToLower(email)] }

func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	api.HandlerWithOptions(s, api.StdHTTPServerOptions{
		BaseURL:    "/api",
		BaseRouter: mux,
		ErrorHandlerFunc: func(w http.ResponseWriter, r *http.Request, err error) {
			writeError(w, http.StatusBadRequest, err.Error())
		},
	})
	var h http.Handler = mux
	h = s.csrf(h)
	h = s.authenticate(h)
	h = limitBody(h, 1<<20)
	return h
}

func (s *Server) GetHealth(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, api.Health{Status: "ok"})
}
```

`apps/server/internal/httpapi/stubs.go` — every operation not implemented yet returns 501. Later tasks delete the stubs they implement. Copy exact signatures from `ServerInterface` in `api.gen.go` if any differ from these:
```go
package httpapi

import (
	"net/http"

	"cctrack/server/internal/api"
)

func notImplemented(w http.ResponseWriter) { writeError(w, http.StatusNotImplemented, "not implemented") }

// Task 6
func (s *Server) Ingest(w http.ResponseWriter, r *http.Request) { notImplemented(w) }
func (s *Server) SetSessionSummary(w http.ResponseWriter, r *http.Request, sessionId string) {
	notImplemented(w)
}

// Task 7
func (s *Server) GetMyUsage(w http.ResponseWriter, r *http.Request, params api.GetMyUsageParams) {
	notImplemented(w)
}
func (s *Server) ListMySessions(w http.ResponseWriter, r *http.Request, params api.ListMySessionsParams) {
	notImplemented(w)
}

// Task 8
func (s *Server) AdminByLogin(w http.ResponseWriter, r *http.Request, params api.AdminByLoginParams) {
	notImplemented(w)
}
func (s *Server) AdminByAccount(w http.ResponseWriter, r *http.Request, params api.AdminByAccountParams) {
	notImplemented(w)
}
func (s *Server) AdminListAccounts(w http.ResponseWriter, r *http.Request) { notImplemented(w) }
func (s *Server) AdminUpdateAccount(w http.ResponseWriter, r *http.Request, id int64) {
	notImplemented(w)
}
func (s *Server) AdminListUsers(w http.ResponseWriter, r *http.Request) { notImplemented(w) }
func (s *Server) AdminListUserSessions(w http.ResponseWriter, r *http.Request, id int64, params api.AdminListUserSessionsParams) {
	notImplemented(w)
}
func (s *Server) AdminListTokens(w http.ResponseWriter, r *http.Request)          { notImplemented(w) }
func (s *Server) AdminRevokeToken(w http.ResponseWriter, r *http.Request, id int64) { notImplemented(w) }
func (s *Server) AdminListPrices(w http.ResponseWriter, r *http.Request)          { notImplemented(w) }
func (s *Server) AdminPutPrice(w http.ResponseWriter, r *http.Request, model string) {
	notImplemented(w)
}
```

`apps/server/internal/httpapi/html.go`:
```go
package httpapi

import (
	"html/template"
	"net/http"
)

var pageTmpl = template.Must(template.New("page").Parse(`<!doctype html>
<html><head><meta charset="utf-8"><title>cctrack — {{.Title}}</title>
<style>body{font-family:system-ui,sans-serif;max-width:36rem;margin:4rem auto;padding:0 1rem;line-height:1.5}
code{display:block;font-size:1.4rem;padding:1rem;background:#f3f3f3;border-radius:8px;word-break:break-all;user-select:all}</style>
</head><body><h1>{{.Title}}</h1><p>{{.Message}}</p>{{if .Code}}<code>{{.Code}}</code>{{end}}</body></html>`))

func htmlPage(w http.ResponseWriter, status int, title, message, code string) {
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = pageTmpl.Execute(w, struct{ Title, Message, Code string }{title, message, code})
}
```

- [ ] **Step 3: Auth handlers**

`apps/server/internal/httpapi/auth_handlers.go`:
```go
package httpapi

import (
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"regexp"
	"strings"
	"time"

	"cctrack/server/internal/api"
	"cctrack/server/internal/auth"
	"cctrack/server/internal/store"
)

const (
	authRequestTTL = 10 * time.Minute
	cliCodeTTL     = 60 * time.Second
	sessionTTL     = 30 * 24 * time.Hour
)

var b64url = regexp.MustCompile(`^[A-Za-z0-9_-]+$`)

func (s *Server) redirectURL() string { return s.cfg.PublicURL + "/api/auth/google/callback" }

func (s *Server) CliStart(w http.ResponseWriter, r *http.Request, params api.CliStartParams) {
	mode := "loopback"
	if params.Mode != nil {
		mode = string(*params.Mode)
	}
	port := 0
	switch mode {
	case "loopback":
		if params.Port == nil || *params.Port < 1024 || *params.Port > 65535 {
			writeError(w, http.StatusBadRequest, "port must be 1024-65535 in loopback mode")
			return
		}
		port = *params.Port
	case "paste":
	default:
		writeError(w, http.StatusBadRequest, "mode must be loopback or paste")
		return
	}
	if len(params.State) < 16 || len(params.State) > 128 || !b64url.MatchString(params.State) {
		writeError(w, http.StatusBadRequest, "state must be 16-128 base64url characters")
		return
	}
	if len(params.CodeChallenge) < 43 || len(params.CodeChallenge) > 128 || !b64url.MatchString(params.CodeChallenge) {
		writeError(w, http.StatusBadRequest, "code_challenge must be 43-128 base64url characters")
		return
	}
	_ = s.store.PurgeExpired(r.Context())
	state := auth.RandomString(32)
	err := s.store.CreateAuthRequest(r.Context(), store.AuthRequest{
		State: state, Mode: mode, CLIState: params.State, CLIPort: port,
		CodeChallenge: params.CodeChallenge, ExpiresAt: s.now().Add(authRequestTTL),
	})
	if err != nil {
		s.internal(w, err)
		return
	}
	http.Redirect(w, r, s.idp.AuthCodeURL(state, s.redirectURL()), http.StatusFound)
}

func (s *Server) WebStart(w http.ResponseWriter, r *http.Request) {
	state := auth.RandomString(32)
	err := s.store.CreateAuthRequest(r.Context(), store.AuthRequest{
		State: state, Mode: "web", ExpiresAt: s.now().Add(authRequestTTL),
	})
	if err != nil {
		s.internal(w, err)
		return
	}
	http.SetCookie(w, &http.Cookie{
		Name: stateCookie, Value: state, Path: "/api/auth", MaxAge: int(authRequestTTL.Seconds()),
		HttpOnly: true, Secure: s.cfg.SecureCookies(), SameSite: http.SameSiteLaxMode,
	})
	http.Redirect(w, r, s.idp.AuthCodeURL(state, s.redirectURL()), http.StatusFound)
}

func (s *Server) GoogleCallback(w http.ResponseWriter, r *http.Request, params api.GoogleCallbackParams) {
	ctx := r.Context()
	if params.Error != nil {
		htmlPage(w, http.StatusBadRequest, "Sign-in cancelled", "Google returned: "+*params.Error, "")
		return
	}
	if params.State == nil || params.Code == nil {
		htmlPage(w, http.StatusBadRequest, "Invalid request", "Missing code or state.", "")
		return
	}
	req, err := s.store.TakeAuthRequest(ctx, *params.State)
	if errors.Is(err, store.ErrNotFound) {
		htmlPage(w, http.StatusBadRequest, "Link expired", "This sign-in link has expired or was already used. Start again.", "")
		return
	}
	if err != nil {
		s.internal(w, err)
		return
	}
	if req.Mode == "web" {
		c, err := r.Cookie(stateCookie)
		if err != nil || c.Value != req.State {
			htmlPage(w, http.StatusBadRequest, "Invalid request", "Sign-in must finish in the browser that started it.", "")
			return
		}
		http.SetCookie(w, &http.Cookie{Name: stateCookie, Value: "", Path: "/api/auth", MaxAge: -1})
	}

	id, err := s.idp.Exchange(ctx, *params.Code, s.redirectURL())
	if err != nil {
		htmlPage(w, http.StatusBadGateway, "Google sign-in failed", err.Error(), "")
		return
	}
	if !s.allow.Allows(id) {
		htmlPage(w, http.StatusForbidden, "Not allowed",
			fmt.Sprintf("%s is not allowed to use this cctrack server. Ask your admin to add your email or Google Workspace domain.", id.Email), "")
		return
	}
	user, err := s.store.UpsertUser(ctx, id.Subject, id.Email, id.Name)
	if err != nil {
		s.internal(w, err)
		return
	}

	if req.Mode == "web" {
		http.SetCookie(w, &http.Cookie{
			Name: sessionCookie, Value: s.sessions.Encode(user.ID, s.now().Add(sessionTTL)),
			Path: "/", MaxAge: int(sessionTTL.Seconds()),
			HttpOnly: true, Secure: s.cfg.SecureCookies(), SameSite: http.SameSiteLaxMode,
		})
		http.Redirect(w, r, s.cfg.PublicURL+"/", http.StatusFound)
		return
	}

	code := auth.RandomString(32)
	if err := s.store.CreateCLICode(ctx, auth.HashToken(code), user.ID, req.CodeChallenge, s.now().Add(cliCodeTTL)); err != nil {
		s.internal(w, err)
		return
	}
	if req.Mode == "paste" {
		htmlPage(w, http.StatusOK, "Paste this code into your terminal",
			"Signed in as "+user.Email+". The code expires in 60 seconds.", code)
		return
	}
	loopback := fmt.Sprintf("http://127.0.0.1:%d/callback?code=%s&state=%s",
		req.CLIPort, url.QueryEscape(code), url.QueryEscape(req.CLIState))
	http.Redirect(w, r, loopback, http.StatusFound)
}

func (s *Server) CliExchange(w http.ResponseWriter, r *http.Request) {
	var body api.CliExchangeRequest
	if !decodeJSON(w, r, &body) {
		return
	}
	device := strings.TrimSpace(body.DeviceName)
	if device == "" {
		device = "unknown"
	}
	if len(device) > 100 {
		device = device[:100]
	}
	userID, challenge, err := s.store.ConsumeCLICode(r.Context(), auth.HashToken(body.Code))
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusBadRequest, "invalid or expired code")
		return
	}
	if err != nil {
		s.internal(w, err)
		return
	}
	if !auth.VerifyPKCE(body.CodeVerifier, challenge) {
		writeError(w, http.StatusBadRequest, "code_verifier does not match")
		return
	}
	user, err := s.store.UserByID(r.Context(), userID)
	if err != nil {
		s.internal(w, err)
		return
	}
	plain, hash := auth.NewToken()
	if _, err := s.store.CreateToken(r.Context(), userID, hash, device); err != nil {
		s.internal(w, err)
		return
	}
	writeJSON(w, http.StatusOK, api.CliExchangeResponse{
		Token: plain,
		User:  api.User{Id: user.ID, Email: user.Email, Name: user.Name},
	})
}

func (s *Server) RevokeOwnToken(w http.ResponseWriter, r *http.Request) {
	p, ok := s.requireUser(w, r)
	if !ok {
		return
	}
	if p.TokenID == 0 {
		writeError(w, http.StatusUnauthorized, "bearer token required")
		return
	}
	if err := s.store.RevokeToken(r.Context(), p.TokenID); err != nil && !errors.Is(err, store.ErrNotFound) {
		s.internal(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) WebLogout(w http.ResponseWriter, r *http.Request) {
	http.SetCookie(w, &http.Cookie{
		Name: sessionCookie, Value: "", Path: "/", MaxAge: -1,
		HttpOnly: true, Secure: s.cfg.SecureCookies(), SameSite: http.SameSiteLaxMode,
	})
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) GetMe(w http.ResponseWriter, r *http.Request) {
	p, ok := s.requireUser(w, r)
	if !ok {
		return
	}
	writeJSON(w, http.StatusOK, api.Me{
		Id: p.User.ID, Email: p.User.Email, Name: p.User.Name, IsAdmin: s.isAdmin(p.User.Email),
	})
}
```

- [ ] **Step 4: main**

`apps/server/cmd/server/main.go`:
```go
// Command server runs the cctrack API.
package main

import (
	"context"
	"log"
	"net/http"
	"os"
	"time"

	"cctrack/server/internal/auth"
	"cctrack/server/internal/config"
	"cctrack/server/internal/db"
	"cctrack/server/internal/httpapi"
	"cctrack/server/internal/store"
)

func main() {
	cfg, err := config.Load(os.Getenv)
	if err != nil {
		log.Fatalf("config: %v", err)
	}
	ctx := context.Background()
	if cfg.AutoMigrate {
		if err := db.MigrateURL(ctx, cfg.MigrateURL()); err != nil {
			log.Fatalf("migrate: %v", err)
		}
	}
	pool, err := db.Open(ctx, cfg.DatabaseURL)
	if err != nil {
		log.Fatalf("db: %v", err)
	}
	defer pool.Close()

	srv := httpapi.New(cfg, store.New(pool), auth.NewGoogle(ctx, cfg.GoogleClientID, cfg.GoogleClientSecret))
	httpSrv := &http.Server{
		Addr:              ":" + cfg.Port,
		Handler:           srv.Handler(),
		ReadHeaderTimeout: 10 * time.Second,
	}
	log.Printf("cctrack server listening on :%s", cfg.Port)
	log.Fatal(httpSrv.ListenAndServe())
}
```

- [ ] **Step 5: Test helpers**

`apps/server/internal/httpapi/helpers_test.go`:
```go
package httpapi

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"cctrack/server/internal/auth"
	"cctrack/server/internal/config"
	"cctrack/server/internal/store"
	"cctrack/server/internal/testdb"
)

type fakeIDP struct {
	id  auth.Identity
	err error
}

func (f *fakeIDP) AuthCodeURL(state, redirectURL string) string {
	return "https://idp.test/auth?state=" + url.QueryEscape(state)
}

func (f *fakeIDP) Exchange(ctx context.Context, code, redirectURL string) (auth.Identity, error) {
	return f.id, f.err
}

func testConfig() config.Config {
	return config.Config{
		SessionSecret:  []byte(strings.Repeat("k", 32)),
		PublicURL:      "https://cctrack.test",
		AllowedDomains: []string{"example.com"},
		AdminEmails:    []string{"admin@example.com"},
	}
}

type harness struct {
	t     *testing.T
	srv   *Server
	h     http.Handler
	idp   *fakeIDP
	store *store.Store
}

func newHarness(t *testing.T) *harness {
	t.Helper()
	st := store.New(testdb.New(t))
	idp := &fakeIDP{}
	srv := New(testConfig(), st, idp)
	return &harness{t: t, srv: srv, h: srv.Handler(), idp: idp, store: st}
}

func bearer(token string) func(*http.Request) {
	return func(r *http.Request) { r.Header.Set("Authorization", "Bearer "+token) }
}

func withCookie(c *http.Cookie) func(*http.Request) {
	return func(r *http.Request) { r.AddCookie(c) }
}

func withCSRF() func(*http.Request) {
	return func(r *http.Request) { r.Header.Set(csrfHeader, "1") }
}

func (h *harness) do(method, path string, body any, mods ...func(*http.Request)) *httptest.ResponseRecorder {
	h.t.Helper()
	var buf bytes.Buffer
	if body != nil {
		if err := json.NewEncoder(&buf).Encode(body); err != nil {
			h.t.Fatal(err)
		}
	}
	req := httptest.NewRequest(method, path, &buf)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	for _, m := range mods {
		m(req)
	}
	rec := httptest.NewRecorder()
	h.h.ServeHTTP(rec, req)
	return rec
}

func decode[T any](t *testing.T, rec *httptest.ResponseRecorder) T {
	t.Helper()
	var v T
	if err := json.Unmarshal(rec.Body.Bytes(), &v); err != nil {
		t.Fatalf("decode %q: %v", rec.Body.String(), err)
	}
	return v
}

func identity(email string) auth.Identity {
	domain := email[strings.IndexByte(email, '@')+1:]
	return auth.Identity{Subject: "sub-" + email, Email: email, Name: strings.Split(email, "@")[0],
		HostedDomain: domain, EmailVerified: true}
}

func pkcePair() (verifier, challenge string) {
	verifier = auth.RandomString(48)
	sum := sha256.Sum256([]byte(verifier))
	return verifier, base64.RawURLEncoding.EncodeToString(sum[:])
}

func locationParam(t *testing.T, rec *httptest.ResponseRecorder, key string) string {
	t.Helper()
	u, err := url.Parse(rec.Header().Get("Location"))
	if err != nil {
		t.Fatal(err)
	}
	return u.Query().Get(key)
}

// loginCLI runs the full loopback login flow and returns the issued API token.
func (h *harness) loginCLI(email string) string {
	h.t.Helper()
	verifier, challenge := pkcePair()
	cliState := auth.RandomString(16)
	rec := h.do("GET", "/api/auth/cli/start?port=5555&state="+cliState+"&code_challenge="+challenge, nil)
	if rec.Code != http.StatusFound {
		h.t.Fatalf("start: %d %s", rec.Code, rec.Body)
	}
	serverState := locationParam(h.t, rec, "state")
	h.idp.id = identity(email)
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(serverState), nil)
	if rec.Code != http.StatusFound {
		h.t.Fatalf("callback: %d %s", rec.Code, rec.Body)
	}
	if got := locationParam(h.t, rec, "state"); got != cliState {
		h.t.Fatalf("loopback state = %q, want %q", got, cliState)
	}
	code := locationParam(h.t, rec, "code")
	rec = h.do("POST", "/api/auth/cli/exchange", map[string]string{
		"code": code, "code_verifier": verifier, "device_name": "laptop"})
	if rec.Code != http.StatusOK {
		h.t.Fatalf("exchange: %d %s", rec.Code, rec.Body)
	}
	return decode[map[string]any](h.t, rec)["token"].(string)
}

// loginWeb runs the browser login flow and returns the session cookie.
func (h *harness) loginWeb(email string) *http.Cookie {
	h.t.Helper()
	rec := h.do("GET", "/api/auth/web/start", nil)
	if rec.Code != http.StatusFound {
		h.t.Fatalf("web start: %d", rec.Code)
	}
	var stateC *http.Cookie
	for _, c := range rec.Result().Cookies() {
		if c.Name == stateCookie {
			stateC = c
		}
	}
	state := locationParam(h.t, rec, "state")
	h.idp.id = identity(email)
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil, withCookie(stateC))
	if rec.Code != http.StatusFound {
		h.t.Fatalf("web callback: %d %s", rec.Code, rec.Body)
	}
	for _, c := range rec.Result().Cookies() {
		if c.Name == sessionCookie {
			return c
		}
	}
	h.t.Fatal("no session cookie")
	return nil
}
```

- [ ] **Step 6: Write auth tests**

`apps/server/internal/httpapi/auth_test.go`:
```go
package httpapi

import (
	"net/http"
	"net/url"
	"strings"
	"testing"

	"cctrack/server/internal/api"
)

func TestHealth(t *testing.T) {
	h := newHarness(t)
	rec := h.do("GET", "/api/healthz", nil)
	if rec.Code != 200 || decode[api.Health](t, rec).Status != "ok" {
		t.Fatalf("got %d %s", rec.Code, rec.Body)
	}
}

func TestCLILoginFlowIssuesWorkingToken(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	rec := h.do("GET", "/api/me", nil, bearer(tok))
	if rec.Code != 200 {
		t.Fatalf("me: %d %s", rec.Code, rec.Body)
	}
	me := decode[api.Me](t, rec)
	if me.Email != "dev@example.com" || me.IsAdmin {
		t.Fatalf("me = %+v", me)
	}
}

func TestCLIStartRedirectsLoopbackTo127(t *testing.T) {
	h := newHarness(t)
	_, challenge := pkcePair()
	rec := h.do("GET", "/api/auth/cli/start?port=5555&state=abcdefghijklmnop&code_challenge="+challenge, nil)
	state := locationParam(t, rec, "state")
	h.idp.id = identity("dev@example.com")
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil)
	if loc := rec.Header().Get("Location"); !strings.HasPrefix(loc, "http://127.0.0.1:5555/callback?") {
		t.Fatalf("Location = %q", loc)
	}
}

func TestCLIStartValidation(t *testing.T) {
	h := newHarness(t)
	_, challenge := pkcePair()
	for _, q := range []string{
		"state=abcdefghijklmnop&code_challenge=" + challenge,               // loopback without port
		"port=80&state=abcdefghijklmnop&code_challenge=" + challenge,       // privileged port
		"port=5555&state=short&code_challenge=" + challenge,                // short state
		"port=5555&state=abcdefghijklmnop&code_challenge=tooshort",         // short challenge
		"mode=weird&port=5555&state=abcdefghijklmnop&code_challenge=" + challenge,
	} {
		if rec := h.do("GET", "/api/auth/cli/start?"+q, nil); rec.Code != http.StatusBadRequest {
			t.Errorf("%s: got %d, want 400", q, rec.Code)
		}
	}
}

func TestPasteModeShowsCode(t *testing.T) {
	h := newHarness(t)
	_, challenge := pkcePair()
	rec := h.do("GET", "/api/auth/cli/start?mode=paste&state=abcdefghijklmnop&code_challenge="+challenge, nil)
	state := locationParam(t, rec, "state")
	h.idp.id = identity("dev@example.com")
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil)
	if rec.Code != 200 || !strings.Contains(rec.Body.String(), "<code>") {
		t.Fatalf("paste page: %d %s", rec.Code, rec.Body)
	}
}

func TestExchangeRejectsWrongVerifierAndReuse(t *testing.T) {
	h := newHarness(t)
	verifier, challenge := pkcePair()
	rec := h.do("GET", "/api/auth/cli/start?port=5555&state=abcdefghijklmnop&code_challenge="+challenge, nil)
	state := locationParam(t, rec, "state")
	h.idp.id = identity("dev@example.com")
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil)
	code := locationParam(t, rec, "code")

	wrong, _ := pkcePair()
	rec = h.do("POST", "/api/auth/cli/exchange", map[string]string{"code": code, "code_verifier": wrong, "device_name": "x"})
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("wrong verifier: %d", rec.Code)
	}
	// The code was burned by the failed attempt.
	rec = h.do("POST", "/api/auth/cli/exchange", map[string]string{"code": code, "code_verifier": verifier, "device_name": "x"})
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("reuse: %d", rec.Code)
	}
}

func TestCallbackStateIsSingleUse(t *testing.T) {
	h := newHarness(t)
	_, challenge := pkcePair()
	rec := h.do("GET", "/api/auth/cli/start?port=5555&state=abcdefghijklmnop&code_challenge="+challenge, nil)
	state := locationParam(t, rec, "state")
	h.idp.id = identity("dev@example.com")
	h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil)
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("second callback: %d", rec.Code)
	}
}

func TestCallbackRejectsDisallowedAccount(t *testing.T) {
	h := newHarness(t)
	_, challenge := pkcePair()
	rec := h.do("GET", "/api/auth/cli/start?port=5555&state=abcdefghijklmnop&code_challenge="+challenge, nil)
	state := locationParam(t, rec, "state")
	h.idp.id = identity("stranger@other.com")
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil)
	if rec.Code != http.StatusForbidden {
		t.Fatalf("got %d, want 403", rec.Code)
	}
}

func TestWebLoginCookieAndCSRF(t *testing.T) {
	h := newHarness(t)
	c := h.loginWeb("admin@example.com")
	rec := h.do("GET", "/api/me", nil, withCookie(c))
	if rec.Code != 200 || !decode[api.Me](t, rec).IsAdmin {
		t.Fatalf("me: %d %s", rec.Code, rec.Body)
	}
	if rec := h.do("POST", "/api/auth/logout", nil, withCookie(c)); rec.Code != http.StatusForbidden {
		t.Fatalf("logout without CSRF header: %d, want 403", rec.Code)
	}
	if rec := h.do("POST", "/api/auth/logout", nil, withCookie(c), withCSRF()); rec.Code != http.StatusNoContent {
		t.Fatalf("logout: %d", rec.Code)
	}
}

func TestWebCallbackRequiresStateCookie(t *testing.T) {
	h := newHarness(t)
	rec := h.do("GET", "/api/auth/web/start", nil)
	state := locationParam(t, rec, "state")
	h.idp.id = identity("dev@example.com")
	rec = h.do("GET", "/api/auth/google/callback?code=g&state="+url.QueryEscape(state), nil) // no cookie
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("got %d, want 400", rec.Code)
	}
}

func TestRevokedTokenIs401(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	if rec := h.do("DELETE", "/api/auth/token", nil, bearer(tok)); rec.Code != http.StatusNoContent {
		t.Fatalf("revoke: %d", rec.Code)
	}
	if rec := h.do("GET", "/api/me", nil, bearer(tok)); rec.Code != http.StatusUnauthorized {
		t.Fatalf("me after revoke: %d, want 401", rec.Code)
	}
}

func TestBadBearerIs401AndAnonymousIs401(t *testing.T) {
	h := newHarness(t)
	if rec := h.do("GET", "/api/me", nil, bearer("cct_nope")); rec.Code != http.StatusUnauthorized {
		t.Fatalf("bad bearer: %d", rec.Code)
	}
	if rec := h.do("GET", "/api/me", nil); rec.Code != http.StatusUnauthorized {
		t.Fatalf("anonymous: %d", rec.Code)
	}
}
```

- [ ] **Step 7: Run tests**

Run: `cd apps/server && go vet ./... && go test ./...`
Expected: PASS (httpapi tests SKIP without `TEST_DATABASE_URL`; report which). If the compiler reports a `ServerInterface` mismatch, fix the method signature to match `api.gen.go`.

- [ ] **Step 8: Commit**

```bash
git add apps/server
git commit -m "feat(server): HTTP skeleton, Google login for CLI and web, token auth

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Ingest and session summary

**Files:**
- Create: `apps/server/internal/store/plan.go`, `plan_test.go`, `ingest.go`
- Create: `apps/server/internal/httpapi/validate.go`, `validate_test.go`, `ingest.go`, `ingest_test.go`
- Modify: `apps/server/internal/httpapi/stubs.go` (delete `Ingest`, `SetSessionSummary`)

**Interfaces:**
- Consumes: `principal`, `writeJSON/writeError/decodeJSON`, `harness`.
- Produces:
  - `store.GuessPlan(orgType, tier string) string`
  - `store.APIKeyAccount = "api_key"`
  - `store.IngestResult{Accepted, Duplicates int}`, `(*Store).Ingest(ctx, userID int64, req *api.IngestRequest) (IngestResult, error)` (returns `ErrForbidden` for a session owned by someone else)
  - `(*Store).SetSummary(ctx, userID int64, sessionID, summary string) error` (`ErrNotFound` if not the caller's)
  - `httpapi.validateIngest(req *api.IngestRequest, now time.Time) error`
  - test helper `ingestFixture(sessionID, accountUUID string, entries ...api.IngestEntry) api.IngestRequest`, `entry(sessionID, accountUUID, msgID, model string, ts time.Time, input, output int64) api.IngestEntry`

- [ ] **Step 1: Write failing plan + validation tests**

`apps/server/internal/store/plan_test.go`:
```go
package store

import "testing"

func TestGuessPlan(t *testing.T) {
	cases := map[[2]string]string{
		{"claude_pro", "default_claude_ai"}:              "pro",
		{"claude_max", "default_claude_max_20x"}:         "max_20x",
		{"claude_max", "default_claude_max_5x"}:          "max_5x",
		{"claude_max", ""}:                               "unknown",
		{"claude_team", "default_claude_team"}:           "team",
		{"claude_enterprise", "anything"}:                "enterprise",
		{"", ""}:                                         "unknown",
	}
	for in, want := range cases {
		if got := GuessPlan(in[0], in[1]); got != want {
			t.Errorf("GuessPlan(%q,%q) = %q, want %q", in[0], in[1], got, want)
		}
	}
}
```

`apps/server/internal/httpapi/validate_test.go`:
```go
package httpapi

import (
	"testing"
	"time"

	"cctrack/server/internal/api"
)

var t0 = time.Date(2026, 10, 1, 12, 0, 0, 0, time.UTC)

func entry(sessionID, accountUUID, msgID, model string, ts time.Time, input, output int64) api.IngestEntry {
	return api.IngestEntry{SessionId: sessionID, AccountUuid: accountUUID, MessageId: msgID, Model: model,
		Ts: ts, Input: input, Output: output}
}

func ingestFixture(sessionID, accountUUID string, entries ...api.IngestEntry) api.IngestRequest {
	start, end := t0, t0
	for _, e := range entries {
		if e.Ts.Before(start) {
			start = e.Ts
		}
		if e.Ts.After(end) {
			end = e.Ts
		}
	}
	cwd := "/home/dev/proj"
	return api.IngestRequest{
		Device:   api.Device{Hostname: "laptop", Os: "macos"},
		Accounts: []api.IngestAccount{{AccountUuid: accountUUID}},
		Sessions: []api.IngestSession{{SessionId: sessionID, Cwd: &cwd, StartedAt: start, EndedAt: end}},
		Entries:  entries,
	}
}

func TestValidateIngestOK(t *testing.T) {
	req := ingestFixture("s1", "acc-1", entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 10, 5))
	if err := validateIngest(&req, t0); err != nil {
		t.Fatal(err)
	}
}

func TestValidateIngestErrors(t *testing.T) {
	cases := map[string]func(*api.IngestRequest){
		"session_id":    func(r *api.IngestRequest) { r.Entries[0].SessionId = "other" },
		"account_uuid":  func(r *api.IngestRequest) { r.Entries[0].AccountUuid = "nope" },
		"message_id":    func(r *api.IngestRequest) { r.Entries[0].MessageId = "" },
		"negative":      func(r *api.IngestRequest) { r.Entries[0].Output = -1 },
		"future":        func(r *api.IngestRequest) { r.Entries[0].Ts = t0.Add(48 * time.Hour) },
		"model":         func(r *api.IngestRequest) { r.Entries[0].Model = "" },
		"hostname":      func(r *api.IngestRequest) { r.Device.Hostname = "" },
		"too many":      func(r *api.IngestRequest) { r.Entries = make([]api.IngestEntry, 5001) },
		"ended_at":      func(r *api.IngestRequest) { r.Sessions[0].EndedAt = r.Sessions[0].StartedAt.Add(-time.Hour) },
		"snapshot acct": func(r *api.IngestRequest) { r.LimitSnapshot = &api.LimitSnapshotIn{AccountUuid: "zzz", CapturedAt: t0} },
		"snapshot pct": func(r *api.IngestRequest) {
			p := -5.0
			r.LimitSnapshot = &api.LimitSnapshotIn{AccountUuid: "acc-1", CapturedAt: t0, FiveHourPct: &p}
		},
	}
	for name, mutate := range cases {
		req := ingestFixture("s1", "acc-1", entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 10, 5))
		mutate(&req)
		if err := validateIngest(&req, t0); err == nil {
			t.Errorf("%s: expected error", name)
		}
	}
}

func TestValidateIngestAllowsAPIKeyAccount(t *testing.T) {
	req := ingestFixture("s1", "api_key", entry("s1", "api_key", "m1", "claude-opus-4-5", t0, 1, 1))
	if err := validateIngest(&req, t0); err != nil {
		t.Fatal(err)
	}
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd apps/server && go test ./internal/store/ ./internal/httpapi/ -run 'GuessPlan|ValidateIngest'`
Expected: FAIL — undefined `GuessPlan`, `validateIngest`.

- [ ] **Step 3: Implement plan guess and validation**

`apps/server/internal/store/plan.go`:
```go
package store

import "strings"

// APIKeyAccount is the account_uuid the CLI sends for API-key authentication.
const APIKeyAccount = "api_key"

// GuessPlan infers a plan from Claude Code's oauthAccount fields
// (organizationType, organizationRateLimitTier). Admins can override it.
func GuessPlan(orgType, tier string) string {
	t := strings.ToLower(orgType + " " + tier)
	switch {
	case strings.Contains(t, "enterprise"):
		return "enterprise"
	case strings.Contains(t, "team"):
		return "team"
	case strings.Contains(t, "20x"):
		return "max_20x"
	case strings.Contains(t, "5x"):
		return "max_5x"
	case strings.Contains(t, "max"):
		return "unknown"
	case strings.Contains(t, "pro"):
		return "pro"
	}
	return "unknown"
}
```

`apps/server/internal/httpapi/validate.go`:
```go
package httpapi

import (
	"fmt"
	"time"

	"cctrack/server/internal/api"
	"cctrack/server/internal/store"
)

const (
	maxEntries  = 5000
	maxSessions = 500
	maxAccounts = 20
)

var earliestTS = time.Date(2024, 1, 1, 0, 0, 0, 0, time.UTC)

func validateIngest(req *api.IngestRequest, now time.Time) error {
	if req.Device.Hostname == "" || len(req.Device.Hostname) > 255 {
		return fmt.Errorf("device.hostname is required (max 255 chars)")
	}
	if len(req.Entries) > maxEntries || len(req.Sessions) > maxSessions || len(req.Accounts) > maxAccounts {
		return fmt.Errorf("batch too large: max %d entries, %d sessions, %d accounts", maxEntries, maxSessions, maxAccounts)
	}
	latest := now.Add(24 * time.Hour)

	accounts := map[string]bool{store.APIKeyAccount: true}
	for i, a := range req.Accounts {
		if a.AccountUuid == "" || len(a.AccountUuid) > 100 {
			return fmt.Errorf("accounts[%d].account_uuid is required (max 100 chars)", i)
		}
		accounts[a.AccountUuid] = true
	}
	sessions := map[string]bool{}
	for i, s := range req.Sessions {
		if s.SessionId == "" || len(s.SessionId) > 100 {
			return fmt.Errorf("sessions[%d].session_id is required (max 100 chars)", i)
		}
		if s.EndedAt.Before(s.StartedAt) {
			return fmt.Errorf("sessions[%d]: ended_at before started_at", i)
		}
		sessions[s.SessionId] = true
	}
	for i, e := range req.Entries {
		switch {
		case !sessions[e.SessionId]:
			return fmt.Errorf("entries[%d].session_id %q not listed in sessions", i, e.SessionId)
		case !accounts[e.AccountUuid]:
			return fmt.Errorf("entries[%d].account_uuid %q not listed in accounts", i, e.AccountUuid)
		case e.MessageId == "" || len(e.MessageId) > 200:
			return fmt.Errorf("entries[%d].message_id is required (max 200 chars)", i)
		case e.Model == "" || len(e.Model) > 200:
			return fmt.Errorf("entries[%d].model is required (max 200 chars)", i)
		case e.Input < 0 || e.Output < 0 || e.CacheWrite5m < 0 || e.CacheWrite1h < 0 || e.CacheRead < 0:
			return fmt.Errorf("entries[%d]: token counts must be >= 0", i)
		case e.Ts.Before(earliestTS) || e.Ts.After(latest):
			return fmt.Errorf("entries[%d].ts out of range", i)
		}
	}
	if s := req.LimitSnapshot; s != nil {
		if !accounts[s.AccountUuid] || s.AccountUuid == store.APIKeyAccount {
			return fmt.Errorf("limit_snapshot.account_uuid %q not listed in accounts", s.AccountUuid)
		}
		for _, p := range []*float64{s.FiveHourPct, s.SevenDayPct} {
			if p != nil && (*p < 0 || *p > 1000) {
				return fmt.Errorf("limit_snapshot percentages must be within 0-1000")
			}
		}
		if s.CapturedAt.After(latest) {
			return fmt.Errorf("limit_snapshot.captured_at out of range")
		}
	}
	return nil
}
```

- [ ] **Step 4: Run unit tests**

Run: `cd apps/server && go test ./internal/store/ ./internal/httpapi/ -run 'GuessPlan|ValidateIngest'`
Expected: PASS.

- [ ] **Step 5: Implement store ingest + summary**

`apps/server/internal/store/ingest.go`:
```go
package store

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"

	"cctrack/server/internal/api"
)

type IngestResult struct {
	Accepted, Duplicates int
}

func project(cwd *string) *string {
	if cwd == nil || *cwd == "" {
		return nil
	}
	p := strings.TrimRight(strings.ReplaceAll(*cwd, `\`, "/"), "/")
	if i := strings.LastIndexByte(p, '/'); i >= 0 {
		p = p[i+1:]
	}
	return &p
}

func ptrOrNil(s *string) *string {
	if s == nil || *s == "" {
		return nil
	}
	return s
}

// Ingest stores a batch atomically. Entries are deduplicated on message_id.
func (s *Store) Ingest(ctx context.Context, userID int64, req *api.IngestRequest) (IngestResult, error) {
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return IngestResult{}, err
	}
	defer tx.Rollback(ctx)

	var deviceID int64
	if err := tx.QueryRow(ctx, `
		INSERT INTO devices (user_id, hostname, os) VALUES ($1, $2, $3)
		ON CONFLICT (user_id, hostname) DO UPDATE SET os = EXCLUDED.os, last_seen = now()
		RETURNING id`, userID, req.Device.Hostname, req.Device.Os).Scan(&deviceID); err != nil {
		return IngestResult{}, fmt.Errorf("device: %w", err)
	}

	accountIDs := map[string]int64{}
	upsertAccount := func(uuid string, a api.IngestAccount, label, plan string) error {
		var id int64
		err := tx.QueryRow(ctx, `
			INSERT INTO claude_accounts (account_uuid, org_uuid, email, org_type, rate_limit_tier, label, plan)
			VALUES ($1, $2, $3, $4, $5, $6, $7)
			ON CONFLICT (account_uuid) DO UPDATE SET
			  org_uuid = COALESCE(EXCLUDED.org_uuid, claude_accounts.org_uuid),
			  email = COALESCE(EXCLUDED.email, claude_accounts.email),
			  org_type = COALESCE(EXCLUDED.org_type, claude_accounts.org_type),
			  rate_limit_tier = COALESCE(EXCLUDED.rate_limit_tier, claude_accounts.rate_limit_tier)
			RETURNING id`,
			uuid, ptrOrNil(a.OrgUuid), ptrOrNil(a.Email), ptrOrNil(a.OrgType), ptrOrNil(a.RateLimitTier), label, plan).Scan(&id)
		accountIDs[a.AccountUuid] = id
		return err
	}
	for _, a := range req.Accounts {
		if a.AccountUuid == APIKeyAccount {
			continue
		}
		orgType, tier := deref(a.OrgType), deref(a.RateLimitTier)
		if err := upsertAccount(a.AccountUuid, a, "", GuessPlan(orgType, tier)); err != nil {
			return IngestResult{}, fmt.Errorf("account: %w", err)
		}
	}
	needsAPIKey := false
	for _, e := range req.Entries {
		if e.AccountUuid == APIKeyAccount {
			needsAPIKey = true
			break
		}
	}
	if needsAPIKey {
		uuid := fmt.Sprintf("api_key:%d:%d", userID, deviceID)
		label := "API key (" + req.Device.Hostname + ")"
		if err := upsertAccount(uuid, api.IngestAccount{AccountUuid: APIKeyAccount}, label, "api_key"); err != nil {
			return IngestResult{}, fmt.Errorf("api key account: %w", err)
		}
	}

	// Each session's account is the account of its latest entry in this batch.
	sessionAccount := map[string]int64{}
	latest := map[string]time.Time{}
	for _, e := range req.Entries {
		if e.Ts.After(latest[e.SessionId]) || latest[e.SessionId].IsZero() {
			latest[e.SessionId] = e.Ts
			sessionAccount[e.SessionId] = accountIDs[e.AccountUuid]
		}
	}
	for _, sess := range req.Sessions {
		var acct *int64
		if id, ok := sessionAccount[sess.SessionId]; ok {
			acct = &id
		}
		var got string
		err := tx.QueryRow(ctx, `
			INSERT INTO sessions (session_id, user_id, device_id, account_id, cwd, project, git_branch, started_at, ended_at)
			VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
			ON CONFLICT (session_id) DO UPDATE SET
			  device_id = EXCLUDED.device_id,
			  account_id = COALESCE(EXCLUDED.account_id, sessions.account_id),
			  cwd = COALESCE(EXCLUDED.cwd, sessions.cwd),
			  project = COALESCE(EXCLUDED.project, sessions.project),
			  git_branch = COALESCE(EXCLUDED.git_branch, sessions.git_branch),
			  started_at = LEAST(sessions.started_at, EXCLUDED.started_at),
			  ended_at = GREATEST(sessions.ended_at, EXCLUDED.ended_at)
			WHERE sessions.user_id = EXCLUDED.user_id
			RETURNING session_id`,
			sess.SessionId, userID, deviceID, acct, ptrOrNil(sess.Cwd), project(sess.Cwd), ptrOrNil(sess.GitBranch),
			sess.StartedAt, sess.EndedAt).Scan(&got)
		if errors.Is(err, pgx.ErrNoRows) {
			return IngestResult{}, ErrForbidden
		}
		if err != nil {
			return IngestResult{}, fmt.Errorf("session: %w", err)
		}
	}

	var res IngestResult
	if n := len(req.Entries); n > 0 {
		cols := struct {
			session, msg, req, model, speed []string
			account                         []int64
			ts                              []time.Time
			in, out, w5, w1, cr             []int64
			sub                             []bool
		}{}
		for _, e := range req.Entries {
			speed := deref(e.Speed)
			if speed == "" {
				speed = "standard"
			}
			cols.session = append(cols.session, e.SessionId)
			cols.account = append(cols.account, accountIDs[e.AccountUuid])
			cols.msg = append(cols.msg, e.MessageId)
			cols.req = append(cols.req, deref(e.RequestId))
			cols.model = append(cols.model, e.Model)
			cols.ts = append(cols.ts, e.Ts)
			cols.in = append(cols.in, e.Input)
			cols.out = append(cols.out, e.Output)
			cols.w5 = append(cols.w5, e.CacheWrite5m)
			cols.w1 = append(cols.w1, e.CacheWrite1h)
			cols.cr = append(cols.cr, e.CacheRead)
			cols.speed = append(cols.speed, speed)
			cols.sub = append(cols.sub, e.IsSubagent)
		}
		tag, err := tx.Exec(ctx, `
			INSERT INTO usage_entries (session_id, user_id, account_id, message_id, request_id, model, ts,
			  input, output, cache_write_5m, cache_write_1h, cache_read, speed, is_subagent)
			SELECT s, $2, a, m, rq, md, t, i, o, w5, w1, cr, sp, sb
			FROM unnest($1::text[], $3::bigint[], $4::text[], $5::text[], $6::text[], $7::timestamptz[],
			            $8::bigint[], $9::bigint[], $10::bigint[], $11::bigint[], $12::bigint[], $13::text[], $14::bool[])
			  AS x(s, a, m, rq, md, t, i, o, w5, w1, cr, sp, sb)
			ON CONFLICT (message_id) DO NOTHING`,
			cols.session, userID, cols.account, cols.msg, cols.req, cols.model, cols.ts,
			cols.in, cols.out, cols.w5, cols.w1, cols.cr, cols.speed, cols.sub)
		if err != nil {
			return IngestResult{}, fmt.Errorf("entries: %w", err)
		}
		res.Accepted = int(tag.RowsAffected())
		res.Duplicates = n - res.Accepted
	}

	if snap := req.LimitSnapshot; snap != nil {
		if _, err := tx.Exec(ctx, `
			INSERT INTO limit_snapshots (account_id, user_id, captured_at, five_hour_pct, five_hour_resets_at,
			  seven_day_pct, seven_day_resets_at)
			VALUES ($1, $2, $3, $4, $5, $6, $7)
			ON CONFLICT (account_id, captured_at) DO NOTHING`,
			accountIDs[snap.AccountUuid], userID, snap.CapturedAt, snap.FiveHourPct, snap.FiveHourResetsAt,
			snap.SevenDayPct, snap.SevenDayResetsAt); err != nil {
			return IngestResult{}, fmt.Errorf("snapshot: %w", err)
		}
	}
	return res, tx.Commit(ctx)
}

func deref(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

func (s *Store) SetSummary(ctx context.Context, userID int64, sessionID, summary string) error {
	tag, err := s.pool.Exec(ctx, `
		UPDATE sessions SET summary = $3, summary_at = now()
		WHERE session_id = $1 AND user_id = $2`, sessionID, userID, summary)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}
```

Note the API-key branch: `upsertAccount` stores the id under `accountIDs[a.AccountUuid]`, which is `"api_key"` for the synthetic account, so entries carrying `account_uuid: "api_key"` resolve to it.

- [ ] **Step 6: Handlers**

Delete `Ingest` and `SetSessionSummary` from `stubs.go`, then create `apps/server/internal/httpapi/ingest.go`:
```go
package httpapi

import (
	"errors"
	"net/http"
	"strings"
	"unicode/utf8"

	"cctrack/server/internal/api"
	"cctrack/server/internal/store"
)

const maxSummaryRunes = 500

func (s *Server) Ingest(w http.ResponseWriter, r *http.Request) {
	p, ok := s.requireUser(w, r)
	if !ok {
		return
	}
	var req api.IngestRequest
	if !decodeJSON(w, r, &req) {
		return
	}
	if err := validateIngest(&req, s.now()); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	res, err := s.store.Ingest(r.Context(), p.User.ID, &req)
	if errors.Is(err, store.ErrForbidden) {
		writeError(w, http.StatusForbidden, "a session in this batch belongs to another user")
		return
	}
	if err != nil {
		s.internal(w, err)
		return
	}
	writeJSON(w, http.StatusOK, api.IngestResponse{Accepted: res.Accepted, Duplicates: res.Duplicates})
}

func (s *Server) SetSessionSummary(w http.ResponseWriter, r *http.Request, sessionId string) {
	p, ok := s.requireUser(w, r)
	if !ok {
		return
	}
	var body api.SummaryRequest
	if !decodeJSON(w, r, &body) {
		return
	}
	summary := strings.TrimSpace(body.Summary)
	if summary == "" || utf8.RuneCountInString(summary) > maxSummaryRunes {
		writeError(w, http.StatusBadRequest, "summary must be 1-500 characters")
		return
	}
	err := s.store.SetSummary(r.Context(), p.User.ID, sessionId, summary)
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, "session not found")
		return
	}
	if err != nil {
		s.internal(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
```

- [ ] **Step 7: DB-backed ingest tests**

`apps/server/internal/httpapi/ingest_test.go`:
```go
package httpapi

import (
	"context"
	"net/http"
	"strings"
	"testing"
	"time"

	"cctrack/server/internal/api"
)

func str(s string) *string { return &s }

func TestIngestIsIdempotent(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	req := ingestFixture("s1", "acc-1",
		entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 100, 50),
		entry("s1", "acc-1", "m2", "claude-opus-4-5", t0.Add(time.Minute), 10, 5))
	rec := h.do("POST", "/api/ingest", req, bearer(tok))
	if rec.Code != 200 {
		t.Fatalf("ingest: %d %s", rec.Code, rec.Body)
	}
	if got := decode[api.IngestResponse](t, rec); got.Accepted != 2 || got.Duplicates != 0 {
		t.Fatalf("first = %+v", got)
	}
	rec = h.do("POST", "/api/ingest", req, bearer(tok))
	if got := decode[api.IngestResponse](t, rec); got.Accepted != 0 || got.Duplicates != 2 {
		t.Fatalf("resend = %+v", got)
	}
}

func TestIngestRejectsOtherUsersSession(t *testing.T) {
	h := newHarness(t)
	alice := h.loginCLI("alice@example.com")
	bob := h.loginCLI("bob@example.com")
	h.do("POST", "/api/ingest", ingestFixture("shared", "acc-1",
		entry("shared", "acc-1", "m1", "claude-opus-4-5", t0, 1, 1)), bearer(alice))
	rec := h.do("POST", "/api/ingest", ingestFixture("shared", "acc-1",
		entry("shared", "acc-1", "m2", "claude-opus-4-5", t0, 1, 1)), bearer(bob))
	if rec.Code != http.StatusForbidden {
		t.Fatalf("got %d, want 403", rec.Code)
	}
	var n int
	_ = h.store.Pool().QueryRow(context.Background(), "SELECT count(*) FROM usage_entries").Scan(&n)
	if n != 1 {
		t.Fatalf("entries = %d, want 1 (bob's batch must be rolled back)", n)
	}
}

func TestIngestAccountPlanGuessAndAdminEditsSurvive(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	req := ingestFixture("s1", "acc-max", entry("s1", "acc-max", "m1", "claude-opus-4-5", t0, 1, 1))
	req.Accounts[0].OrgType = str("claude_max")
	req.Accounts[0].RateLimitTier = str("default_claude_max_20x")
	req.Accounts[0].Email = str("shared@example.com")
	h.do("POST", "/api/ingest", req, bearer(tok))

	ctx := context.Background()
	var plan, label string
	_ = h.store.Pool().QueryRow(ctx, "SELECT plan, label FROM claude_accounts WHERE account_uuid='acc-max'").Scan(&plan, &label)
	if plan != "max_20x" {
		t.Fatalf("plan = %q", plan)
	}
	_, _ = h.store.Pool().Exec(ctx, "UPDATE claude_accounts SET plan='team', label='Shared Max' WHERE account_uuid='acc-max'")
	req.Entries[0].MessageId = "m2"
	h.do("POST", "/api/ingest", req, bearer(tok))
	_ = h.store.Pool().QueryRow(ctx, "SELECT plan, label FROM claude_accounts WHERE account_uuid='acc-max'").Scan(&plan, &label)
	if plan != "team" || label != "Shared Max" {
		t.Fatalf("admin edits overwritten: plan=%q label=%q", plan, label)
	}
}

func TestIngestAPIKeyAccount(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	req := ingestFixture("s1", "api_key", entry("s1", "api_key", "m1", "claude-opus-4-5", t0, 1, 1))
	if rec := h.do("POST", "/api/ingest", req, bearer(tok)); rec.Code != 200 {
		t.Fatalf("%d %s", rec.Code, rec.Body)
	}
	var plan, label, uuid string
	_ = h.store.Pool().QueryRow(context.Background(),
		"SELECT plan, label, account_uuid FROM claude_accounts").Scan(&plan, &label, &uuid)
	if plan != "api_key" || label != "API key (laptop)" || !strings.HasPrefix(uuid, "api_key:") {
		t.Fatalf("got plan=%q label=%q uuid=%q", plan, label, uuid)
	}
}

func TestIngestLimitSnapshotDedup(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	pct := 42.0
	req := ingestFixture("s1", "acc-1", entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 1, 1))
	req.LimitSnapshot = &api.LimitSnapshotIn{AccountUuid: "acc-1", CapturedAt: t0, FiveHourPct: &pct}
	h.do("POST", "/api/ingest", req, bearer(tok))
	h.do("POST", "/api/ingest", req, bearer(tok))
	var n int
	_ = h.store.Pool().QueryRow(context.Background(), "SELECT count(*) FROM limit_snapshots").Scan(&n)
	if n != 1 {
		t.Fatalf("snapshots = %d, want 1", n)
	}
}

func TestIngestRequiresBearer(t *testing.T) {
	h := newHarness(t)
	if rec := h.do("POST", "/api/ingest", ingestFixture("s1", "a")); rec.Code != http.StatusUnauthorized {
		t.Fatalf("got %d", rec.Code)
	}
}

func TestSessionSummary(t *testing.T) {
	h := newHarness(t)
	alice := h.loginCLI("alice@example.com")
	bob := h.loginCLI("bob@example.com")
	h.do("POST", "/api/ingest", ingestFixture("s1", "acc-1",
		entry("s1", "acc-1", "m1", "claude-opus-4-5", t0, 1, 1)), bearer(alice))

	if rec := h.do("POST", "/api/sessions/s1/summary", map[string]string{"summary": "Added login flow."}, bearer(alice)); rec.Code != http.StatusNoContent {
		t.Fatalf("set: %d %s", rec.Code, rec.Body)
	}
	if rec := h.do("POST", "/api/sessions/s1/summary", map[string]string{"summary": "hijack"}, bearer(bob)); rec.Code != http.StatusNotFound {
		t.Fatalf("other user: %d, want 404", rec.Code)
	}
	if rec := h.do("POST", "/api/sessions/s1/summary", map[string]string{"summary": strings.Repeat("x", 501)}, bearer(alice)); rec.Code != http.StatusBadRequest {
		t.Fatalf("too long: %d, want 400", rec.Code)
	}
	var summary string
	_ = h.store.Pool().QueryRow(context.Background(), "SELECT summary FROM sessions WHERE session_id='s1'").Scan(&summary)
	if summary != "Added login flow." {
		t.Fatalf("summary = %q", summary)
	}
}
```

Add a test-only accessor to `apps/server/internal/store/store.go`:
```go
// Pool exposes the pool for tests and diagnostics.
func (s *Store) Pool() *pgxpool.Pool { return s.pool }
```

- [ ] **Step 8: Run tests**

Run: `cd apps/server && go vet ./... && go test ./...`
Expected: PASS (DB tests SKIP without `TEST_DATABASE_URL`; report which).

- [ ] **Step 9: Commit**

```bash
git add apps/server
git commit -m "feat(server): idempotent usage ingest and session summaries

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Aggregation, cost folding, user dashboard endpoints

**Files:**
- Create: `apps/server/internal/store/query.go`
- Create: `apps/server/internal/report/report.go`, `report_test.go`
- Create: `apps/server/internal/httpapi/rangeparams.go`, `convert.go`, `me.go`, `me_test.go`
- Modify: `apps/server/internal/httpapi/stubs.go` (delete `GetMyUsage`, `ListMySessions`)

**Interfaces:**
- Consumes: `pricing.*`, `store.Store`, harness helpers, `ingestFixture`, `entry`.
- Produces:
  - `store.GroupBy{Day, User, Account, Session bool}`, `store.Filter{From, To time.Time; TZ string; UserID, AccountID *int64; SessionIDs []string}`, `store.AggRow{Day string; UserID, AccountID int64; SessionID, Model, Speed string; LongCtx bool; Counts pricing.Counts}`
  - `(*Store).Aggregate(ctx, GroupBy, Filter) ([]AggRow, error)`, `(*Store).Prices(ctx) ([]pricing.Price, error)`, `(*Store).PriceTable(ctx) (*pricing.Table, error)`
  - `store.Account{ID int64; AccountUUID string; OrgUUID, Email, OrgType, RateLimitTier *string; Label, Plan string; CreatedAt time.Time}` + `DisplayLabel() string`; `(*Store).AccountsByIDs(ctx, []int64) (map[int64]Account, error)`; `(*Store).ListAccounts(ctx) ([]Account, error)`
  - `store.LimitSnapshot{AccountID int64; CapturedAt time.Time; FiveHourPct *float64; FiveHourResetsAt *time.Time; SevenDayPct *float64; SevenDayResetsAt *time.Time}`; `(*Store).LatestLimits(ctx, accountIDs []int64) ([]LimitSnapshot, error)`; `(*Store).LimitHistory(ctx, accountID int64, from, to time.Time) ([]LimitSnapshot, error)`
  - `store.SessionRow{SessionID string; UserID int64; UserEmail, UserName string; AccountID *int64; DeviceHostname, Project, GitBranch, Summary *string; StartedAt, EndedAt time.Time}`; `(*Store).ListSessions(ctx, userID int64, limit, offset int) ([]SessionRow, int, error)`
  - `store.UserRef{ID int64; Email, Name string}`; `(*Store).UsersByIDs(ctx, []int64) (map[int64]UserRef, error)`
  - `report.Bucket{Counts pricing.Counts; Cost float64; Unpriced int64}` + `Add(*pricing.Table, store.AggRow)` + `Totals() api.Totals`; `report.Group[K comparable]` with `NewGroup[K]()`, `Add(K, *pricing.Table, store.AggRow)`, `Get(K) report.Bucket`, `Keys() []K`, `SortedKeys(less func(a, b K) bool) []K`
  - httpapi: `parseRange(from, to time.Time, tz *string) (store.Filter, error)`, `pageParams(limit, offset *int) (int, int)`, `accountRef(store.Account) api.AccountRef`, `userRef(store.UserRef) api.UserRef`, `limitToAPI(store.LimitSnapshot) api.LimitSnapshot`, `(*Server).sessionPage(ctx, rows []store.SessionRow, total int) (api.SessionPage, error)`

- [ ] **Step 1: Write failing report tests**

`apps/server/internal/report/report_test.go`:
```go
package report

import (
	"math"
	"testing"

	"cctrack/server/internal/pricing"
	"cctrack/server/internal/store"
)

func tbl() *pricing.Table {
	return pricing.NewTable([]pricing.Price{
		{Model: "claude-opus-4-5", Base: pricing.Rates{Input: 5, Output: 25}},
	})
}

func TestBucketAddsCostAndUnpriced(t *testing.T) {
	var b Bucket
	b.Add(tbl(), store.AggRow{Model: "claude-opus-4-5-20251101", Speed: "standard",
		Counts: pricing.Counts{Input: 1_000_000, Output: 100_000}})
	b.Add(tbl(), store.AggRow{Model: "mystery-model", Speed: "standard",
		Counts: pricing.Counts{Input: 7, Output: 3}})
	tot := b.Totals()
	if math.Abs(tot.CostUsd-7.5) > 1e-9 {
		t.Fatalf("cost = %v, want 7.5", tot.CostUsd)
	}
	if tot.UnpricedTokens != 10 || tot.TotalTokens != 1_100_010 || tot.Input != 1_000_007 {
		t.Fatalf("totals = %+v", tot)
	}
}

func TestGroupKeepsInsertionOrderAndSorts(t *testing.T) {
	g := NewGroup[string]()
	g.Add("b", tbl(), store.AggRow{Model: "claude-opus-4-5", Counts: pricing.Counts{Input: 1_000_000}})
	g.Add("a", tbl(), store.AggRow{Model: "claude-opus-4-5", Counts: pricing.Counts{Input: 2_000_000}})
	g.Add("b", tbl(), store.AggRow{Model: "claude-opus-4-5", Counts: pricing.Counts{Input: 2_000_000}})
	if k := g.Keys(); len(k) != 2 || k[0] != "b" {
		t.Fatalf("Keys = %v", k)
	}
	if g.Get("b").Counts.Input != 3_000_000 {
		t.Fatalf("b = %+v", g.Get("b"))
	}
	byCost := g.SortedKeys(func(x, y string) bool { return g.Get(x).Cost > g.Get(y).Cost })
	if byCost[0] != "b" {
		t.Fatalf("sorted = %v", byCost)
	}
	if g.Get("zzz").Counts.Total() != 0 {
		t.Fatal("missing key should be zero")
	}
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd apps/server && go test ./internal/report/`
Expected: FAIL — package does not compile (missing `store.AggRow`, `Bucket`).

- [ ] **Step 3: Implement store queries**

`apps/server/internal/store/query.go`:
```go
package store

import (
	"context"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5"

	"cctrack/server/internal/pricing"
)

type GroupBy struct {
	Day, User, Account, Session bool
}

type Filter struct {
	From, To   time.Time
	TZ         string
	UserID     *int64
	AccountID  *int64
	SessionIDs []string
}

type AggRow struct {
	Day       string
	UserID    int64
	AccountID int64
	SessionID string
	Model     string
	Speed     string
	LongCtx   bool
	Counts    pricing.Counts
}

// Aggregate sums tokens over the filter, grouped by the requested dimensions
// plus model, speed and the long-context flag (all needed for pricing).
func (s *Store) Aggregate(ctx context.Context, g GroupBy, f Filter) ([]AggRow, error) {
	pick := func(on bool, expr, zero string) string {
		if on {
			return expr
		}
		return zero
	}
	q := fmt.Sprintf(`
		SELECT %s, %s, %s, %s, model, speed,
		       (input + cache_write_5m + cache_write_1h + cache_read) > %d AS long_ctx,
		       sum(input)::bigint, sum(output)::bigint, sum(cache_write_5m)::bigint,
		       sum(cache_write_1h)::bigint, sum(cache_read)::bigint
		FROM usage_entries
		WHERE ts >= $1 AND ts < $2 AND $3::text IS NOT NULL
		  AND ($4::bigint IS NULL OR user_id = $4)
		  AND ($5::bigint IS NULL OR account_id = $5)
		  AND (cardinality($6::text[]) = 0 OR session_id = ANY($6))
		GROUP BY 1, 2, 3, 4, 5, 6, 7`,
		pick(g.Day, "to_char(ts AT TIME ZONE $3::text, 'YYYY-MM-DD')", "''::text"),
		pick(g.User, "user_id", "0::bigint"),
		pick(g.Account, "account_id", "0::bigint"),
		pick(g.Session, "session_id", "''::text"),
		pricing.LongContextThreshold)
	// Grouping by a constant select-list column ("GROUP BY 1" over ''::text) is valid
	// Postgres; if a future version rejects it, build GROUP BY from the enabled dims only.
	tz := f.TZ
	if tz == "" {
		tz = "UTC"
	}
	ids := f.SessionIDs
	if ids == nil {
		ids = []string{}
	}
	rows, err := s.pool.Query(ctx, q, f.From, f.To, tz, f.UserID, f.AccountID, ids)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (AggRow, error) {
		var r AggRow
		err := row.Scan(&r.Day, &r.UserID, &r.AccountID, &r.SessionID, &r.Model, &r.Speed, &r.LongCtx,
			&r.Counts.Input, &r.Counts.Output, &r.Counts.CacheWrite5m, &r.Counts.CacheWrite1h, &r.Counts.CacheRead)
		return r, err
	})
}

func (s *Store) Prices(ctx context.Context) ([]pricing.Price, error) {
	rows, err := s.pool.Query(ctx, `
		SELECT model, input, cache_write_5m, cache_write_1h, cache_read, output, fast_input, fast_output,
		       long_input, long_cache_write_5m, long_cache_write_1h, long_cache_read, long_output, updated_at
		FROM model_prices ORDER BY model`)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, scanPrice)
}

func scanPrice(row pgx.CollectableRow) (pricing.Price, error) {
	var p pricing.Price
	var li, l5, l1, lr, lo *float64
	err := row.Scan(&p.Model, &p.Base.Input, &p.Base.CacheWrite5m, &p.Base.CacheWrite1h, &p.Base.CacheRead,
		&p.Base.Output, &p.FastInput, &p.FastOutput, &li, &l5, &l1, &lr, &lo, &p.UpdatedAt)
	if li != nil && l5 != nil && l1 != nil && lr != nil && lo != nil {
		p.Long = &pricing.Rates{Input: *li, CacheWrite5m: *l5, CacheWrite1h: *l1, CacheRead: *lr, Output: *lo}
	}
	return p, err
}

func (s *Store) PriceTable(ctx context.Context) (*pricing.Table, error) {
	prices, err := s.Prices(ctx)
	if err != nil {
		return nil, err
	}
	return pricing.NewTable(prices), nil
}

type Account struct {
	ID            int64
	AccountUUID   string
	OrgUUID       *string
	Email         *string
	OrgType       *string
	RateLimitTier *string
	Label         string
	Plan          string
	CreatedAt     time.Time
}

// DisplayLabel is the admin label, else the account email, else its uuid.
func (a Account) DisplayLabel() string {
	if a.Label != "" {
		return a.Label
	}
	if a.Email != nil && *a.Email != "" {
		return *a.Email
	}
	return a.AccountUUID
}

const accountCols = "id, account_uuid, org_uuid, email, org_type, rate_limit_tier, label, plan, created_at"

func scanAccount(row pgx.CollectableRow) (Account, error) {
	var a Account
	err := row.Scan(&a.ID, &a.AccountUUID, &a.OrgUUID, &a.Email, &a.OrgType, &a.RateLimitTier,
		&a.Label, &a.Plan, &a.CreatedAt)
	return a, err
}

func (s *Store) ListAccounts(ctx context.Context) ([]Account, error) {
	rows, err := s.pool.Query(ctx, `SELECT `+accountCols+` FROM claude_accounts ORDER BY id`)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, scanAccount)
}

func (s *Store) AccountsByIDs(ctx context.Context, ids []int64) (map[int64]Account, error) {
	rows, err := s.pool.Query(ctx, `SELECT `+accountCols+` FROM claude_accounts WHERE id = ANY($1)`, ids)
	if err != nil {
		return nil, err
	}
	list, err := pgx.CollectRows(rows, scanAccount)
	out := make(map[int64]Account, len(list))
	for _, a := range list {
		out[a.ID] = a
	}
	return out, err
}

type UserRef struct {
	ID          int64
	Email, Name string
}

func (s *Store) UsersByIDs(ctx context.Context, ids []int64) (map[int64]UserRef, error) {
	rows, err := s.pool.Query(ctx, `SELECT id, email, name FROM users WHERE id = ANY($1)`, ids)
	if err != nil {
		return nil, err
	}
	list, err := pgx.CollectRows(rows, func(row pgx.CollectableRow) (UserRef, error) {
		var u UserRef
		return u, row.Scan(&u.ID, &u.Email, &u.Name)
	})
	out := make(map[int64]UserRef, len(list))
	for _, u := range list {
		out[u.ID] = u
	}
	return out, err
}

type LimitSnapshot struct {
	AccountID        int64
	CapturedAt       time.Time
	FiveHourPct      *float64
	FiveHourResetsAt *time.Time
	SevenDayPct      *float64
	SevenDayResetsAt *time.Time
}

func scanLimit(row pgx.CollectableRow) (LimitSnapshot, error) {
	var l LimitSnapshot
	err := row.Scan(&l.AccountID, &l.CapturedAt, &l.FiveHourPct, &l.FiveHourResetsAt, &l.SevenDayPct, &l.SevenDayResetsAt)
	return l, err
}

const limitCols = "account_id, captured_at, five_hour_pct, five_hour_resets_at, seven_day_pct, seven_day_resets_at"

func (s *Store) LatestLimits(ctx context.Context, accountIDs []int64) ([]LimitSnapshot, error) {
	rows, err := s.pool.Query(ctx, `
		SELECT DISTINCT ON (account_id) `+limitCols+`
		FROM limit_snapshots WHERE account_id = ANY($1)
		ORDER BY account_id, captured_at DESC`, accountIDs)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, scanLimit)
}

func (s *Store) LimitHistory(ctx context.Context, accountID int64, from, to time.Time) ([]LimitSnapshot, error) {
	rows, err := s.pool.Query(ctx, `
		SELECT `+limitCols+` FROM limit_snapshots
		WHERE account_id = $1 AND captured_at >= $2 AND captured_at < $3
		ORDER BY captured_at`, accountID, from, to)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, scanLimit)
}

type SessionRow struct {
	SessionID      string
	UserID         int64
	UserEmail      string
	UserName       string
	AccountID      *int64
	DeviceHostname *string
	Project        *string
	GitBranch      *string
	Summary        *string
	StartedAt      time.Time
	EndedAt        time.Time
}

// ListSessions returns one page of a user's sessions (newest first) and the total count.
func (s *Store) ListSessions(ctx context.Context, userID int64, limit, offset int) ([]SessionRow, int, error) {
	var total int
	if err := s.pool.QueryRow(ctx, `SELECT count(*) FROM sessions WHERE user_id = $1`, userID).Scan(&total); err != nil {
		return nil, 0, err
	}
	rows, err := s.pool.Query(ctx, `
		SELECT s.session_id, s.user_id, u.email, u.name, s.account_id, d.hostname, s.project, s.git_branch,
		       s.summary, s.started_at, s.ended_at
		FROM sessions s
		JOIN users u ON u.id = s.user_id
		LEFT JOIN devices d ON d.id = s.device_id
		WHERE s.user_id = $1
		ORDER BY s.started_at DESC, s.session_id
		LIMIT $2 OFFSET $3`, userID, limit, offset)
	if err != nil {
		return nil, 0, err
	}
	list, err := pgx.CollectRows(rows, func(row pgx.CollectableRow) (SessionRow, error) {
		var r SessionRow
		err := row.Scan(&r.SessionID, &r.UserID, &r.UserEmail, &r.UserName, &r.AccountID, &r.DeviceHostname,
			&r.Project, &r.GitBranch, &r.Summary, &r.StartedAt, &r.EndedAt)
		return r, err
	})
	return list, total, err
}
```

- [ ] **Step 4: Implement report**

`apps/server/internal/report/report.go`:
```go
// Package report folds aggregated token rows into priced totals.
package report

import (
	"sort"

	"cctrack/server/internal/api"
	"cctrack/server/internal/pricing"
	"cctrack/server/internal/store"
)

type Bucket struct {
	Counts   pricing.Counts
	Cost     float64
	Unpriced int64
}

func (b *Bucket) Add(t *pricing.Table, r store.AggRow) {
	b.Counts.Add(r.Counts)
	if usd, ok := t.Cost(r.Model, r.Speed, r.LongCtx, r.Counts); ok {
		b.Cost += usd
	} else {
		b.Unpriced += r.Counts.Total()
	}
}

func (b Bucket) Totals() api.Totals {
	return api.Totals{
		Input:          b.Counts.Input,
		Output:         b.Counts.Output,
		CacheWrite5m:   b.Counts.CacheWrite5m,
		CacheWrite1h:   b.Counts.CacheWrite1h,
		CacheRead:      b.Counts.CacheRead,
		TotalTokens:    b.Counts.Total(),
		CostUsd:        b.Cost,
		UnpricedTokens: b.Unpriced,
	}
}

// Group accumulates buckets by key, remembering first-seen order.
type Group[K comparable] struct {
	m     map[K]*Bucket
	order []K
}

func NewGroup[K comparable]() *Group[K] { return &Group[K]{m: map[K]*Bucket{}} }

func (g *Group[K]) Add(k K, t *pricing.Table, r store.AggRow) {
	b, ok := g.m[k]
	if !ok {
		b = &Bucket{}
		g.m[k] = b
		g.order = append(g.order, k)
	}
	b.Add(t, r)
}

func (g *Group[K]) Get(k K) Bucket {
	if b, ok := g.m[k]; ok {
		return *b
	}
	return Bucket{}
}

func (g *Group[K]) Keys() []K { return append([]K(nil), g.order...) }

func (g *Group[K]) SortedKeys(less func(a, b K) bool) []K {
	keys := g.Keys()
	sort.SliceStable(keys, func(i, j int) bool { return less(keys[i], keys[j]) })
	return keys
}
```

- [ ] **Step 5: Run report tests**

Run: `cd apps/server && go test ./internal/report/`
Expected: PASS.

- [ ] **Step 6: Implement handlers**

`apps/server/internal/httpapi/rangeparams.go`:
```go
package httpapi

import (
	"errors"
	"time"

	"cctrack/server/internal/store"
)

const maxRange = 400 * 24 * time.Hour

func parseRange(from, to time.Time, tz *string) (store.Filter, error) {
	zone := "UTC"
	if tz != nil && *tz != "" {
		zone = *tz
	}
	if _, err := time.LoadLocation(zone); err != nil {
		return store.Filter{}, errors.New("unknown tz: " + zone)
	}
	if !to.After(from) {
		return store.Filter{}, errors.New("to must be after from")
	}
	if to.Sub(from) > maxRange {
		return store.Filter{}, errors.New("range must be at most 400 days")
	}
	return store.Filter{From: from, To: to, TZ: zone}, nil
}

func pageParams(limit, offset *int) (int, int) {
	l, o := 50, 0
	if limit != nil && *limit > 0 {
		l = min(*limit, 200)
	}
	if offset != nil && *offset > 0 {
		o = *offset
	}
	return l, o
}
```

`apps/server/internal/httpapi/convert.go`:
```go
package httpapi

import (
	"context"
	"time"

	"cctrack/server/internal/api"
	"cctrack/server/internal/report"
	"cctrack/server/internal/store"
)

var (
	epoch   = time.Unix(0, 0).UTC()
	farFuture = time.Date(9999, 1, 1, 0, 0, 0, 0, time.UTC)
)

func accountRef(a store.Account) api.AccountRef {
	return api.AccountRef{Id: a.ID, Label: a.DisplayLabel(), Plan: api.Plan(a.Plan), Email: a.Email}
}

func accountToAPI(a store.Account) api.Account {
	return api.Account{
		Id: a.ID, AccountUuid: a.AccountUUID, OrgUuid: a.OrgUUID, Email: a.Email, OrgType: a.OrgType,
		RateLimitTier: a.RateLimitTier, Label: a.DisplayLabel(), Plan: api.Plan(a.Plan), CreatedAt: a.CreatedAt,
	}
}

func userRef(u store.UserRef) api.UserRef { return api.UserRef{Id: u.ID, Email: u.Email, Name: u.Name} }

func limitToAPI(l store.LimitSnapshot) api.LimitSnapshot {
	return api.LimitSnapshot{
		AccountId: l.AccountID, CapturedAt: l.CapturedAt,
		FiveHourPct: l.FiveHourPct, FiveHourResetsAt: l.FiveHourResetsAt,
		SevenDayPct: l.SevenDayPct, SevenDayResetsAt: l.SevenDayResetsAt,
	}
}

func limitsToAPI(ls []store.LimitSnapshot) []api.LimitSnapshot {
	out := make([]api.LimitSnapshot, 0, len(ls))
	for _, l := range ls {
		out = append(out, limitToAPI(l))
	}
	return out
}

func accountIDs(rows []store.AggRow) []int64 {
	seen := map[int64]bool{}
	var ids []int64
	for _, r := range rows {
		if !seen[r.AccountID] {
			seen[r.AccountID] = true
			ids = append(ids, r.AccountID)
		}
	}
	return ids
}

// sessionPage prices each session in rows and converts to the API shape.
func (s *Server) sessionPage(ctx context.Context, rows []store.SessionRow, total int) (api.SessionPage, error) {
	page := api.SessionPage{Items: []api.Session{}, Total: total}
	if len(rows) == 0 {
		return page, nil
	}
	ids := make([]string, len(rows))
	acctIDs := []int64{}
	for i, r := range rows {
		ids[i] = r.SessionID
		if r.AccountID != nil {
			acctIDs = append(acctIDs, *r.AccountID)
		}
	}
	table, err := s.store.PriceTable(ctx)
	if err != nil {
		return page, err
	}
	agg, err := s.store.Aggregate(ctx, store.GroupBy{Session: true}, store.Filter{From: epoch, To: farFuture, SessionIDs: ids})
	if err != nil {
		return page, err
	}
	accounts, err := s.store.AccountsByIDs(ctx, acctIDs)
	if err != nil {
		return page, err
	}
	g := report.NewGroup[string]()
	for _, r := range agg {
		g.Add(r.SessionID, table, r)
	}
	for _, r := range rows {
		item := api.Session{
			SessionId: r.SessionID,
			User:      api.UserRef{Id: r.UserID, Email: r.UserEmail, Name: r.UserName},
			DeviceHostname: r.DeviceHostname, Project: r.Project, GitBranch: r.GitBranch, Summary: r.Summary,
			StartedAt: r.StartedAt, EndedAt: r.EndedAt,
			Totals:    g.Get(r.SessionID).Totals(),
		}
		if r.AccountID != nil {
			if a, ok := accounts[*r.AccountID]; ok {
				ref := accountRef(a)
				item.Account = &ref
			}
		}
		page.Items = append(page.Items, item)
	}
	return page, nil
}
```

Delete `GetMyUsage` and `ListMySessions` from `stubs.go`, then create `apps/server/internal/httpapi/me.go`:
```go
package httpapi

import (
	"net/http"
	"strconv"

	"cctrack/server/internal/api"
	"cctrack/server/internal/pricing"
	"cctrack/server/internal/report"
	"cctrack/server/internal/store"
)

type dayKey struct {
	Day, Key string
}

func series(g *report.Group[dayKey], label func(key string) string) []api.SeriesPoint {
	keys := g.SortedKeys(func(a, b dayKey) bool {
		if a.Day != b.Day {
			return a.Day < b.Day
		}
		return a.Key < b.Key
	})
	out := make([]api.SeriesPoint, 0, len(keys))
	for _, k := range keys {
		b := g.Get(k)
		out = append(out, api.SeriesPoint{Date: k.Day, Key: k.Key, Label: label(k.Key),
			TotalTokens: b.Counts.Total(), CostUsd: b.Cost})
	}
	return out
}

func byCostDesc[K comparable](g *report.Group[K]) []K {
	return g.SortedKeys(func(a, b K) bool {
		ga, gb := g.Get(a), g.Get(b)
		if ga.Cost != gb.Cost {
			return ga.Cost > gb.Cost
		}
		return ga.Counts.Total() > gb.Counts.Total()
	})
}

func (s *Server) GetMyUsage(w http.ResponseWriter, r *http.Request, params api.GetMyUsageParams) {
	p, ok := s.requireUser(w, r)
	if !ok {
		return
	}
	f, err := parseRange(params.From, params.To, params.Tz)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	f.UserID = &p.User.ID
	ctx := r.Context()
	table, err := s.store.PriceTable(ctx)
	if err != nil {
		s.internal(w, err)
		return
	}
	rows, err := s.store.Aggregate(ctx, store.GroupBy{Day: true, Account: true}, f)
	if err != nil {
		s.internal(w, err)
		return
	}
	ids := accountIDs(rows)
	accounts, err := s.store.AccountsByIDs(ctx, ids)
	if err != nil {
		s.internal(w, err)
		return
	}
	limits, err := s.store.LatestLimits(ctx, ids)
	if err != nil {
		s.internal(w, err)
		return
	}

	var total report.Bucket
	byModel := report.NewGroup[string]()
	byAccount := report.NewGroup[int64]()
	daily := report.NewGroup[dayKey]()
	for _, row := range rows {
		model := pricing.Normalize(row.Model)
		total.Add(table, row)
		byModel.Add(model, table, row)
		byAccount.Add(row.AccountID, table, row)
		daily.Add(dayKey{row.Day, model}, table, row)
	}

	out := api.Usage{
		Totals:       total.Totals(),
		ByModel:      []api.BreakdownRow{},
		ByAccount:    []api.BreakdownRow{},
		DailyByModel: series(daily, func(k string) string { return k }),
		Limits:       limitsToAPI(limits),
	}
	for _, m := range byCostDesc(byModel) {
		out.ByModel = append(out.ByModel, api.BreakdownRow{Key: m, Label: m, Totals: byModel.Get(m).Totals()})
	}
	for _, id := range byCostDesc(byAccount) {
		out.ByAccount = append(out.ByAccount, api.BreakdownRow{
			Key: strconv.FormatInt(id, 10), Label: accounts[id].DisplayLabel(), Totals: byAccount.Get(id).Totals()})
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) ListMySessions(w http.ResponseWriter, r *http.Request, params api.ListMySessionsParams) {
	p, ok := s.requireUser(w, r)
	if !ok {
		return
	}
	limit, offset := pageParams(params.Limit, params.Offset)
	rows, total, err := s.store.ListSessions(r.Context(), p.User.ID, limit, offset)
	if err != nil {
		s.internal(w, err)
		return
	}
	page, err := s.sessionPage(r.Context(), rows, total)
	if err != nil {
		s.internal(w, err)
		return
	}
	writeJSON(w, http.StatusOK, page)
}
```

- [ ] **Step 7: DB-backed tests**

`apps/server/internal/httpapi/me_test.go`:
```go
package httpapi

import (
	"math"
	"net/http"
	"net/url"
	"testing"
	"time"

	"cctrack/server/internal/api"
)

func usageURL(from, to time.Time, tz string) string {
	q := url.Values{"from": {from.Format(time.RFC3339)}, "to": {to.Format(time.RFC3339)}}
	if tz != "" {
		q.Set("tz", tz)
	}
	return "/api/me/usage?" + q.Encode()
}

func TestMyUsageTotalsCostAndBreakdowns(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	other := h.loginCLI("other@example.com")

	req := ingestFixture("s1", "acc-1",
		entry("s1", "acc-1", "m1", "claude-opus-4-5-20251101", t0, 1_000_000, 100_000), // $5 + $2.5
		entry("s1", "acc-1", "m2", "mystery-model", t0, 10, 0))                       // unpriced
	req.Accounts = append(req.Accounts, ingestFixture("s2", "acc-2").Accounts...)
	req.Sessions = append(req.Sessions, api.IngestSession{SessionId: "s2", StartedAt: t0, EndedAt: t0})
	req.Entries = append(req.Entries, entry("s2", "acc-2", "m3", "claude-opus-4-5", t0, 2_000_000, 0)) // $10
	if rec := h.do("POST", "/api/ingest", req, bearer(tok)); rec.Code != 200 {
		t.Fatalf("ingest: %d %s", rec.Code, rec.Body)
	}
	// Another user's usage must not leak into mine.
	h.do("POST", "/api/ingest", ingestFixture("s9", "acc-1",
		entry("s9", "acc-1", "m9", "claude-opus-4-5", t0, 9_000_000, 0)), bearer(other))

	rec := h.do("GET", usageURL(t0.Add(-time.Hour), t0.Add(time.Hour), ""), nil, bearer(tok))
	if rec.Code != 200 {
		t.Fatalf("usage: %d %s", rec.Code, rec.Body)
	}
	u := decode[api.Usage](t, rec)
	if math.Abs(u.Totals.CostUsd-17.5) > 1e-9 || u.Totals.UnpricedTokens != 10 {
		t.Fatalf("totals = %+v", u.Totals)
	}
	if len(u.ByAccount) != 2 || u.ByAccount[0].Label != "acc-2" {
		t.Fatalf("by_account = %+v", u.ByAccount)
	}
	if len(u.ByModel) != 2 || u.ByModel[0].Key != "claude-opus-4-5" {
		t.Fatalf("by_model = %+v", u.ByModel)
	}
}

func TestMyUsageBucketsDaysInRequestedTZ(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	late := time.Date(2026, 10, 1, 23, 30, 0, 0, time.UTC) // 05:00 on Oct 2 in Asia/Kolkata
	h.do("POST", "/api/ingest", ingestFixture("s1", "acc-1",
		entry("s1", "acc-1", "m1", "claude-opus-4-5", late, 1, 1)), bearer(tok))

	rec := h.do("GET", usageURL(late.Add(-24*time.Hour), late.Add(24*time.Hour), "Asia/Kolkata"), nil, bearer(tok))
	u := decode[api.Usage](t, rec)
	if len(u.DailyByModel) != 1 || u.DailyByModel[0].Date != "2026-10-02" {
		t.Fatalf("daily = %+v", u.DailyByModel)
	}
	rec = h.do("GET", usageURL(late.Add(-24*time.Hour), late.Add(24*time.Hour), ""), nil, bearer(tok))
	if d := decode[api.Usage](t, rec).DailyByModel[0].Date; d != "2026-10-01" {
		t.Fatalf("UTC day = %q", d)
	}
}

func TestMyUsageRejectsBadRange(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	for _, u := range []string{
		usageURL(t0, t0.Add(time.Hour), "Mars/Olympus"),
		usageURL(t0, t0.Add(-time.Hour), ""),
		usageURL(t0, t0.Add(500*24*time.Hour), ""),
		"/api/me/usage?from=yesterday&to=today",
	} {
		if rec := h.do("GET", u, nil, bearer(tok)); rec.Code != http.StatusBadRequest {
			t.Errorf("%s: got %d, want 400", u, rec.Code)
		}
	}
}

func TestMyUsageLatestLimit(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	for i, pct := range []float64{10, 55} {
		p := pct
		req := ingestFixture("s1", "acc-1", entry("s1", "acc-1", "m"+string(rune('a'+i)), "claude-opus-4-5", t0, 1, 1))
		req.LimitSnapshot = &api.LimitSnapshotIn{AccountUuid: "acc-1", CapturedAt: t0.Add(time.Duration(i) * time.Minute), FiveHourPct: &p}
		h.do("POST", "/api/ingest", req, bearer(tok))
	}
	u := decode[api.Usage](t, h.do("GET", usageURL(t0.Add(-time.Hour), t0.Add(time.Hour), ""), nil, bearer(tok)))
	if len(u.Limits) != 1 || *u.Limits[0].FiveHourPct != 55 {
		t.Fatalf("limits = %+v", u.Limits)
	}
}

func TestMySessionsPagedWithCostAndSummary(t *testing.T) {
	h := newHarness(t)
	tok := h.loginCLI("dev@example.com")
	h.do("POST", "/api/ingest", ingestFixture("old", "acc-1",
		entry("old", "acc-1", "m1", "claude-opus-4-5", t0, 1_000_000, 0)), bearer(tok))
	h.do("POST", "/api/ingest", ingestFixture("new", "acc-1",
		entry("new", "acc-1", "m2", "claude-opus-4-5", t0.Add(time.Hour), 2_000_000, 0)), bearer(tok))
	h.do("POST", "/api/sessions/new/summary", map[string]string{"summary": "Built the thing."}, bearer(tok))

	page := decode[api.SessionPage](t, h.do("GET", "/api/me/sessions?limit=1", nil, bearer(tok)))
	if page.Total != 2 || len(page.Items) != 1 {
		t.Fatalf("page = %+v", page)
	}
	s := page.Items[0]
	if s.SessionId != "new" || *s.Summary != "Built the thing." || math.Abs(s.Totals.CostUsd-10) > 1e-9 ||
		s.Account == nil || *s.Project != "proj" || *s.DeviceHostname != "laptop" {
		t.Fatalf("item = %+v", s)
	}
	page = decode[api.SessionPage](t, h.do("GET", "/api/me/sessions?limit=1&offset=1", nil, bearer(tok)))
	if page.Items[0].SessionId != "old" {
		t.Fatalf("second page = %+v", page.Items)
	}
}
```

- [ ] **Step 8: Run tests**

Run: `cd apps/server && go vet ./... && go test ./...`
Expected: PASS (DB tests SKIP without `TEST_DATABASE_URL`; report which).

- [ ] **Step 9: Commit**

```bash
git add apps/server
git commit -m "feat(server): priced aggregation and user usage/session endpoints

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Admin endpoints

**Files:**
- Create: `apps/server/internal/store/admin.go`
- Create: `apps/server/internal/httpapi/admin.go`, `admin_test.go`
- Delete: `apps/server/internal/httpapi/stubs.go` (all remaining operations implemented here)

**Interfaces:**
- Consumes: everything from Task 7.
- Produces:
  - `store.AdminUser{ID int64; Email, Name string; CreatedAt time.Time; LastSeenAt *time.Time}`; `(*Store).ListUsers(ctx) ([]AdminUser, error)`
  - `store.Token{ID int64; User UserRef; DeviceName string; CreatedAt time.Time; LastUsedAt, RevokedAt *time.Time}`; `(*Store).ListTokens(ctx) ([]Token, error)`
  - `(*Store).UpdateAccount(ctx, id int64, label, plan *string) (Account, error)`
  - `(*Store).UpsertPrice(ctx, pricing.Price) (pricing.Price, error)`

- [ ] **Step 1: Store admin queries**

`apps/server/internal/store/admin.go`:
```go
package store

import (
	"context"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"

	"cctrack/server/internal/pricing"
)

type AdminUser struct {
	ID         int64
	Email      string
	Name       string
	CreatedAt  time.Time
	LastSeenAt *time.Time
}

func (s *Store) ListUsers(ctx context.Context) ([]AdminUser, error) {
	rows, err := s.pool.Query(ctx, `
		SELECT u.id, u.email, u.name, u.created_at, max(d.last_seen)
		FROM users u LEFT JOIN devices d ON d.user_id = u.id
		GROUP BY u.id ORDER BY u.email`)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (AdminUser, error) {
		var u AdminUser
		return u, row.Scan(&u.ID, &u.Email, &u.Name, &u.CreatedAt, &u.LastSeenAt)
	})
}

type Token struct {
	ID         int64
	User       UserRef
	DeviceName string
	CreatedAt  time.Time
	LastUsedAt *time.Time
	RevokedAt  *time.Time
}

func (s *Store) ListTokens(ctx context.Context) ([]Token, error) {
	rows, err := s.pool.Query(ctx, `
		SELECT t.id, u.id, u.email, u.name, t.device_name, t.created_at, t.last_used_at, t.revoked_at
		FROM api_tokens t JOIN users u ON u.id = t.user_id
		ORDER BY t.revoked_at IS NOT NULL, t.created_at DESC`)
	if err != nil {
		return nil, err
	}
	return pgx.CollectRows(rows, func(row pgx.CollectableRow) (Token, error) {
		var t Token
		return t, row.Scan(&t.ID, &t.User.ID, &t.User.Email, &t.User.Name, &t.DeviceName, &t.CreatedAt,
			&t.LastUsedAt, &t.RevokedAt)
	})
}

func (s *Store) UpdateAccount(ctx context.Context, id int64, label, plan *string) (Account, error) {
	rows, err := s.pool.Query(ctx, `
		UPDATE claude_accounts SET label = COALESCE($2, label), plan = COALESCE($3, plan)
		WHERE id = $1 RETURNING `+accountCols, id, label, plan)
	if err != nil {
		return Account{}, err
	}
	a, err := pgx.CollectExactlyOneRow(rows, scanAccount)
	if errors.Is(err, pgx.ErrNoRows) {
		return Account{}, ErrNotFound
	}
	return a, err
}

func (s *Store) UpsertPrice(ctx context.Context, p pricing.Price) (pricing.Price, error) {
	var li, l5, l1, lr, lo *float64
	if p.Long != nil {
		li, l5, l1, lr, lo = &p.Long.Input, &p.Long.CacheWrite5m, &p.Long.CacheWrite1h, &p.Long.CacheRead, &p.Long.Output
	}
	rows, err := s.pool.Query(ctx, `
		INSERT INTO model_prices (model, input, cache_write_5m, cache_write_1h, cache_read, output,
		  fast_input, fast_output, long_input, long_cache_write_5m, long_cache_write_1h, long_cache_read, long_output, updated_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, now())
		ON CONFLICT (model) DO UPDATE SET
		  input = EXCLUDED.input, cache_write_5m = EXCLUDED.cache_write_5m, cache_write_1h = EXCLUDED.cache_write_1h,
		  cache_read = EXCLUDED.cache_read, output = EXCLUDED.output, fast_input = EXCLUDED.fast_input,
		  fast_output = EXCLUDED.fast_output, long_input = EXCLUDED.long_input,
		  long_cache_write_5m = EXCLUDED.long_cache_write_5m, long_cache_write_1h = EXCLUDED.long_cache_write_1h,
		  long_cache_read = EXCLUDED.long_cache_read, long_output = EXCLUDED.long_output, updated_at = now()
		RETURNING model, input, cache_write_5m, cache_write_1h, cache_read, output, fast_input, fast_output,
		  long_input, long_cache_write_5m, long_cache_write_1h, long_cache_read, long_output, updated_at`,
		p.Model, p.Base.Input, p.Base.CacheWrite5m, p.Base.CacheWrite1h, p.Base.CacheRead, p.Base.Output,
		p.FastInput, p.FastOutput, li, l5, l1, lr, lo)
	if err != nil {
		return pricing.Price{}, err
	}
	return pgx.CollectExactlyOneRow(rows, scanPrice)
}
```

- [ ] **Step 2: Admin handlers**

Delete `apps/server/internal/httpapi/stubs.go`, then create `apps/server/internal/httpapi/admin.go`:
```go
package httpapi

import (
	"errors"
	"net/http"
	"strconv"
	"strings"

	"cctrack/server/internal/api"
	"cctrack/server/internal/pricing"
	"cctrack/server/internal/report"
	"cctrack/server/internal/store"
)

var validPlans = map[string]bool{
	"pro": true, "max_5x": true, "max_20x": true, "team": true,
	"enterprise": true, "api_key": true, "unknown": true,
}

type userAcct struct{ User, Account int64 }

func (s *Server) AdminByLogin(w http.ResponseWriter, r *http.Request, params api.AdminByLoginParams) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	f, err := parseRange(params.From, params.To, params.Tz)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	ctx := r.Context()
	table, err := s.store.PriceTable(ctx)
	if err != nil {
		s.internal(w, err)
		return
	}
	rows, err := s.store.Aggregate(ctx, store.GroupBy{Day: true, User: true, Account: true}, f)
	if err != nil {
		s.internal(w, err)
		return
	}
	userIDs := []int64{}
	for _, row := range rows {
		userIDs = append(userIDs, row.UserID)
	}
	users, err := s.store.UsersByIDs(ctx, userIDs)
	if err != nil {
		s.internal(w, err)
		return
	}
	accounts, err := s.store.AccountsByIDs(ctx, accountIDs(rows))
	if err != nil {
		s.internal(w, err)
		return
	}

	byUser := report.NewGroup[int64]()
	byUserAcct := report.NewGroup[userAcct]()
	daily := report.NewGroup[dayKey]()
	for _, row := range rows {
		byUser.Add(row.UserID, table, row)
		byUserAcct.Add(userAcct{row.UserID, row.AccountID}, table, row)
		daily.Add(dayKey{row.Day, strconv.FormatInt(row.UserID, 10)}, table, row)
	}
	out := api.ByLogin{Users: []api.LoginUsage{}}
	for _, uid := range byCostDesc(byUser) {
		lu := api.LoginUsage{User: userRef(users[uid]), Totals: byUser.Get(uid).Totals(), ByAccount: []api.AccountUsage{}}
		for _, k := range byCostDesc(byUserAcct) {
			if k.User == uid {
				lu.ByAccount = append(lu.ByAccount, api.AccountUsage{
					Account: accountRef(accounts[k.Account]), Totals: byUserAcct.Get(k).Totals()})
			}
		}
		out.Users = append(out.Users, lu)
	}
	out.DailyByUser = series(daily, func(k string) string {
		id, _ := strconv.ParseInt(k, 10, 64)
		return users[id].Email
	})
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) AdminByAccount(w http.ResponseWriter, r *http.Request, params api.AdminByAccountParams) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	f, err := parseRange(params.From, params.To, params.Tz)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	ctx := r.Context()
	table, err := s.store.PriceTable(ctx)
	if err != nil {
		s.internal(w, err)
		return
	}
	rows, err := s.store.Aggregate(ctx, store.GroupBy{Day: true, User: true, Account: true}, f)
	if err != nil {
		s.internal(w, err)
		return
	}
	accounts, err := s.store.ListAccounts(ctx)
	if err != nil {
		s.internal(w, err)
		return
	}
	userIDs := []int64{}
	for _, row := range rows {
		userIDs = append(userIDs, row.UserID)
	}
	users, err := s.store.UsersByIDs(ctx, userIDs)
	if err != nil {
		s.internal(w, err)
		return
	}

	byAcct := report.NewGroup[int64]()
	byAcctUser := report.NewGroup[userAcct]()
	daily := map[int64]*report.Group[dayKey]{}
	for _, row := range rows {
		byAcct.Add(row.AccountID, table, row)
		byAcctUser.Add(userAcct{row.UserID, row.AccountID}, table, row)
		if daily[row.AccountID] == nil {
			daily[row.AccountID] = report.NewGroup[dayKey]()
		}
		daily[row.AccountID].Add(dayKey{row.Day, strconv.FormatInt(row.UserID, 10)}, table, row)
	}
	userLabel := func(k string) string {
		id, _ := strconv.ParseInt(k, 10, 64)
		return users[id].Email
	}

	out := api.ByAccount{Accounts: []api.AccountDetail{}}
	for _, a := range accounts {
		total := byAcct.Get(a.ID)
		detail := api.AccountDetail{
			Account: accountToAPI(a), Totals: total.Totals(),
			ByUser: []api.UserShare{}, DailyByUser: []api.SeriesPoint{},
		}
		for _, k := range byCostDesc(byAcctUser) {
			if k.Account != a.ID {
				continue
			}
			b := byAcctUser.Get(k)
			share := 0.0
			if total.Cost > 0 {
				share = b.Cost / total.Cost
			} else if total.Counts.Total() > 0 {
				share = float64(b.Counts.Total()) / float64(total.Counts.Total())
			}
			detail.ByUser = append(detail.ByUser, api.UserShare{User: userRef(users[k.User]), Totals: b.Totals(), Share: share})
		}
		if g := daily[a.ID]; g != nil {
			detail.DailyByUser = series(g, userLabel)
		}
		limits, err := s.store.LimitHistory(ctx, a.ID, f.From, f.To)
		if err != nil {
			s.internal(w, err)
			return
		}
		detail.Limits = limitsToAPI(limits)
		out.Accounts = append(out.Accounts, detail)
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) AdminListAccounts(w http.ResponseWriter, r *http.Request) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	accounts, err := s.store.ListAccounts(r.Context())
	if err != nil {
		s.internal(w, err)
		return
	}
	out := make([]api.Account, 0, len(accounts))
	for _, a := range accounts {
		out = append(out, accountToAPI(a))
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) AdminUpdateAccount(w http.ResponseWriter, r *http.Request, id int64) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	var body api.AccountPatch
	if !decodeJSON(w, r, &body) {
		return
	}
	var label, plan *string
	if body.Label != nil {
		l := strings.TrimSpace(*body.Label)
		if len(l) > 100 {
			writeError(w, http.StatusBadRequest, "label must be at most 100 characters")
			return
		}
		label = &l
	}
	if body.Plan != nil {
		p := string(*body.Plan)
		if !validPlans[p] {
			writeError(w, http.StatusBadRequest, "invalid plan")
			return
		}
		plan = &p
	}
	a, err := s.store.UpdateAccount(r.Context(), id, label, plan)
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, "account not found")
		return
	}
	if err != nil {
		s.internal(w, err)
		return
	}
	writeJSON(w, http.StatusOK, accountToAPI(a))
}

func (s *Server) AdminListUsers(w http.ResponseWriter, r *http.Request) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	users, err := s.store.ListUsers(r.Context())
	if err != nil {
		s.internal(w, err)
		return
	}
	out := make([]api.AdminUser, 0, len(users))
	for _, u := range users {
		out = append(out, api.AdminUser{Id: u.ID, Email: u.Email, Name: u.Name, IsAdmin: s.isAdmin(u.Email),
			CreatedAt: u.CreatedAt, LastSeenAt: u.LastSeenAt})
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) AdminListUserSessions(w http.ResponseWriter, r *http.Request, id int64, params api.AdminListUserSessionsParams) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	limit, offset := pageParams(params.Limit, params.Offset)
	rows, total, err := s.store.ListSessions(r.Context(), id, limit, offset)
	if err != nil {
		s.internal(w, err)
		return
	}
	page, err := s.sessionPage(r.Context(), rows, total)
	if err != nil {
		s.internal(w, err)
		return
	}
	writeJSON(w, http.StatusOK, page)
}

func (s *Server) AdminListTokens(w http.ResponseWriter, r *http.Request) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	tokens, err := s.store.ListTokens(r.Context())
	if err != nil {
		s.internal(w, err)
		return
	}
	out := make([]api.ApiToken, 0, len(tokens))
	for _, t := range tokens {
		out = append(out, api.ApiToken{Id: t.ID, User: userRef(t.User), DeviceName: t.DeviceName,
			CreatedAt: t.CreatedAt, LastUsedAt: t.LastUsedAt, RevokedAt: t.RevokedAt})
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) AdminRevokeToken(w http.ResponseWriter, r *http.Request, id int64) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	err := s.store.RevokeToken(r.Context(), id)
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, "token not found or already revoked")
		return
	}
	if err != nil {
		s.internal(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func priceToAPI(p pricing.Price) api.ModelPrice {
	rates := api.PriceRates{
		Input: p.Base.Input, CacheWrite5m: p.Base.CacheWrite5m, CacheWrite1h: p.Base.CacheWrite1h,
		CacheRead: p.Base.CacheRead, Output: p.Base.Output, FastInput: p.FastInput, FastOutput: p.FastOutput,
	}
	if p.Long != nil {
		l := *p.Long
		rates.LongInput, rates.LongCacheWrite5m, rates.LongCacheWrite1h = &l.Input, &l.CacheWrite5m, &l.CacheWrite1h
		rates.LongCacheRead, rates.LongOutput = &l.CacheRead, &l.Output
	}
	return api.ModelPrice{Model: p.Model, Rates: rates, UpdatedAt: p.UpdatedAt}
}

func (s *Server) AdminListPrices(w http.ResponseWriter, r *http.Request) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	prices, err := s.store.Prices(r.Context())
	if err != nil {
		s.internal(w, err)
		return
	}
	out := make([]api.ModelPrice, 0, len(prices))
	for _, p := range prices {
		out = append(out, priceToAPI(p))
	}
	writeJSON(w, http.StatusOK, out)
}

func (s *Server) AdminPutPrice(w http.ResponseWriter, r *http.Request, model string) {
	if _, ok := s.requireAdmin(w, r); !ok {
		return
	}
	var body api.PriceRates
	if !decodeJSON(w, r, &body) {
		return
	}
	model = pricing.Normalize(model)
	if model == "" {
		writeError(w, http.StatusBadRequest, "model is required")
		return
	}
	for _, v := range []float64{body.Input, body.CacheWrite5m, body.CacheWrite1h, body.CacheRead, body.Output} {
		if v < 0 {
			writeError(w, http.StatusBadRequest, "rates must be >= 0")
			return
		}
	}
	if (body.FastInput == nil) != (body.FastOutput == nil) {
		writeError(w, http.StatusBadRequest, "set both fast_input and fast_output, or neither")
		return
	}
	p := pricing.Price{
		Model: model,
		Base: pricing.Rates{Input: body.Input, CacheWrite5m: body.CacheWrite5m, CacheWrite1h: body.CacheWrite1h,
			CacheRead: body.CacheRead, Output: body.Output},
		FastInput: body.FastInput, FastOutput: body.FastOutput,
	}
	longs := []*float64{body.LongInput, body.LongCacheWrite5m, body.LongCacheWrite1h, body.LongCacheRead, body.LongOutput}
	set := 0
	for _, v := range longs {
		if v != nil {
			set++
		}
	}
	switch set {
	case 0:
	case len(longs):
		p.Long = &pricing.Rates{Input: *body.LongInput, CacheWrite5m: *body.LongCacheWrite5m,
			CacheWrite1h: *body.LongCacheWrite1h, CacheRead: *body.LongCacheRead, Output: *body.LongOutput}
	default:
		writeError(w, http.StatusBadRequest, "set all long_* rates, or none")
		return
	}
	saved, err := s.store.UpsertPrice(r.Context(), p)
	if err != nil {
		s.internal(w, err)
		return
	}
	writeJSON(w, http.StatusOK, priceToAPI(saved))
}
```

- [ ] **Step 3: Admin tests**

`apps/server/internal/httpapi/admin_test.go`:
```go
package httpapi

import (
	"fmt"
	"math"
	"net/http"
	"net/url"
	"testing"
	"time"

	"cctrack/server/internal/api"
)

func rangeQ(path string) string {
	q := url.Values{"from": {t0.Add(-time.Hour).Format(time.RFC3339)}, "to": {t0.Add(time.Hour).Format(time.RFC3339)}}
	return path + "?" + q.Encode()
}

// seedTwoUsersTwoAccounts: alice uses acc-1 ($5) and acc-2 ($10); bob uses acc-1 ($15).
func seedTwoUsersTwoAccounts(t *testing.T, h *harness) (alice, bob string) {
	alice = h.loginCLI("alice@example.com")
	bob = h.loginCLI("bob@example.com")
	req := ingestFixture("a1", "acc-1", entry("a1", "acc-1", "m1", "claude-opus-4-5", t0, 1_000_000, 0))
	req.Accounts = append(req.Accounts, api.IngestAccount{AccountUuid: "acc-2"})
	req.Sessions = append(req.Sessions, api.IngestSession{SessionId: "a2", StartedAt: t0, EndedAt: t0})
	req.Entries = append(req.Entries, entry("a2", "acc-2", "m2", "claude-opus-4-5", t0, 2_000_000, 0))
	if rec := h.do("POST", "/api/ingest", req, bearer(alice)); rec.Code != 200 {
		t.Fatalf("alice ingest: %d %s", rec.Code, rec.Body)
	}
	if rec := h.do("POST", "/api/ingest", ingestFixture("b1", "acc-1",
		entry("b1", "acc-1", "m3", "claude-opus-4-5", t0, 3_000_000, 0)), bearer(bob)); rec.Code != 200 {
		t.Fatalf("bob ingest: %d %s", rec.Code, rec.Body)
	}
	return alice, bob
}

func TestAdminEndpointsRequireAdmin(t *testing.T) {
	h := newHarness(t)
	c := h.loginWeb("dev@example.com")
	for _, p := range []string{rangeQ("/api/admin/by-login"), rangeQ("/api/admin/by-account"),
		"/api/admin/accounts", "/api/admin/users", "/api/admin/tokens", "/api/admin/prices"} {
		if rec := h.do("GET", p, nil, withCookie(c)); rec.Code != http.StatusForbidden {
			t.Errorf("%s: got %d, want 403", p, rec.Code)
		}
	}
}

func TestAdminByLogin(t *testing.T) {
	h := newHarness(t)
	seedTwoUsersTwoAccounts(t, h)
	admin := h.loginWeb("admin@example.com")
	out := decode[api.ByLogin](t, h.do("GET", rangeQ("/api/admin/by-login"), nil, withCookie(admin)))
	if len(out.Users) != 2 {
		t.Fatalf("users = %+v", out.Users)
	}
	// Both spent $15; alice's split is acc-2 $10 then acc-1 $5.
	for _, u := range out.Users {
		if math.Abs(u.Totals.CostUsd-15) > 1e-9 {
			t.Fatalf("%s cost = %v", u.User.Email, u.Totals.CostUsd)
		}
		if u.User.Email == "alice@example.com" {
			if len(u.ByAccount) != 2 || u.ByAccount[0].Account.Label != "acc-2" {
				t.Fatalf("alice by_account = %+v", u.ByAccount)
			}
		}
	}
	if len(out.DailyByUser) != 2 {
		t.Fatalf("daily = %+v", out.DailyByUser)
	}
}

func TestAdminByAccountShares(t *testing.T) {
	h := newHarness(t)
	seedTwoUsersTwoAccounts(t, h)
	admin := h.loginWeb("admin@example.com")
	out := decode[api.ByAccount](t, h.do("GET", rangeQ("/api/admin/by-account"), nil, withCookie(admin)))
	if len(out.Accounts) != 2 {
		t.Fatalf("accounts = %+v", out.Accounts)
	}
	acc1 := out.Accounts[0]
	if acc1.Account.AccountUuid != "acc-1" || math.Abs(acc1.Totals.CostUsd-20) > 1e-9 {
		t.Fatalf("acc-1 = %+v", acc1)
	}
	sum := 0.0
	for _, u := range acc1.ByUser {
		sum += u.Share
	}
	if len(acc1.ByUser) != 2 || acc1.ByUser[0].User.Email != "bob@example.com" ||
		math.Abs(acc1.ByUser[0].Share-0.75) > 1e-9 || math.Abs(sum-1) > 1e-9 {
		t.Fatalf("acc-1 by_user = %+v", acc1.ByUser)
	}
}

func TestAdminUpdateAccount(t *testing.T) {
	h := newHarness(t)
	seedTwoUsersTwoAccounts(t, h)
	admin := h.loginWeb("admin@example.com")
	accounts := decode[[]api.Account](t, h.do("GET", "/api/admin/accounts", nil, withCookie(admin)))
	id := accounts[0].Id
	path := fmt.Sprintf("/api/admin/accounts/%d", id)

	rec := h.do("PATCH", path, map[string]string{"label": "Team Max", "plan": "max_20x"}, withCookie(admin), withCSRF())
	if rec.Code != 200 {
		t.Fatalf("patch: %d %s", rec.Code, rec.Body)
	}
	if a := decode[api.Account](t, rec); a.Label != "Team Max" || a.Plan != "max_20x" {
		t.Fatalf("account = %+v", a)
	}
	if rec := h.do("PATCH", path, map[string]string{"plan": "gold"}, withCookie(admin), withCSRF()); rec.Code != http.StatusBadRequest {
		t.Fatalf("invalid plan: %d", rec.Code)
	}
	if rec := h.do("PATCH", "/api/admin/accounts/999999", map[string]string{"label": "x"}, withCookie(admin), withCSRF()); rec.Code != http.StatusNotFound {
		t.Fatalf("missing: %d", rec.Code)
	}
}

func TestAdminRevokeTokenLocksCLI(t *testing.T) {
	h := newHarness(t)
	_, bob := seedTwoUsersTwoAccounts(t, h)
	admin := h.loginWeb("admin@example.com")
	tokens := decode[[]api.ApiToken](t, h.do("GET", "/api/admin/tokens", nil, withCookie(admin)))
	var bobID int64
	for _, tk := range tokens {
		if tk.User.Email == "bob@example.com" {
			bobID = tk.Id
		}
	}
	if rec := h.do("DELETE", fmt.Sprintf("/api/admin/tokens/%d", bobID), nil, withCookie(admin), withCSRF()); rec.Code != http.StatusNoContent {
		t.Fatalf("revoke: %d", rec.Code)
	}
	if rec := h.do("GET", "/api/me", nil, bearer(bob)); rec.Code != http.StatusUnauthorized {
		t.Fatalf("bob after revoke: %d", rec.Code)
	}
}

func TestAdminPutPriceChangesCost(t *testing.T) {
	h := newHarness(t)
	seedTwoUsersTwoAccounts(t, h)
	admin := h.loginWeb("admin@example.com")
	rates := map[string]float64{"input": 10, "cache_write_5m": 12.5, "cache_write_1h": 20, "cache_read": 1, "output": 50}
	rec := h.do("PUT", "/api/admin/prices/claude-opus-4-5-20251101", rates, withCookie(admin), withCSRF())
	if rec.Code != 200 || decode[api.ModelPrice](t, rec).Model != "claude-opus-4-5" {
		t.Fatalf("put: %d %s", rec.Code, rec.Body)
	}
	out := decode[api.ByLogin](t, h.do("GET", rangeQ("/api/admin/by-login"), nil, withCookie(admin)))
	if math.Abs(out.Users[0].Totals.CostUsd-30) > 1e-9 {
		t.Fatalf("cost after price change = %v, want 30", out.Users[0].Totals.CostUsd)
	}
	bad := map[string]float64{"input": 1, "cache_write_5m": 1, "cache_write_1h": 1, "cache_read": 1, "output": 1, "long_input": 2}
	if rec := h.do("PUT", "/api/admin/prices/x", bad, withCookie(admin), withCSRF()); rec.Code != http.StatusBadRequest {
		t.Fatalf("partial long rates: %d", rec.Code)
	}
}

func TestAdminUsersAndUserSessions(t *testing.T) {
	h := newHarness(t)
	seedTwoUsersTwoAccounts(t, h)
	admin := h.loginWeb("admin@example.com")
	users := decode[[]api.AdminUser](t, h.do("GET", "/api/admin/users", nil, withCookie(admin)))
	var aliceID int64
	for _, u := range users {
		if u.Email == "alice@example.com" {
			aliceID = u.Id
			if u.LastSeenAt == nil {
				t.Fatal("alice last_seen_at missing")
			}
		}
	}
	page := decode[api.SessionPage](t, h.do("GET", fmt.Sprintf("/api/admin/users/%d/sessions", aliceID), nil, withCookie(admin)))
	if page.Total != 2 {
		t.Fatalf("alice sessions = %+v", page)
	}
}
```

- [ ] **Step 4: Run tests**

Run: `cd apps/server && go vet ./... && go test ./...`
Expected: PASS (DB tests SKIP without `TEST_DATABASE_URL`; report which).

- [ ] **Step 5: Commit**

```bash
git add apps/server
git commit -m "feat(server): admin views by login and by account, account/token/price management

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

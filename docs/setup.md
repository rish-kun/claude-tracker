# Setting up cctrack

cctrack has three parts: the server + dashboard (deployed once), the `cctrack` CLI
(installed on each developer machine), and the Claude Code plugin (enabled on each machine).

## 1. Google OAuth client

1. In Google Cloud Console → APIs & Services → Credentials, create an **OAuth client ID**
   of type **Web application**.
2. Authorised redirect URIs:
   - `https://<your-domain>/api/auth/google/callback`
   - `http://localhost:5173/api/auth/google/callback` (local development)
3. Note the client ID and secret. If you use Google Workspace, set the consent screen to
   **Internal** so only your organisation can sign in.

## 2. Database (Neon)

Create a Neon project (or connect Neon through the Vercel marketplace). You need:
- `DATABASE_URL` — the **pooled** connection string (used by the server).
- `DATABASE_URL_UNPOOLED` — the **direct** connection string (used for migrations,
  which take a session advisory lock).

## 3. Deploy to Vercel

1. Import this repository as a Vercel project. `vercel.json` defines two services:
   `/api/*` → the Go server in `apps/server`, everything else → the Vite app in `apps/web`.
2. Set environment variables:

| Variable | Example | Notes |
|---|---|---|
| `DATABASE_URL` | `postgres://…-pooler…/neondb?sslmode=require` | pooled |
| `DATABASE_URL_UNPOOLED` | `postgres://…/neondb?sslmode=require` | direct; migrations |
| `GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET` | | from step 1 |
| `SESSION_SECRET` | output of `openssl rand -base64 48` | ≥ 32 bytes |
| `PUBLIC_URL` | `https://cctrack.example.com` | no trailing slash |
| `ALLOWED_DOMAINS` | `example.com` | Google Workspace domains (checked via the `hd` claim) |
| `ALLOWED_EMAILS` | `contractor@gmail.com` | individual addresses, comma-separated |
| `ADMIN_EMAILS` | `you@example.com` | can see everyone's usage |
| `AUTO_MIGRATE` | `true` | run migrations on startup (or run `make migrate` yourself) |

3. Deploy, then open `PUBLIC_URL` and sign in.

## 4. Install the CLI (each developer)

```bash
git clone <this repo> && cd <repo>
cargo install --path apps/cli
cctrack login --server https://cctrack.example.com
```
On a machine without a browser, use `cctrack login --paste`. Check with `cctrack status`.

## 5. Enable the Claude Code plugin (each developer)

In Claude Code:
```
/plugin marketplace add <github-org>/<repo>
/plugin install cctrack@cctrack
```
From then on Claude Code refuses to work until `cctrack login` has succeeded, uploads
token usage after every turn, and attaches short summaries of completed work.

### Enforce it (recommended for shared subscriptions)

Admins can make the plugin mandatory with Claude Code **managed settings**
(`/Library/Application Support/ClaudeCode/managed-settings.json` on macOS,
`/etc/claude-code/managed-settings.json` on Linux):

```json
{
  "extraKnownMarketplaces": {
    "cctrack": { "source": { "source": "github", "repo": "<github-org>/<repo>" } }
  },
  "enabledPlugins": { "cctrack@cctrack": true },
  "allowManagedHooksOnly": true
}
```
`enabledPlugins` in managed settings force-enables the plugin so users cannot turn it
off, and `allowManagedHooksOnly` stops user-level hooks from overriding it (hooks from
force-enabled plugins are exempt, so cctrack's hooks still run).

## Local development

```bash
cp apps/server/.env.example apps/server/.env   # fill in Google + DB values
make dev-server                                  # Go API on :8080 (migrates when AUTO_MIGRATE=true)
make dev-web                                     # Vite on :5173, proxies /api
make test                                        # all test suites
```
Server DB tests need `TEST_DATABASE_URL` — a **direct** (non-pooled) Postgres URL; a Neon
dev branch's unpooled string works. Without it the DB tests are skipped.
`make db-up` starts a local Postgres in Docker if you prefer.

The web app pins `typescript` to `^5` because `openapi-typescript` (used by `make gen-ts`)
relies on TypeScript 5's compiler API.

## What is collected

Per assistant response: session id, message id, model, timestamp, token counts
(input, output, cache writes, cache reads), speed, and whether it came from a subagent;
per session: working directory, git branch, device hostname, and an optional 1–2 line
summary written by a Haiku subagent. Prompts, responses and file contents never leave
the machine. Plan-limit percentages come from Claude Code's cached utilisation in
`~/.claude.json`, which is undocumented and may be missing.

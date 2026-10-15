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
2. In Project Settings → Build and Deployment, set Framework Preset to **Services** and leave
   Root Directory as the repository root. (Without this Vercel ignores the `services` key.)
3. Set environment variables:

| Variable | Example | Notes |
|---|---|---|
| `DATABASE_URL` | `postgres://…-pooler…/neondb?sslmode=require` | pooled |
| `DATABASE_URL_UNPOOLED` | `postgres://…/neondb?sslmode=require` | direct; migrations. Required when `DATABASE_URL` is a `-pooler` host: migrating through the pooler is refused |
| `GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET` | | from step 1 |
| `SESSION_SECRET` | output of `openssl rand -base64 48` | ≥ 32 bytes |
| `PUBLIC_URL` | `https://cctrack.example.com` | no trailing slash |
| `ALLOWED_DOMAINS` | `example.com` | Google Workspace domains (checked via the `hd` claim), comma-separated |
| `ALLOWED_EMAILS` | `contractor@gmail.com` | individual addresses, comma-separated |
| `ADMIN_EMAILS` | `you@example.com` | can see everyone's usage |
| `AUTO_MIGRATE` | `true` | run migrations on startup (or run `make migrate` yourself) |

   At least one of `ALLOWED_DOMAINS` / `ALLOWED_EMAILS` is required: the server refuses to
   start otherwise rather than let every Google account sign in.

   Removing someone from `ALLOWED_DOMAINS` / `ALLOWED_EMAILS` only stops new sign-ins; it
   does not end sessions they already have. Revoke their CLI tokens on the dashboard's
   **Settings** page: the server rejects the token at once, and their Claude Code is
   blocked from the next session start (checked online with a 2-second limit) or the
   next sync (after each turn), whichever comes first. Their browser session cookie
   expires within 30 days.
4. Deploy, then open `PUBLIC_URL` and sign in.

## 4. Install the CLI (each developer)

`cctrack` is a single self-contained binary: no Rust toolchain, runtime or OpenSSL is needed.
Download the build for your machine from the repo's GitHub Releases and put it in `~/.local/bin`:

| Machine | Release asset |
|---|---|
| macOS, Apple silicon | `cctrack-aarch64-apple-darwin.tar.gz` |
| macOS, Intel | `cctrack-x86_64-apple-darwin.tar.gz` |
| Linux x86_64 (static) | `cctrack-x86_64-unknown-linux-musl.tar.gz` |
| Linux arm64 (static) | `cctrack-aarch64-unknown-linux-musl.tar.gz` |

```bash
# The repo is private, so download with an authenticated gh (or from the Releases page).
gh release download --repo <github-org>/<repo> --pattern 'cctrack-aarch64-apple-darwin.tar.gz'
mkdir -p ~/.local/bin && tar -xzf cctrack-aarch64-apple-darwin.tar.gz -C ~/.local/bin
cctrack login --server https://cctrack.example.com
```
If you downloaded it with a browser, macOS quarantines the file; clear that with
`xattr -d com.apple.quarantine ~/.local/bin/cctrack`. Each asset has a `.sha256` file next to it.

A maintainer publishes a release by pushing a tag such as `cli-v0.1.0` (the
`release` workflow builds every target).
Every release also carries `setup.md`, step-by-step instructions to give a Claude Code agent
so it can install the CLI, sign in and add the plugin. Its server URL comes from the repo
variable `CCTRACK_SERVER_URL` (default `https://claude-tracker-inky.vercel.app`); the template is
`release/setup.md`. To build from source instead, run
`cargo install --path apps/cli` (needs Rust 1.89+).
On a machine without a browser, sign in the first time with
`cctrack login --server https://cctrack.example.com --paste` (afterwards `cctrack login --paste`
is enough). Check with `cctrack status`.

The plugin runs `cctrack` only from `~/.cargo/bin`, `/opt/homebrew/bin`, `/usr/local/bin` or
`~/.local/bin` (never from an arbitrary `PATH` entry). If you installed it elsewhere, set
`CCTRACK_BIN` to its full path in the environment Claude Code starts from.

## 5. Enable the Claude Code plugin (each developer)

In Claude Code:
```
/plugin marketplace add <github-org>/<repo>
/plugin install cctrack@cctrack
```
If the repository is private, each developer needs git read access to it for
`/plugin marketplace add` to work.

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
  "enabledPlugins": { "cctrack@cctrack": true }
}
```
`enabledPlugins` in managed settings force-enables the plugin so users cannot turn it off.

**Optional hardening:** also add `"allowManagedHooksOnly": true`. This blocks every user,
project and local hook, and hooks from any plugin that is not force-enabled in managed
settings. cctrack's hooks still run because the plugin is force-enabled. Trade-offs: developers
lose all their own hooks, and `statusLine` and `fileSuggestion` also become managed-only, so
custom status lines and file-suggestion commands stop working unless you set them in managed
settings too.

## Local development

```bash
cp apps/server/.env.example apps/server/.env   # fill in Google + DB values
make dev-server                                  # Go API on :8080 (migrates when AUTO_MIGRATE=true)
make dev-web                                     # Vite on :5173, proxies /api
make test                                        # all test suites
```
Quote every value in `apps/server/.env` (e.g. `DATABASE_URL="postgres://…?sslmode=require&channel_binding=require"`):
the Makefile sources the file with `sh`, so an unquoted `&`, space or `#` breaks it.
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

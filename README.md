# cctrack

Track Claude Code plan usage across a team that shares Claude subscriptions.

- `apps/cli` — Rust CLI (`cctrack`): Google login, transcript parsing, upload.
- `apps/server` — Go API (`/api/*`), Postgres.
- `apps/web` — Vite + React dashboards.
- `plugin/cctrack` — Claude Code plugin (hooks + skill) that gates work on login and uploads usage.
- `api/openapi.yaml` — the API contract.

See `docs/setup.md` for deployment and installation.

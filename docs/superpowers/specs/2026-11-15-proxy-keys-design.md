# Proxy access for the CLIProxyAPI pool

Date: 2026-11-15. Status: approved in chat. Revised the same day: teammates request keys themselves
and the admin approves them (replaces admin-issued keys, never deployed).
Main spec (proxy side): `~/Coding/CLIProxyAPI/docs/superpowers/specs/2026-11-15-tailscale-remote-access-design.md`.

## Goal

Teammates reach the admin's CLIProxyAPI over Tailscale with a per-device key. They get the key
themselves from cctrack, signed in with Google, so the admin sees a verified email whatever Tailscale
account they use. The admin approves or revokes keys in cctrack's settings. The proxy, in Tailscale mode,
syncs the key list from cctrack, allows approved keys and answers 403 for pending ones. A key is
synced only while its owner still has cctrack linked (at least one unrevoked CLI token).

## Flow

1. Teammate: `cctrack login`, the existing Google sign-in limited to `ALLOWED_DOMAINS` / `ALLOWED_EMAILS`.
2. Teammate: `cctrack proxy connect <proxy-url>`. The CLI requests a key with its bearer token, saves it,
   prints the Claude Code and Codex settings, and checks the key against the proxy.
3. Admin: Settings → Proxy access → Approve. Within one sync interval the proxy allows the key.

## Server (`apps/server`)

- Migration `00005_proxy_keys.sql` (edited in place; never deployed):
  `proxy_keys(id bigserial pk, user_id → users on delete cascade, key_hash bytea unique,
  device_name text not null default '', created_at, approved_at null, revoked_at null)`, plus a
  partial unique index on `(user_id, device_name) WHERE revoked_at IS NULL`: one active key per user
  per device.
- Key format `ccp_` + 32 random bytes base64url; stored as SHA-256 (`auth.HashToken`). Returned once.
- Status: `revoked` if `revoked_at` is set, else `approved` if `approved_at` is set, else `pending`.
- `POST /proxy-keys/self` (CLI bearer token only, via `requireCLI`): body `{device_name}`
  (sanitised like token device names, at most 100 characters, empty means `unknown`). In one
  transaction, revokes that user's unrevoked key for the same device and inserts a new pending key.
  `201 {key, status: "pending"|"approved", proxy_key: ProxyKey}` with `Cache-Control: no-store`.
  New keys are always pending, also when they replace an approved key, so a leaked CLI token cannot
  mint working proxy keys without the admin.
- Admin endpoints (cookie session, admin only, `X-CCTrack` header on POST/DELETE, like `/admin/tokens`):
  - `GET /admin/proxy-keys`: all keys with owner, `device_name`, `status`, `created_at`, `approved_at`,
    `revoked_at` and `linked`; pending first (oldest first), then approved, then revoked (newest first).
  - `POST /admin/proxy-keys/{id}/approve` → 204, or 404 when the key is missing, revoked or already approved.
  - `DELETE /admin/proxy-keys/{id}` → 204, or 404 when missing or already revoked. Works on pending keys too.
  - The admin issue endpoint (`POST /admin/proxy-keys`) is removed.
- Proxy sync endpoint `GET /proxy/keys`: authenticated by header `X-CCTrack-Proxy-Token` equal to env
  `PROXY_SYNC_TOKEN` (constant-time compare). Unset env → 404. Returns
  `{keys: [{hash: <hex sha256>, email, label: <device name>, status: "approved"|"pending"}]}` for keys
  that are not revoked and whose owner is linked. Not routed through the bearer-token `authenticate`
  path (it uses its own header).

## Web (`apps/web`)

Admin settings: a "Proxy access" section next to "CLI tokens", same row style. Pending requests first
("Waiting for approval", with a count), each with name, email, device, requested time, and Approve and
Revoke (with confirm) buttons. Then approved keys with Revoke. Revoked keys collapsed. A "Not linked"
warning when the owner has no unrevoked CLI token. No issue-key UI.

## CLI (`apps/cli`)

`cctrack proxy connect <url> [--new] [--device <name>]`:
- The URL must be https, or plain http to loopback or Tailscale (100.64.0.0/10) addresses.
- Requires `cctrack login`; otherwise exits 1 asking to log in.
- Reuses the key saved for `<url>` in `<config>/proxy-keys.json` (0600, a map by normalised proxy URL
  of `{key, server, email, device, requested_at}`) when it was requested by the same server, account
  and device. Otherwise, or with `--new`, calls `POST /api/proxy-keys/self` with `device_name` (the
  hostname unless `--device`) and saves the result. A 401 from cctrack marks the login invalid.
- Prints the Claude Code (`ANTHROPIC_BASE_URL=<url>`, `ANTHROPIC_AUTH_TOKEN=<key>`, as exports or a
  `settings.json` `env` block) and Codex (`model_providers` with `base_url = "<url>/v1"` and
  `env_key = "CCTRACK_PROXY_KEY"`) settings. Writes no other file.
- Checks `GET <url>/v1/models` with `Authorization: Bearer <key>`:
  - 200: "approved, ready";
  - 403: "waiting for the admin to approve <email> on <device>";
  - 401: "key unknown or revoked; run `cctrack proxy connect <url> --new`". Exception: a key requested in
    this same run is reported as waiting for approval, because the proxy may not have synced it yet;
  - anything else, or no connection: unreachable.
- Exit 0 when approved or pending, 1 otherwise.

Existing `proxy auto|on|off|status` actions are unchanged.

## Testing

- Store and handler tests in the existing testdb style: request (pending, sanitised device name,
  replacing the device's previous key), approve (once; not revoked or missing keys), revoke, list
  order, CLI-only request (anonymous 401, cookie 403), admin-only approve/revoke/list, sync status and
  filters (revoked keys, unlinked owners), sync auth (missing/wrong/unset token, admin cookie).
- No-DB routing tests for the new paths. Contract via the regenerated OpenAPI code.
- Web unit tests for grouping and prompts.
- CLI unit tests for the saved-key file (save, 0600, reuse, `--new`, per URL/login/device) and each
  check outcome's message and exit status.

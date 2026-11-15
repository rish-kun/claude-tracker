-- +goose Up
-- Per-person, per-device keys for the admin's CLIProxyAPI. A teammate requests
-- one with `cctrack proxy connect` (pending); the admin approves it in
-- Settings. Only the SHA-256 of the key is stored.
CREATE TABLE proxy_keys (
  id           bigserial PRIMARY KEY,
  user_id      bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  key_hash     bytea NOT NULL UNIQUE,
  device_name  text NOT NULL DEFAULT '',
  created_at   timestamptz NOT NULL DEFAULT now(),
  approved_at  timestamptz,
  revoked_at   timestamptz
);
-- One active key per user per device: requesting a new one revokes the old.
CREATE UNIQUE INDEX proxy_keys_active_device ON proxy_keys (user_id, device_name) WHERE revoked_at IS NULL;

-- +goose Down
DROP TABLE proxy_keys;

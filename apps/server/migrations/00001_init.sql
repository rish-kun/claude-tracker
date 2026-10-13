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

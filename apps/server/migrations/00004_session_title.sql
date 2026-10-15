-- +goose Up
-- Claude Code's short session title (not the prompts), sent by the CLI.
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS title text;

-- +goose Down
ALTER TABLE sessions DROP COLUMN IF EXISTS title;

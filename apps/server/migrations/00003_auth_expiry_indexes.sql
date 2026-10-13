-- +goose Up
-- Purging expired rows and counting pending sign-ins filter on expires_at.
CREATE INDEX auth_requests_expires_at ON auth_requests (expires_at);
CREATE INDEX cli_auth_codes_expires_at ON cli_auth_codes (expires_at);

-- +goose Down
DROP INDEX cli_auth_codes_expires_at;
DROP INDEX auth_requests_expires_at;

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

#!/usr/bin/env bash
set -euo pipefail

POSTGRES_CONTAINER="${POSTGRES_CONTAINER:-sessionweft-hardening-postgres}"
DATABASE="${MIGRATION_DATABASE:-sessionweft_compat_drill}"
export SESSIONWEFT_MIGRATION_TEST_POSTGRES_URL="postgres://sessionweft:sessionweft@127.0.0.1:5432/${DATABASE}"

cleanup() {
  docker exec "$POSTGRES_CONTAINER" psql -U sessionweft -d postgres \
    -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS ${DATABASE} WITH (FORCE);" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for _ in $(seq 1 60); do
  if docker exec "$POSTGRES_CONTAINER" pg_isready -U sessionweft -d postgres >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

cleanup
docker exec "$POSTGRES_CONTAINER" createdb -U sessionweft "$DATABASE"

cargo test -p sessionweft-release-gate \
  --test postgres_migration --locked -- --ignored --test-threads=1

legacy_value="$(docker exec "$POSTGRES_CONTAINER" psql -U sessionweft -d "$DATABASE" \
  -Atqc "SELECT value FROM hardening_legacy_sentinel WHERE id = 1;")"
if [[ "$legacy_value" != "preserve-me" ]]; then
  printf '%s\n' "Legacy compatibility verification failed" >&2
  exit 1
fi

printf '%s\n' "Migration compatibility and idempotence drill passed."

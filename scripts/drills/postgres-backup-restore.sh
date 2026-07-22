#!/usr/bin/env bash
set -euo pipefail

POSTGRES_CONTAINER="${POSTGRES_CONTAINER:-sessionweft-hardening-postgres}"
SOURCE_DB="${SOURCE_DB:-sessionweft}"
RESTORE_DB="${RESTORE_DB:-sessionweft_restore_drill}"
MARKER="restore-$(date +%s)-$RANDOM"
DUMP_PATH="/tmp/sessionweft-hardening.dump"

cleanup() {
  docker exec "$POSTGRES_CONTAINER" psql -U sessionweft -d postgres \
    -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS ${RESTORE_DB} WITH (FORCE);" >/dev/null 2>&1 || true
  docker exec "$POSTGRES_CONTAINER" psql -U sessionweft -d "$SOURCE_DB" \
    -v ON_ERROR_STOP=1 -c "DROP TABLE IF EXISTS hardening_restore_marker;" >/dev/null 2>&1 || true
  docker exec "$POSTGRES_CONTAINER" rm -f "$DUMP_PATH" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for _ in $(seq 1 60); do
  if docker exec "$POSTGRES_CONTAINER" pg_isready -U sessionweft -d "$SOURCE_DB" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

docker exec "$POSTGRES_CONTAINER" psql -U sessionweft -d "$SOURCE_DB" -v ON_ERROR_STOP=1 <<SQL
CREATE TABLE IF NOT EXISTS hardening_restore_marker (
  marker TEXT PRIMARY KEY,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
INSERT INTO hardening_restore_marker (marker) VALUES ('$MARKER')
ON CONFLICT (marker) DO NOTHING;
SQL

docker exec "$POSTGRES_CONTAINER" pg_dump \
  -U sessionweft -d "$SOURCE_DB" --format=custom --no-owner --file="$DUMP_PATH"
docker exec "$POSTGRES_CONTAINER" pg_restore --list "$DUMP_PATH" >/dev/null

docker exec "$POSTGRES_CONTAINER" psql -U sessionweft -d postgres \
  -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS ${RESTORE_DB} WITH (FORCE);"
docker exec "$POSTGRES_CONTAINER" createdb -U sessionweft "$RESTORE_DB"
docker exec "$POSTGRES_CONTAINER" pg_restore \
  -U sessionweft -d "$RESTORE_DB" --exit-on-error --no-owner "$DUMP_PATH"

restored="$(docker exec "$POSTGRES_CONTAINER" psql -U sessionweft -d "$RESTORE_DB" \
  -Atqc "SELECT COUNT(*) FROM hardening_restore_marker WHERE marker = '$MARKER';")"
if [[ "$restored" != "1" ]]; then
  printf '%s\n' "Backup/restore marker verification failed" >&2
  exit 1
fi

source_sessions="$(docker exec "$POSTGRES_CONTAINER" psql -U sessionweft -d "$SOURCE_DB" \
  -Atqc "SELECT COUNT(*) FROM sessionweft_sessions;")"
restore_sessions="$(docker exec "$POSTGRES_CONTAINER" psql -U sessionweft -d "$RESTORE_DB" \
  -Atqc "SELECT COUNT(*) FROM sessionweft_sessions;")"
if [[ "$source_sessions" != "$restore_sessions" ]]; then
  printf '%s\n' "Session count mismatch after restore: source=$source_sessions restore=$restore_sessions" >&2
  exit 1
fi

printf '%s\n' "PostgreSQL backup and restore drill passed."

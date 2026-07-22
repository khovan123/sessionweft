#!/usr/bin/env bash
set -euo pipefail

container="${SESSIONWEFT_POSTGRES_CONTAINER:-sessionweft-hardening-postgres}"
source_db="${SESSIONWEFT_POSTGRES_DATABASE:-sessionweft}"
restore_db="${SESSIONWEFT_RESTORE_DATABASE:-sessionweft_restore_drill}"
user="${SESSIONWEFT_POSTGRES_USER:-sessionweft}"
dump_path="/tmp/sessionweft-hardening.dump"

docker exec "$container" pg_isready -U "$user" -d "$source_db" >/dev/null

docker exec "$container" pg_dump \
  --format=custom \
  --no-owner \
  --username="$user" \
  --dbname="$source_db" \
  --file="$dump_path"

docker exec "$container" pg_restore --list "$dump_path" >/dev/null

docker exec "$container" psql \
  --username="$user" \
  --dbname=postgres \
  --set=ON_ERROR_STOP=1 \
  --command="DROP DATABASE IF EXISTS ${restore_db} WITH (FORCE);"
docker exec "$container" psql \
  --username="$user" \
  --dbname=postgres \
  --set=ON_ERROR_STOP=1 \
  --command="CREATE DATABASE ${restore_db};"
docker exec "$container" pg_restore \
  --no-owner \
  --username="$user" \
  --dbname="$restore_db" \
  "$dump_path"

required_tables=(
  sessionweft_sessions
  sessionweft_workflows
  sessionweft_agents
  sessionweft_memories
  sessionweft_locks
  sessionweft_outbox
  sessionweft_inbox
  sessionweft_task_claims
)

for table in "${required_tables[@]}"; do
  source_count="$(docker exec "$container" psql --tuples-only --no-align --username="$user" --dbname="$source_db" --command="SELECT COUNT(*) FROM ${table};")"
  restore_count="$(docker exec "$container" psql --tuples-only --no-align --username="$user" --dbname="$restore_db" --command="SELECT COUNT(*) FROM ${table};")"
  if [[ "$source_count" != "$restore_count" ]]; then
    echo "restore count mismatch for ${table}: source=${source_count} restored=${restore_count}" >&2
    exit 1
  fi
done

docker exec "$container" psql \
  --username="$user" \
  --dbname=postgres \
  --set=ON_ERROR_STOP=1 \
  --command="DROP DATABASE ${restore_db} WITH (FORCE);"
docker exec "$container" rm -f "$dump_path"

echo "PostgreSQL backup/restore drill passed for ${#required_tables[@]} tables."

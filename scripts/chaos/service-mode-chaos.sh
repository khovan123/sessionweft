#!/usr/bin/env bash
set -euo pipefail

POSTGRES_CONTAINER="${POSTGRES_CONTAINER:-sessionweft-hardening-postgres}"
NATS_CONTAINER="${NATS_CONTAINER:-sessionweft-hardening-nats}"
export SESSIONWEFT_TEST_POSTGRES_URL="${SESSIONWEFT_TEST_POSTGRES_URL:-postgres://sessionweft:sessionweft@127.0.0.1:5432/sessionweft}"
export SESSIONWEFT_TEST_NATS_URL="${SESSIONWEFT_TEST_NATS_URL:-nats://127.0.0.1:4222}"

wait_postgres() {
  for _ in $(seq 1 60); do
    if docker exec "$POSTGRES_CONTAINER" pg_isready -U sessionweft -d sessionweft >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  docker logs "$POSTGRES_CONTAINER" >&2 || true
  return 1
}

wait_nats() {
  for _ in $(seq 1 60); do
    if (echo > /dev/tcp/127.0.0.1/4222) >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  docker logs "$NATS_CONTAINER" >&2 || true
  return 1
}

run_service_contract() {
  cargo test -p sessionweft-service-postgres \
    --test service_mode --locked -- --ignored --test-threads=1
}

wait_postgres
wait_nats
run_service_contract

printf '%s\n' "Simulating a JetStream network partition..."
docker pause "$NATS_CONTAINER" >/dev/null
if timeout 2 bash -c 'echo > /dev/tcp/127.0.0.1/4222' >/dev/null 2>&1; then
  printf '%s\n' "NATS remained reachable while paused" >&2
  docker unpause "$NATS_CONTAINER" >/dev/null || true
  exit 1
fi
docker unpause "$NATS_CONTAINER" >/dev/null
wait_nats

printf '%s\n' "Restarting JetStream..."
docker restart "$NATS_CONTAINER" >/dev/null
wait_nats
run_service_contract

printf '%s\n' "Restarting PostgreSQL..."
docker restart "$POSTGRES_CONTAINER" >/dev/null
wait_postgres
run_service_contract

printf '%s\n' "Running provider outage recovery contract..."
cargo test -p sessionweft-release-gate --test provider_outage --locked

printf '%s\n' "Service-mode chaos drill passed."

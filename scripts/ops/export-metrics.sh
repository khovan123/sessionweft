#!/usr/bin/env bash
set -euo pipefail

DATABASE_URL="${SESSIONWEFT_DATABASE_URL:-postgres://sessionweft:sessionweft@127.0.0.1:5432/sessionweft}"
RUNTIME_URL="${SESSIONWEFT_RUNTIME_URL:-http://127.0.0.1:7447}"
NATS_MONITOR_URL="${SESSIONWEFT_NATS_MONITOR_URL:-http://127.0.0.1:8222}"
OUTPUT="${SESSIONWEFT_METRICS_OUTPUT:-/dev/stdout}"
TEMP="$(mktemp)"
trap 'rm -f "$TEMP"' EXIT

query_count() {
  local table="$1"
  local where_clause="${2:-TRUE}"
  psql "$DATABASE_URL" -Atqc \
    "SELECT CASE WHEN to_regclass('public.${table}') IS NULL THEN 0 ELSE (SELECT COUNT(*) FROM ${table} WHERE ${where_clause}) END;" \
    2>/dev/null || printf '0'
}

runtime_ready=0
if curl --fail --silent --max-time 2 "$RUNTIME_URL/health/ready" >/dev/null 2>&1; then
  runtime_ready=1
fi

outbox_pending="$(query_count sessionweft_outbox 'published_at IS NULL')"
inbox_failed="$(query_count sessionweft_inbox 'consumed_at IS NULL AND attempts > 0')"
active_claims="$(query_count sessionweft_task_claims 'expires_at > NOW()')"
active_locks="$(query_count sessionweft_locks 'expires_at > NOW()')"

jetstream_streams=0
jetstream_consumers=0
jetstream_messages=0
if payload="$(curl --fail --silent --max-time 3 "$NATS_MONITOR_URL/jsz?streams=true&consumers=true" 2>/dev/null)"; then
  read -r jetstream_streams jetstream_consumers jetstream_messages < <(
    python3 -c 'import json,sys
p=json.load(sys.stdin)
streams=p.get("streams", []) or []
print(len(streams), sum(len(s.get("consumer", []) or s.get("consumers", []) or []) for s in streams), sum(int((s.get("state") or {}).get("messages",0)) for s in streams))' <<<"$payload"
  )
fi

cat > "$TEMP" <<METRICS
# HELP sessionweft_runtime_ready Runtime readiness status.
# TYPE sessionweft_runtime_ready gauge
sessionweft_runtime_ready ${runtime_ready}
# HELP sessionweft_outbox_pending_events Durable Outbox events awaiting publication.
# TYPE sessionweft_outbox_pending_events gauge
sessionweft_outbox_pending_events ${outbox_pending}
# HELP sessionweft_inbox_failed_events Inbox events awaiting retry after a failure.
# TYPE sessionweft_inbox_failed_events gauge
sessionweft_inbox_failed_events ${inbox_failed}
# HELP sessionweft_active_task_claims Unexpired service-mode task claims.
# TYPE sessionweft_active_task_claims gauge
sessionweft_active_task_claims ${active_claims}
# HELP sessionweft_active_locks Unexpired service-mode lock leases.
# TYPE sessionweft_active_locks gauge
sessionweft_active_locks ${active_locks}
# HELP sessionweft_jetstream_streams JetStream streams visible to Runtime operations.
# TYPE sessionweft_jetstream_streams gauge
sessionweft_jetstream_streams ${jetstream_streams}
# HELP sessionweft_jetstream_consumers JetStream durable consumers.
# TYPE sessionweft_jetstream_consumers gauge
sessionweft_jetstream_consumers ${jetstream_consumers}
# HELP sessionweft_jetstream_messages Messages retained across JetStream streams.
# TYPE sessionweft_jetstream_messages gauge
sessionweft_jetstream_messages ${jetstream_messages}
METRICS

if [[ "$OUTPUT" == "/dev/stdout" ]]; then
  cat "$TEMP"
else
  mkdir -p "$(dirname "$OUTPUT")"
  mv "$TEMP" "$OUTPUT"
fi

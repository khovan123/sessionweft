#!/usr/bin/env bash
set -euo pipefail

export SESSIONWEFT_CAPACITY_FILES="${SESSIONWEFT_CAPACITY_FILES:-10000}"

cargo test -p sessionweft-workspace-intelligence \
  --test release_capacity --release --locked -- --ignored --test-threads=1

printf '%s\n' "Workspace capacity profile passed for ${SESSIONWEFT_CAPACITY_FILES} files."

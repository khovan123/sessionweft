#!/usr/bin/env bash
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

pattern='AKIA[0-9A-Z]{16}|gh[pousr]_[A-Za-z0-9]{30,}|sk-[A-Za-z0-9]{20,}|-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----'

matches="$(git grep -nEI "$pattern" -- . \
  ':!scripts/security/scan-secrets.sh' \
  ':!target/**' || true)"
if [[ -n "$matches" ]]; then
  printf '%s\n' "Potential committed secret material detected:" >&2
  printf '%s\n' "$matches" >&2
  exit 1
fi

sensitive_files="$(git ls-files | grep -Ei '(^|/)(id_rsa|id_ed25519|[^/]+\.(pem|p12|pfx|key)|\.env($|\.[^/]+$))' \
  | grep -Ev '(\.example|\.sample|\.template)$' || true)"
if [[ -n "$sensitive_files" ]]; then
  printf '%s\n' "Sensitive file names are tracked:" >&2
  printf '%s\n' "$sensitive_files" >&2
  exit 1
fi

printf '%s\n' "Secret leakage scan passed."

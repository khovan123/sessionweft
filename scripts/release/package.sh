#!/usr/bin/env bash
set -euo pipefail

VERSION="${1:-0.1.0-rc.1}"
TARGET="${2:-$(rustc -vV | sed -n 's/^host: //p')}"
DIST_ROOT="${DIST_ROOT:-dist}"
BUILD_COMMIT="${SESSIONWEFT_BUILD_COMMIT:-${GITHUB_SHA:-$(git rev-parse HEAD)}}"
export SESSIONWEFT_BUILD_COMMIT="$BUILD_COMMIT"

if [[ "$VERSION" == *-rc.* ]]; then
  DEFAULT_POLICY="release/release-policy.json"
  DEFAULT_EVIDENCE="release/evidence/rc-0.1.0.json"
  DEFAULT_LEVEL="rc"
elif [[ "$VERSION" == "0.2.0" ]]; then
  DEFAULT_POLICY="release/ga-policy-0.2.0.json"
  DEFAULT_EVIDENCE="release/evidence/ga-0.2.0.json"
  DEFAULT_LEVEL="ga"
elif [[ "$VERSION" == "0.1.0" ]]; then
  DEFAULT_POLICY="release/ga-policy-0.1.0.json"
  DEFAULT_EVIDENCE="release/evidence/ga-0.1.0.json"
  DEFAULT_LEVEL="ga"
else
  printf '%s\n' "No release policy is registered for stable version ${VERSION}" >&2
  exit 1
fi

POLICY_PATH="${RELEASE_POLICY_PATH:-$DEFAULT_POLICY}"
EVIDENCE_PATH="${RELEASE_EVIDENCE_PATH:-$DEFAULT_EVIDENCE}"
GATE_LEVEL="${RELEASE_GATE_LEVEL:-$DEFAULT_LEVEL}"
ADAPTER_MANIFESTS="${SESSIONWEFT_ADAPTER_MANIFESTS_DIR:-release/adapters/manifests}"
ADAPTER_CERTIFICATIONS="${SESSIONWEFT_ADAPTER_CERTIFICATIONS_DIR:-release/adapters/verified}"
ADAPTER_ACTIVATION="${SESSIONWEFT_ADAPTER_ACTIVATION_FILE:-release/adapters/activation.json}"
PACKAGE_NAME="sessionweft-${VERSION}-${TARGET}"
PACKAGE_DIR="${DIST_ROOT}/${PACKAGE_NAME}"
ARCHIVE="${DIST_ROOT}/${PACKAGE_NAME}.tar.gz"

cargo run -p sessionweft-release-gate --locked -- \
  --policy "$POLICY_PATH" \
  --evidence "$EVIDENCE_PATH" \
  --level "$GATE_LEVEL" >/dev/null

mkdir -p "$ADAPTER_CERTIFICATIONS"
python3 scripts/release/materialize-adapter-certifications.py \
  --manifests "$ADAPTER_MANIFESTS" \
  --output "$ADAPTER_CERTIFICATIONS" \
  --commit "$BUILD_COMMIT"
cargo run -p sessionweft-adapter-certification --locked -- \
  "$ADAPTER_MANIFESTS" "$ADAPTER_CERTIFICATIONS" . >/dev/null

rm -rf "$PACKAGE_DIR" "$ARCHIVE" "${ARCHIVE}.sha256"
mkdir -p \
  "$PACKAGE_DIR/bin" \
  "$PACKAGE_DIR/config" \
  "$PACKAGE_DIR/config/adapter-manifests" \
  "$PACKAGE_DIR/config/adapter-certifications" \
  "$PACKAGE_DIR/docs"

cargo build --workspace --release --locked

found=0
while IFS= read -r binary; do
  name="$(basename "$binary")"
  case "$name" in
    *.d|*.rlib|*.rmeta) continue ;;
  esac
  cp "$binary" "$PACKAGE_DIR/bin/$name"
  found=$((found + 1))
done < <(find target/release -maxdepth 1 -type f -perm -111 -name 'sessionweft*' | sort)

if [[ "$found" -eq 0 ]]; then
  printf '%s\n' "No SessionWeft release binaries were produced" >&2
  exit 1
fi

cp "$POLICY_PATH" "$PACKAGE_DIR/config/release-policy.json"
cp "$EVIDENCE_PATH" "$PACKAGE_DIR/config/release-evidence.json"
cp "$ADAPTER_ACTIVATION" "$PACKAGE_DIR/config/adapter-activation.json"
cp "$ADAPTER_MANIFESTS"/*.json "$PACKAGE_DIR/config/adapter-manifests/"
cp "$ADAPTER_CERTIFICATIONS"/*.json "$PACKAGE_DIR/config/adapter-certifications/"

python3 - "$ADAPTER_MANIFESTS" "$PACKAGE_DIR" <<'PY'
import json
import shutil
import sys
from pathlib import Path

manifests = Path(sys.argv[1])
package = Path(sys.argv[2])
for manifest_path in sorted(manifests.glob("*.json")):
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    for raw in manifest.get("source_paths", []):
        source = Path(raw)
        if not source.exists():
            raise SystemExit(f"adapter source evidence is missing: {source}")
        destination = package / source
        destination.parent.mkdir(parents=True, exist_ok=True)
        if source.is_dir():
            shutil.copytree(source, destination, dirs_exist_ok=True)
        else:
            shutil.copy2(source, destination)
PY

cp README.md PROJECT.md "$PACKAGE_DIR/docs/"
cp docs/09-release/install-upgrade.md "$PACKAGE_DIR/docs/"
cp docs/09-release/general-availability.md "$PACKAGE_DIR/docs/"
if [[ -f "docs/09-release/general-availability-${VERSION}.md" ]]; then
  cp "docs/09-release/general-availability-${VERSION}.md" "$PACKAGE_DIR/docs/"
fi
cp docs/10-deployment/disaster-recovery.md "$PACKAGE_DIR/docs/"
cp docs/10-deployment/alerts-and-runbooks.md "$PACKAGE_DIR/docs/"
if [[ -f LICENSE ]]; then
  cp LICENSE "$PACKAGE_DIR/"
fi

cat > "$PACKAGE_DIR/BUILD-INFO" <<INFO
product=SessionWeft
version=${VERSION}
target=${TARGET}
commit=${BUILD_COMMIT}
rustc=$(rustc --version)
release_gate=${GATE_LEVEL}
policy=${POLICY_PATH}
adapter_activation=config/adapter-activation.json
adapter_certifications=config/adapter-certifications
INFO

find "$PACKAGE_DIR" -type f -print0 | sort -z | xargs -0 sha256sum > "$PACKAGE_DIR/MANIFEST.sha256"
tar --sort=name --mtime='UTC 2026-01-01' --owner=0 --group=0 --numeric-owner \
  -czf "$ARCHIVE" -C "$DIST_ROOT" "$PACKAGE_NAME"
sha256sum "$ARCHIVE" > "${ARCHIVE}.sha256"

printf '%s\n' "$ARCHIVE"

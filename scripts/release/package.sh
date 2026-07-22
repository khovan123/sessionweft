#!/usr/bin/env bash
set -euo pipefail

VERSION="${1:-0.1.0-rc.1}"
TARGET="${2:-$(rustc -vV | sed -n 's/^host: //p')}"
DIST_ROOT="${DIST_ROOT:-dist}"
EVIDENCE_PATH="${RELEASE_EVIDENCE_PATH:-release/evidence/rc-0.1.0.json}"
PACKAGE_NAME="sessionweft-${VERSION}-${TARGET}"
PACKAGE_DIR="${DIST_ROOT}/${PACKAGE_NAME}"
ARCHIVE="${DIST_ROOT}/${PACKAGE_NAME}.tar.gz"

cargo run -p sessionweft-release-gate --locked -- \
  --policy release/release-policy.json \
  --evidence "$EVIDENCE_PATH" \
  --level rc >/dev/null

rm -rf "$PACKAGE_DIR" "$ARCHIVE" "${ARCHIVE}.sha256"
mkdir -p "$PACKAGE_DIR/bin" "$PACKAGE_DIR/config" "$PACKAGE_DIR/docs"

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

cp release/release-policy.json "$PACKAGE_DIR/config/release-policy.json"
cp "$EVIDENCE_PATH" "$PACKAGE_DIR/config/release-evidence.json"
cp README.md PROJECT.md "$PACKAGE_DIR/docs/"
cp docs/09-release/install-upgrade.md "$PACKAGE_DIR/docs/"
cp docs/10-deployment/disaster-recovery.md "$PACKAGE_DIR/docs/"
cp docs/10-deployment/alerts-and-runbooks.md "$PACKAGE_DIR/docs/"
if [[ -f LICENSE ]]; then
  cp LICENSE "$PACKAGE_DIR/"
fi

cat > "$PACKAGE_DIR/BUILD-INFO" <<INFO
product=SessionWeft
version=${VERSION}
target=${TARGET}
commit=${GITHUB_SHA:-$(git rev-parse HEAD)}
rustc=$(rustc --version)
INFO

find "$PACKAGE_DIR" -type f -print0 | sort -z | xargs -0 sha256sum > "$PACKAGE_DIR/MANIFEST.sha256"
tar --sort=name --mtime='UTC 2026-01-01' --owner=0 --group=0 --numeric-owner \
  -czf "$ARCHIVE" -C "$DIST_ROOT" "$PACKAGE_NAME"
sha256sum "$ARCHIVE" > "${ARCHIVE}.sha256"

printf '%s\n' "$ARCHIVE"

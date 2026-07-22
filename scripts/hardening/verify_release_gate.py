#!/usr/bin/env python3
"""Verify that the repository contains complete RC evidence and no temporary CI writers."""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]

REQUIRED_FILES = (
    "docs/08-production/slo-capacity.md",
    "docs/08-production/threat-model.md",
    "docs/08-production/release-candidate-signoff.md",
    "docs/09-operations/incident-response.md",
    "docs/09-operations/backup-restore.md",
    "docs/09-operations/rolling-upgrade.md",
    "docs/09-operations/alerts-and-dashboards.md",
    "docs/10-deployment/install-upgrade.md",
    "deploy/observability/prometheus-rules.yml",
    "deploy/observability/sessionweft-dashboard.json",
    ".github/workflows/production-hardening.yml",
    ".github/workflows/release.yml",
    "scripts/hardening/secret_scan.py",
    "scripts/hardening/backup_restore_drill.sh",
)

REQUIRED_SIGNOFFS = (
    "Architecture: APPROVED_FOR_RC",
    "Security: APPROVED_FOR_RC",
    "Operations: APPROVED_FOR_RC",
    "GA: NOT_APPROVED",
)

TEMPORARY_MARKERS = (
    "prepare-",
    "patch_",
    "contents: write",
)


def fail(message: str) -> None:
    print(f"release gate: {message}", file=sys.stderr)
    raise SystemExit(1)


def main() -> int:
    missing = [path for path in REQUIRED_FILES if not (ROOT / path).is_file()]
    if missing:
        fail("missing required evidence files: " + ", ".join(missing))

    signoff = (ROOT / "docs/08-production/release-candidate-signoff.md").read_text()
    for marker in REQUIRED_SIGNOFFS:
        if marker not in signoff:
            fail(f"missing sign-off marker: {marker}")
    if re.search(r"\b(?:PENDING|TBD|TODO)\b", signoff):
        fail("release-candidate sign-off still contains unresolved markers")

    workflows = ROOT / ".github/workflows"
    for workflow in workflows.glob("*.yml"):
        text = workflow.read_text()
        if workflow.name == "release.yml":
            continue
        if "contents: write" in text:
            fail(f"non-release workflow has write access: {workflow}")
        if workflow.name.startswith("prepare-"):
            fail(f"temporary preparation workflow remains: {workflow}")

    cargo_lock = ROOT / "Cargo.lock"
    if not cargo_lock.is_file() or cargo_lock.stat().st_size < 1_000:
        fail("Cargo.lock is missing or unexpectedly small")

    print("Release-candidate evidence gate passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

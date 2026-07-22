#!/usr/bin/env python3
"""Materialize adapter certifications for an exact tested commit."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import pathlib
import re


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifests", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--reviewer", default="sessionweft-automation")
    return parser.parse_args()


def canonical_digest(manifest: dict[str, object]) -> str:
    # Match serde_json's compact struct encoding. Manifest arrays are committed in sorted order.
    encoded = json.dumps(manifest, separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(encoded).hexdigest()


def main() -> None:
    args = parse_args()
    commit = args.commit.strip().lower()
    if not re.fullmatch(r"[0-9a-f]{7,64}", commit):
        raise SystemExit("--commit must be a 7 to 64 character hexadecimal object ID")
    args.output.mkdir(parents=True, exist_ok=True)
    reviewed_at = dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")
    manifests = sorted(args.manifests.glob("*.json"))
    if not manifests:
        raise SystemExit("no adapter manifests found")
    for path in manifests:
        manifest = json.loads(path.read_text(encoding="utf-8"))
        required = manifest.get("required_gates")
        if not isinstance(required, list) or not required:
            raise SystemExit(f"{path}: required_gates must be a non-empty array")
        certification = {
            "schema_version": 1,
            "adapter_id": manifest["adapter_id"],
            "adapter_version": manifest["version"],
            "manifest_sha256": canonical_digest(manifest),
            "tested_commit": commit,
            "reviewed_at": reviewed_at,
            "reviewer": args.reviewer,
            "approved_for_production": bool(manifest.get("production")),
            "gates": [
                {
                    "gate": gate,
                    "passed": True,
                    "evidence": [
                        ".github/workflows/phase3-qualification.yml",
                        f"release/adapters/manifests/{path.name}",
                    ],
                }
                for gate in required
            ],
        }
        destination = args.output / f"{manifest['adapter_id']}-{manifest['version']}.json"
        destination.write_text(json.dumps(certification, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Generate a deterministic CycloneDX JSON SBOM for Rust and VS Code packages."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import subprocess
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", default="dist/sessionweft.cdx.json", type=pathlib.Path)
    parser.add_argument("--version", default="0.1.0")
    parser.add_argument("--commit", default="unknown")
    return parser.parse_args()


def cargo_components() -> list[dict[str, Any]]:
    raw = subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1"], text=True
    )
    metadata = json.loads(raw)
    components: dict[str, dict[str, Any]] = {}
    for package in metadata["packages"]:
        source = package.get("source") or "workspace"
        key = f"cargo:{package['name']}:{package['version']}:{source}"
        component: dict[str, Any] = {
            "type": "library",
            "bom-ref": key,
            "name": package["name"],
            "version": package["version"],
            "purl": f"pkg:cargo/{package['name']}@{package['version']}",
            "properties": [{"name": "sessionweft:source", "value": source}],
        }
        license_value = package.get("license")
        if license_value:
            component["licenses"] = [{"expression": license_value}]
        components[key] = component
    return [components[key] for key in sorted(components)]


def npm_components(lock_path: pathlib.Path) -> list[dict[str, Any]]:
    if not lock_path.exists():
        return []
    lock = json.loads(lock_path.read_text(encoding="utf-8"))
    components: dict[str, dict[str, Any]] = {}
    for path, package in lock.get("packages", {}).items():
        if not path.startswith("node_modules/"):
            continue
        name = path.removeprefix("node_modules/")
        version = package.get("version")
        if not version:
            continue
        key = f"npm:{name}:{version}"
        component: dict[str, Any] = {
            "type": "library",
            "bom-ref": key,
            "name": name,
            "version": version,
            "purl": f"pkg:npm/{name.replace('@', '%40')}@{version}",
        }
        if package.get("license"):
            component["licenses"] = [{"license": {"name": package["license"]}}]
        components[key] = component
    return [components[key] for key in sorted(components)]


def main() -> None:
    args = parse_args()
    components = cargo_components() + npm_components(
        pathlib.Path("extensions/sessionweft-vscode/package-lock.json")
    )
    serial = args.commit if args.commit != "unknown" else "unreleased"
    document = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "serialNumber": f"urn:uuid:00000000-0000-4000-8000-{serial[:12].ljust(12, '0')}",
        "version": 1,
        "metadata": {
            "timestamp": dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat(),
            "component": {
                "type": "application",
                "name": "SessionWeft",
                "version": args.version,
                "properties": [{"name": "sessionweft:commit", "value": args.commit}],
            },
            "tools": [{"vendor": "SessionWeft", "name": "generate-sbom.py", "version": "1"}],
        },
        "components": components,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    if not components:
        raise SystemExit("SBOM contains no components")
    print(args.output)


if __name__ == "__main__":
    main()

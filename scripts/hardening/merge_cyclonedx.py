#!/usr/bin/env python3
"""Merge cargo-cyclonedx crate BOMs into one deterministic release BOM."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import uuid


def identity(component: dict) -> str:
    return str(
        component.get("bom-ref")
        or component.get("purl")
        or "|".join(
            str(component.get(key, ""))
            for key in ("type", "group", "name", "version")
        )
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    parser.add_argument("inputs", nargs="+")
    args = parser.parse_args()

    components: dict[str, dict] = {}
    dependencies: dict[str, set[str]] = {}
    tools: dict[str, dict] = {}
    for raw_path in sorted(set(args.inputs)):
        path = pathlib.Path(raw_path)
        document = json.loads(path.read_text())
        for component in document.get("components", []):
            components.setdefault(identity(component), component)
        root_component = document.get("metadata", {}).get("component")
        if root_component:
            components.setdefault(identity(root_component), root_component)
        for tool in document.get("metadata", {}).get("tools", {}).get("components", []):
            tools.setdefault(identity(tool), tool)
        for dependency in document.get("dependencies", []):
            reference = str(dependency.get("ref", ""))
            if not reference:
                continue
            dependencies.setdefault(reference, set()).update(
                str(value) for value in dependency.get("dependsOn", [])
            )

    release_component = {
        "type": "application",
        "bom-ref": "pkg:github/khovan123/sessionweft@rc",
        "group": "sessionweft",
        "name": "sessionweft-release",
        "version": "rc",
        "licenses": [{"license": {"id": "Apache-2.0"}}],
        "externalReferences": [
            {
                "type": "vcs",
                "url": "https://github.com/khovan123/sessionweft",
            }
        ],
    }
    output = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": f"urn:uuid:{uuid.uuid4()}",
        "version": 1,
        "metadata": {
            "timestamp": dt.datetime.now(dt.timezone.utc)
            .replace(microsecond=0)
            .isoformat()
            .replace("+00:00", "Z"),
            "component": release_component,
            "tools": {"components": sorted(tools.values(), key=identity)},
        },
        "components": sorted(components.values(), key=identity),
        "dependencies": [
            {"ref": reference, "dependsOn": sorted(values)}
            for reference, values in sorted(dependencies.items())
        ],
    }
    pathlib.Path(args.output).write_text(json.dumps(output, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

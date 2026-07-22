#!/usr/bin/env bash
set -euo pipefail

OUTPUT="${1:-sbom/sessionweft.cdx.json}"
mkdir -p "$(dirname "$OUTPUT")"
METADATA="$(mktemp)"
trap 'rm -f "$METADATA"' EXIT
cargo metadata --locked --format-version 1 > "$METADATA"

python3 - "$METADATA" "$OUTPUT" <<'PY'
import json
import pathlib
import sys
import uuid

metadata_path, output_path = sys.argv[1:]
metadata = json.loads(pathlib.Path(metadata_path).read_text())
packages = {package["id"]: package for package in metadata["packages"]}
components = []
for package in sorted(packages.values(), key=lambda item: (item["name"], item["version"], item["id"])):
    source = package.get("source") or "workspace"
    component = {
        "type": "library",
        "bom-ref": package["id"],
        "name": package["name"],
        "version": package["version"],
        "properties": [
            {"name": "sessionweft:source", "value": source},
            {"name": "sessionweft:manifest_path", "value": package["manifest_path"]},
        ],
    }
    if source.startswith("registry+"):
        component["purl"] = f"pkg:cargo/{package['name']}@{package['version']}"
    licenses = package.get("license")
    if licenses:
        component["licenses"] = [{"expression": licenses}]
    components.append(component)

resolve = metadata.get("resolve") or {}
dependencies = []
for node in sorted(resolve.get("nodes", []), key=lambda item: item["id"]):
    dependencies.append({
        "ref": node["id"],
        "dependsOn": sorted(dependency["pkg"] for dependency in node.get("deps", [])),
    })

root = resolve.get("root")
root_component = packages.get(root) if root else None
metadata_component = {
    "type": "application",
    "name": "SessionWeft",
    "version": "0.1.0",
}
if root_component:
    metadata_component.update({
        "bom-ref": root_component["id"],
        "name": root_component["name"],
        "version": root_component["version"],
    })

bom = {
    "bomFormat": "CycloneDX",
    "specVersion": "1.5",
    "serialNumber": f"urn:uuid:{uuid.uuid4()}",
    "version": 1,
    "metadata": {
        "component": metadata_component,
        "tools": {
            "components": [{
                "type": "application",
                "name": "sessionweft-sbom-generator",
                "version": "1",
            }]
        },
    },
    "components": components,
    "dependencies": dependencies,
}
pathlib.Path(output_path).write_text(json.dumps(bom, indent=2, sort_keys=True) + "\n")
PY

python3 -m json.tool "$OUTPUT" >/dev/null
printf '%s\n' "Generated $OUTPUT"

#!/usr/bin/env python3
"""Create immutable release evidence for the exact CI-tested commit."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import re


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--template", required=True, type=pathlib.Path)
    parser.add_argument("--output", required=True, type=pathlib.Path)
    parser.add_argument("--commit", required=True)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    commit = args.commit.strip().lower()
    if not re.fullmatch(r"[0-9a-f]{7,64}", commit):
        raise SystemExit("--commit must be a 7 to 64 character hexadecimal object ID")

    evidence = json.loads(args.template.read_text(encoding="utf-8"))
    evidence["commit"] = commit
    evidence["generated_at"] = (
        dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Fail the release gate on high-confidence committed secret patterns."""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys

PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("private-key", re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----")),
    ("github-token", re.compile(r"\bgh[opsu]_[A-Za-z0-9]{36,}\b")),
    ("aws-access-key", re.compile(r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b")),
    ("openai-key", re.compile(r"\bsk-[A-Za-z0-9_-]{32,}\b")),
    ("anthropic-key", re.compile(r"\bsk-ant-[A-Za-z0-9_-]{32,}\b")),
    ("google-api-key", re.compile(r"\bAIza[0-9A-Za-z_-]{35}\b")),
    ("slack-token", re.compile(r"\bxox[baprs]-[0-9A-Za-z-]{20,}\b")),
    (
        "credentialed-public-url",
        re.compile(r"https?://[^\s/:]+:[^\s/@]{12,}@", re.IGNORECASE),
    ),
)

TEXT_SUFFIXES = {
    ".env",
    ".ini",
    ".json",
    ".md",
    ".py",
    ".rs",
    ".sh",
    ".toml",
    ".ts",
    ".tsx",
    ".txt",
    ".yaml",
    ".yml",
}

IGNORED_PATHS = {
    "Cargo.lock",
    "apps/vscode-sessionweft/package-lock.json",
}


def tracked_files() -> list[pathlib.Path]:
    completed = subprocess.run(
        ["git", "ls-files", "-z"],
        check=True,
        stdout=subprocess.PIPE,
    )
    return [
        pathlib.Path(raw.decode())
        for raw in completed.stdout.split(b"\0")
        if raw
    ]


def should_scan(path: pathlib.Path) -> bool:
    if path.as_posix() in IGNORED_PATHS:
        return False
    if path.name.startswith(".env"):
        return True
    return path.suffix.lower() in TEXT_SUFFIXES


def main() -> int:
    findings: list[str] = []
    for path in tracked_files():
        if not should_scan(path) or not path.is_file():
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for line_number, line in enumerate(text.splitlines(), start=1):
            for name, pattern in PATTERNS:
                if pattern.search(line):
                    findings.append(f"{path}:{line_number}: {name}")

    if findings:
        print("High-confidence secret patterns found:", file=sys.stderr)
        print("\n".join(findings), file=sys.stderr)
        return 1
    print("No high-confidence committed secrets found.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

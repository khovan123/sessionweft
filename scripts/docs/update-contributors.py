#!/usr/bin/env python3
"""Render repository contributors as an avatar-only block in CONTRIBUTING.md."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import urllib.error
import urllib.parse
import urllib.request

START = "<!-- contributors:start -->"
END = "<!-- contributors:end -->"
BOT_LOGINS = {
    "dependabot[bot]",
    "github-actions[bot]",
    "renovate[bot]",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY", ""))
    parser.add_argument("--token", default=os.environ.get("GITHUB_TOKEN", ""))
    parser.add_argument("--output", type=pathlib.Path, default=pathlib.Path("CONTRIBUTING.md"))
    parser.add_argument("--fixture", type=pathlib.Path)
    return parser.parse_args()


def fetch_contributors(repository: str, token: str) -> list[dict[str, object]]:
    if not repository or "/" not in repository:
        raise SystemExit("--repository must be in owner/name form")
    contributors: list[dict[str, object]] = []
    page = 1
    while True:
        query = urllib.parse.urlencode({"per_page": 100, "page": page, "anon": 0})
        request = urllib.request.Request(
            f"https://api.github.com/repos/{repository}/contributors?{query}",
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {token}" if token else "",
                "X-GitHub-Api-Version": "2026-03-10",
                "User-Agent": "sessionweft-contributors",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                batch = json.load(response)
        except urllib.error.HTTPError as error:
            detail = error.read().decode("utf-8", errors="replace")
            raise SystemExit(f"GitHub contributors request failed: {error.code}: {detail}") from error
        if not isinstance(batch, list):
            raise SystemExit("GitHub contributors response was not an array")
        contributors.extend(item for item in batch if isinstance(item, dict))
        if len(batch) < 100:
            break
        page += 1
    return contributors


def normalize(contributors: list[dict[str, object]]) -> list[dict[str, str]]:
    result: list[dict[str, str]] = []
    seen: set[str] = set()
    for item in contributors:
        login = str(item.get("login") or "").strip()
        avatar_url = str(item.get("avatar_url") or "").strip()
        html_url = str(item.get("html_url") or f"https://github.com/{login}").strip()
        account_type = str(item.get("type") or "User")
        if (
            not login
            or not avatar_url
            or account_type.lower() == "bot"
            or login.lower().endswith("[bot]")
            or login.lower() in BOT_LOGINS
            or login.lower() in seen
        ):
            continue
        seen.add(login.lower())
        result.append({"login": login, "avatar_url": avatar_url, "html_url": html_url})
    return result


def render(contributors: list[dict[str, str]]) -> str:
    if not contributors:
        raise SystemExit("no non-bot contributors were returned")
    lines = []
    for contributor in contributors:
        separator = "&" if "?" in contributor["avatar_url"] else "?"
        avatar = f'{contributor["avatar_url"]}{separator}s=80'
        login = contributor["login"]
        lines.append(
            f'<a href="{contributor["html_url"]}" title="@{login}">'
            f'<img src="{avatar}" width="64" height="64" alt="@{login}" /></a>'
        )
    return "\n".join(lines)


def update(path: pathlib.Path, avatars: str) -> None:
    original = path.read_text(encoding="utf-8")
    if START not in original or END not in original:
        raise SystemExit(f"{path} is missing contributor markers")
    prefix, remainder = original.split(START, 1)
    _, suffix = remainder.split(END, 1)
    updated = f"{prefix}{START}\n{avatars}\n{END}{suffix}"
    path.write_text(updated, encoding="utf-8")


def main() -> None:
    args = parse_args()
    if args.fixture:
        raw = json.loads(args.fixture.read_text(encoding="utf-8"))
        if not isinstance(raw, list):
            raise SystemExit("fixture must contain a JSON array")
    else:
        raw = fetch_contributors(args.repository, args.token)
    update(args.output, render(normalize(raw)))


if __name__ == "__main__":
    main()

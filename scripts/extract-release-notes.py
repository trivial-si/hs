#!/usr/bin/env python3
"""Assemble GitHub release notes from CHANGELOG.md and .github/release-notes.md.

Usage:
    python3 scripts/extract-release-notes.py v0.1.0
"""

from pathlib import Path
import re
import sys


def extract_changelog_section(changelog_text: str, version: str) -> str:
    heading_re = re.compile(
        rf"^##\s+{re.escape(version)}(?:\s|$).*$",
        re.MULTILINE,
    )
    match = heading_re.search(changelog_text)
    if match is None:
        raise SystemExit(
            f"CHANGELOG.md has no section for version {version}. "
            "Move release entries from Unreleased under a version heading first."
        )
    rest = changelog_text[match.end() :]
    next_heading = re.search(r"^##\s+", rest, re.MULTILINE)
    body = rest[: next_heading.start()] if next_heading else rest
    body = body.strip()
    if not body:
        raise SystemExit(f"CHANGELOG.md section for version {version} is empty.")
    return body


def main() -> None:
    if len(sys.argv) != 2 or not sys.argv[1].strip():
        raise SystemExit("usage: extract-release-notes.py TAG")

    tag = sys.argv[1].strip()
    version = tag[1:] if tag.startswith("v") else tag
    root = Path(__file__).resolve().parents[1]

    changelog = (root / "CHANGELOG.md").read_text(encoding="utf-8")
    footer = (root / ".github" / "release-notes.md").read_text(encoding="utf-8").strip()
    changes = extract_changelog_section(changelog, version)

    sys.stdout.write(f"## Changes in {tag}\n\n{changes}\n\n{footer}\n")


if __name__ == "__main__":
    main()

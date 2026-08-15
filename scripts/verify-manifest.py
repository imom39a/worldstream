#!/usr/bin/env python3
"""Verify the checked-in specification compatibility manifest without third-party packages."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "compatibility.toml"
MIRROR = ROOT / "compatibility.json"


def main() -> int:
    with SOURCE.open("rb") as source_file:
        authored = tomllib.load(source_file)
    published = json.loads(MIRROR.read_text(encoding="utf-8"))

    canonical = (
        json.dumps(authored, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    )
    failures: list[str] = []

    if authored != published:
        failures.append("compatibility.toml and compatibility.json differ semantically")
    if MIRROR.read_text(encoding="utf-8") != canonical:
        failures.append(
            "compatibility.json is not the deterministic sorted-key rendering"
        )
    if authored.get("reviewed_source") != SOURCE.name:
        failures.append("reviewed_source does not name compatibility.toml")
    if authored.get("canonical_mirror") != MIRROR.name:
        failures.append("canonical_mirror does not name compatibility.json")
    if (
        authored.get("manifest_kind") != "specification"
        or authored.get("release_ready") is not False
    ):
        failures.append(
            "bootstrap manifest must remain specification-only and release_ready=false"
        )
    unresolved = authored.get("unresolved_required_fields")
    if not isinstance(unresolved, list) or not unresolved:
        failures.append("specification manifest must retain unresolved release gates")

    if failures:
        for failure in failures:
            print(f"manifest verification failed: {failure}", file=sys.stderr)
        return 1

    print("compatibility manifest verified (specification-only, release_ready=false)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

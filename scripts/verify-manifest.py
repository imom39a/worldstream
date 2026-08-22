#!/usr/bin/env python3
"""Verify the checked-in embedded compatibility contract without dependencies."""

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
        authored.get("manifest_kind") != "release"
        or authored.get("release_ready") is not True
    ):
        failures.append(
            "embedded contract must be manifest_kind=release and release_ready=true"
        )
    unresolved = authored.get("unresolved_required_fields")
    if unresolved != []:
        failures.append("release contract must have no unresolved embedded identity")
    if authored.get("release_artifact_digest_source") != "detached_release_manifest":
        failures.append(
            "release artifact identities must use the detached release manifest"
        )
    if authored.get("evidence_digest_source") != "detached_release_manifest":
        failures.append(
            "release evidence identities must use the detached release manifest"
        )

    for row in authored.get("release_artifacts", []):
        if (
            not isinstance(row, dict)
            or row.get("status") != "detached"
            or row.get("digest") != ""
            or row.get("digest_location") != "release-manifest.json"
        ):
            failures.append(
                "release artifact rows must use detached non-self-referential identity"
            )
            break
    for row in authored.get("evidence", []):
        if (
            isinstance(row, dict)
            and row.get("release_gate") is True
            and (
                row.get("status") != "detached"
                or row.get("artifact_digest") != ""
                or row.get("artifact_digest_location") != "release-manifest.json"
            )
        ):
            failures.append("release-gated evidence rows must use detached identity")
            break

    sqlite = authored.get("storage", {}).get("sqlite", {})
    if sqlite.get("bundle_source_inventory_status") != "resolved" or not str(
        sqlite.get("bundle_source_inventory_digest", "")
    ).startswith("sha256:"):
        failures.append("bundled SQLite source inventory identity is unresolved")

    if failures:
        for failure in failures:
            print(f"manifest verification failed: {failure}", file=sys.stderr)
        return 1

    print(
        "compatibility manifest verified (embedded release contract; detached evidence required)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

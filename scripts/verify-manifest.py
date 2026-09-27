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
    manifest_kind = authored.get("manifest_kind")
    release_ready = authored.get("release_ready")
    if manifest_kind not in {"specification", "release"}:
        failures.append("manifest_kind must be specification or release")
    if not isinstance(release_ready, bool):
        failures.append("release_ready must be boolean")
    elif (manifest_kind == "specification") != (release_ready is False):
        failures.append(
            "specification requires release_ready=false and release requires release_ready=true"
        )
    unresolved = authored.get("unresolved_required_fields")
    if not isinstance(unresolved, list) or any(
        not isinstance(value, str) or not value for value in unresolved
    ):
        failures.append("unresolved_required_fields must contain non-empty strings")
        unresolved = []
    elif len(unresolved) != len(set(unresolved)):
        failures.append("unresolved_required_fields contains duplicates")
    elif release_ready and unresolved:
        failures.append("release contract must have no unresolved embedded identity")
    if authored.get("release_artifact_digest_source") != "detached_release_manifest":
        failures.append(
            "release artifact identities must use the detached release manifest"
        )
    if authored.get("evidence_digest_source") != "detached_release_manifest":
        failures.append(
            "release evidence identities must use the detached release manifest"
        )
    if (
        authored.get("qualification_evidence_digest_source")
        != "detached_release_qualification_manifest"
    ):
        failures.append(
            "post-sign qualification evidence must use the detached qualification manifest"
        )

    for row in authored.get("release_artifacts", []):
        if isinstance(row, dict) and row.get("id") == "sigstore-bundle":
            if (
                row.get("status") != "verification_material"
                or row.get("digest") != ""
                or row.get("digest_algorithm") != ""
                or row.get("digest_location") is not None
                or row.get("verification_material_location")
                != "release-manifest.json#verification_material.sigstore-bundle.path"
            ):
                failures.append(
                    "Sigstore bundle must be path-only verification material"
                )
            continue
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
    qualification_rows = [
        row
        for row in authored.get("evidence", [])
        if isinstance(row, dict) and row.get("qualification_gate") is True
    ]
    expected_qualification = {
        "starter-distribution-and-custom-pack-recovery",
        "outside-adopter-pack-author-journey",
        "outside-adopter-application-integrator-journey",
    }
    if {row.get("id") for row in qualification_rows} != expected_qualification:
        failures.append("qualification evidence inventory is incomplete or unsupported")
    for row in qualification_rows:
        if (
            row.get("release_gate") is not False
            or row.get("status") != "detached"
            or row.get("artifact_digest") != ""
            or row.get("artifact_digest_location")
            != "release-qualification-manifest.json"
        ):
            failures.append(
                "qualification evidence rows must use the post-sign detached identity"
            )
            break

    sqlite = authored.get("storage", {}).get("sqlite", {})
    if sqlite.get("bundle_source_inventory_status") != "resolved" or not str(
        sqlite.get("bundle_source_inventory_digest", "")
    ).startswith("sha256:"):
        failures.append("bundled SQLite source inventory identity is unresolved")

    pack_rows = authored.get("pack_executors", [])
    unresolved_pack_rows = [
        row
        for row in pack_rows
        if isinstance(row, dict) and row.get("status") == "unresolved"
    ]
    digest_fields = (
        "revision_digest",
        "descriptor_digest",
        "executor_artifact_digest",
        "schema_bundle_digest",
        "codec_bundle_digest",
        "golden_corpus_digest",
    )
    if release_ready and unresolved_pack_rows:
        failures.append("release contract contains an unresolved pack executor")
    for row in unresolved_pack_rows:
        pack_id = row.get("pack_id")
        if (
            not isinstance(pack_id, str)
            or not pack_id
            or row.get("required_for_release") is not True
            or row.get("selectable_for_new_rooms") is not False
            or row.get("runnable_for_retained_rooms") is not False
            or any(row.get(field) != "" for field in digest_fields)
        ):
            failures.append("unresolved pack executor row is not fail-closed")
            continue
        for field in digest_fields:
            path = f"pack_executors.{pack_id}.{field}"
            if path not in unresolved:
                failures.append(f"unresolved pack identity is not listed: {path}")

    bundle_rows = authored.get("activity_pack_bundles", [])
    negotiate_rows = [
        row
        for row in pack_rows
        if isinstance(row, dict)
        and row.get("pack_id") == "worldstream.negotiate"
        and row.get("explanatory_version") == "0.1.0"
        and row.get("status") == "resolved"
    ]
    if len(bundle_rows) != 1 or len(negotiate_rows) != 1:
        failures.append(
            "official Negotiate bundle inventory must contain exactly one resolved row"
        )
    else:
        bundle = bundle_rows[0]
        negotiate = negotiate_rows[0]
        bundle_digest = (
            bundle.get("bundle_digest") if isinstance(bundle, dict) else None
        )
        bundle_path = bundle.get("path") if isinstance(bundle, dict) else None
        bare_bundle_digest = (
            bundle_digest.removeprefix("blake3:")
            if isinstance(bundle_digest, str)
            else ""
        )
        if (
            not isinstance(bundle, dict)
            or bundle.get("pack_id") != "worldstream.negotiate"
            or bundle.get("explanatory_version") != "0.1.0"
            or bundle.get("bundle_format_id") != "worldstream/activity-pack-bundle/v1"
            or bundle.get("revision_digest") != negotiate.get("revision_digest")
            or bundle.get("bundle_digest_algorithm") != "blake3"
            or not isinstance(bundle_digest, str)
            or not bundle_digest.startswith("blake3:")
            or len(bundle_digest) != 71
            or any(
                character not in "0123456789abcdef" for character in bare_bundle_digest
            )
            or not isinstance(bundle_path, str)
            or bundle_path
            != f"examples/packs/negotiate/releases/0.1.0/worldstream-negotiate-{bare_bundle_digest}.wspack"
            or bundle.get("status") != "resolved"
            or bundle.get("required_for_release") is not True
        ):
            failures.append("official Negotiate bundle inventory identity is invalid")

    if failures:
        for failure in failures:
            print(f"manifest verification failed: {failure}", file=sys.stderr)
        return 1

    print(
        f"compatibility manifest verified ({manifest_kind}; release_ready={str(release_ready).lower()})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

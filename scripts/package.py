#!/usr/bin/env python3
"""Build and verify deterministic WorldStream distribution artifacts.

This module intentionally treats the checked-in compatibility pair as an input,
not as release evidence. A real archive can only be emitted from a manifest
whose release gates have already been completed by the owning gate framework.
The dry-run path is useful while those implementation frontiers are incomplete.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import platform
import re
import shutil
import stat
import sys
import tarfile
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath

import tomllib


def load_release_build_identity():
    path = Path(__file__).with_name("release_build_identity.py")
    spec = importlib.util.spec_from_file_location(
        "worldstream_package_release_build_identity", path
    )
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load release build identity verifier: {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


BUILD_IDENTITY = load_release_build_identity()

ROOT = Path(__file__).resolve().parents[1]
INVENTORY_PATH = Path(__file__).with_name("release_inventory.py")
INVENTORY_SPEC = importlib.util.spec_from_file_location(
    "worldstream_package_release_inventory", INVENTORY_PATH
)
if INVENTORY_SPEC is None or INVENTORY_SPEC.loader is None:  # pragma: no cover
    raise RuntimeError(f"cannot load {INVENTORY_PATH}")
INVENTORY = importlib.util.module_from_spec(INVENTORY_SPEC)
sys.modules[INVENTORY_SPEC.name] = INVENTORY
INVENTORY_SPEC.loader.exec_module(INVENTORY)
DEFAULT_UI = ROOT / "web/console/dist"
DEFAULT_UI_VERSION = ROOT / "web/console/package.json"
DEFAULT_SDK = ROOT / "sdk/python"
DEFAULT_EXAMPLES = ROOT / "examples"
DEFAULT_LICENSES = ROOT / "licenses"
OCI_FILES = ROOT / "packaging/oci"
MANIFEST_TOML = ROOT / "compatibility.toml"
MANIFEST_JSON = ROOT / "compatibility.json"
SHA1_DIGEST = re.compile(r"[0-9a-f]{40}")
SHA256_DIGEST = re.compile(r"[0-9a-f]{64}")
SHA256_REFERENCE = re.compile(r"sha256:[0-9a-f]{64}")
SPDX_CREATED = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z")
SPDX_TOOL_CREATOR = re.compile(
    r"Tool: [A-Za-z0-9][A-Za-z0-9._-]*-[0-9]+(?:\.[0-9]+)+(?:[-+][0-9A-Za-z.-]+)?"
)
VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?")
MAX_NATIVE_ARCHIVE_BYTES = 8 * 1024 * 1024 * 1024
MAX_NATIVE_ARCHIVE_MEMBERS = 20_000
MAX_NATIVE_ARCHIVE_MEMBER_BYTES = 2 * 1024 * 1024 * 1024
MAX_NATIVE_ARCHIVE_UNPACKED_BYTES = 32 * 1024 * 1024 * 1024
MAX_RELEASE_JSON_BYTES = BUILD_IDENTITY.MAX_RELEASE_JSON_BYTES
CLIENT_IDENTITY_SCHEMA = "worldstream/client-contract-identity/v1"
CLIENT_IDENTITY_SDK_PATH = "sdk/python/src/worldstream_sdk/compatibility_identity.json"
CLIENT_IDENTITY_UI_PATH = "ui/compatibility-identity.json"
CLIENT_IDENTITY_SOURCE_SDK_PATH = (
    "source/sdk/python/src/worldstream_sdk/compatibility_identity.json"
)
CLIENT_IDENTITY_SOURCE_UI_PATH = "source/web/console/public/compatibility-identity.json"
HEIST_PARITY_FIXTURE_PATH = "examples/heist/parity_fixture.json"
HEIST_PARITY_FIXTURE_SOURCE_PATH = "source/examples/heist/parity_fixture.json"
UI_SOURCE_IDENTITY_MODULE_PATH = "source/web/console/src/compatibilityIdentity.ts"
UI_SOURCE_ENTRYPOINT_PATH = "source/web/console/src/main.tsx"
UI_CONSUMED_IDENTITY_MARKER = b"__WORLDSTREAM_CLIENT_CONTRACT_IDENTITY__"
PACK_DIGEST_REFERENCE = re.compile(rb'"pack_digest"\s*:\s*"([^"]+)"')
DETACHED_RELEASE_MANIFEST_SCHEMA = "worldstream/release-artifact-manifest/v2"
CLIENT_CONTRACT_FIELDS = (
    "wire",
    "config",
    "storage_schema",
    "core_schema_version",
    "hash_suite",
)

RELEASE_ARTIFACT_IDS = (
    "source-archive",
    "native-linux-x86_64-archive",
    "native-windows-x64-archive",
    "oci-linux-amd64-image",
    "worldstream-a202-adapter",
    "worldstream-deterministic-agents",
    "worldstream-documentation",
    "worldstream-examples",
    "worldstream-licenses",
    "worldstream-negotiate-bundle",
    "worldstream-negotiate-evidence-verifier",
    "worldstream-pack-toolchain",
    "worldstream-participant-console",
    "worldstream-release-metadata",
    "worldstream-studio",
    "worldstream-typescript-pack-sdk",
    "checksums",
    "sigstore-bundle",
    "spdx-sbom",
    "slsa-provenance",
)
RELEASE_ARTIFACT_PROFILES = {
    "source-archive": "source",
    "native-linux-x86_64-archive": "native-linux-x86_64",
    "native-windows-x64-archive": "native-windows-x64",
    "oci-linux-amd64-image": "oci-linux-amd64",
    "worldstream-a202-adapter": "all",
    "worldstream-deterministic-agents": "all",
    "worldstream-documentation": "all",
    "worldstream-examples": "all",
    "worldstream-licenses": "all",
    "worldstream-negotiate-bundle": "all",
    "worldstream-negotiate-evidence-verifier": "all",
    "worldstream-pack-toolchain": "all",
    "worldstream-participant-console": "all",
    "worldstream-release-metadata": "all",
    "worldstream-studio": "all",
    "worldstream-typescript-pack-sdk": "all",
    "checksums": "all",
    "sigstore-bundle": "all",
    "spdx-sbom": "all",
    "slsa-provenance": "all",
}
RELEASE_ARTIFACT_STATUSES = {
    "resolved",
    "unresolved",
    "detached",
    "verification_material",
}
OCI_CONTEXT_REQUIRED_FILES = frozenset(
    {
        "Dockerfile",
        "entrypoint.sh",
        "oci-metadata.json",
        "bin/worldstreamd",
        "bin/worldstreamctl",
        "manifest/compatibility.toml",
        "manifest/compatibility.json",
        "metadata/profile.json",
        "metadata/release.json",
        "metadata/build.json",
        "checksums.sha256",
    }
)
BUILD_IDENTITY_PAYLOAD_ARTIFACT_IDS = {
    "source-archive",
    "native-linux-x86_64-archive",
    "native-windows-x64-archive",
    "oci-linux-amd64-image",
}
CHECKSUM_PAYLOAD_ARTIFACT_IDS = BUILD_IDENTITY_PAYLOAD_ARTIFACT_IDS | {
    "worldstream-a202-adapter",
    "worldstream-deterministic-agents",
    "worldstream-documentation",
    "worldstream-examples",
    "worldstream-licenses",
    "worldstream-negotiate-bundle",
    "worldstream-negotiate-evidence-verifier",
    "worldstream-pack-toolchain",
    "worldstream-participant-console",
    "worldstream-release-metadata",
    "worldstream-studio",
    "worldstream-typescript-pack-sdk",
}
RELEASE_EVIDENCE_DIRECTORY = "evidence"
REQUIRED_RELEASE_EVIDENCE_COUNT = 18
PRE_SIGN_SUBJECT_DIRECTORY = "supply-chain/subjects"
PRE_SIGN_SUPPLY_CHAIN_FILES = {
    "supply-chain/subject-inventory.json",
    "supply-chain/subject-inventory.bundle.json",
}
# The signature bundle is verification material for release-manifest.json. Its
# bytes necessarily depend on the manifest being signed, so including its own
# digest in that manifest would recreate the self-reference ADR 0012 removes.
DETACHED_DIGEST_ARTIFACT_IDS = set(RELEASE_ARTIFACT_IDS) - {"sigstore-bundle"}
UNSUPPORTED_ARTIFACT_MARKERS = (
    "arm64",
    "aarch64",
    "darwin",
    "macos-binary",
    "windows-container",
    "windows_container",
    ".msi",
    ".msix",
    "windows-service",
    "windows_service",
    "kubernetes",
    "helm",
    "cloud-",
    "cloud_",
)


class PackageError(RuntimeError):
    """A fail-closed package input or verification error."""


@dataclass(frozen=True)
class Target:
    name: str
    archive_suffix: str
    binary_names: tuple[str, ...]
    expected_system: str
    expected_machine: tuple[str, ...]
    oci: bool = False
    source: bool = False


TARGETS = {
    "linux-x86_64": Target(
        "linux-x86_64",
        ".tar.gz",
        ("worldstreamd", "worldstreamctl"),
        "Linux",
        ("x86_64", "amd64"),
    ),
    "windows-x64": Target(
        "windows-x64",
        ".zip",
        ("worldstreamd.exe", "worldstreamctl.exe"),
        "Windows",
        ("AMD64", "x86_64", "amd64"),
    ),
    "oci-linux-amd64": Target(
        "oci-linux-amd64",
        "",
        ("worldstreamd", "worldstreamctl"),
        "Linux",
        ("x86_64", "amd64"),
        oci=True,
    ),
    "source": Target(
        "source",
        ".tar.gz",
        (),
        "",
        (),
        source=True,
    ),
}


def inventory_contract(release_inventory: str | None = None):
    try:
        return INVENTORY.resolve(release_inventory)
    except ValueError as error:
        raise PackageError(str(error)) from error


def target_for_inventory(
    target: Target, release_inventory: str | None = None
) -> Target:
    """Return the native payload contract for one release inventory."""

    profile = inventory_contract(release_inventory)
    if target.source or target.oci or profile is INVENTORY.LEGACY:
        return target
    suffix = ".exe" if target.name == "windows-x64" else ""
    return Target(
        target.name,
        target.archive_suffix,
        tuple(name + suffix for name in profile.native_binaries),
        target.expected_system,
        target.expected_machine,
        oci=target.oci,
        source=target.source,
    )


def fail(message: str) -> None:
    raise PackageError(message)


def release_evidence_verifier():
    """Load the single normalized-evidence verifier used by final assembly."""

    path = ROOT / "scripts/release-evidence-assemble.py"
    spec = importlib.util.spec_from_file_location(
        "worldstream_package_release_evidence_verifier", path
    )
    if spec is None or spec.loader is None:
        fail(f"cannot load release evidence verifier: {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def validate_manifest_shape(manifest: dict) -> None:
    """Validate identity and release inventory without approving a release.

    The checked-in pair is the reviewed compatibility contract. This
    validation is intentionally usable for both specification and release
    manifests, but contract identity alone is not detached release evidence.
    """

    if manifest.get("schema") != "worldstream/storage-compatibility-manifest/v1":
        fail("compatibility manifest schema identity is invalid")
    revision = manifest.get("manifest_revision")
    if isinstance(revision, bool) or not isinstance(revision, int) or revision < 1:
        fail("compatibility manifest revision must be a positive integer")
    if manifest.get("validation_policy") != "fail_closed":
        fail("compatibility manifest validation policy is not fail_closed")
    if manifest.get("reviewed_source") != MANIFEST_TOML.name:
        fail("compatibility manifest reviewed_source identity is inconsistent")
    if manifest.get("canonical_mirror") != MANIFEST_JSON.name:
        fail("compatibility manifest canonical_mirror identity is inconsistent")
    kind = manifest.get("manifest_kind")
    ready = manifest.get("release_ready")
    if kind not in {"specification", "release"} or not isinstance(ready, bool):
        fail("compatibility manifest kind/readiness identity is invalid")
    if (kind == "specification") != (ready is False):
        fail("specification manifests must remain release_ready=false")
    contracts = manifest.get("contracts")
    if not isinstance(contracts, dict):
        fail("compatibility manifest has no contracts object")
    product = contracts.get("product")
    if not isinstance(product, str) or VERSION.fullmatch(product) is None:
        fail("compatibility manifest product is not a valid release version")
    if manifest.get("release_candidate") != product:
        fail("compatibility release_candidate does not equal contracts.product")
    unresolved = manifest.get("unresolved_required_fields")
    if not isinstance(unresolved, list) or any(
        not isinstance(value, str) or not value for value in unresolved
    ):
        fail("compatibility manifest unresolved_required_fields is invalid")
    if len(unresolved) != len(set(unresolved)):
        fail("compatibility manifest unresolved_required_fields contains duplicates")

    rows = manifest.get("release_artifacts")
    if not isinstance(rows, list):
        fail("compatibility manifest has no release artifact inventory")
    seen: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            fail("compatibility manifest release artifact row is not an object")
        artifact_id = row.get("id")
        if artifact_id not in RELEASE_ARTIFACT_IDS:
            fail(
                f"compatibility manifest has unsupported release artifact id: {artifact_id!r}"
            )
        if artifact_id in seen:
            fail(
                f"compatibility manifest has duplicate release artifact id: {artifact_id}"
            )
        seen.add(artifact_id)
        if row.get("profile") != RELEASE_ARTIFACT_PROFILES[artifact_id]:
            fail(
                f"compatibility manifest release artifact profile mismatch: {artifact_id}"
            )
        if artifact_id == "sigstore-bundle":
            if (
                row.get("status") != "verification_material"
                or row.get("digest") != ""
                or row.get("digest_algorithm") != ""
                or row.get("digest_location") is not None
                or row.get("verification_material_location")
                != "release-manifest.json#verification_material.sigstore-bundle.path"
            ):
                fail(
                    "compatibility manifest Sigstore bundle is not path-only "
                    "verification material"
                )
            continue
        if row.get("verification_material_location") is not None:
            fail(
                "non-Sigstore compatibility artifact claims verification material: "
                f"{artifact_id}"
            )
        if row.get("digest_algorithm") != "sha256":
            fail(
                f"compatibility manifest release artifact digest algorithm mismatch: {artifact_id}"
            )
        status = row.get("status")
        if status not in RELEASE_ARTIFACT_STATUSES:
            fail(
                f"compatibility manifest release artifact status is invalid: {artifact_id}"
            )
        digest = row.get("digest")
        if not isinstance(digest, str) or (
            digest and not SHA256_REFERENCE.fullmatch(digest)
        ):
            fail(
                f"compatibility manifest release artifact digest is invalid: {artifact_id}"
            )
        if status == "resolved" and not digest:
            fail(
                f"compatibility manifest release artifact status/digest mismatch: {artifact_id}"
            )
        if status in {"unresolved", "detached"} and digest:
            fail(
                f"compatibility manifest {status} artifact must not embed a digest: {artifact_id}"
            )
        digest_location = row.get("digest_location")
        if digest_location is not None and (
            not isinstance(digest_location, str) or not digest_location
        ):
            fail(
                f"compatibility manifest artifact digest_location is invalid: {artifact_id}"
            )
        if status == "detached" and digest_location != "release-manifest.json":
            fail(
                "detached compatibility artifact must locate its digest in "
                f"release-manifest.json: {artifact_id}"
            )
        if ready and status == "unresolved":
            fail(
                f"release-ready manifest has unresolved artifact identity: {artifact_id}"
            )
    if set(seen) != set(RELEASE_ARTIFACT_IDS):
        missing = sorted(set(RELEASE_ARTIFACT_IDS) - seen)
        extra = sorted(seen - set(RELEASE_ARTIFACT_IDS))
        fail(
            "compatibility manifest release artifact inventory is incomplete"
            + (f"; missing={missing}" if missing else "")
            + (f"; unsupported={extra}" if extra else "")
        )

    evidence_rows = manifest.get("evidence")
    if not isinstance(evidence_rows, list):
        fail("compatibility manifest has no evidence inventory")
    release_evidence_ids: set[str] = set()
    for index, row in enumerate(evidence_rows):
        if not isinstance(row, dict):
            fail(f"compatibility manifest evidence row {index} is not an object")
        if row.get("release_gate") is not True:
            continue
        evidence_id = row.get("id")
        if not isinstance(evidence_id, str) or not evidence_id:
            fail(f"release-gated compatibility evidence row {index} has no id")
        if evidence_id in release_evidence_ids:
            fail(
                f"compatibility release-gated evidence ID is duplicated: {evidence_id}"
            )
        if (
            row.get("status") != "detached"
            or row.get("artifact_digest") != ""
            or row.get("artifact_digest_location") != "release-manifest.json"
        ):
            fail(
                f"release-gated compatibility evidence {evidence_id} must be "
                "detached with an empty embedded digest and "
                "artifact_digest_location=release-manifest.json"
            )
        release_evidence_ids.add(evidence_id)
    if len(release_evidence_ids) != REQUIRED_RELEASE_EVIDENCE_COUNT:
        fail(
            "compatibility manifest must declare exactly "
            f"{REQUIRED_RELEASE_EVIDENCE_COUNT} unique release-gated evidence rows"
        )
    if ready and (kind != "release" or unresolved):
        fail("release-ready compatibility manifest has unresolved release identity")


def read_manifest(
    manifest_toml_path: Path = MANIFEST_TOML,
    manifest_json_path: Path = MANIFEST_JSON,
) -> tuple[dict, bytes, bytes]:
    try:
        authored_bytes = bounded_regular_bytes(manifest_toml_path, "compatibility.toml")
        authored = tomllib.loads(authored_bytes.decode("utf-8"))
        mirror_bytes = bounded_regular_bytes(manifest_json_path, "compatibility.json")
    except (OSError, ValueError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot read compatibility manifest pair: {error}")
    mirror = json_object(mirror_bytes, "compatibility.json")

    canonical = (
        json.dumps(authored, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode()
    if authored != mirror:
        fail("compatibility.toml and compatibility.json differ semantically")
    if mirror_bytes != canonical:
        fail("compatibility.json is not the deterministic sorted-key mirror")
    validate_manifest_shape(authored)
    return authored, authored_bytes, mirror_bytes


def canonical_client_contract_identity(manifest: dict) -> dict:
    """Derive the exact client-facing contract identity from the manifest."""

    contracts = manifest.get("contracts")
    if not isinstance(contracts, dict):
        fail("client contract identity has no contracts object")
    missing_contracts = [
        field for field in CLIENT_CONTRACT_FIELDS if field not in contracts
    ]
    if missing_contracts:
        fail(
            "client contract identity is missing contract fields: "
            + ", ".join(missing_contracts)
        )
    rows = manifest.get("pack_executors")
    if not isinstance(rows, list) or not rows:
        fail("client contract identity requires a non-empty pack_executors set")

    required_pack_fields = {
        "pack_id",
        "explanatory_version",
        "host_contract_id",
        "revision_lock_id",
        "revision_digest_algorithm",
        "revision_digest",
        "descriptor_digest",
        "executor_artifact_digest",
        "schema_bundle_digest",
        "codec_bundle_digest",
        "golden_corpus_digest",
        "selectable_for_new_rooms",
        "runnable_for_retained_rooms",
        "status",
        "required_for_release",
    }
    identities: list[dict] = []
    seen: set[tuple[str, str]] = set()
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            fail(f"client contract identity pack row {index} is not an object")
        missing = sorted(required_pack_fields - set(row))
        if missing:
            fail(
                f"client contract identity pack row {index} is missing: "
                + ", ".join(missing)
            )
        key = (row["pack_id"], row["explanatory_version"])
        if any(not isinstance(value, str) or not value for value in key):
            fail(f"client contract identity pack row {index} has an invalid identity")
        if key in seen:
            fail(
                "client contract identity contains duplicate pack identity: "
                + "/".join(key)
            )
        seen.add(key)
        # Preserve every manifest field in each row.  This prevents a newly
        # introduced digest or retained-room policy from being silently
        # omitted from the SDK/UI contract.
        identities.append({key: row[key] for key in sorted(row)})

    return {
        "schema": CLIENT_IDENTITY_SCHEMA,
        "manifest_schema": manifest.get("schema"),
        "manifest_revision": manifest.get("manifest_revision"),
        "product": contracts.get("product"),
        **{field: contracts[field] for field in CLIENT_CONTRACT_FIELDS},
        "pack_executors": sorted(
            identities,
            key=lambda row: (row["pack_id"], row["explanatory_version"]),
        ),
    }


def canonical_client_contract_identity_bytes(manifest: dict) -> bytes:
    return (
        json.dumps(
            canonical_client_contract_identity(manifest),
            ensure_ascii=False,
            indent=2,
            sort_keys=True,
        )
        + "\n"
    ).encode()


def validate_client_contract_identity(
    content: bytes, manifest: dict, label: str
) -> None:
    actual = json_object(content, label)
    expected = canonical_client_contract_identity(manifest)
    if content != canonical_client_contract_identity_bytes(manifest):
        fail(f"{label} is not the canonical deterministic identity")
    if actual != expected:
        fail(f"{label} differs from the compatibility manifest contract identity")


def validate_packaged_client_identities(
    files: list[tuple[str, bytes]], manifest: dict, target: Target
) -> None:
    by_path = dict(files)
    if target.source:
        sdk_path = CLIENT_IDENTITY_SOURCE_SDK_PATH
        ui_path = CLIENT_IDENTITY_SOURCE_UI_PATH
    else:
        sdk_path = CLIENT_IDENTITY_SDK_PATH
        ui_path = CLIENT_IDENTITY_UI_PATH
    missing = [path for path in (sdk_path, ui_path) if path not in by_path]
    if missing:
        fail("packaged client contract identity is missing: " + ", ".join(missing))
    validate_client_contract_identity(
        by_path[sdk_path], manifest, "packaged Python SDK client identity"
    )
    validate_client_contract_identity(
        by_path[ui_path], manifest, "packaged UI client identity"
    )
    if by_path[sdk_path] != by_path[ui_path]:
        fail("packaged Python SDK and UI client identities disagree")


def validate_packaged_ui_consumed_assets(
    files: list[tuple[str, bytes]], manifest: dict, target: Target
) -> None:
    """Reject UI-consumed fixtures or bundles that name an orphan pack digest."""

    by_path = dict(files)
    fixture_path = (
        HEIST_PARITY_FIXTURE_SOURCE_PATH if target.source else HEIST_PARITY_FIXTURE_PATH
    )
    fixture_content = by_path.get(fixture_path)
    if fixture_content is None:
        fail(f"packaged UI-consumed Heist parity fixture is missing: {fixture_path}")
    fixture = json_object(fixture_content, "packaged Heist parity fixture")
    retained = fixture.get("retained_executor")
    if not isinstance(retained, dict):
        fail("packaged Heist parity fixture has no retained_executor object")
    pack_id = retained.get("pack_id")
    pack_version = retained.get("pack_version")
    pack_digest = retained.get("pack_digest")
    if not all(
        isinstance(value, str) and value
        for value in (pack_id, pack_version, pack_digest)
    ):
        fail("packaged Heist parity fixture has an incomplete retained pack identity")
    rows = canonical_client_contract_identity(manifest)["pack_executors"]
    matching_rows = [
        row
        for row in rows
        if row["pack_id"] == pack_id and row["explanatory_version"] == pack_version
    ]
    if len(matching_rows) != 1:
        fail(
            "packaged Heist parity fixture names a missing pack identity: "
            f"{pack_id}/{pack_version}"
        )
    if pack_digest != matching_rows[0]["revision_digest"]:
        fail(f"packaged Heist parity fixture names a stale pack digest: {pack_digest}")

    allowed_revision_digests = {row["revision_digest"] for row in rows}
    for relative, content in files:
        if not relative.startswith("ui/") or relative == CLIENT_IDENTITY_UI_PATH:
            continue
        for match in PACK_DIGEST_REFERENCE.finditer(content):
            try:
                digest = match.group(1).decode("ascii")
            except UnicodeDecodeError:
                fail(f"packaged UI asset has a non-ASCII pack digest: {relative}")
            if digest not in allowed_revision_digests:
                fail(
                    "packaged UI asset names an orphan pack digest: "
                    f"{digest} ({relative})"
                )


def validate_packaged_ui_consumed_identity(
    files: list[tuple[str, bytes]], manifest: dict, target: Target
) -> None:
    """Prove production UI consumes the same identity artifact it packages."""

    by_path = dict(files)
    expected = canonical_client_contract_identity_bytes(manifest)
    if target.source:
        module = by_path.get(UI_SOURCE_IDENTITY_MODULE_PATH)
        entrypoint = by_path.get(UI_SOURCE_ENTRYPOINT_PATH)
        if module is None or entrypoint is None:
            fail("source archive is missing the production UI identity path")
        if (
            b"../public/compatibility-identity.json?raw" not in module
            or b"JSON.parse(identitySource)" not in module
            or b"CLIENT_CONTRACT_IDENTITY_JSON" not in module
        ):
            fail(
                "source UI identity module does not consume the public identity artifact"
            )
        if UI_CONSUMED_IDENTITY_MARKER not in entrypoint:
            fail("source UI entrypoint does not expose the consumed client identity")
        return

    bundles = [
        (relative, content)
        for relative, content in files
        if relative.startswith("ui/assets/")
    ]
    if not bundles:
        fail("packaged UI has no production asset bundle")
    if not any(UI_CONSUMED_IDENTITY_MARKER in content for _, content in bundles):
        fail("packaged UI bundle does not expose the consumed client identity")
    if not any(expected in content for _, content in bundles):
        fail(
            "packaged UI bundle identity does not exactly match the manifest-derived identity"
        )


def validate_release_manifest(manifest: dict, *, dry_run: bool) -> None:
    validate_manifest_shape(manifest)
    unresolved = manifest["unresolved_required_fields"]
    if dry_run:
        return
    if manifest.get("manifest_kind") != "release":
        fail("release packaging requires manifest_kind=release")
    if manifest.get("release_ready") is not True:
        fail("release packaging requires compatibility manifest release_ready=true")
    if unresolved:
        fail(
            "release packaging requires every unresolved release field to be populated"
        )


def version_of(manifest: dict) -> str:
    contracts = manifest.get("contracts")
    if not isinstance(contracts, dict):
        fail("compatibility manifest has no contracts object")
    value = contracts.get("product")
    if not isinstance(value, str) or not value:
        fail("compatibility manifest has no non-empty contracts.product")
    validate_artifact_component(value, "compatibility manifest product version")
    return value


def validate_artifact_component(value: str, label: str) -> None:
    if (
        not value
        or value in {".", ".."}
        or "/" in value
        or "\\" in value
        or any(ord(character) < 0x20 or ord(character) == 0x7F for character in value)
    ):
        fail(f"{label} is not a safe artifact path component: {value!r}")


def path_exists(path: Path) -> bool:
    """Return true for regular, directory, and dangling-symlink paths."""

    try:
        path.lstat()
    except FileNotFoundError:
        return False
    except OSError as error:
        fail(f"cannot inspect path {path}: {error}")
    return True


def validate_output_directory(path: Path, label: str) -> None:
    if path.is_symlink():
        fail(f"{label} must not be a symlink: {path}")
    if path_exists(path) and not path.is_dir():
        fail(f"{label} is not a directory: {path}")


def validate_target_profile(manifest: dict, target: Target) -> None:
    if target.source:
        release_artifacts = manifest.get("release_artifacts", [])
        if release_artifacts and not any(
            isinstance(row, dict)
            and row.get("id") == "source-archive"
            and row.get("profile") == "source"
            for row in release_artifacts
        ):
            fail("compatibility manifest has no source archive profile")
        return
    profiles = manifest.get("platforms")
    if not isinstance(profiles, list):
        fail("compatibility manifest has no platform profile list")
    expected_target = {
        "linux-x86_64": "x86_64-unknown-linux-musl",
        "windows-x64": "x86_64-pc-windows-msvc",
        "oci-linux-amd64": "linux/amd64",
    }[target.name]
    expected_profile_id = {
        "linux-x86_64": "native-linux-x86_64",
        "windows-x64": "native-windows-x64",
        "oci-linux-amd64": "oci-linux-amd64",
    }[target.name]
    matches = [
        profile
        for profile in profiles
        if isinstance(profile, dict) and profile.get("id") == expected_profile_id
    ]
    if len(matches) != 1 or matches[0].get("target") != expected_target:
        fail(
            f"compatibility manifest has no exact profile for {target.name} ({expected_target})"
        )
    profile = matches[0]
    if profile.get("support") != "release":
        fail(f"compatibility profile is not release-supported: {target.name}")
    if profile.get("storage_profiles") != ["sqlite-bundled", "postgres-primary"]:
        fail(f"compatibility profile storage selection mismatch: {target.name}")
    if target.oci:
        if profile.get("persistent_data_path") != "/var/lib/worldstream":
            fail("OCI profile persistent data path is not /var/lib/worldstream")
        if (
            profile.get("read_only_root_compatible") is not True
            or profile.get("sqlite_overlay_allowed") is not False
        ):
            fail(
                "OCI profile does not declare the required read-only/SQLite overlay policy"
            )


def validate_source_file(path: Path, relative: str) -> None:
    try:
        metadata = path.lstat()
    except OSError as error:
        fail(f"cannot inspect package input {path}: {error}")
    if not stat.S_ISREG(metadata.st_mode):
        fail(f"package input is not a regular file: {path}")
    if path.is_symlink():
        fail(f"package input symlink is not allowed: {path}")
    # Windows junctions and other reparse points are not always reported by
    # pathlib as symlinks.  Treat every reparse-point input as unsafe so a
    # package cannot escape its declared source tree on a native Windows
    # runner.
    reparse_flag = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
    if os.name == "nt" and getattr(metadata, "st_file_attributes", 0) & reparse_flag:
        fail(f"package input reparse point is not allowed: {path}")
    if os.name != "nt" and metadata.st_mode & (stat.S_IWGRP | stat.S_IWOTH):
        fail(f"group/world-writable package input is not allowed: {path}")
    basename = PurePosixPath(relative.replace("\\", "/")).name.casefold()
    suffix = PurePosixPath(basename).suffix
    secret_suffixes = {".pem", ".key", ".p12", ".pfx", ".jks", ".kdbx"}
    secret_basenames = {
        ".env",
        "credential",
        "credential.json",
        "credentials",
        "credentials.json",
        "dsn",
        "dsn.txt",
        "id_dsa",
        "id_ecdsa",
        "id_ed25519",
        "id_rsa",
        "secret",
        "secret.json",
        "secrets",
        "secrets.json",
    }
    secret_prefixes = ("credential.", "credentials.", "dsn.", "secret.", "secrets.")
    if (
        suffix in secret_suffixes
        or basename in secret_basenames
        or basename.startswith(".env.")
        or basename.endswith(".env")
        or basename.startswith(secret_prefixes)
    ):
        fail(f"secret-like package input is not allowed: {relative}")


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_package_path(relative: str) -> None:
    if not isinstance(relative, str):
        fail("package path is not text")
    if (
        not relative
        or "\\" in relative
        or relative.startswith("/")
        or relative.endswith("/")
        or any(
            ord(character) < 0x20 or ord(character) == 0x7F for character in relative
        )
    ):
        fail(f"unsafe package path: {relative!r}")
    path = PurePosixPath(relative)
    if (
        path.is_absolute()
        or ".." in path.parts
        or "." in path.parts
        or str(path) != relative
    ):
        fail(f"unsafe package path: {relative!r}")


def sorted_file_entries(files: list[tuple[str, bytes]]) -> list[tuple[str, bytes]]:
    seen: set[str] = set()
    for relative, content in files:
        validate_package_path(relative)
        if not isinstance(content, bytes):
            fail(f"package content is not bytes: {relative}")
        if relative in seen:
            fail(f"duplicate package path: {relative}")
        seen.add(relative)
    return sorted(files, key=lambda entry: entry[0])


def source_date_epoch_of(value: str) -> int:
    try:
        epoch = int(value)
    except (TypeError, ValueError) as error:
        fail(f"SOURCE_DATE_EPOCH must be a non-negative integer: {error}")
    if epoch < 0:
        fail("SOURCE_DATE_EPOCH must be a non-negative integer")
    try:
        datetime.fromtimestamp(epoch, timezone.utc)
    except (OverflowError, OSError, ValueError) as error:
        fail(f"SOURCE_DATE_EPOCH is outside the supported timestamp range: {error}")
    return epoch


SOURCE_ARCHIVE_EXCLUDED_DIRECTORY_NAMES = frozenset(
    {
        ".git",
        ".hypothesis",
        ".mypy_cache",
        ".next",
        ".nyc_output",
        ".pnpm-store",
        ".venv",
        ".coverage",
        "__pycache__",
        ".pytest_cache",
        ".ruff_cache",
        ".turbo",
        "artifacts",
        "coverage",
        "dist",
        "tmp",
        "target",
        "node_modules",
        "package-extracted",
        "package-input",
        "release-inputs",
        "reports",
    }
)


def relative_files(
    directory: Path,
    *,
    include_empty_dirs: bool = False,
    excluded_directories: tuple[Path, ...] = (),
) -> list[Path]:
    if directory.is_symlink() or not directory.is_dir():
        fail(f"required directory is missing: {directory}")
    excluded_paths = tuple(path.resolve(strict=False) for path in excluded_directories)
    files: list[Path] = []
    for path in sorted(directory.rglob("*"), key=lambda item: item.as_posix()):
        relative = path.relative_to(directory)
        if any(
            part in SOURCE_ARCHIVE_EXCLUDED_DIRECTORY_NAMES for part in relative.parts
        ):
            continue
        resolved = path.resolve(strict=False)
        if any(
            resolved == excluded_path or excluded_path in resolved.parents
            for excluded_path in excluded_paths
        ):
            continue
        if path.is_symlink():
            fail(f"package input symlink is not allowed: {path}")
        if path.is_dir():
            continue
        if not path.is_file():
            fail(f"package input is not a regular file: {path}")
        files.append(path)
    if not files and not include_empty_dirs:
        fail(f"required directory is empty: {directory}")
    for path in files:
        validate_source_file(path, path.relative_to(directory).as_posix())
    return files


def validate_commit_bound_source_inventory(
    source_root: Path, source_inputs: list[tuple[Path, str]]
) -> None:
    """Require the deterministic release-source subset of tracked Git files."""

    tracked = BUILD_IDENTITY.tracked_source_paths(source_root)
    if tracked is None:
        return
    expected = {
        relative
        for relative in tracked
        if not any(
            part in SOURCE_ARCHIVE_EXCLUDED_DIRECTORY_NAMES
            for part in PurePosixPath(relative).parts
        )
    }
    packaged = {
        destination.removeprefix("source/")
        for _source, destination in source_inputs
        if destination.startswith("source/")
    }
    if packaged != expected:
        missing = sorted(expected - packaged)[:10]
        extra = sorted(packaged - expected)[:10]
        fail(
            "source archive inputs must equal the commit-bound release-source "
            f"inventory: missing={missing}; extra={extra}"
        )


def required_inputs(
    target: Target,
    binary_dir: Path,
    ui_dir: Path,
    sdk_dir: Path,
    examples_dir: Path,
    licenses_dir: Path,
    *,
    allow_missing: bool = False,
    source_dir: Path | None = None,
    source_excludes: tuple[Path, ...] = (),
) -> dict[str, list[tuple[Path, str]]]:
    entries: dict[str, list[tuple[Path, str]]] = {
        "bin": [],
        "ui": [],
        "sdk": [],
        "examples": [],
        "licenses": [],
        "source": [],
    }
    for binary in target.binary_names:
        source = binary_dir / binary
        if not source.is_file():
            if allow_missing:
                continue
            fail(f"required binary is missing: {source}")
        validate_source_file(source, binary)
        entries["bin"].append((source, f"bin/{binary}"))

    if target.source:
        if source_dir is None:
            fail("source packaging requires --source-dir")
        source_files = relative_files(
            source_dir,
            excluded_directories=source_excludes,
        )
        entries["source"] = [
            (path, f"source/{path.relative_to(source_dir).as_posix()}")
            for path in source_files
        ]
        return entries

    if not (ui_dir / "index.html").is_file() and not allow_missing:
        fail(f"built UI is missing index.html: {ui_dir}")
    if ui_dir.is_dir():
        entries["ui"] = [
            (path, f"ui/{path.relative_to(ui_dir).as_posix()}")
            for path in relative_files(ui_dir)
        ]

    required_sdk_files = [
        sdk_dir / "pyproject.toml",
        sdk_dir / "uv.lock",
        sdk_dir / "README.md",
    ]
    if any(not path.is_file() for path in required_sdk_files) and not allow_missing:
        missing = ", ".join(
            str(path) for path in required_sdk_files if not path.is_file()
        )
        fail(f"Python SDK source is incomplete; missing: {missing}")
    if not (sdk_dir / "src").is_dir() and not allow_missing:
        fail(f"Python SDK source package is missing: {sdk_dir / 'src'}")
    if sdk_dir.is_dir():
        entries["sdk"] = [
            (path, f"sdk/python/{path.relative_to(sdk_dir).as_posix()}")
            for path in relative_files(sdk_dir)
        ]

    heist_dir = examples_dir / "heist"
    if not heist_dir.is_dir() and not allow_missing:
        fail(f"required Heist client tree is missing: {heist_dir}")
    if examples_dir.is_dir():
        entries["examples"] = [
            (path, f"examples/{path.relative_to(examples_dir).as_posix()}")
            for path in relative_files(examples_dir)
        ]

    if licenses_dir.is_dir():
        license_files = relative_files(licenses_dir)
        entries["licenses"] = [
            (path, f"licenses/{path.relative_to(licenses_dir).as_posix()}")
            for path in license_files
        ]
    elif not allow_missing:
        fail(f"required license directory is missing: {licenses_dir}")
    return entries


def expected_input_paths(
    target: Target,
    binary_dir: Path,
    ui_dir: Path,
    sdk_dir: Path,
    examples_dir: Path,
    licenses_dir: Path,
    source_dir: Path | None = None,
) -> list[Path]:
    if target.source:
        return [source_dir or ROOT]
    return [
        *(binary_dir / name for name in target.binary_names),
        ui_dir / "index.html",
        sdk_dir / "pyproject.toml",
        sdk_dir / "uv.lock",
        sdk_dir / "README.md",
        sdk_dir / "src",
        examples_dir / "heist",
        licenses_dir,
    ]


def check_native_host(target: Target, *, dry_run: bool) -> None:
    if dry_run or target.source:
        return
    if platform.system() != target.expected_system:
        fail(
            f"{target.name} must be packaged on {target.expected_system}; found {platform.system()}"
        )
    if platform.machine() not in target.expected_machine:
        fail(f"{target.name} must be packaged on x86-64; found {platform.machine()}")


LINUX_MOUNTINFO = Path("/proc/self/mountinfo")


def decode_mountinfo_path(value: str) -> str:
    """Decode the escapes Linux uses for path fields in mountinfo."""

    for encoded, decoded in (
        (r"\040", " "),
        (r"\011", "\t"),
        (r"\012", "\n"),
        (r"\134", "\\"),
    ):
        value = value.replace(encoded, decoded)
    return value


def linux_mount_filesystem(path: Path) -> str:
    """Return the kernel filesystem name for the most-specific mount."""

    try:
        resolved = path.resolve(strict=True)
        lines = LINUX_MOUNTINFO.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        fail(f"cannot determine packaging mount identity for {path}: {error}")
    matches: list[tuple[int, int, str]] = []
    for index, line in enumerate(lines):
        fields = line.split()
        try:
            separator = fields.index("-")
        except ValueError:
            continue
        if len(fields) < 6 or separator + 1 >= len(fields):
            continue
        mount_point = Path(decode_mountinfo_path(fields[4]))
        try:
            resolved.relative_to(mount_point)
        except ValueError:
            continue
        matches.append((len(mount_point.parts), index, fields[separator + 1]))
    if not matches:
        fail(f"cannot determine packaging mount identity for {path}")
    # Later records win when a mount has been overmounted at the same path.
    return max(matches)[2]


def validate_source_filesystem(
    target: Target, paths: list[Path], *, dry_run: bool
) -> None:
    if dry_run:
        return
    if platform.system() == "Linux" and target.name in {
        "linux-x86_64",
        "oci-linux-amd64",
    }:
        for path in paths:
            probe = path if path.exists() else path.parent
            filesystem = linux_mount_filesystem(probe)
            if filesystem not in {"ext4", "xfs"}:
                fail(
                    f"unsupported packaging filesystem for {probe}: {filesystem} (need ext4 or xfs)"
                )
    if platform.system() == "Windows":
        for path in paths:
            if str(path).startswith("\\\\"):
                fail(f"UNC/network packaging path is not allowed: {path}")


def canonical_metadata(
    *,
    target: Target,
    version: str,
    manifest_bytes: bytes,
    files: list[tuple[str, bytes]],
    source_date_epoch: int,
) -> bytes:
    files = sorted_file_entries(files)
    metadata = {
        "artifact": "worldstream-distribution/v1",
        "target": target.name,
        "version": version,
        "manifest": {
            "schema": "worldstream/storage-compatibility-manifest/v1",
            "sha256": sha256_bytes(manifest_bytes),
            "file": "manifest/compatibility.json",
        },
        "source_date_epoch": source_date_epoch,
        "files": [path for path, _ in files],
        "profile_metadata": "metadata/profile.json",
    }
    return (
        json.dumps(metadata, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode()


def canonical_profile_metadata(
    *, target: Target, version: str, manifest_bytes: bytes
) -> bytes:
    """Return the immutable platform/runtime contract embedded in an artifact."""

    if target.source:
        runtime = {
            "mode": "source_only",
            "native_binary": False,
            "probes": {},
        }
    else:
        runtime = {
            "mode": "native_binary",
            "native_binary": True,
            "probes": {
                "health": {
                    "method": "GET",
                    "path": "/healthz",
                    "expected_status": [200],
                    "semantics": "process_liveness_only",
                },
                "readiness": {
                    "method": "GET",
                    "path": "/readyz",
                    "expected_status": [200, 503],
                    "semantics": "durable_storage_and_writer_readiness",
                },
                "version": {
                    "method": "GET",
                    "path": "/version",
                    "expected_status": [200],
                    "version": version,
                    "manifest_sha256": sha256_bytes(manifest_bytes),
                },
            },
        }
    if target.source:
        data_filesystems = ["apfs"]
    elif target.name == "windows-x64":
        data_filesystems = ["ntfs", "refs"]
    else:
        data_filesystems = ["ext4", "xfs"]
    metadata = {
        "artifact": "worldstream-release-profile/v1",
        "profile": target.name,
        "target": {
            "system": target.expected_system or "source",
            "machine": list(target.expected_machine),
            "native_build_required": not target.source,
        },
        "storage_profiles": ["sqlite-bundled", "postgres-primary"],
        "backend": {
            "selection": "startup_fixed",
            "default_profile": "sqlite-bundled",
            "sqlite_bundled": {
                "filesystem_scope": "local_only",
                "supported_filesystems": data_filesystems,
                "backup_manifest": "worldstream/backup-manifest/v1",
            },
            "postgres_primary": {
                "administration": "direct_offline",
                "runtime_connection_modes": [
                    "direct",
                    "session_pool",
                    "transaction_pool",
                ],
                "secret_inputs": ["owner_readable_secret_file", "inherited_handle"],
                "remote_tls_required": True,
            },
        },
        "paths": {
            "data": {
                "selection": "explicit_config_storage_data_dir",
                "implicit_cwd_or_home_search": False,
                "packaged_payload": False,
            },
            "backup": {
                "selection": "operator_managed_destination",
                "packaged_payload": False,
                "symlink_or_reparse": "reject",
            },
            "export": {
                "selection": "operator_managed_destination",
                "packaged_payload": False,
                "symlink_or_reparse": "reject",
            },
        },
        "runtime": runtime,
        "permissions": {
            "directories": "0755",
            "regular_files": "0644",
            "executables": "0755",
            "group_world_write": False,
            "symlinks": False,
            "windows_acl": "owner_or_service_system_administrators_only",
        },
        "supply_chain": {
            "checksums": "embedded",
            "sigstore": "external-release-evidence-required",
            "spdx_sbom": "external-release-evidence-required",
            "slsa_provenance": "external-release-evidence-required",
            "signing_claim": False,
        },
    }
    return (
        json.dumps(metadata, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode()


def expected_profile_id(target: Target) -> str:
    if target.source:
        return "source"
    return target.name


def validate_profile_metadata(
    content: bytes, *, target: Target, version: str, manifest_bytes: bytes
) -> None:
    actual = json_object(content, "archive profile metadata")
    expected = json_object(
        canonical_profile_metadata(
            target=target, version=version, manifest_bytes=manifest_bytes
        ),
        "expected archive profile metadata",
    )
    if actual != expected:
        fail("archive profile metadata is not canonical for its target")


def validate_oci_inputs(directory: Path) -> None:
    required = ("Dockerfile", "entrypoint.sh", "oci-metadata.json")
    for name in required:
        path = directory / name
        validate_source_file(path, name)
    try:
        dockerfile = (directory / "Dockerfile").read_text(encoding="utf-8")
        entrypoint = (directory / "entrypoint.sh").read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        fail(f"OCI packaging input is not valid UTF-8 text: {error}")
    if "FROM ${WORLDSTREAM_BASE_IMAGE}" not in dockerfile:
        fail("OCI Dockerfile does not consume the pinned base image argument")
    for required_line in (
        "USER 65532:65532",
        'VOLUME ["/var/lib/worldstream"]',
        "HEALTHCHECK",
        'CMD ["/usr/local/bin/worldstreamctl", "--data-dir", "/var/lib/worldstream", "health"]',
        'io.worldstream.target="linux/amd64"',
        'org.opencontainers.image.revision="${SOURCE_REVISION}"',
        "ARG BUILD_ENVIRONMENT_BASE64=unresolved",
        'io.worldstream.build-identity="${BUILD_IDENTITY_SHA256}"',
        'io.worldstream.build-environment="${BUILD_ENVIRONMENT_BASE64}"',
        'io.worldstream.base-image="${WORLDSTREAM_BASE_IMAGE}"',
        "COPY metadata /opt/worldstream/metadata",
        "COPY oci-metadata.json /opt/worldstream/metadata/oci.json",
    ):
        if required_line not in dockerfile:
            fail(f"OCI Dockerfile is missing required policy: {required_line}")
    if any(marker in dockerfile.casefold() for marker in UNSUPPORTED_ARTIFACT_MARKERS):
        fail("OCI Dockerfile names an explicitly unsupported platform or distribution")
    for required_line in (
        'data_dir="${WORLDSTREAM__STORAGE__DATA_DIR:-/var/lib/worldstream}"',
        'profile="${WORLDSTREAM__STORAGE__PROFILE:-sqlite-bundled}"',
        "data directory must be /var/lib/worldstream",
        "ext4|xfs",
    ):
        if required_line not in entrypoint:
            fail(
                f"OCI entrypoint is missing required filesystem policy: {required_line}"
            )
    template = load_json_file(directory / "oci-metadata.json", "OCI metadata template")
    if template.get("artifact") != "worldstream-oci/v1":
        fail("OCI metadata template artifact identity is invalid")
    if (
        template.get("profile") != "oci-linux-amd64"
        or template.get("image", {}).get("architecture") != "amd64"
        or template.get("image", {}).get("os") != "linux"
        or template.get("image", {}).get("healthcheck")
        != "worldstreamctl --data-dir /var/lib/worldstream health"
    ):
        fail("OCI metadata template target identity is invalid")
    base_image = template.get("base_image")
    if (
        not isinstance(base_image, dict)
        or base_image.get("digest") != ""
        or base_image.get("status") != "must_be_pinned_by_release_evidence"
    ):
        fail("OCI metadata template must remain unpinned specification-only metadata")


def canonical_oci_metadata(
    *,
    version: str,
    manifest_bytes: bytes,
    base_image: str,
    epoch: int,
    source_revision: str,
    build_identity_sha256: str,
    observed_build_environment: dict,
) -> dict:
    return {
        "artifact": "worldstream-oci/v1",
        "version": version,
        "profile": "oci-linux-amd64",
        "target": "linux/amd64",
        "manifest_sha256": sha256_bytes(manifest_bytes),
        "base_image": base_image,
        "source_revision": source_revision,
        "build_identity_sha256": build_identity_sha256,
        "observed_build_environment": observed_build_environment,
        "build_environment_base64": BUILD_IDENTITY.observed_build_environment_label(
            observed_build_environment
        ),
        "source_date_epoch": epoch,
        "runtime": {
            "uid": 65532,
            "gid": 65532,
            "read_only_root": True,
            "volume": "/var/lib/worldstream",
            "healthcheck": "worldstreamctl --data-dir /var/lib/worldstream health",
            "probes": {
                "healthz": "GET /healthz -> 200 status=ok",
                "readyz": "GET /readyz -> 200 status=ready or 503 with stable error code",
                "version": "GET /version -> 200 manifest-backed product and engine identity",
            },
            "sqlite_filesystems": ["ext4", "xfs"],
            "reject_filesystems": [
                "overlay",
                "tmpfs",
                "nfs",
                "cifs",
                "fuse",
                "fuseblk",
                "smb",
            ],
        },
        "permissions": {
            "user": "65532:65532",
            "root_filesystem": "read_only_compatible",
            "data_volume": "/var/lib/worldstream",
        },
    }


def validate_generated_oci_metadata(
    content: bytes,
    *,
    version: str,
    manifest_bytes: bytes,
    base_image: str,
    epoch: int,
    source_revision: str,
    build_identity_sha256: str,
    observed_build_environment: dict,
) -> None:
    metadata = json_object(content, "generated OCI metadata")
    if metadata != canonical_oci_metadata(
        version=version,
        manifest_bytes=manifest_bytes,
        base_image=base_image,
        epoch=epoch,
        source_revision=source_revision,
        build_identity_sha256=build_identity_sha256,
        observed_build_environment=observed_build_environment,
    ):
        fail("generated OCI metadata is not canonical for the release inputs")


def checksums_file(files: list[tuple[str, bytes]]) -> bytes:
    lines = [
        f"{sha256_bytes(content)}  {path}"
        for path, content in sorted_file_entries(files)
    ]
    return ("\n".join(lines) + "\n").encode()


def toml_version(content: bytes, label: str) -> str:
    try:
        value = tomllib.loads(content.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        fail(f"{label} is not valid TOML: {error}")
    package = value.get("package")
    project = value.get("project")
    workspace = value.get("workspace")
    version = (
        package.get("version")
        if isinstance(package, dict)
        else project.get("version")
        if isinstance(project, dict)
        else workspace.get("package", {}).get("version")
        if isinstance(workspace, dict)
        else None
    )
    if not isinstance(version, str) or VERSION.fullmatch(version) is None:
        fail(f"{label} has no valid package version")
    return version


def json_version(content: bytes, label: str) -> str:
    value = json_object(content, label)
    version = value.get("version")
    if not isinstance(version, str) or VERSION.fullmatch(version) is None:
        fail(f"{label} has no valid package version")
    return version


def validate_component_version_sources(
    *,
    version: str,
    daemon_version_file: Path,
    sdk_version_file: Path,
    ui_version_file: Path,
) -> None:
    """Keep packaged client surfaces aligned with the daemon release version."""

    sources = (
        (daemon_version_file, toml_version, "daemon Cargo.toml"),
        (sdk_version_file, toml_version, "Python SDK pyproject.toml"),
        (ui_version_file, json_version, "console package.json"),
    )
    for path, parser, label in sources:
        try:
            content = bounded_regular_bytes(path, label)
        except OSError as error:
            fail(f"{label} is missing: {path}: {error}")
        observed = parser(content, label)
        if observed != version:
            fail(
                f"{label} version {observed} differs from daemon release version {version}"
            )


def validate_packaged_version_identity(
    files: list[tuple[str, bytes]], target: Target, version: str
) -> None:
    by_path = dict(files)
    if target.source:
        sdk_path = (
            "source/sdk/python/pyproject.toml"
            if "source/sdk/python/pyproject.toml" in by_path
            else "source/sdk/pyproject.toml"
        )
    else:
        sdk_path = "sdk/python/pyproject.toml"
    if sdk_path not in by_path:
        fail(f"packaged Python SDK version source is missing: {sdk_path}")
    sdk_version = toml_version(by_path[sdk_path], "packaged Python SDK pyproject.toml")
    if sdk_version != version:
        fail(
            f"packaged Python SDK version {sdk_version} differs from manifest product {version}"
        )
    if target.source:
        cargo_path = "source/Cargo.toml"
        if cargo_path not in by_path:
            fail(f"packaged Cargo version source is missing: {cargo_path}")
        cargo_version = toml_version(by_path[cargo_path], "packaged Cargo.toml")
        if cargo_version != version:
            fail(
                f"packaged Cargo version {cargo_version} differs from manifest product {version}"
            )


def collect_package_files(
    target: Target,
    version: str,
    manifest_toml: bytes,
    manifest_json: bytes,
    inputs: dict[str, list[tuple[Path, str]]],
    source_date_epoch: int,
    *,
    source_root: Path = ROOT,
    source_revision: str | None = None,
    base_image: str | None = None,
    observed_build_environment: dict | None = None,
    require_hosted_environment: bool = False,
    require_clean_checkout: bool = True,
    release_inventory: str | None = None,
) -> list[tuple[str, bytes]]:
    files = [
        ("manifest/compatibility.toml", manifest_toml),
        ("manifest/compatibility.json", manifest_json),
    ]
    for group in ("bin", "ui", "sdk", "examples", "licenses", "source"):
        for source, destination in inputs.get(group, []):
            files.append((destination, source.read_bytes()))
    files = sorted_file_entries(files)
    manifest = json_object(manifest_json, "package compatibility.json")
    validate_manifest_shape(manifest)
    validate_packaged_artifact_paths(files, target)
    validate_packaged_version_identity(files, target, version)
    validate_packaged_client_identities(files, manifest, target)
    validate_packaged_ui_consumed_assets(files, manifest, target)
    validate_packaged_ui_consumed_identity(files, manifest, target)
    revision = BUILD_IDENTITY.source_revision(
        source_root,
        source_revision,
        require_clean_checkout=require_clean_checkout,
    )
    if target.source and require_clean_checkout:
        validate_commit_bound_source_inventory(source_root, inputs.get("source", []))
    source_entries = BUILD_IDENTITY.source_entries_from_root(source_root)
    source_entries["compatibility.toml"] = manifest_toml
    source_entries["compatibility.json"] = manifest_json
    try:
        BUILD_IDENTITY.validate_third_party_notices(source_entries)
    except BUILD_IDENTITY.IdentityError as error:
        fail(f"third-party notice bundle rejected: {error}")
    if not target.source:
        packaged_files = dict(files)
        for relative in (
            "licenses/LICENSE-APACHE-2.0.txt",
            BUILD_IDENTITY.THIRD_PARTY_NOTICE_MANIFEST_PATH,
            BUILD_IDENTITY.THIRD_PARTY_NOTICE_TEXT_PATH,
        ):
            if packaged_files.get(relative) != source_entries.get(relative):
                fail(
                    f"packaged legal notice differs from the pinned source: {relative}"
                )
    if observed_build_environment is None:
        observed_build_environment = BUILD_IDENTITY.local_observed_build_environment(
            target.name,
            rustc_version=BUILD_IDENTITY.toolchains_from_materials(source_entries)[
                "rustc"
            ]["version"],
        )
    BUILD_IDENTITY.validate_observed_build_environment(
        observed_build_environment,
        target=target.name,
        expected_rustc_version=BUILD_IDENTITY.toolchains_from_materials(source_entries)[
            "rustc"
        ]["version"],
        require_hosted=require_hosted_environment,
    )
    if target.source:
        files.append(
            (
                f"source/{BUILD_IDENTITY.SOURCE_REVISION_FILE}",
                (revision + "\n").encode("ascii"),
            )
        )
    build = BUILD_IDENTITY.build_identity(
        target=target.name,
        revision=revision,
        source_entries=source_entries,
        source_date_epoch=source_date_epoch,
        manifest_sha256=sha256_bytes(manifest_json),
        base_image=base_image,
        observed_build_environment=observed_build_environment,
        release_inventory=release_inventory,
    )
    files.append(
        (BUILD_IDENTITY.BUILD_METADATA_PATH, BUILD_IDENTITY.canonical_json(build))
    )
    files.append(
        (
            "metadata/profile.json",
            canonical_profile_metadata(
                target=target, version=version, manifest_bytes=manifest_json
            ),
        )
    )
    files = sorted_file_entries(files)
    metadata = canonical_metadata(
        target=target,
        version=version,
        manifest_bytes=manifest_json,
        files=files,
        source_date_epoch=source_date_epoch,
    )
    files.append(("metadata/release.json", metadata))
    files = sorted_file_entries(files)
    files.append(("checksums.sha256", checksums_file(files)))
    return sorted_file_entries(files)


def write_tar_gz(
    path: Path, root_name: str, files: list[tuple[str, bytes]], epoch: int
) -> None:
    validate_package_path(root_name)
    files = sorted_file_entries(files)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("wb") as output:
        import gzip

        with (
            gzip.GzipFile(
                fileobj=output, mode="wb", filename="", mtime=epoch
            ) as compressed,
            tarfile.open(
                fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT
            ) as archive,
        ):
            directory_names = {
                str(PurePosixPath(root_name) / PurePosixPath(file).parent)
                for file, _ in files
            }
            for directory in sorted(directory_names):
                info = tarfile.TarInfo(directory + "/")
                info.type = tarfile.DIRTYPE
                info.mode = 0o755
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                info.mtime = epoch
                archive.addfile(info)
            for relative, content in files:
                info = tarfile.TarInfo(str(PurePosixPath(root_name) / relative))
                info.size = len(content)
                info.mode = 0o755 if relative.startswith("bin/") else 0o644
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                info.mtime = epoch
                archive.addfile(info, __import__("io").BytesIO(content))


def zip_timestamp_for_epoch(epoch: int) -> tuple[int, int, int, int, int, int]:
    date = datetime.fromtimestamp(epoch, timezone.utc)
    if date.year < 1980:
        date = datetime(1980, 1, 1, tzinfo=timezone.utc)
    return (
        date.year,
        date.month,
        date.day,
        date.hour,
        date.minute,
        date.second // 2 * 2,
    )


def write_zip(
    path: Path, root_name: str, files: list[tuple[str, bytes]], epoch: int
) -> None:
    validate_package_path(root_name)
    files = sorted_file_entries(files)
    path.parent.mkdir(parents=True, exist_ok=True)
    timestamp = zip_timestamp_for_epoch(epoch)
    with zipfile.ZipFile(
        path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9
    ) as archive:
        directory_names = {
            str(PurePosixPath(root_name) / PurePosixPath(file).parent)
            for file, _ in files
        }
        for directory in sorted(directory_names):
            info = zipfile.ZipInfo(directory + "/", timestamp)
            info.external_attr = (0o755 << 16) | 0x10
            archive.writestr(info, b"")
        for relative, content in files:
            info = zipfile.ZipInfo(str(PurePosixPath(root_name) / relative), timestamp)
            mode = 0o755 if relative.startswith("bin/") else 0o644
            info.external_attr = mode << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, content)


def write_archive_atomically(
    path: Path,
    root_name: str,
    files: list[tuple[str, bytes]],
    epoch: int,
    *,
    is_zip: bool,
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    os.close(descriptor)
    temporary = Path(temporary_name)
    try:
        if is_zip:
            write_zip(temporary, root_name, files, epoch)
        else:
            write_tar_gz(temporary, root_name, files, epoch)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def package(args: argparse.Namespace) -> int:
    release_inventory = getattr(args, "release_inventory", None)
    target = target_for_inventory(TARGETS[args.target], release_inventory)
    source_dir = getattr(args, "source_dir", None)
    binary_dir = Path(args.binary_dir)
    ui_dir = Path(args.ui_dir)
    sdk_dir = Path(args.sdk_dir)
    examples_dir = Path(args.examples_dir)
    licenses_dir = Path(args.licenses_dir)
    output = Path(args.output)
    source_path = Path(source_dir) if source_dir else None
    daemon_version_file = Path(
        getattr(args, "daemon_version_file", ROOT / "Cargo.toml")
    )
    ui_version_file = Path(getattr(args, "ui_version_file", DEFAULT_UI_VERSION))
    validate_output_directory(output, "package output")
    manifest, manifest_toml, manifest_json = read_manifest()
    validate_target_profile(manifest, target)
    validate_release_manifest(manifest, dry_run=args.dry_run)
    version = version_of(manifest)
    if not args.dry_run:
        validate_component_version_sources(
            version=manifest["release_candidate"],
            daemon_version_file=(
                source_path / "Cargo.toml"
                if target.source and source_path
                else daemon_version_file
            ),
            sdk_version_file=(
                source_path / "sdk/python/pyproject.toml"
                if target.source and source_path
                else sdk_dir / "pyproject.toml"
            ),
            ui_version_file=(
                source_path / "web/console/package.json"
                if target.source and source_path
                else ui_version_file
            ),
        )
    check_native_host(target, dry_run=args.dry_run)
    validate_source_filesystem(
        target,
        [
            binary_dir,
            ui_dir,
            sdk_dir,
            examples_dir,
            licenses_dir,
            output,
        ],
        dry_run=args.dry_run,
    )
    inputs = required_inputs(
        target,
        binary_dir,
        ui_dir,
        sdk_dir,
        examples_dir,
        licenses_dir,
        allow_missing=args.dry_run,
        source_dir=source_path,
        source_excludes=(output,),
    )
    epoch = source_date_epoch_of(args.source_date_epoch)
    observed_build_environment, require_hosted_environment = build_environment_argument(
        args
    )
    files = collect_package_files(
        target,
        version,
        manifest_toml,
        manifest_json,
        inputs,
        epoch,
        source_root=source_path or ROOT,
        source_revision=getattr(args, "source_revision", None),
        observed_build_environment=observed_build_environment,
        require_hosted_environment=require_hosted_environment,
        release_inventory=release_inventory,
    )
    archive_name = f"worldstream-{version}-{target.name}{target.archive_suffix}"
    output = Path(args.output)
    if args.dry_run:
        print(f"dry-run: {archive_name}")
        print(
            f"dry-run: release_ready={manifest.get('release_ready')!r} manifest_kind={manifest.get('manifest_kind')!r}"
        )
        for path in expected_input_paths(
            target,
            binary_dir,
            ui_dir,
            sdk_dir,
            examples_dir,
            licenses_dir,
            source_path,
        ):
            if not path.exists():
                print(f"dry-run: blocked input: {path}")
        if not target.source and not ui_version_file.exists():
            print(f"dry-run: blocked input: {ui_version_file}")
        print(f"dry-run: {len(files)} deterministic files")
        for relative, _ in files:
            print(f"dry-run: {relative}")
        return 0

    output.mkdir(parents=True, exist_ok=True)
    archive_path = output / archive_name
    if path_exists(archive_path):
        fail(f"refusing to overwrite existing archive: {archive_path}")
    write_archive_atomically(
        archive_path,
        f"worldstream-{version}-{target.name}",
        files,
        epoch,
        is_zip=target.name == "windows-x64",
    )
    print(f"wrote {archive_path} ({sha256_file(archive_path)})")
    return 0


def archive_entries(path: Path) -> dict[str, bytes]:
    try:
        metadata = path.lstat()
    except OSError as error:
        fail(f"cannot inspect archive: {error}")
    if (
        not stat.S_ISREG(metadata.st_mode)
        or stat.S_ISLNK(metadata.st_mode)
        or not (0 < metadata.st_size <= MAX_NATIVE_ARCHIVE_BYTES)
    ):
        fail("archive is not a bounded regular file")
    if path.suffix == ".zip":
        with zipfile.ZipFile(path) as archive:
            infos = archive.infolist()
            if len(infos) > MAX_NATIVE_ARCHIVE_MEMBERS:
                fail("archive member count exceeds the verification limit")
            names = [info.filename for info in infos]
            if len(names) != len(set(names)):
                fail("archive contains duplicate member names")
            unpacked_size = 0
            for info in infos:
                name = info.filename
                validate_archive_member(name)
                mode = (info.external_attr >> 16) & 0o170000
                if info.is_dir():
                    if mode not in {0, stat.S_IFDIR}:
                        fail(f"archive contains an unsafe directory member: {name}")
                    continue
                if (
                    mode not in {0, stat.S_IFREG}
                    or info.flag_bits & 0x1
                    or not (0 <= info.file_size <= MAX_NATIVE_ARCHIVE_MEMBER_BYTES)
                ):
                    fail(f"archive contains an unsafe or non-regular member: {name}")
                unpacked_size += info.file_size
                if unpacked_size > MAX_NATIVE_ARCHIVE_UNPACKED_BYTES:
                    fail("archive unpacked size exceeds the verification limit")
            entries = {
                name: archive.read(name) for name in names if not name.endswith("/")
            }
            if sum(len(content) for content in entries.values()) != unpacked_size:
                fail("archive member size changed while reading")
            return entries
    if path.name.endswith(".tar.gz"):
        with tarfile.open(path, "r:gz") as archive:
            entries: dict[str, bytes] = {}
            members = archive.getmembers()
            if len(members) > MAX_NATIVE_ARCHIVE_MEMBERS:
                fail("archive member count exceeds the verification limit")
            unpacked_size = 0
            for member in members:
                validate_archive_member(member.name)
                if member.isdir():
                    continue
                if member.name in entries:
                    fail(f"archive contains duplicate member names: {member.name}")
                if not member.isfile():
                    fail(
                        f"archive contains an unsafe or non-regular member: {member.name}"
                    )
                if not (0 <= member.size <= MAX_NATIVE_ARCHIVE_MEMBER_BYTES):
                    fail("archive member size exceeds the verification limit")
                unpacked_size += member.size
                if unpacked_size > MAX_NATIVE_ARCHIVE_UNPACKED_BYTES:
                    fail("archive unpacked size exceeds the verification limit")
                source = archive.extractfile(member)
                if source is None:
                    fail(f"archive member has no contents: {member.name}")
                content = source.read(MAX_NATIVE_ARCHIVE_MEMBER_BYTES + 1)
                if len(content) != member.size:
                    fail("archive member size changed while reading")
                entries[member.name] = content
            return entries
    fail(f"unsupported archive format: {path}")


def archive_member_modes(path: Path) -> dict[str, tuple[bool, int]]:
    """Return member type and POSIX permission bits for archive verification."""

    modes: dict[str, tuple[bool, int]] = {}
    if path.suffix == ".zip":
        with zipfile.ZipFile(path) as archive:
            for info in archive.infolist():
                validate_archive_member(info.filename)
                if info.filename in modes:
                    fail(f"archive contains duplicate member names: {info.filename}")
                file_type = (info.external_attr >> 16) & 0o170000
                if file_type not in {0, stat.S_IFREG, stat.S_IFDIR}:
                    fail(f"archive contains an unsafe member type: {info.filename}")
                modes[info.filename] = (
                    info.is_dir() or info.filename.endswith("/"),
                    (info.external_attr >> 16) & 0o777,
                )
        return modes
    if path.name.endswith(".tar.gz"):
        with tarfile.open(path, "r:gz") as archive:
            for member in archive.getmembers():
                validate_archive_member(member.name)
                if member.name in modes:
                    fail(f"archive contains duplicate member names: {member.name}")
                if not member.isdir() and not member.isfile():
                    fail(f"archive contains an unsafe member type: {member.name}")
                modes[member.name] = (member.isdir(), member.mode & 0o777)
        return modes
    fail(f"unsupported archive format: {path}")


def verify_archive_permissions(
    path: Path, root: str, entries: dict[str, bytes], source_date_epoch: int
) -> None:
    modes = archive_member_modes(path)
    for name, (is_directory, mode) in modes.items():
        if name != root and not name.startswith(root + "/"):
            fail(f"archive member is outside the package root: {name}")
        if is_directory and mode != 0o755:
            fail(f"archive directory does not have mode 0755: {name}")
    for name in entries:
        is_directory, mode = modes.get(name, (False, -1))
        if is_directory:
            fail(f"archive entry unexpectedly resolves to a directory: {name}")
        expected = 0o755 if f"{root}/bin/" in name else 0o644
        if mode != expected:
            fail(f"archive member has mode {mode:04o}, expected {expected:04o}: {name}")
        if mode & 0o022:
            fail(f"archive member is group/world writable: {name}")
    if path.name.endswith(".tar.gz"):
        with tarfile.open(path, "r:gz") as archive:
            for member in archive.getmembers():
                if member.uid != 0 or member.gid != 0:
                    fail(f"archive member has non-canonical ownership: {member.name}")
                if member.mode & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX):
                    fail(f"archive member has special permission bits: {member.name}")
                if member.mtime != source_date_epoch:
                    fail(
                        f"archive member timestamp is not SOURCE_DATE_EPOCH: {member.name}"
                    )
    elif path.suffix == ".zip":
        timestamp = zip_timestamp_for_epoch(source_date_epoch)
        with zipfile.ZipFile(path) as archive:
            for info in archive.infolist():
                if info.date_time != timestamp:
                    fail(
                        f"ZIP member timestamp is not SOURCE_DATE_EPOCH: {info.filename}"
                    )


def validate_archive_member(name: str) -> None:
    validate_package_path(name.removesuffix("/"))


def validate_archive_layout(names: list[str], root: str, target: Target) -> None:
    """Reject payload paths outside the documented distribution layout."""

    allowed_exact = {
        "checksums.sha256",
        "manifest/compatibility.json",
        "manifest/compatibility.toml",
        "metadata/profile.json",
        "metadata/release.json",
        BUILD_IDENTITY.BUILD_METADATA_PATH,
    }
    binary_names = set(target.binary_names)
    for name in names:
        if name == root or not name.startswith(root + "/"):
            fail(f"archive member is outside the package root: {name}")
        relative = name.removeprefix(root + "/")
        if relative in allowed_exact:
            continue
        if target.source:
            if relative.startswith("source/"):
                continue
        elif relative.startswith(("ui/", "sdk/python/", "examples/", "licenses/")) or (
            relative.startswith("bin/")
            and relative.removeprefix("bin/") in binary_names
        ):
            continue
        fail(f"archive member is outside the {target.name} layout: {relative}")


def parse_checksums(content: bytes) -> dict[str, str]:
    try:
        text = content.decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"checksums.sha256 is not UTF-8: {error}")
    if not text.endswith("\n") or "\r" in text:
        fail("checksums.sha256 must use LF-terminated lines")
    actual: dict[str, str] = {}
    for line in text[:-1].split("\n"):
        digest, separator, relative = line.partition("  ")
        if (
            not separator
            or not SHA256_DIGEST.fullmatch(digest)
            or not relative
            or relative in actual
        ):
            fail("checksums.sha256 is malformed or contains duplicate paths")
        validate_package_path(relative)
        actual[relative] = digest
    if not actual:
        fail("checksums.sha256 is empty")
    return actual


def json_object(content: bytes, label: str) -> dict:
    try:
        return BUILD_IDENTITY.strict_json(content, label)
    except BUILD_IDENTITY.IdentityError as error:
        fail(str(error))


def bounded_regular_bytes(path: Path, label: str) -> bytes:
    try:
        return BUILD_IDENTITY.regular_bytes(
            path,
            label,
            maximum=MAX_RELEASE_JSON_BYTES,
        )
    except BUILD_IDENTITY.IdentityError as error:
        fail(str(error))


def safe_release_path(release_dir: Path, relative: str, label: str) -> Path:
    if not isinstance(relative, str) or not relative or "\\" in relative:
        fail(f"{label} has an unsafe path: {relative!r}")
    try:
        validate_package_path(relative)
    except PackageError:
        fail(f"{label} has an unsafe path: {relative!r}")
    candidate = release_dir.joinpath(*PurePosixPath(relative).parts)
    resolved_root = release_dir.resolve()
    resolved_candidate = candidate.resolve(strict=False)
    if not resolved_candidate.is_relative_to(resolved_root):
        fail(f"{label} escapes the release directory: {relative!r}")
    return candidate


def load_json_file(path: Path, label: str) -> dict:
    return json_object(bounded_regular_bytes(path, label), label)


def release_artifact_basename(version: str, artifact_id: str) -> str | None:
    if artifact_id == "source-archive":
        return f"worldstream-{version}-source.tar.gz"
    if artifact_id == "native-linux-x86_64-archive":
        return f"worldstream-{version}-linux-x86_64.tar.gz"
    if artifact_id == "native-windows-x64-archive":
        return f"worldstream-{version}-windows-x64.zip"
    portable_subject_names = {
        "worldstream-a202-adapter": f"worldstream-{version}-a202-adapter.tar.gz",
        "worldstream-deterministic-agents": f"worldstream-{version}-deterministic-agents.tar.gz",
        "worldstream-documentation": f"worldstream-{version}-documentation.tar.gz",
        "worldstream-examples": f"worldstream-{version}-examples.tar.gz",
        "worldstream-licenses": f"worldstream-{version}-licenses.tar.gz",
        "worldstream-negotiate-bundle": f"worldstream-{version}-negotiate.wspack",
        "worldstream-negotiate-evidence-verifier": f"worldstream-{version}-negotiate-evidence-verifier.tar.gz",
        "worldstream-pack-toolchain": f"worldstream-{version}-pack-toolchain.tar.gz",
        "worldstream-participant-console": f"worldstream-{version}-participant-console.tar.gz",
        "worldstream-release-metadata": f"worldstream-{version}-release-metadata.tar.gz",
        "worldstream-studio": f"worldstream-{version}-studio.tar.gz",
        "worldstream-typescript-pack-sdk": f"worldstream-{version}-typescript-pack-sdk.tar.gz",
    }
    if artifact_id in portable_subject_names:
        return portable_subject_names[artifact_id]
    if artifact_id == "checksums":
        return "SHA256SUMS"
    if artifact_id == "sigstore-bundle":
        return "sigstore.bundle.json"
    if artifact_id == "spdx-sbom":
        return "sbom.spdx.json"
    if artifact_id == "slsa-provenance":
        return "provenance.json"
    return None


def validate_release_artifact_path(
    relative: str, artifact_id: str, version: str
) -> None:
    validate_package_path(relative)
    lowered = relative.casefold()
    if any(marker in lowered for marker in UNSUPPORTED_ARTIFACT_MARKERS):
        fail(
            f"release artifact {artifact_id} names an explicitly unsupported platform or distribution: {relative}"
        )
    basename = PurePosixPath(relative).name
    expected = release_artifact_basename(version, artifact_id)
    if expected is not None and basename != expected:
        fail(
            f"release artifact {artifact_id} must use deterministic filename {expected}: {relative}"
        )
    if artifact_id == "oci-linux-amd64-image":
        pattern = re.compile(
            rf"worldstream-{re.escape(version)}-oci-linux-amd64(?:\.oci)?\.(?:tar|tar\.gz|digest|json)"
        )
        if pattern.fullmatch(basename) is None:
            fail(f"OCI artifact has an unsupported filename: {relative}")


def validate_packaged_artifact_paths(
    files: list[tuple[str, bytes]], target: Target
) -> None:
    """Reject unsupported and runtime-state artifacts in payload trees."""

    for relative, _ in files:
        package_relative = (
            relative.removeprefix("source/") if target.source else relative
        )
        components = PurePosixPath(package_relative).parts
        if any(
            component.casefold() in {"data", "backup", "backups", "export", "exports"}
            for component in components
        ):
            fail(
                "packaged payload contains runtime data/backup/export state: "
                f"{relative}"
            )
        if target.source:
            continue
        lowered = relative.casefold()
        if any(marker in lowered for marker in UNSUPPORTED_ARTIFACT_MARKERS):
            fail(
                "packaged payload names an explicitly unsupported platform or "
                f"distribution: {relative}"
            )


def validate_manifest_identity(metadata: dict, manifest: dict, manifest_json: bytes):
    try:
        profile = INVENTORY.identity_from_manifest(metadata)
    except ValueError as error:
        fail(str(error))
    fields = {
        "schema",
        "product",
        "source_version",
        "manifest",
        "artifacts",
        "artifact_digests",
        "evidence",
        "evidence_digests",
        "verification_material",
    }
    if profile.serialized_discriminator:
        fields.add("release_inventory")
    if set(metadata) != fields:
        fail("release manifest has unknown or missing fields")
    if metadata.get("product") != manifest["release_candidate"]:
        fail("release manifest product differs from compatibility manifest")
    if metadata.get("source_version") != manifest["release_candidate"]:
        fail("release manifest source_version differs from compatibility manifest")
    identity = metadata.get("manifest")
    if not isinstance(identity, dict):
        fail("release manifest has no manifest identity object")
    if identity.get("source") != MANIFEST_TOML.name:
        fail("release manifest source manifest identity is invalid")
    if identity.get("mirror") != MANIFEST_JSON.name:
        fail("release manifest mirror manifest identity is invalid")
    if identity.get("sha256") != sha256_bytes(manifest_json):
        fail("release manifest compatibility JSON digest does not match manifest bytes")
    return profile


def validate_sigstore_shape(value: dict) -> None:
    media_type = value.get("mediaType")
    if not isinstance(media_type, str) or not media_type.startswith(
        "application/vnd.dev.sigstore.bundle+json"
    ):
        fail("Sigstore bundle mediaType is missing or unsupported")
    verification_material = value.get("verificationMaterial")
    if not isinstance(verification_material, dict) or not verification_material:
        fail("Sigstore bundle verificationMaterial must be a non-empty object")
    dsse = value.get("dsseEnvelope")
    message_signature = value.get("messageSignature")
    if dsse is not None:
        if not isinstance(dsse, dict):
            fail("Sigstore dsseEnvelope must be an object")
        if not isinstance(dsse.get("payloadType"), str) or not isinstance(
            dsse.get("payload"), str
        ):
            fail("Sigstore DSSE envelope payload shape is invalid")
        signatures = dsse.get("signatures")
        if not isinstance(signatures, list) or not signatures:
            fail("Sigstore DSSE envelope has no signatures")
        if any(
            not isinstance(signature, dict)
            or not isinstance(signature.get("sig"), str)
            or not signature["sig"]
            for signature in signatures
        ):
            fail("Sigstore DSSE envelope signature shape is invalid")
    elif message_signature is not None:
        if not isinstance(message_signature, dict):
            fail("Sigstore messageSignature must be an object")
        digest = message_signature.get("messageDigest")
        if (
            not isinstance(digest, dict)
            or digest.get("algorithm")
            not in {
                "SHA2_256",
                "SHA256",
            }
            or not isinstance(digest.get("digest"), str)
            or not digest["digest"]
        ):
            fail("Sigstore messageSignature digest shape is invalid")
    else:
        fail("Sigstore bundle has neither a DSSE envelope nor message signature")


def validate_spdx_shape(value: dict) -> None:
    namespace = value.get("documentNamespace")
    parsed_namespace = (
        urllib.parse.urlparse(namespace) if isinstance(namespace, str) else None
    )
    if not (
        isinstance(value.get("spdxVersion"), str)
        and value["spdxVersion"] == "SPDX-2.3"
        and value.get("SPDXID") == "SPDXRef-DOCUMENT"
        and isinstance(value.get("name"), str)
        and value.get("dataLicense") == "CC0-1.0"
        and parsed_namespace is not None
        and bool(parsed_namespace.scheme)
        and not parsed_namespace.fragment
    ):
        fail("SBOM is not a valid SPDX JSON document identity")
    creation = value.get("creationInfo")
    if (
        not isinstance(creation, dict)
        or not isinstance(creation.get("created"), str)
        or SPDX_CREATED.fullmatch(creation["created"]) is None
    ):
        fail("SBOM creationInfo is incomplete")
    try:
        BUILD_IDENTITY.validate_spdx_created(creation["created"])
    except BUILD_IDENTITY.IdentityError as error:
        fail(f"SBOM creationInfo timestamp is invalid: {error}")
    creators = creation.get("creators")
    if (
        not isinstance(creators, list)
        or not creators
        or not any(
            isinstance(item, str) and SPDX_TOOL_CREATOR.fullmatch(item) is not None
            for item in creators
        )
    ):
        fail("SBOM creationInfo creators are incomplete")
    for field in ("packages", "files", "relationships", "documentDescribes"):
        if not isinstance(value.get(field), list):
            fail(f"SBOM {field} must be a list")
    if (
        not value["packages"]
        or not value["relationships"]
        or not value["documentDescribes"]
    ):
        fail("SBOM source/component relationship graph must be non-empty")
    for field in ("packages", "files"):
        if any(
            not isinstance(item, dict)
            or not isinstance(item.get("SPDXID"), str)
            or not item["SPDXID"]
            for item in value[field]
        ):
            fail(f"SBOM {field} contains an invalid SPDX item")
    for item in value["files"]:
        checksums = item.get("checksums")
        if not isinstance(item.get("fileName"), str) or not isinstance(checksums, list):
            fail("SBOM file entry has no filename or checksums")
        if any(
            not isinstance(checksum, dict)
            or set(checksum) != {"algorithm", "checksumValue"}
            for checksum in checksums
        ):
            fail("SBOM file checksum entry is malformed")
        by_algorithm = {
            str(checksum["algorithm"]).replace("-", "").upper(): checksum[
                "checksumValue"
            ]
            for checksum in checksums
        }
        if (
            len(checksums) != 2
            or set(by_algorithm) != {"SHA1", "SHA256"}
            or not isinstance(by_algorithm["SHA1"], str)
            or SHA1_DIGEST.fullmatch(by_algorithm["SHA1"]) is None
            or not isinstance(by_algorithm["SHA256"], str)
            or SHA256_DIGEST.fullmatch(by_algorithm["SHA256"]) is None
        ):
            fail("SBOM file must have exactly one valid SHA-1 and SHA-256 checksum")
    element_ids = {"SPDXRef-DOCUMENT"}
    for item in value["packages"]:
        identifier = item["SPDXID"]
        if identifier in element_ids:
            fail("SBOM element identifiers must be unique")
        element_ids.add(identifier)
    for item in value["files"]:
        identifier = item["SPDXID"]
        if (
            identifier != BUILD_IDENTITY._spdx_id("ReleaseSubject", item["fileName"])
            or identifier in element_ids
        ):
            fail("SBOM file identifier is not exact or unique")
        element_ids.add(identifier)
    for relationship in value["relationships"]:
        if not isinstance(relationship, dict) or set(relationship) != {
            "spdxElementId",
            "relationshipType",
            "relatedSpdxElement",
        }:
            fail("SBOM relationship is malformed")
        for endpoint in ("spdxElementId", "relatedSpdxElement"):
            identifier = relationship[endpoint]
            if (
                identifier not in {"NONE", "NOASSERTION"}
                and identifier not in element_ids
            ):
                fail("SBOM relationship contains a dangling element identifier")
    if any(identifier not in element_ids for identifier in value["documentDescribes"]):
        fail("SBOM documentDescribes contains a dangling element identifier")


def validate_slsa_shape(value: dict) -> None:
    if value.get("_type") != "https://in-toto.io/Statement/v1":
        fail("provenance is not an in-toto statement")
    if value.get("predicateType") != "https://slsa.dev/provenance/v1":
        fail("provenance is not a supported SLSA provenance document")
    subjects = value.get("subject")
    if not isinstance(subjects, list) or not subjects:
        fail("SLSA provenance subject must be a non-empty list")
    for subject in subjects:
        digest = subject.get("digest") if isinstance(subject, dict) else None
        if (
            not isinstance(subject, dict)
            or not isinstance(subject.get("name"), str)
            or not subject["name"]
            or not isinstance(digest, dict)
            or not isinstance(digest.get("sha256"), str)
            or SHA256_DIGEST.fullmatch(digest["sha256"]) is None
        ):
            fail("SLSA provenance subject shape is invalid")
    predicate = value.get("predicate")
    if not isinstance(predicate, dict):
        fail("SLSA provenance predicate must be an object")
    if not isinstance(predicate.get("buildDefinition"), dict) or not isinstance(
        predicate.get("runDetails"), dict
    ):
        fail("SLSA v1 provenance buildDefinition/runDetails are incomplete")
    definition = predicate["buildDefinition"]
    if (
        not isinstance(definition.get("externalParameters"), dict)
        or not definition["externalParameters"]
        or not isinstance(definition.get("internalParameters"), dict)
        or not isinstance(definition.get("resolvedDependencies"), list)
        or not definition["resolvedDependencies"]
    ):
        fail("SLSA source/material/toolchain/build graph must be non-empty")


def validate_spdx_subject_identity(
    value: dict, expected_subjects: dict[str, str]
) -> None:
    """Require SPDX file checksums to cover exactly release subjects."""

    observed: dict[str, str] = {}
    for item in value.get("files", []):
        if not isinstance(item, dict) or not isinstance(item.get("fileName"), str):
            fail("SPDX SBOM contains an invalid file entry")
        relative = item["fileName"]
        checksums = item.get("checksums")
        if not isinstance(checksums, list):
            fail(f"SPDX file has no checksums: {relative}")
        sha256 = [
            checksum.get("checksumValue")
            for checksum in checksums
            if isinstance(checksum, dict)
            and str(checksum.get("algorithm", "")).replace("-", "").upper()
            in {"SHA256", "SHA2_256"}
        ]
        if (
            len(sha256) != 1
            or not isinstance(sha256[0], str)
            or not SHA256_DIGEST.fullmatch(sha256[0].lower())
        ):
            fail(f"SPDX file has no single SHA-256 checksum: {relative}")
        if relative in observed:
            fail(f"SPDX contains duplicate file subject: {relative}")
        observed[relative] = sha256[0].lower()
    expected = {
        relative: digest.removeprefix("sha256:")
        for relative, digest in expected_subjects.items()
    }
    if set(observed) != set(expected):
        fail(
            "SPDX subject coverage does not match payload+evidence: "
            f"missing={sorted(set(expected) - set(observed))}; "
            f"extra={sorted(set(observed) - set(expected))}"
        )
    mismatches = sorted(
        relative for relative in expected if observed[relative] != expected[relative]
    )
    if mismatches:
        fail("SPDX subject SHA-256 mismatch: " + ", ".join(mismatches))


def validate_release_directory_inventory(
    release_dir: Path, expected_files: set[str]
) -> None:
    for candidate in release_dir.rglob("*"):
        relative = candidate.relative_to(release_dir).as_posix()
        if candidate.is_symlink():
            fail(f"release directory contains a symlink: {relative}")
        if candidate.is_dir():
            if relative not in {
                RELEASE_EVIDENCE_DIRECTORY,
                "supply-chain",
                PRE_SIGN_SUBJECT_DIRECTORY,
            }:
                fail(f"release directory contains an unexpected directory: {relative}")
            continue
        if not candidate.is_file():
            fail(f"release directory contains a non-regular entry: {relative}")
        if relative not in expected_files:
            fail(f"release directory contains an unlisted file: {relative}")


def directory_inventory(directory: Path) -> tuple[list[dict[str, object]], int, str]:
    """Return a canonical content inventory for a generated directory.

    The inventory intentionally covers bytes, paths, and sizes only.  It is a
    reproducibility fingerprint, not a platform/runtime or signing claim.
    Directory metadata is verified separately where the target contract
    requires it.
    """

    if not directory.is_dir() or directory.is_symlink():
        fail(f"artifact directory is missing or is a symlink: {directory}")
    files: list[tuple[str, bytes]] = []
    for candidate in sorted(
        directory.rglob("*"), key=lambda item: item.relative_to(directory).as_posix()
    ):
        relative = candidate.relative_to(directory).as_posix()
        if candidate.is_symlink():
            fail(f"artifact directory contains a symlink: {relative}")
        if candidate.is_dir():
            continue
        if not candidate.is_file():
            fail(f"artifact directory contains a non-regular file: {relative}")
        validate_package_path(relative)
        files.append((relative, candidate.read_bytes()))

    records = [
        {
            "path": relative,
            "sha256": sha256_bytes(content),
            "size_bytes": len(content),
        }
        for relative, content in files
    ]
    canonical = json.dumps(
        {"schema": "worldstream/artifact-inventory/v1", "files": records},
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    return (
        records,
        sum(record["size_bytes"] for record in records),
        sha256_bytes(canonical),
    )


def write_json_atomically(path: Path, value: dict) -> None:
    """Write a canonical JSON report without partially replacing a report."""

    if path.exists() and path.is_dir():
        fail(f"report path is a directory: {path}")
    if path.is_symlink():
        fail(f"report path must not be a symlink: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            output.write(
                json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True)
            )
            output.write("\n")
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def reject_report_overlap(report: Path, artifact: Path, label: str) -> None:
    """Prevent a report write from mutating the artifact it describes."""

    report_resolved = report.resolve(strict=False)
    artifact_resolved = artifact.resolve(strict=False)
    if (
        report_resolved == artifact_resolved
        or artifact_resolved in report_resolved.parents
    ):
        fail(f"{label} must not be inside the artifact: {report}")


def build_environment_argument(
    args: argparse.Namespace,
) -> tuple[dict | None, bool]:
    """Load a hosted build observation for CLI packaging; direct test APIs stay local."""

    if not hasattr(args, "build_environment"):
        return None, False
    raw = args.build_environment
    if getattr(args, "dry_run", False) and not raw:
        return None, False
    if not isinstance(raw, str) or not raw:
        fail("release packaging requires --build-environment from the capture step")
    path = Path(raw)
    try:
        content = BUILD_IDENTITY.regular_bytes(
            path, "observed payload build environment", maximum=64 * 1024
        )
        value = BUILD_IDENTITY.strict_json(
            content, "observed payload build environment"
        )
    except BUILD_IDENTITY.IdentityError as error:
        fail(f"observed payload build environment rejected: {error}")
    return value, True


def capture_build_environment(args: argparse.Namespace) -> int:
    """Capture one hosted payload runner/toolchain witness before packaging."""

    output = Path(args.output)
    if path_exists(output):
        fail(f"refusing to overwrite observed build environment: {output}")
    source_entries = BUILD_IDENTITY.source_entries_from_root(ROOT)
    rustc_version = BUILD_IDENTITY.toolchains_from_materials(source_entries)["rustc"]
    try:
        value = BUILD_IDENTITY.capture_observed_build_environment(
            args.target, rustc_version=rustc_version["version"]
        )
    except BUILD_IDENTITY.IdentityError as error:
        fail(f"cannot capture payload build environment: {error}")
    write_json_atomically(output, value)
    print(f"wrote observed build environment {output}")
    return 0


def oci_context_report(context: Path, *, verified: bool) -> dict:
    records, size_bytes, digest = directory_inventory(context)
    build_identity = load_json_file(
        context / BUILD_IDENTITY.BUILD_METADATA_PATH,
        "OCI context build identity",
    )
    return {
        "schema": "worldstream/oci-context-report/v1",
        "artifact": context.name,
        "kind": "oci-context",
        # Keep the artifact path portable. The nested build identity
        # intentionally records the selected hosted tool paths.
        "path": context.name,
        "sha256": "sha256:" + digest,
        "size_bytes": size_bytes,
        "file_count": len(records),
        "inventory": {
            "schema": "worldstream/artifact-inventory/v1",
            "files": records,
        },
        "release_evidence": False,
        "verified": verified,
        "identity": {
            "target": "oci-linux-amd64",
            "source_revision": build_identity["source"]["revision"],
            "build_identity_sha256": BUILD_IDENTITY.build_identity_digest(
                build_identity
            ),
            "observed_build_environment": build_identity["observed_build_environment"],
        },
    }


def validate_oci_context_permissions(context: Path, epoch: int) -> None:
    if context.stat().st_mtime_ns != epoch * 1_000_000_000:
        fail("OCI context root timestamp is not SOURCE_DATE_EPOCH")
    for candidate in sorted(context.rglob("*")):
        relative = candidate.relative_to(context).as_posix()
        mode = stat.S_IMODE(candidate.lstat().st_mode)
        if candidate.is_dir():
            expected = 0o755
        elif candidate.is_file():
            expected = (
                0o755
                if relative == "entrypoint.sh" or relative.startswith("bin/")
                else 0o644
            )
        else:
            fail(f"OCI context contains an unsafe member: {relative}")
        if mode != expected:
            fail(
                f"OCI context member has mode {mode:04o}, expected {expected:04o}: {relative}"
            )
        if mode & 0o022:
            fail(f"OCI context member is group/world writable: {relative}")
        if candidate.stat().st_mtime_ns != epoch * 1_000_000_000:
            fail(f"OCI context member timestamp is not SOURCE_DATE_EPOCH: {relative}")


def verify_oci_context(context: Path) -> dict:
    """Verify a generated OCI build context without claiming an image exists."""

    records, size_bytes, digest = directory_inventory(context)
    entries = {record["path"]: context / record["path"] for record in records}
    actual = set(entries)
    missing = sorted(OCI_CONTEXT_REQUIRED_FILES - actual)
    if missing:
        fail("OCI context is incomplete; missing: " + ", ".join(missing))
    if not any(path.startswith("licenses/") for path in actual):
        fail("OCI context is missing the required licenses tree")
    if not any(path.startswith("examples/heist/") for path in actual):
        fail("OCI context is missing the required examples/heist tree")

    for name in ("Dockerfile", "entrypoint.sh"):
        context_file = context / name
        validate_source_file(context_file, name)
        try:
            template = (OCI_FILES / name).read_bytes()
        except OSError as error:
            fail(f"cannot read OCI packaging template {name}: {error}")
        if context_file.read_bytes() != template:
            fail(f"OCI context {name} differs from the packaging template")

    manifest, manifest_toml, manifest_json = read_manifest()
    validate_target_profile(manifest, TARGETS["oci-linux-amd64"])
    validate_release_manifest(manifest, dry_run=False)
    if (
        bounded_regular_bytes(
            entries["manifest/compatibility.toml"], "OCI compatibility.toml"
        )
        != manifest_toml
    ):
        fail("OCI context compatibility.toml differs from the workspace manifest")
    if (
        bounded_regular_bytes(
            entries["manifest/compatibility.json"], "OCI compatibility.json"
        )
        != manifest_json
    ):
        fail("OCI context compatibility.json differs from the workspace manifest")

    metadata = load_json_file(entries["oci-metadata.json"], "generated OCI metadata")
    version = version_of(manifest)
    base_image = metadata.get("base_image")
    epoch = metadata.get("source_date_epoch")
    source_revision = metadata.get("source_revision")
    build_identity_sha256 = metadata.get("build_identity_sha256")
    if not isinstance(base_image, str):
        fail("OCI metadata has no pinned base image")
    if isinstance(epoch, bool) or not isinstance(epoch, int):
        fail("OCI metadata source_date_epoch is not an integer")
    source_date_epoch_of(epoch)
    build_value = load_json_file(
        entries[BUILD_IDENTITY.BUILD_METADATA_PATH],
        "OCI context build identity",
    )
    validate_generated_oci_metadata(
        bounded_regular_bytes(entries["oci-metadata.json"], "generated OCI metadata"),
        version=version,
        manifest_bytes=manifest_json,
        base_image=base_image,
        epoch=epoch,
        source_revision=source_revision,
        build_identity_sha256=build_identity_sha256,
        observed_build_environment=build_value.get("observed_build_environment"),
    )
    source_entries = BUILD_IDENTITY.source_entries_from_root(ROOT)
    source_entries["compatibility.toml"] = manifest_toml
    source_entries["compatibility.json"] = manifest_json
    try:
        BUILD_IDENTITY.validate_third_party_notices(source_entries)
        for relative in (
            "licenses/LICENSE-APACHE-2.0.txt",
            BUILD_IDENTITY.THIRD_PARTY_NOTICE_MANIFEST_PATH,
            BUILD_IDENTITY.THIRD_PARTY_NOTICE_TEXT_PATH,
        ):
            if bounded_regular_bytes(entries[relative], relative) != source_entries.get(
                relative
            ):
                fail(f"OCI legal notice differs from the pinned source: {relative}")
        BUILD_IDENTITY.validate_build_identity(
            build_value,
            target="oci-linux-amd64",
            source_entries=source_entries,
            revision=source_revision,
            source_date_epoch=epoch,
            manifest_sha256=sha256_bytes(manifest_json),
            base_image=base_image,
        )
    except BUILD_IDENTITY.IdentityError as error:
        fail(f"OCI context build identity rejected: {error}")
    if BUILD_IDENTITY.build_identity_digest(build_value) != build_identity_sha256:
        fail("OCI metadata build identity digest differs from metadata/build.json")
    validate_oci_context_permissions(context, epoch)

    package_paths = sorted(
        actual - {"Dockerfile", "entrypoint.sh", "oci-metadata.json"}
    )
    package_files = [(path, entries[path].read_bytes()) for path in package_paths]
    validate_packaged_version_identity(
        package_files, TARGETS["oci-linux-amd64"], version
    )
    validate_packaged_client_identities(
        package_files, manifest, TARGETS["oci-linux-amd64"]
    )
    validate_packaged_ui_consumed_assets(
        package_files, manifest, TARGETS["oci-linux-amd64"]
    )
    validate_packaged_ui_consumed_identity(
        package_files, manifest, TARGETS["oci-linux-amd64"]
    )
    profile = bounded_regular_bytes(
        entries["metadata/profile.json"], "OCI profile metadata"
    )
    validate_profile_metadata(
        profile,
        target=TARGETS["oci-linux-amd64"],
        version=version,
        manifest_bytes=manifest_json,
    )
    metadata_release = load_json_file(
        entries["metadata/release.json"], "OCI release metadata"
    )
    expected_release = json_object(
        canonical_metadata(
            target=TARGETS["oci-linux-amd64"],
            version=version,
            manifest_bytes=manifest_json,
            files=[
                (path, content)
                for path, content in package_files
                if path not in {"metadata/release.json", "checksums.sha256"}
            ],
            source_date_epoch=epoch,
        ),
        "expected OCI release metadata",
    )
    if metadata_release != expected_release:
        fail("OCI metadata/release.json is not canonical")

    checksums = parse_checksums(
        bounded_regular_bytes(entries["checksums.sha256"], "OCI checksums")
    )
    expected_checksum_paths = {
        path for path in package_paths if path != "checksums.sha256"
    }
    if set(checksums) != expected_checksum_paths:
        fail("OCI checksums.sha256 does not cover exactly the packaged payload")
    for relative, checksum in checksums.items():
        if sha256_file(entries[relative]) != checksum:
            fail(f"OCI checksum mismatch: {relative}")

    report = oci_context_report(context, verified=True)
    if (
        report["sha256"] != "sha256:" + digest
        or report["size_bytes"] != size_bytes
        or report["file_count"] != len(records)
        or report["inventory"]["files"] != records
    ):
        fail("OCI context changed while it was being verified")
    print(
        f"verified OCI context {context}: {len(records)} files, "
        f"inventory {report['sha256']}"
    )
    return report


def verify_release_directory(
    release_dir: Path,
    *,
    structural_only: bool,
    manifest_toml_path: Path = MANIFEST_TOML,
    manifest_json_path: Path = MANIFEST_JSON,
    release_inventory: str | None = None,
) -> int:
    """Verify the release bundle without ever treating shape as a signature."""

    if not release_dir.is_dir() or release_dir.is_symlink():
        fail(f"release directory is missing or is a symlink: {release_dir}")
    metadata_path = safe_release_path(
        release_dir, "release-manifest.json", "release manifest"
    )
    metadata = load_json_file(metadata_path, "release manifest")
    if manifest_toml_path == MANIFEST_TOML and manifest_json_path == MANIFEST_JSON:
        manifest, _, manifest_json = read_manifest()
    else:
        manifest, _, manifest_json = read_manifest(
            manifest_toml_path, manifest_json_path
        )
    profile = validate_manifest_identity(metadata, manifest, manifest_json)
    if (
        release_inventory is not None
        and inventory_contract(release_inventory) is not profile
    ):
        fail(
            "release manifest inventory differs from the explicitly selected "
            "release inventory"
        )
    validate_release_manifest(manifest, dry_run=False)
    artifacts = metadata.get("artifacts")
    if not isinstance(artifacts, dict) or not artifacts:
        fail("release manifest artifacts must be a non-empty id-to-path object")
    declared = set(profile.release_artifact_ids)
    if set(artifacts) != declared:
        fail("release manifest artifact IDs do not match compatibility manifest")
    artifact_digests = metadata.get("artifact_digests")
    if not isinstance(artifact_digests, dict) or set(artifact_digests) != set(
        profile.release_artifact_ids
    ) - {"sigstore-bundle"}:
        fail(
            "release manifest artifact_digests must cover every signed subject "
            "and must exclude the Sigstore verification bundle"
        )
    verification_material = metadata.get("verification_material")
    sigstore_material = (
        verification_material.get("sigstore-bundle")
        if isinstance(verification_material, dict)
        else None
    )
    if not isinstance(sigstore_material, dict) or "digest" in sigstore_material:
        fail(
            "release manifest must describe Sigstore by path only; its digest must "
            "never appear in the signed manifest"
        )
    if sigstore_material.get("path") != "sigstore.bundle.json":
        fail("release manifest Sigstore verification-material path is invalid")
    release_evidence_ids = tuple(
        row["id"]
        for row in manifest.get("evidence", [])
        if isinstance(row, dict) and row.get("release_gate") is True
    )
    if len(release_evidence_ids) != len(set(release_evidence_ids)):
        fail("compatibility release-gated evidence IDs are duplicated")
    evidence_paths_metadata = metadata.get("evidence")
    evidence_digests = metadata.get("evidence_digests")
    if (
        not isinstance(evidence_paths_metadata, dict)
        or set(evidence_paths_metadata) != set(release_evidence_ids)
        or not isinstance(evidence_digests, dict)
        or set(evidence_digests) != set(release_evidence_ids)
    ):
        fail(
            "release manifest evidence/evidence_digests must cover exactly all "
            "release-gated evidence IDs"
        )
    evidence_paths: dict[str, str] = {}
    for evidence_id in sorted(release_evidence_ids):
        relative = evidence_paths_metadata[evidence_id]
        expected_relative = f"{RELEASE_EVIDENCE_DIRECTORY}/{evidence_id}.json"
        if relative != expected_relative:
            fail(
                f"release evidence path is not deterministic for {evidence_id}: "
                f"{relative!r}"
            )
        path = safe_release_path(
            release_dir, relative, f"release evidence {evidence_id}"
        )
        if not path.is_file() or path.is_symlink():
            fail(f"release evidence is not a regular file: {relative}")
        digest = evidence_digests[evidence_id]
        if not isinstance(digest, str) or not SHA256_REFERENCE.fullmatch(digest):
            fail(f"release evidence digest is invalid: {evidence_id}")
        observed = "sha256:" + sha256_file(path)
        if digest != observed:
            fail(f"release evidence digest mismatch: {evidence_id}")
        evidence_paths[evidence_id] = relative
    artifact_paths: set[str] = set()
    artifact_path_by_id: dict[str, str] = {}
    for artifact_id, relative in artifacts.items():
        if not isinstance(relative, str) or relative in artifact_paths:
            fail(f"release artifact path is missing or duplicated: {artifact_id}")
        artifact_paths.add(relative)
        artifact_path_by_id[artifact_id] = relative
        validate_release_artifact_path(
            relative, artifact_id, manifest["release_candidate"]
        )
        path = safe_release_path(
            release_dir, relative, f"release artifact {artifact_id}"
        )
        if not path.is_file() or path.is_symlink():
            fail(f"release artifact is not a regular file: {relative}")
        manifest_row = next(
            row for row in manifest["release_artifacts"] if row["id"] == artifact_id
        )
        if artifact_id == "sigstore-bundle":
            if (
                manifest_row.get("status") != "verification_material"
                or manifest_row.get("digest") != ""
                or manifest_row.get("digest_algorithm") != ""
                or manifest_row.get("digest_location") is not None
                or manifest_row.get("verification_material_location")
                != "release-manifest.json#verification_material.sigstore-bundle.path"
            ):
                fail(
                    "Sigstore bundle must use the path-only verification-material declaration"
                )
            continue
        expected_digest = artifact_digests.get(artifact_id)
        if not isinstance(expected_digest, str) or not SHA256_REFERENCE.fullmatch(
            expected_digest
        ):
            fail(f"release artifact digest is invalid: {artifact_id}")
        if expected_digest != "sha256:" + sha256_file(path):
            fail(f"release artifact digest mismatch: {artifact_id}")
        embedded_digest = manifest_row["digest"]
        if embedded_digest:
            if embedded_digest != expected_digest:
                fail(
                    f"release artifact digest differs from compatibility manifest: {artifact_id}"
                )
        elif manifest_row["status"] != "detached":
            fail(
                f"release artifact has no detached inventory declaration: {artifact_id}"
            )

    verifier = release_evidence_verifier()
    normalized_evidence_paths = {
        evidence_id: release_dir / relative
        for evidence_id, relative in evidence_paths.items()
    }
    payload_subject_paths = {
        artifact_id: release_dir / artifact_path_by_id[artifact_id]
        for artifact_id in profile.payload_artifact_ids
    }
    try:
        verifier.validate_normalized_evidence_reports(
            normalized_evidence_paths,
            release_evidence_ids,
            manifest["release_candidate"],
            manifest["contracts"],
        )
        verifier.validate_pre_sign_material(
            release_dir,
            payload_subject_paths,
            normalized_evidence_paths,
            release_evidence_ids,
            manifest["release_candidate"],
            manifest["contracts"],
            sha256_bytes(manifest_json),
            profile.identity,
        )
    except verifier.AssemblyError as error:
        fail(f"release evidence verification failed: {error}")

    expected_files = (
        artifact_paths
        | set(evidence_paths.values())
        | {"release-manifest.json"}
        | PRE_SIGN_SUPPLY_CHAIN_FILES
        | {
            f"{PRE_SIGN_SUBJECT_DIRECTORY}/{evidence_id}.json"
            for evidence_id in release_evidence_ids
            if evidence_id != "checksums-signature-sbom-provenance"
        }
    )
    validate_release_directory_inventory(release_dir, expected_files)

    checksums_path = safe_release_path(release_dir, "SHA256SUMS", "checksums")
    if artifacts.get("checksums") != "SHA256SUMS":
        fail("checksums artifact must be named SHA256SUMS")
    checksums = parse_checksums(bounded_regular_bytes(checksums_path, "SHA256SUMS"))
    checksum_artifact_paths = {artifacts["checksums"]}
    # Checksums cover every payload in the selected release profile and every
    # signed non-supply-chain source report. The supply-chain report is emitted
    # after these sidecars exist and is deliberately excluded.
    pre_sign_subject_paths = {
        f"supply-chain/subjects/{evidence_id}.json"
        for evidence_id in release_evidence_ids
        if evidence_id != "checksums-signature-sbom-provenance"
    }
    required_checksum_paths = {
        artifact_path_by_id[artifact_id] for artifact_id in profile.payload_artifact_ids
    } | pre_sign_subject_paths
    if set(checksums) != required_checksum_paths:
        missing = sorted(required_checksum_paths - set(checksums))
        extra = sorted(set(checksums) - required_checksum_paths)
        fail(
            "SHA256SUMS does not cover exactly the release members"
            + ("; missing=" + ", ".join(missing) if missing else "")
            + ("; extra=" + ", ".join(extra) if extra else "")
        )
    if checksum_artifact_paths & set(checksums):
        fail("SHA256SUMS must not contain a self-referential checksum")
    for relative, digest in checksums.items():
        path = safe_release_path(release_dir, relative, "checksum member")
        if not path.is_file() or path.is_symlink():
            fail(f"checksum names a missing or symlinked file: {relative}")
        if sha256_file(path) != digest:
            fail(f"release checksum mismatch: {relative}")

    sigstore_path = safe_release_path(
        release_dir, artifacts["sigstore-bundle"], "Sigstore bundle"
    )
    sigstore = load_json_file(sigstore_path, "Sigstore bundle")
    validate_sigstore_shape(sigstore)
    sbom = load_json_file(
        safe_release_path(release_dir, artifacts["spdx-sbom"], "SPDX SBOM"), "SPDX SBOM"
    )
    validate_spdx_shape(sbom)
    provenance = load_json_file(
        safe_release_path(release_dir, artifacts["slsa-provenance"], "SLSA provenance"),
        "SLSA provenance",
    )
    validate_slsa_shape(provenance)
    subjects: dict[str, str] = {}
    for subject in provenance["subject"]:
        name = subject["name"]
        if name in subjects:
            fail(f"SLSA provenance contains duplicate subject: {name}")
        subjects[name] = "sha256:" + subject["digest"]["sha256"]
    expected_subjects = {
        artifact_path_by_id[artifact_id]: "sha256:"
        + sha256_file(
            safe_release_path(
                release_dir,
                artifact_path_by_id[artifact_id],
                "provenance payload subject",
            )
        )
        for artifact_id in CHECKSUM_PAYLOAD_ARTIFACT_IDS
        if artifact_id in profile.payload_artifact_ids
    }
    expected_subjects.update(
        {
            relative: "sha256:"
            + sha256_file(
                safe_release_path(release_dir, relative, "pre-sign source report")
            )
            for relative in pre_sign_subject_paths
        }
    )
    if set(subjects) != set(expected_subjects):
        fail(
            "SLSA provenance subjects do not cover exactly payload+evidence: "
            f"missing={sorted(set(expected_subjects) - set(subjects))}; "
            f"extra={sorted(set(subjects) - set(expected_subjects))}"
        )
    for relative, digest in subjects.items():
        if digest != expected_subjects[relative]:
            fail(f"SLSA provenance subject digest mismatch: {relative}")
    validate_spdx_subject_identity(sbom, expected_subjects)
    try:
        BUILD_IDENTITY.validate_identity_documents(
            spdx=sbom,
            provenance=provenance,
            version=manifest["release_candidate"],
            subjects_by_relative={
                relative: safe_release_path(
                    release_dir, relative, "supply-chain identity subject"
                )
                for relative in expected_subjects
            },
            payloads_by_id={
                artifact_id: safe_release_path(
                    release_dir,
                    artifact_path_by_id[artifact_id],
                    "supply-chain identity payload",
                )
                for artifact_id in BUILD_IDENTITY_PAYLOAD_ARTIFACT_IDS
            },
            require_github=True,
            release_inventory=profile.identity,
        )
    except BUILD_IDENTITY.IdentityError as error:
        fail(f"release source/component/build identity graph rejected: {error}")

    if structural_only:
        print(
            f"verified release structure {release_dir}; "
            "cryptographic verification of both Sigstore signatures was not attempted"
        )
        return 11
    try:
        verifier.verify_release_signatures(release_dir)
    except verifier.AssemblyError as error:
        fail(f"release signature verification failed: {error}")
    print(
        f"verified release bundle {release_dir}: both signatures, SBOM, provenance, checksums"
    )
    return 0


def http_json(base_url: str, path: str, timeout: float) -> tuple[int, dict]:
    url = urllib.parse.urljoin(base_url.rstrip("/") + "/", path.lstrip("/"))
    request = urllib.request.Request(url, headers={"Accept": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            status = response.status
            body = response.read()
    except urllib.error.HTTPError as error:
        status = error.code
        body = error.read()
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        fail(f"runtime probe could not reach {url}: {error}")
    value = json_object(body, f"runtime response {path}")
    return status, value


def canonical_runtime_manifest_summary(manifest: dict) -> dict:
    """Project the exact manifest identity exposed by ``/version``."""

    canonical_client_contract_identity(manifest)
    contracts = manifest.get("contracts")
    toolchains = manifest.get("toolchains")
    storage = manifest.get("storage")
    sqlite = storage.get("sqlite") if isinstance(storage, dict) else None
    postgresql = storage.get("postgresql") if isinstance(storage, dict) else None
    rows = manifest.get("pack_executors")
    if (
        not isinstance(contracts, dict)
        or not isinstance(toolchains, dict)
        or not isinstance(sqlite, dict)
        or not isinstance(postgresql, dict)
        or not isinstance(rows, list)
    ):
        fail("compatibility manifest cannot produce the runtime version summary")
    required_values = {
        "rust_toolchain": toolchains.get("rust"),
        "rust_edition": toolchains.get("rust_edition"),
        "sqlite_version": sqlite.get("version"),
        "postgresql_minimum_patch": postgresql.get("minimum_patch"),
    }
    if any(
        not isinstance(value, str) or not value for value in required_values.values()
    ):
        fail("compatibility manifest runtime summary has an invalid string identity")
    postgresql_major = postgresql.get("major")
    if isinstance(postgresql_major, bool) or not isinstance(postgresql_major, int):
        fail("compatibility manifest runtime summary has an invalid PostgreSQL major")
    summary_pack_fields = (
        "pack_id",
        "explanatory_version",
        "revision_digest",
        "selectable_for_new_rooms",
        "runnable_for_retained_rooms",
        "status",
        "required_for_release",
    )
    pack_executors = [
        {field: row[field] for field in summary_pack_fields} for row in rows
    ]
    return {
        "schema": manifest.get("schema"),
        "revision": manifest.get("manifest_revision"),
        "kind": manifest.get("manifest_kind"),
        "release_ready": manifest.get("release_ready"),
        "contracts": {
            field: contracts[field] for field in ("product", *CLIENT_CONTRACT_FIELDS)
        },
        **required_values,
        "postgresql_major": postgresql_major,
        "pack_executors": pack_executors,
    }


def validate_runtime_engine_identity(
    engine: dict, manifest: dict, storage_profile: str
) -> None:
    """Require the startup-selected engine's exact manifest-backed identity."""

    if engine.get("profile") != storage_profile:
        fail(
            f"/version storage profile mismatch: expected {storage_profile!r}, "
            f"got {engine.get('profile')!r}"
        )
    if engine.get("status") != "verified":
        fail("/version engine status is not verified")
    exact_identity = engine.get("exact_identity")
    if not isinstance(exact_identity, str) or not exact_identity:
        fail("/version engine exact_identity is missing")
    storage = manifest.get("storage")
    if not isinstance(storage, dict):
        fail("compatibility manifest has no storage identity")
    if storage_profile == "sqlite-bundled":
        sqlite = storage.get("sqlite")
        if not isinstance(sqlite, dict):
            fail("compatibility manifest has no SQLite identity")
        expected = (
            f"sqlite/{sqlite.get('version')}; source_id={sqlite.get('source_id')}"
        )
        if exact_identity != expected:
            fail(
                "/version SQLite exact identity differs from the compatibility manifest"
            )
        return
    postgresql = storage.get("postgresql")
    if not isinstance(postgresql, dict):
        fail("compatibility manifest has no PostgreSQL identity")
    verified_patches = postgresql.get("release_verified_patches")
    if not isinstance(verified_patches, list) or not verified_patches:
        fail("compatibility manifest has no release-verified PostgreSQL patch")
    expected_identities: set[str] = set()
    for patch in verified_patches:
        if not isinstance(patch, str) or re.fullmatch(r"[0-9]+\.[0-9]+", patch) is None:
            fail(
                "compatibility manifest has an invalid release-verified PostgreSQL patch"
            )
        major_text, patch_text = patch.split(".", 1)
        server_version_num = int(major_text) * 10_000 + int(patch_text)
        expected_identities.add(
            f"postgresql/{patch}; server_version_num={server_version_num}"
        )
    if exact_identity not in expected_identities:
        fail(
            "/version PostgreSQL exact identity is not a release-verified manifest patch"
        )


def runtime_probe(args: argparse.Namespace) -> int:
    version = args.version
    validate_artifact_component(version, "runtime probe version")
    storage_profile = getattr(args, "storage_profile", "sqlite-bundled")
    if storage_profile not in {"sqlite-bundled", "postgres-primary"}:
        fail(f"runtime probe storage profile is unsupported: {storage_profile}")
    manifest, _, _ = read_manifest()
    try:
        expected_source_revision = BUILD_IDENTITY.source_revision(
            ROOT, getattr(args, "source_revision", None)
        )
    except BUILD_IDENTITY.IdentityError as error:
        fail(f"runtime probe source revision is invalid: {error}")
    contracts = manifest["contracts"]
    health_status, health = http_json(args.base_url, "/healthz", args.timeout)
    if health_status != 200 or health.get("status") not in {"ok", "live", "healthy"}:
        fail("/healthz did not satisfy the 200 liveness contract")
    ready_status, ready = http_json(args.base_url, "/readyz", args.timeout)
    if ready_status not in {200, 503}:
        fail("/readyz returned an unsupported status")
    if ready_status == 200 and ready.get("status") != "ready":
        fail("/readyz returned 200 without status=ready")
    if ready_status == 503:
        code = (
            ready.get("error", {}).get("code")
            if isinstance(ready.get("error"), dict)
            else None
        )
        if not isinstance(code, str) or not code:
            fail("/readyz fail-closed response has no stable error code")
    version_status, version_body = http_json(args.base_url, "/version", args.timeout)
    if version_status != 200:
        fail("/version did not return HTTP 200")
    product_build = version_body.get("product_build")
    observed = product_build.get("product") if isinstance(product_build, dict) else None
    if observed != version:
        fail(f"/version product mismatch: expected {version}, got {observed!r}")
    if (
        product_build.get("binary") != "worldstreamd"
        or product_build.get("build_version") != version
        or product_build.get("source_revision") != expected_source_revision
    ):
        fail(
            "/version product build identity is incomplete or source revision differs "
            f"from exact release commit {expected_source_revision}"
        )
    required_fields = {
        "product_build",
        "wire",
        "config",
        "storage_schema",
        "core_schema_version",
        "hash_suite",
        "manifest",
        "engine",
    }
    missing = sorted(field for field in required_fields if field not in version_body)
    if missing:
        fail("/version omitted required identity fields: " + ", ".join(missing))
    expected_contracts = {
        "wire": contracts["wire"],
        "config": contracts["config"],
        "storage_schema": contracts["storage_schema"],
        "core_schema_version": contracts["core_schema_version"],
        "hash_suite": contracts["hash_suite"],
    }
    for field, expected in expected_contracts.items():
        if version_body.get(field) != expected:
            fail(f"/version {field} mismatch: expected {expected!r}")
    summary = version_body.get("manifest")
    expected_summary = canonical_runtime_manifest_summary(manifest)
    if summary != expected_summary:
        fail(
            "/version manifest summary differs from the embedded compatibility identity"
        )
    engine = version_body.get("engine")
    if not isinstance(engine, dict):
        fail("/version engine identity is not an object")
    validate_runtime_engine_identity(engine, manifest, storage_profile)
    print(f"runtime probes passed: health=200 ready={ready_status} version={version}")
    return 0


def _verify_archive(path: Path, release_inventory: str | None = None) -> None:
    if path.is_symlink() or not path.is_file():
        fail(f"archive is missing: {path}")
    entries = archive_entries(path)
    names = sorted(entries)
    if not names:
        fail("archive is empty")
    roots = {PurePosixPath(name).parts[0] for name in names}
    if len(roots) != 1:
        fail("archive must have exactly one top-level directory")
    root = next(iter(roots))
    metadata_path = f"{root}/metadata/release.json"
    checksums_path = f"{root}/checksums.sha256"
    manifest_path = f"{root}/manifest/compatibility.json"
    for required in (
        metadata_path,
        checksums_path,
        manifest_path,
        f"{root}/manifest/compatibility.toml",
        f"{root}/metadata/profile.json",
        f"{root}/{BUILD_IDENTITY.BUILD_METADATA_PATH}",
    ):
        if required not in entries:
            fail(f"archive is missing {required}")
    metadata = json_object(entries[metadata_path], "archive metadata")
    version = metadata.get("version")
    if not isinstance(version, str) or not version:
        fail("archive metadata has no version")
    validate_artifact_component(version, "archive metadata version")
    target = metadata.get("target")
    if not isinstance(target, str) or target not in {
        "linux-x86_64",
        "windows-x64",
        "source",
    }:
        fail("archive metadata has an unsupported target profile")
    profile = inventory_contract(release_inventory)
    target_profile = target_for_inventory(TARGETS[target], profile.identity)
    validate_archive_layout(names, root, target_profile)
    expected_root = f"worldstream-{version}-{target}"
    if root != expected_root:
        fail(f"archive root does not match metadata version/target: {root}")
    expected_name = expected_root + TARGETS[target].archive_suffix
    if path.name != expected_name:
        fail(f"archive filename does not match its embedded identity: {path.name}")
    if target == "source":
        required_prefixes = [
            f"{root}/source/Cargo.toml",
            f"{root}/source/Cargo.lock",
            f"{root}/source/compatibility.toml",
            f"{root}/source/compatibility.json",
        ]
    else:
        required_prefixes = [
            *(f"{root}/bin/{binary}" for binary in target_profile.binary_names),
            f"{root}/ui/index.html",
            f"{root}/sdk/python/pyproject.toml",
            f"{root}/sdk/python/uv.lock",
        ]
        if not any(name.startswith(f"{root}/examples/heist/") for name in entries):
            fail("archive is missing examples/heist")
        if not any(name.startswith(f"{root}/licenses/") for name in entries):
            fail("archive is missing licenses")
    for required in required_prefixes:
        if required not in entries:
            fail(f"archive is missing {required}")
    metadata_manifest = metadata.get("manifest")
    if (
        not isinstance(metadata_manifest, dict)
        or metadata_manifest.get("schema")
        != "worldstream/storage-compatibility-manifest/v1"
        or metadata_manifest.get("file") != "manifest/compatibility.json"
        or metadata_manifest.get("sha256") != sha256_bytes(entries[manifest_path])
    ):
        fail("archive metadata manifest identity does not match manifest bytes")
    try:
        authored = tomllib.loads(
            entries[f"{root}/manifest/compatibility.toml"].decode("utf-8")
        )
        mirror = json_object(entries[manifest_path], "archive compatibility.json")
    except (UnicodeDecodeError, ValueError, tomllib.TOMLDecodeError) as error:
        fail(f"archive manifest pair is not parseable: {error}")
    if authored != mirror:
        fail("archive compatibility.toml and compatibility.json differ semantically")
    canonical = (
        json.dumps(authored, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode()
    if entries[manifest_path] != canonical:
        fail("archive compatibility.json is not the canonical sorted mirror")
    validate_manifest_shape(authored)
    expected_files = [
        name.removeprefix(root + "/")
        for name in names
        if not name.endswith("/") and name != checksums_path
    ]
    actual_checksums = parse_checksums(entries[checksums_path])
    if sorted(actual_checksums) != sorted(expected_files):
        fail("checksums.sha256 does not cover exactly the packaged payload")
    for relative, digest in actual_checksums.items():
        full = f"{root}/{relative}"
        if full not in entries:
            fail(f"checksums.sha256 names a missing payload: {relative}")
        if sha256_bytes(entries[full]) != digest:
            fail(f"checksum mismatch: {relative}")
    manifest = mirror
    manifest_contracts = manifest.get("contracts")
    if (
        not isinstance(manifest_contracts, dict)
        or manifest_contracts.get("product") != version
    ):
        fail("archive version does not match manifest contracts.product")
    if (
        manifest.get("release_ready") is not True
        or manifest.get("manifest_kind") != "release"
    ):
        fail("archive contains a non-release manifest")
    if manifest.get("unresolved_required_fields"):
        fail("archive contains unresolved release fields")
    validate_target_profile(manifest, target_profile)
    validate_packaged_artifact_paths(
        [
            (relative, entries[f"{root}/{relative}"])
            for relative in expected_files
            if f"{root}/{relative}" in entries
        ],
        target_profile,
    )
    validate_packaged_version_identity(
        [
            (relative, entries[f"{root}/{relative}"])
            for relative in expected_files
            if f"{root}/{relative}" in entries
        ],
        target_profile,
        version,
    )
    validate_packaged_client_identities(
        [
            (relative, entries[f"{root}/{relative}"])
            for relative in expected_files
            if f"{root}/{relative}" in entries
        ],
        manifest,
        target_profile,
    )
    validate_packaged_ui_consumed_assets(
        [
            (relative, entries[f"{root}/{relative}"])
            for relative in expected_files
            if f"{root}/{relative}" in entries
        ],
        manifest,
        target_profile,
    )
    validate_packaged_ui_consumed_identity(
        [
            (relative, entries[f"{root}/{relative}"])
            for relative in expected_files
            if f"{root}/{relative}" in entries
        ],
        manifest,
        target_profile,
    )
    source_date_epoch = metadata.get("source_date_epoch")
    if (
        isinstance(source_date_epoch, bool)
        or not isinstance(source_date_epoch, int)
        or source_date_epoch < 0
    ):
        fail("archive metadata source_date_epoch is not a non-negative integer")
    source_date_epoch_of(source_date_epoch)
    build_value = json_object(
        entries[f"{root}/{BUILD_IDENTITY.BUILD_METADATA_PATH}"],
        "archive build identity",
    )
    try:
        BUILD_IDENTITY.validate_build_identity_shape(
            build_value,
            target=target,
            manifest_sha256=sha256_bytes(entries[manifest_path]),
            source_date_epoch=source_date_epoch,
            release_inventory=profile.identity,
        )
        if target == "source":
            source_entries = {
                relative.removeprefix("source/"): entries[f"{root}/{relative}"]
                for relative in expected_files
                if relative.startswith("source/")
            }
            revision_content = source_entries.get(BUILD_IDENTITY.SOURCE_REVISION_FILE)
            if not isinstance(revision_content, bytes):
                fail("source archive has no exact source revision file")
            BUILD_IDENTITY.validate_third_party_notices(source_entries)
            BUILD_IDENTITY.validate_build_identity(
                build_value,
                target=target,
                source_entries=source_entries,
                revision=revision_content.decode("ascii").strip(),
                source_date_epoch=source_date_epoch,
                manifest_sha256=sha256_bytes(entries[manifest_path]),
                release_inventory=profile.identity,
            )
        else:
            source_entries = BUILD_IDENTITY.source_entries_from_root(ROOT)
            BUILD_IDENTITY.validate_third_party_notices(source_entries)
            for relative in (
                "licenses/LICENSE-APACHE-2.0.txt",
                BUILD_IDENTITY.THIRD_PARTY_NOTICE_MANIFEST_PATH,
                BUILD_IDENTITY.THIRD_PARTY_NOTICE_TEXT_PATH,
            ):
                if entries.get(f"{root}/{relative}") != source_entries.get(relative):
                    fail(
                        "archive legal notice differs from the pinned source: "
                        f"{relative}"
                    )
    except (BUILD_IDENTITY.IdentityError, UnicodeError) as error:
        fail(f"archive build identity rejected: {error}")
    payload_files = [
        (relative, entries[f"{root}/{relative}"])
        for relative in sorted(expected_files)
        if relative != "metadata/release.json"
    ]
    expected_metadata = json_object(
        canonical_metadata(
            target=target_profile,
            version=version,
            manifest_bytes=entries[manifest_path],
            files=payload_files,
            source_date_epoch=source_date_epoch,
        ),
        "expected archive metadata",
    )
    if metadata != expected_metadata:
        fail("archive metadata is not the canonical release metadata")
    validate_profile_metadata(
        entries[f"{root}/metadata/profile.json"],
        target=target_profile,
        version=version,
        manifest_bytes=entries[manifest_path],
    )
    verify_archive_permissions(path, root, entries, source_date_epoch)
    print(
        f"verified {path}: {len(entries)} files, version {version}, manifest {metadata['manifest']['sha256']}"
    )


def verify_archive(path: Path, release_inventory: str | None = None) -> None:
    """Fail closed with one stable package error for malformed container bytes."""

    try:
        _verify_archive(path, release_inventory)
    except PackageError:
        raise
    except (EOFError, OSError, tarfile.TarError, zipfile.BadZipFile) as error:
        fail(f"archive container is malformed: {error}")


def archive_report(path: Path) -> dict:
    entries = archive_entries(path)
    roots = {PurePosixPath(name).parts[0] for name in entries}
    root = next(iter(roots))
    metadata = json_object(entries[f"{root}/metadata/release.json"], "archive metadata")
    manifest_json_sha256 = sha256_bytes(entries[f"{root}/manifest/compatibility.json"])
    manifest_toml_sha256 = sha256_bytes(entries[f"{root}/manifest/compatibility.toml"])
    build_identity_bytes = entries[f"{root}/{BUILD_IDENTITY.BUILD_METADATA_PATH}"]
    build_identity = json_object(build_identity_bytes, "archive build identity")
    if metadata["manifest"]["sha256"] != manifest_json_sha256:
        fail("package report manifest identity differs from archived JSON bytes")
    return {
        "schema": "worldstream/package-report/v1",
        "artifact": path.name,
        "kind": "archive",
        # Keep the artifact path portable. The nested build identity
        # intentionally records the selected hosted tool paths.
        "path": path.name,
        "sha256": "sha256:" + sha256_file(path),
        "size_bytes": path.stat().st_size,
        "inventory": {
            "archive_verified": True,
            "manifest_source": "compatibility.toml",
            "manifest_mirror": "compatibility.json",
            "release_evidence": False,
        },
        "identity": {
            "target": metadata["target"],
            "version": metadata["version"],
            # Retained for v1 report readers: this field has always named the
            # canonical JSON mirror selected by metadata/release.json.
            "manifest_sha256": manifest_json_sha256,
            "manifest_json_sha256": manifest_json_sha256,
            "manifest_toml_sha256": manifest_toml_sha256,
            "source_revision": build_identity["source"]["revision"],
            "build_identity_sha256": "sha256:" + sha256_bytes(build_identity_bytes),
            "observed_build_environment": build_identity["observed_build_environment"],
        },
    }


def write_archive_report(args: argparse.Namespace) -> int:
    path = Path(args.artifact)
    verify_archive(path, getattr(args, "release_inventory", None))
    report = archive_report(path)
    if args.check:
        if Path(args.check).is_symlink() or not Path(args.check).is_file():
            fail(f"package report is missing or is a symlink: {args.check}")
        content = bounded_regular_bytes(Path(args.check), "package report")
        actual = json_object(content, "package report")
        if actual != report:
            fail("package report does not exactly identify the verified archive")
    elif args.report:
        reject_report_overlap(Path(args.report), path, "archive report")
        write_json_atomically(Path(args.report), report)
    print(json.dumps(report, sort_keys=True, separators=(",", ":")))
    return 0


def oci_context(args: argparse.Namespace) -> int:
    target = TARGETS["oci-linux-amd64"]
    base_image = getattr(args, "base_image", "")
    validate_oci_inputs(OCI_FILES)
    manifest, manifest_toml, manifest_json = read_manifest()
    validate_target_profile(manifest, target)
    validate_release_manifest(manifest, dry_run=args.dry_run)
    version = version_of(manifest)
    if not args.dry_run:
        validate_component_version_sources(
            version=version,
            daemon_version_file=Path(
                getattr(args, "daemon_version_file", ROOT / "Cargo.toml")
            ),
            sdk_version_file=Path(args.sdk_dir) / "pyproject.toml",
            ui_version_file=Path(getattr(args, "ui_version_file", DEFAULT_UI_VERSION)),
        )
    validate_source_filesystem(
        target,
        [
            Path(args.binary_dir),
            Path(args.ui_dir),
            Path(args.sdk_dir),
            Path(args.examples_dir),
            Path(args.licenses_dir),
            Path(args.output),
        ],
        dry_run=args.dry_run,
    )
    inputs = required_inputs(
        target,
        Path(args.binary_dir),
        Path(args.ui_dir),
        Path(args.sdk_dir),
        Path(args.examples_dir),
        Path(args.licenses_dir),
        allow_missing=args.dry_run,
    )
    if args.dry_run:
        if not base_image:
            print("dry-run: blocked input: --base-image IMAGE@sha256:DIGEST")
        print(f"dry-run: OCI linux/amd64 context for {version}")
        for path in expected_input_paths(
            target,
            Path(args.binary_dir),
            Path(args.ui_dir),
            Path(args.sdk_dir),
            Path(args.examples_dir),
            Path(args.licenses_dir),
        ):
            if not path.exists():
                print(f"dry-run: blocked input: {path}")
        print(
            "dry-run: USER 65532:65532; read-only-root compatible; VOLUME /var/lib/worldstream"
        )
        print(
            "dry-run: SQLite data directory must be /var/lib/worldstream on ext4 or xfs"
        )
        return 0
    if not isinstance(base_image, str) or not re.fullmatch(
        r"[^\s@]+@sha256:[0-9a-f]{64}", base_image
    ):
        fail(
            "OCI packaging requires a pinned base image IMAGE@sha256:<64 lowercase hex>"
        )
    context = Path(args.output)
    validate_output_directory(context, "OCI context output")
    report_path = getattr(args, "report", None)
    if report_path:
        reject_report_overlap(Path(report_path), context, "OCI context report")
    if path_exists(context):
        fail(f"OCI context output already exists; refusing to overwrite: {context}")
    epoch = source_date_epoch_of(args.source_date_epoch)
    observed_build_environment, require_hosted_environment = build_environment_argument(
        args
    )
    files = collect_package_files(
        target,
        version,
        manifest_toml,
        manifest_json,
        inputs,
        epoch,
        source_root=ROOT,
        source_revision=getattr(args, "source_revision", None),
        base_image=base_image,
        observed_build_environment=observed_build_environment,
        require_hosted_environment=require_hosted_environment,
    )
    build_identity = json_object(
        dict(files)[BUILD_IDENTITY.BUILD_METADATA_PATH], "OCI build identity"
    )
    source_revision = build_identity["source"]["revision"]
    build_identity_sha256 = BUILD_IDENTITY.build_identity_digest(build_identity)
    parent = context.parent
    parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=f".{context.name}.", dir=parent))
    try:
        for relative, content in files:
            destination = staging / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(content)
        for source_name in ("Dockerfile", "entrypoint.sh", "oci-metadata.json"):
            source = OCI_FILES / source_name
            validate_source_file(source, source_name)
            shutil.copyfile(source, staging / source_name)
        metadata = canonical_oci_metadata(
            version=version,
            manifest_bytes=manifest_json,
            base_image=base_image,
            epoch=epoch,
            source_revision=source_revision,
            build_identity_sha256=build_identity_sha256,
            observed_build_environment=build_identity["observed_build_environment"],
        )
        (staging / "oci-metadata.json").write_text(
            json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        validate_generated_oci_metadata(
            bounded_regular_bytes(
                staging / "oci-metadata.json", "generated OCI metadata"
            ),
            version=version,
            manifest_bytes=manifest_json,
            base_image=base_image,
            epoch=epoch,
            source_revision=source_revision,
            build_identity_sha256=build_identity_sha256,
            observed_build_environment=build_identity["observed_build_environment"],
        )
        for file in staging.rglob("*"):
            if file.is_file():
                executable = file.name == "entrypoint.sh" or file.parent.name == "bin"
                file.chmod(0o755 if executable else 0o644)
                os.utime(file, (epoch, epoch))
            elif file.is_dir():
                file.chmod(0o755)
                os.utime(file, (epoch, epoch))
        os.utime(staging, (epoch, epoch))
        staging.rename(context)
    except BaseException:
        shutil.rmtree(staging, ignore_errors=True)
        raise
    report = verify_oci_context(context)
    if report_path:
        write_json_atomically(Path(report_path), report)
        print(f"wrote OCI context report {report_path}")
    print(f"wrote OCI context {context}")
    print(json.dumps(report, sort_keys=True, separators=(",", ":")))
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    package_parser = subparsers.add_parser("package")
    package_parser.add_argument(
        "--target", choices=["linux-x86_64", "windows-x64", "source"], required=True
    )
    package_parser.add_argument("--binary-dir", default=str(ROOT / "target/release"))
    package_parser.add_argument("--ui-dir", default=str(DEFAULT_UI))
    package_parser.add_argument("--ui-version-file", default=str(DEFAULT_UI_VERSION))
    package_parser.add_argument(
        "--daemon-version-file", default=str(ROOT / "Cargo.toml")
    )
    package_parser.add_argument("--sdk-dir", default=str(DEFAULT_SDK))
    package_parser.add_argument("--examples-dir", default=str(DEFAULT_EXAMPLES))
    package_parser.add_argument("--licenses-dir", default=str(DEFAULT_LICENSES))
    package_parser.add_argument("--source-dir", default=str(ROOT))
    package_parser.add_argument("--output", default=str(ROOT / "dist"))
    package_parser.add_argument(
        "--source-date-epoch", default=os.environ.get("SOURCE_DATE_EPOCH", "0")
    )
    package_parser.add_argument(
        "--source-revision", default=os.environ.get("WORLDSTREAM_BUILD_REVISION")
    )
    package_parser.add_argument(
        "--build-environment",
        help="canonical observation emitted by capture-build-environment",
    )
    package_parser.add_argument(
        "--release-inventory",
        choices=tuple(INVENTORY.BY_ID),
        help="closed release inventory; omission preserves the historical profile",
    )
    package_parser.add_argument("--dry-run", action="store_true")
    package_parser.set_defaults(function=package)

    verify_parser = subparsers.add_parser("verify")
    verify_parser.add_argument("artifact")
    verify_parser.add_argument(
        "--structural-only",
        action="store_true",
        help="verify release documents without claiming cryptographic signature verification",
    )
    verify_parser.add_argument(
        "--release-inventory",
        choices=tuple(INVENTORY.BY_ID),
        help="required when verifying a successor native archive directly",
    )
    verify_parser.add_argument(
        "--report",
        help="write a deterministic inventory report when verifying an OCI context",
    )
    verify_parser.add_argument(
        "--manifest-toml",
        type=Path,
        default=MANIFEST_TOML,
        help="trusted compatibility TOML used to verify a release evidence directory",
    )
    verify_parser.add_argument(
        "--manifest-json",
        type=Path,
        default=MANIFEST_JSON,
        help="canonical compatibility JSON used to verify a release evidence directory",
    )

    report_parser = subparsers.add_parser("report")
    report_parser.add_argument("artifact")
    report_parser.add_argument("--report")
    report_parser.add_argument("--check")
    report_parser.add_argument("--release-inventory", choices=tuple(INVENTORY.BY_ID))
    report_parser.set_defaults(function=write_archive_report)

    def verify_path(args: argparse.Namespace) -> int:
        path = Path(args.artifact)
        if path.is_dir():
            if (path / "Dockerfile").is_file() and (
                path / "oci-metadata.json"
            ).is_file():
                if args.structural_only:
                    fail(
                        "--structural-only is only valid for a release evidence directory"
                    )
                report = verify_oci_context(path)
                if args.report:
                    reject_report_overlap(Path(args.report), path, "OCI context report")
                    write_json_atomically(Path(args.report), report)
                return 0
            if args.report:
                fail("--report is only supported when verifying an OCI context")
            return verify_release_directory(
                path,
                structural_only=args.structural_only,
                manifest_toml_path=args.manifest_toml,
                manifest_json_path=args.manifest_json,
                release_inventory=args.release_inventory,
            )
        if args.structural_only:
            fail("--structural-only is only valid for a release evidence directory")
        if args.report:
            fail("--report is only supported when verifying an OCI context")
        verify_archive(path, args.release_inventory)
        return 0

    verify_parser.set_defaults(function=verify_path)

    probe_parser = subparsers.add_parser(
        "probe", help="verify packaged runtime health/readiness/version endpoints"
    )
    probe_parser.add_argument("--base-url", required=True)
    probe_parser.add_argument("--version", required=True)
    probe_parser.add_argument(
        "--source-revision", default=os.environ.get("WORLDSTREAM_BUILD_REVISION")
    )
    probe_parser.add_argument(
        "--storage-profile",
        choices=["sqlite-bundled", "postgres-primary"],
        default="sqlite-bundled",
        help="expected startup-fixed backend profile",
    )
    probe_parser.add_argument("--timeout", type=float, default=5.0)
    probe_parser.set_defaults(function=runtime_probe)

    oci_parser = subparsers.add_parser("oci-context")
    oci_parser.add_argument("--binary-dir", default=str(ROOT / "target/release"))
    oci_parser.add_argument("--ui-dir", default=str(DEFAULT_UI))
    oci_parser.add_argument("--ui-version-file", default=str(DEFAULT_UI_VERSION))
    oci_parser.add_argument("--daemon-version-file", default=str(ROOT / "Cargo.toml"))
    oci_parser.add_argument("--sdk-dir", default=str(DEFAULT_SDK))
    oci_parser.add_argument("--examples-dir", default=str(DEFAULT_EXAMPLES))
    oci_parser.add_argument("--licenses-dir", default=str(DEFAULT_LICENSES))
    oci_parser.add_argument(
        "--base-image", default=os.environ.get("WORLDSTREAM_BASE_IMAGE", "")
    )
    oci_parser.add_argument("--output", default=str(ROOT / "dist/oci"))
    oci_parser.add_argument(
        "--source-date-epoch", default=os.environ.get("SOURCE_DATE_EPOCH", "0")
    )
    oci_parser.add_argument(
        "--source-revision", default=os.environ.get("WORLDSTREAM_BUILD_REVISION")
    )
    oci_parser.add_argument(
        "--build-environment",
        help="canonical observation emitted by capture-build-environment",
    )
    oci_parser.add_argument(
        "--report",
        help="write a deterministic content inventory report for the generated context",
    )
    oci_parser.add_argument("--dry-run", action="store_true")
    oci_parser.set_defaults(function=oci_context)

    capture_parser = subparsers.add_parser(
        "capture-build-environment",
        help="capture the exact hosted runner and selected native build tools",
    )
    capture_parser.add_argument(
        "--target",
        choices=["source", "linux-x86_64", "windows-x64", "oci-linux-amd64"],
        required=True,
    )
    capture_parser.add_argument("--output", required=True)
    capture_parser.set_defaults(function=capture_build_environment)
    return parser


def main(argv: list[str] | None = None) -> int:
    try:
        args = build_parser().parse_args(argv)
        return int(args.function(args))
    except PackageError as error:
        print(f"package verification failed: {error}", file=sys.stderr)
        return 1
    except (
        BUILD_IDENTITY.IdentityError,
        OSError,
        ValueError,
        json.JSONDecodeError,
        tarfile.TarError,
        zipfile.BadZipFile,
    ) as error:
        print(f"package verification failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

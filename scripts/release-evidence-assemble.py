#!/usr/bin/env python3
"""Assemble the detached WorldStream release inventory.

The compatibility pair describes the release contract.  This command binds
that contract to the exact bytes produced by release jobs.  It deliberately
does not manufacture evidence: every release-gated evidence id must have a
separate, passed report with the same embedded id.

The command is also importable by the reviewed supply-chain generator.  Keep
the input validation and path mapping here so the two release steps cannot
silently disagree about which bytes are subjects.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import urllib.parse
from datetime import datetime, timezone
from functools import cache
from pathlib import Path, PurePosixPath
from typing import Any

import tomllib

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST_TOML = ROOT / "compatibility.toml"
DEFAULT_MANIFEST_JSON = ROOT / "compatibility.json"
SCHEMA = "worldstream/release-artifact-manifest/v2"
NORMALIZED_EVIDENCE_SCHEMA = "worldstream/release-evidence-report/v1"
NORMALIZED_EVIDENCE_FIELDS = {
    "schema",
    "evidence_id",
    "release_gate",
    "release_evidence",
    "fail_closed",
    "status",
    "summary",
    "version",
    "platform",
    "contract",
    "checks",
    "producer_details",
    "source_report",
}
PRODUCER_DETAIL_FIELDS = {"producer_id", "phase", "outcomes", "artifacts"}
OUTCOME_FIELDS = {"status", "observations"}
ARTIFACT_BINDING_FIELDS = {"sha256", "size_bytes"}
SOURCE_REPORT_FIELDS = {"source_id", "evidence_id", "schema", "sha256"}
BASE_RELEASE_EVIDENCE_COUNT = 14
REQUIRED_RELEASE_EVIDENCE_COUNT = 18
RUNTIME_PAYLOAD_ARTIFACT_IDS = (
    "source-archive",
    "native-linux-x86_64-archive",
    "native-windows-x64-archive",
    "oci-linux-amd64-image",
)
STARTER_SUBJECT_ARTIFACT_IDS = (
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
)
PAYLOAD_ARTIFACT_IDS = RUNTIME_PAYLOAD_ARTIFACT_IDS + STARTER_SUBJECT_ARTIFACT_IDS
RELEASE_ARTIFACT_IDS = PAYLOAD_ARTIFACT_IDS + (
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
    **{artifact_id: "all" for artifact_id in STARTER_SUBJECT_ARTIFACT_IDS},
    "checksums": "all",
    "sigstore-bundle": "all",
    "spdx-sbom": "all",
    "slsa-provenance": "all",
}
SIGNED_ARTIFACT_IDS = frozenset(RELEASE_ARTIFACT_IDS) - {"sigstore-bundle"}
SIDECAR_PATHS = {
    "checksums": "SHA256SUMS",
    "sigstore-bundle": "sigstore.bundle.json",
    "spdx-sbom": "sbom.spdx.json",
    "slsa-provenance": "provenance.json",
}
PRE_SIGN_SUBJECT_DIRECTORY = "supply-chain/subjects"
PRE_SIGN_INVENTORY_PATH = "supply-chain/subject-inventory.json"
PRE_SIGN_SIGNATURE_PATH = "supply-chain/subject-inventory.bundle.json"
MAX_RELEASE_PAYLOAD_BYTES = 8 * 1024 * 1024 * 1024
COPY_CHUNK_BYTES = 1024 * 1024
HEX_DIGEST = set("0123456789abcdef")
SPDX_CREATED = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z\Z")
SPDX_TOOL_CREATOR = re.compile(
    r"Tool: [A-Za-z0-9][A-Za-z0-9._-]*-[0-9]+(?:\.[0-9]+)+(?:[-+][0-9A-Za-z.-]+)?\Z"
)
PAYLOAD_EVIDENCE_BINDINGS = (
    (
        "native-linux-release-profile",
        "native-linux-x86_64-archive",
        "linux-release-profile",
    ),
    (
        "native-windows-release-profile",
        "native-windows-x64-archive",
        "windows-release-profile",
    ),
    (
        "oci-linux-amd64-release-profile",
        "oci-linux-amd64-image",
        "oci-release-profile",
    ),
    (
        "failure-fuzz-resource-and-one-hour-sqlite-soak",
        "native-linux-x86_64-archive",
        "linux-release-profile",
    ),
    (
        "reference-performance-per-backend",
        "native-linux-x86_64-archive",
        "linux-release-profile",
    ),
    (
        "worldstream-negotiate-evidence",
        "worldstream-negotiate-bundle",
        "negotiate-bundle",
    ),
    (
        "worldstream-negotiate-evidence",
        "native-linux-x86_64-archive",
        "linux-release-profile",
    ),
    (
        "negotiate-oracle-a202-and-privacy",
        "worldstream-a202-adapter",
        "a202-adapter",
    ),
    (
        "negotiate-released-artifact-sqlite-restart-replay",
        "native-linux-x86_64-archive",
        "linux-release-profile",
    ),
    (
        "negotiate-released-artifact-sqlite-restart-replay",
        "worldstream-negotiate-bundle",
        "negotiate-bundle",
    ),
    (
        "negotiate-released-artifact-postgresql-restart-replay",
        "native-linux-x86_64-archive",
        "linux-release-profile",
    ),
    (
        "negotiate-released-artifact-postgresql-restart-replay",
        "worldstream-negotiate-bundle",
        "negotiate-bundle",
    ),
)


@cache
def evidence_collector():
    """Load the code-owned evidence/source/producer identity mapping."""

    path = ROOT / "scripts/release-evidence-collect.py"
    spec = importlib.util.spec_from_file_location(
        "worldstream_release_evidence_assembly_collector", path
    )
    if spec is None or spec.loader is None:
        fail(f"cannot load release evidence collector: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def package_verifier():
    """Load the canonical native/source archive verifier without duplicating it."""

    path = ROOT / "scripts/package.py"
    name = "worldstream_release_payload_package_verifier"
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        fail(f"cannot load package verifier: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def release_build_identity_verifier():
    """Load the shared SPDX/SLSA identity verifier."""

    path = ROOT / "scripts/release_build_identity.py"
    name = "worldstream_release_assembly_build_identity"
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        fail(f"cannot load release build identity verifier: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def oci_layout_verifier():
    """Load the canonical closed OCI-layout verifier."""

    path = ROOT / "scripts/verify-oci-layout.py"
    name = "worldstream_release_payload_oci_verifier"
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        fail(f"cannot load OCI layout verifier: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


@cache
def starter_subject_verifier():
    """Load the canonical portable Starter subject verifier."""

    path = ROOT / "scripts/starter-release-subjects.py"
    name = "worldstream_release_assembly_starter_subjects"
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        fail(f"cannot load Starter subject verifier: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


class AssemblyError(RuntimeError):
    """A fail-closed release input or output error."""


def fail(message: str) -> None:
    raise AssemblyError(message)


def strict_json_bytes(content: bytes, label: str) -> dict[str, Any]:
    """Use the shared release parser so signed bytes have one interpretation."""

    collector = evidence_collector()
    try:
        return collector.strict_json_object(content, label)
    except collector.CollectionError as error:
        fail(str(error))


def bounded_regular_bytes(path: Path, label: str) -> bytes:
    """Read a release control document through the shared allocation bound."""

    collector = evidence_collector()
    try:
        return collector.bounded_regular_bytes(path, label)
    except collector.CollectionError as error:
        fail(str(error))


def validate_spdx_created(value: object) -> None:
    if not isinstance(value, str):
        fail("SPDX creation time is not a string")
    try:
        parsed = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(
            tzinfo=timezone.utc
        )
    except ValueError as error:
        raise AssemblyError("SPDX creation time is not a real UTC second") from error
    if parsed.strftime("%Y-%m-%dT%H:%M:%SZ") != value:
        fail("SPDX creation time is not canonical UTC-second text")


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    try:
        digest = hashlib.sha256()
        with path.open("rb") as stream:
            while chunk := stream.read(COPY_CHUNK_BYTES):
                digest.update(chunk)
        return digest.hexdigest()
    except OSError as error:
        fail(f"cannot read {path}: {error}")


def sha1_file(path: Path) -> str:
    """Return SPDX's mandatory SHA-1 integrity checksum."""

    try:
        digest = hashlib.sha1(usedforsecurity=False)
        with path.open("rb") as stream:
            while chunk := stream.read(1024 * 1024):
                digest.update(chunk)
        return digest.hexdigest()
    except OSError as error:
        fail(f"cannot read {path}: {error}")


def safe_relative(value: str, label: str) -> str:
    if (
        not isinstance(value, str)
        or not value
        or "\\" in value
        or value != value.strip()
        or PurePosixPath(value).is_absolute()
        or any(part in {"", ".", ".."} for part in PurePosixPath(value).parts)
    ):
        fail(f"{label} is not a safe relative path: {value!r}")
    return value


def regular_file(path: Path, label: str) -> Path:
    try:
        mode = path.lstat().st_mode
    except FileNotFoundError:
        fail(f"missing {label}: {path}")
    except OSError as error:
        fail(f"cannot inspect {label} {path}: {error}")
    if stat.S_ISLNK(mode):
        fail(f"{label} must not be a symlink: {path}")
    if not stat.S_ISREG(mode):
        fail(f"{label} must be a regular file: {path}")
    return path


def verify_sigstore_signature(path: Path, bundle: Path, label: str) -> None:
    """Verify one blob with the single configured release identity policy."""

    regular_file(path, f"{label} subject")
    regular_file(bundle, f"{label} Sigstore bundle")
    cosign = shutil.which("cosign")
    identity = os.environ.get("COSIGN_CERTIFICATE_IDENTITY", "").strip()
    issuer = os.environ.get("COSIGN_CERTIFICATE_OIDC_ISSUER", "").strip()
    if not cosign or not identity or not issuer:
        fail(f"{label} verification requires the configured Cosign identity policy")
    try:
        result = subprocess.run(
            [
                cosign,
                "verify-blob",
                "--bundle",
                str(bundle),
                "--certificate-identity",
                identity,
                "--certificate-oidc-issuer",
                issuer,
                str(path),
            ],
            check=False,
            capture_output=True,
            text=True,
            timeout=120,
        )
    except subprocess.TimeoutExpired as error:
        raise AssemblyError(f"{label} signature verification timed out") from error
    if result.returncode != 0:
        fail(f"{label} signature verification failed")


def verify_release_signatures(release_dir: Path) -> None:
    """Verify both levels of the detached release signature chain."""

    verify_sigstore_signature(
        release_dir / PRE_SIGN_INVENTORY_PATH,
        release_dir / PRE_SIGN_SIGNATURE_PATH,
        "pre-sign subject inventory",
    )
    verify_sigstore_signature(
        release_dir / "release-manifest.json",
        release_dir / SIDECAR_PATHS["sigstore-bundle"],
        "final release manifest",
    )


def directory(path: Path, label: str) -> Path:
    if path.is_symlink():
        fail(f"{label} must not be a symlink: {path}")
    if not path.is_dir():
        fail(f"{label} is not a directory: {path}")
    return path


def load_manifest(
    toml_path: Path, json_path: Path
) -> tuple[dict[str, Any], bytes, bytes]:
    try:
        authored_bytes = bounded_regular_bytes(toml_path, "compatibility.toml")
        authored = tomllib.loads(authored_bytes.decode("utf-8"))
        mirror_bytes = bounded_regular_bytes(json_path, "compatibility.json")
    except (
        OSError,
        UnicodeError,
        tomllib.TOMLDecodeError,
    ) as error:
        fail(f"cannot read compatibility manifest pair: {error}")
    mirror = strict_json_bytes(mirror_bytes, "compatibility.json")
    if not isinstance(authored, dict) or not isinstance(mirror, dict):
        fail("compatibility manifest pair must contain objects")
    if authored != mirror:
        fail("compatibility.toml and compatibility.json differ semantically")
    if mirror_bytes != canonical_json(authored):
        fail("compatibility.json is not the deterministic sorted-key mirror")
    if toml_path.name != "compatibility.toml" or json_path.name != "compatibility.json":
        fail(
            "release assembly requires compatibility.toml and compatibility.json filenames"
        )
    return authored, authored_bytes, mirror_bytes


def release_evidence_ids(manifest: dict[str, Any]) -> tuple[str, ...]:
    rows = manifest.get("evidence")
    if not isinstance(rows, list):
        fail("compatibility manifest evidence must be a list")
    ids: list[str] = []
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            fail(f"compatibility evidence row {index} is not an object")
        if row.get("release_gate") is not True:
            continue
        evidence_id = row.get("id")
        if not isinstance(evidence_id, str) or not evidence_id:
            fail(f"release-gated evidence row {index} has no id")
        if evidence_id in ids:
            fail(f"duplicate release-gated evidence id: {evidence_id}")
        if (
            row.get("status") != "detached"
            or row.get("artifact_digest") != ""
            or row.get("artifact_digest_location") != "release-manifest.json"
        ):
            fail(
                f"release-gated evidence row {evidence_id} must be detached with "
                "an empty embedded digest and artifact_digest_location="
                "release-manifest.json"
            )
        ids.append(evidence_id)
    if len(ids) != REQUIRED_RELEASE_EVIDENCE_COUNT:
        fail(
            "release assembly requires exactly "
            f"{REQUIRED_RELEASE_EVIDENCE_COUNT} unique release-gated evidence reports; "
            f"manifest declares {len(ids)}: {', '.join(sorted(ids))}"
        )
    return tuple(sorted(ids))


def validate_release_contract(manifest: dict[str, Any]) -> tuple[str, tuple[str, ...]]:
    if (
        manifest.get("manifest_kind") != "release"
        or manifest.get("release_ready") is not True
    ):
        fail("release assembly requires a release-ready compatibility manifest")
    unresolved = manifest.get("unresolved_required_fields")
    if unresolved != []:
        fail(
            "release assembly cannot claim a manifest with unresolved required fields: "
            + ", ".join(map(str, unresolved or []))
        )
    product = manifest.get("release_candidate")
    contracts = manifest.get("contracts")
    if not isinstance(product, str) or not isinstance(contracts, dict):
        fail("compatibility manifest has no release product identity")
    if contracts.get("product") != product:
        fail("compatibility release_candidate differs from contracts.product")

    rows = manifest.get("release_artifacts")
    if not isinstance(rows, list):
        fail("compatibility manifest release_artifacts must be a list")
    observed: set[str] = set()
    for row in rows:
        if not isinstance(row, dict):
            fail("compatibility release artifact row is not an object")
        artifact_id = row.get("id")
        if artifact_id not in RELEASE_ARTIFACT_IDS or artifact_id in observed:
            fail(f"invalid or duplicate release artifact id: {artifact_id!r}")
        observed.add(artifact_id)
        if row.get("profile") != RELEASE_ARTIFACT_PROFILES[artifact_id]:
            fail(f"release artifact profile is invalid: {artifact_id}")
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
                    "Sigstore bundle must be declared as path-only verification material"
                )
            continue
        if row.get("status") != "detached" or row.get("digest") != "":
            fail(
                "release assembly requires detached artifact identities with empty embedded digests: "
                + str(artifact_id)
            )
        if row.get("digest_location") != "release-manifest.json":
            fail(
                f"artifact digest location is not release-manifest.json: {artifact_id}"
            )
        if row.get("digest_algorithm") != "sha256":
            fail(f"artifact digest algorithm is not sha256: {artifact_id}")
        if row.get("verification_material_location") is not None:
            fail(f"non-Sigstore artifact claims verification material: {artifact_id}")
    if observed != set(RELEASE_ARTIFACT_IDS):
        fail(
            "compatibility release artifact inventory is incomplete: "
            + ", ".join(sorted(set(RELEASE_ARTIFACT_IDS) - observed))
        )
    return product, release_evidence_ids(manifest)


def payload_names(version: str) -> dict[str, str]:
    return {
        "source-archive": f"worldstream-{version}-source.tar.gz",
        "native-linux-x86_64-archive": f"worldstream-{version}-linux-x86_64.tar.gz",
        "native-windows-x64-archive": f"worldstream-{version}-windows-x64.zip",
        "oci-linux-amd64-image": f"worldstream-{version}-oci-linux-amd64.oci.tar",
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


def _flat_regular_entries(path: Path, label: str) -> dict[str, Path]:
    directory(path, label)
    entries: dict[str, Path] = {}
    for candidate in sorted(path.rglob("*")):
        relative = candidate.relative_to(path).as_posix()
        if candidate.is_symlink():
            fail(f"{label} contains a symlink: {relative}")
        if candidate.is_dir():
            fail(f"{label} contains an unexpected directory: {relative}")
        try:
            mode = candidate.lstat().st_mode
        except OSError as error:
            fail(f"cannot inspect {label} entry {relative}: {error}")
        if not stat.S_ISREG(mode):
            fail(f"{label} contains a non-regular entry: {relative}")
        entries[relative] = candidate
    return entries


def validate_payload_inputs(payload_dir: Path, version: str) -> dict[str, Path]:
    entries = _flat_regular_entries(payload_dir, "payload directory")
    expected = payload_names(version)
    missing = sorted(set(expected.values()) - set(entries))
    extra = sorted(set(entries) - set(expected.values()))
    if missing or extra:
        detail: list[str] = []
        if missing:
            detail.append("missing payload artifacts: " + ", ".join(missing))
        if extra:
            detail.append("extra payload files: " + ", ".join(extra))
        fail("; ".join(detail))
    return {
        artifact_id: entries[relative] for artifact_id, relative in expected.items()
    }


def valid_sha256_reference(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == len("sha256:") + 64
        and value.startswith("sha256:")
        and all(character in HEX_DIGEST for character in value[7:])
    )


def report_is_passed(
    value: dict[str, Any],
    evidence_id: str,
    path: Path,
    version: str,
    contract: dict[str, Any],
) -> None:
    missing = sorted(NORMALIZED_EVIDENCE_FIELDS - set(value))
    extra = sorted(set(value) - NORMALIZED_EVIDENCE_FIELDS)
    if missing or extra:
        fail(
            f"evidence report {evidence_id} is not a normalized collector report: "
            f"missing={missing}; extra={extra}"
        )
    if value["schema"] != NORMALIZED_EVIDENCE_SCHEMA:
        fail(f"evidence report {evidence_id} has the wrong normalized schema")
    if value["evidence_id"] != evidence_id:
        fail(
            f"mismatched evidence report {path.name}: expected id {evidence_id!r}, "
            f"observed {value['evidence_id']!r}"
        )
    if value["release_gate"] is not True:
        fail(f"evidence report {evidence_id} is not marked release_gate=true")
    if value["release_evidence"] is not True:
        fail(f"evidence report {evidence_id} must set release_evidence=true")
    if value["fail_closed"] is not False:
        fail(f"evidence report {evidence_id} must set fail_closed=false")
    if value["status"] != "passed":
        fail(f"evidence report {evidence_id} is not passed: status={value['status']!r}")
    summary = value["summary"]
    if (
        not isinstance(summary, dict)
        or set(summary) != {"failures", "incomplete_skips"}
        or type(summary["failures"]) is not int
        or type(summary["incomplete_skips"]) is not int
        or summary["failures"] != 0
        or summary["incomplete_skips"] != 0
    ):
        fail(f"evidence report {evidence_id} contains failures or incomplete skips")
    if value["version"] != version:
        fail(f"evidence report {evidence_id} has the wrong release version")
    if value["contract"] != contract:
        fail(f"evidence report {evidence_id} has the wrong compatibility contract")
    if not isinstance(value["platform"], str) or not value["platform"]:
        fail(f"evidence report {evidence_id} has no platform identity")

    checks = value["checks"]
    details = value["producer_details"]
    if (
        not isinstance(checks, dict)
        or not checks
        or any(result is not True for result in checks.values())
    ):
        fail(f"evidence report {evidence_id} contains an invalid check set")
    if not isinstance(details, dict) or set(details) != PRODUCER_DETAIL_FIELDS:
        fail(f"evidence report {evidence_id} is not a pre-sign producer report")
    if (
        not isinstance(details["producer_id"], str)
        or not details["producer_id"].strip()
        or details["phase"] != "pre-sign"
    ):
        fail(f"evidence report {evidence_id} has invalid producer identity or phase")
    outcomes = details["outcomes"]
    if not isinstance(outcomes, dict) or set(outcomes) != set(checks):
        fail(f"evidence report {evidence_id} outcomes do not match checks")
    for check, outcome in outcomes.items():
        if (
            not isinstance(outcome, dict)
            or set(outcome) != OUTCOME_FIELDS
            or outcome["status"] != "passed"
            or not isinstance(outcome["observations"], list)
            or not outcome["observations"]
        ):
            fail(f"evidence report {evidence_id} has invalid outcome: {check}")
        for observation in outcome["observations"]:
            if (
                not isinstance(observation, dict)
                or set(observation) != {"kind", "value"}
                or not isinstance(observation["kind"], str)
                or not observation["kind"]
                or not isinstance(observation["value"], str)
                or not observation["value"]
            ):
                fail(
                    f"evidence report {evidence_id} has an invalid observation: {check}"
                )
    artifacts = details["artifacts"]
    if not isinstance(artifacts, dict) or not artifacts:
        fail(f"evidence report {evidence_id} has no artifact bindings")
    for binding_id, artifact in artifacts.items():
        if not isinstance(binding_id, str) or not binding_id:
            fail(f"evidence report {evidence_id} has an invalid artifact binding ID")
        if (
            not isinstance(artifact, dict)
            or set(artifact) != ARTIFACT_BINDING_FIELDS
            or not valid_sha256_reference(artifact["sha256"])
            or type(artifact["size_bytes"]) is not int
            or artifact["size_bytes"] < 0
        ):
            fail(
                f"evidence report {evidence_id} has an invalid artifact binding: "
                f"{binding_id}"
            )

    source = value["source_report"]
    if not isinstance(source, dict) or set(source) != SOURCE_REPORT_FIELDS:
        fail(f"evidence report {evidence_id} has an invalid source citation")
    if (
        not isinstance(source["source_id"], str)
        or not source["source_id"]
        or source["evidence_id"] != evidence_id
        or source["schema"] != f"worldstream/release-evidence/{evidence_id}/v1"
        or not valid_sha256_reference(source["sha256"])
    ):
        fail(f"evidence report {evidence_id} has a mismatched source citation")


def validate_normalized_evidence_reports(
    evidence_paths: dict[str, Path],
    evidence_ids: tuple[str, ...],
    version: str,
    contract: dict[str, Any],
) -> dict[str, dict[str, Any]]:
    """Independently validate the closed normalized release-evidence set."""

    if set(evidence_paths) != set(evidence_ids):
        fail(
            "normalized evidence paths do not cover the exact release-gated set: "
            f"missing={sorted(set(evidence_ids) - set(evidence_paths))}; "
            f"extra={sorted(set(evidence_paths) - set(evidence_ids))}"
        )
    collector = evidence_collector()
    if set(evidence_ids) != set(collector.EVIDENCE_BY_ID):
        fail("normalized evidence IDs differ from the code-owned source mapping")
    reports: dict[str, dict[str, Any]] = {}
    for evidence_id in evidence_ids:
        path = evidence_paths[evidence_id]
        value = json_object(path, f"normalized evidence {evidence_id}")
        report_is_passed(value, evidence_id, path, version, contract)
        spec = collector.EVIDENCE_BY_ID[evidence_id]
        details = value["producer_details"]
        source = value["source_report"]
        if (
            value["platform"] != spec.platform
            or set(value["checks"]) != set(spec.checks)
            or set(details["outcomes"]) != set(spec.checks)
            or set(details["artifacts"])
            != set(collector.REQUIRED_ARTIFACT_BINDINGS[spec.source_id])
            or details["producer_id"] != collector.EXPECTED_PRODUCER_IDS[spec.source_id]
            or source["source_id"] != spec.source_id
            or source["evidence_id"] != spec.evidence_id
            or source["schema"] != spec.schema
        ):
            fail(
                f"normalized evidence {evidence_id} differs from its code-owned "
                "source/check/platform/artifact/producer mapping"
            )
        reports[evidence_id] = value
    return reports


def validate_evidence_inputs(
    reports_dir: Path,
    evidence_ids: tuple[str, ...],
    version: str,
    contract: dict[str, Any],
) -> dict[str, Path]:
    entries = _flat_regular_entries(reports_dir, "evidence reports directory")
    expected = {evidence_id + ".json" for evidence_id in evidence_ids}
    missing = sorted(expected - set(entries))
    extra = sorted(set(entries) - expected)
    if missing or extra:
        detail: list[str] = []
        if missing:
            detail.append(
                "missing evidence reports: "
                + ", ".join(path.removesuffix(".json") for path in missing)
            )
        if extra:
            detail.append("extra evidence reports: " + ", ".join(extra))
        fail("; ".join(detail))
    result: dict[str, Path] = {}
    for evidence_id in evidence_ids:
        path = entries[evidence_id + ".json"]
        value = json_object(path, f"evidence report {evidence_id}")
        report_is_passed(value, evidence_id, path, version, contract)
        result[evidence_id] = path
    return result


def validate_release_payloads(
    payload_paths: dict[str, Path],
    report_paths: dict[str, Path],
    expected_manifest_sha256: str,
    manifest: dict[str, Any] | None = None,
) -> None:
    """Deep-verify every payload and bind platform claims to its exact bytes."""

    if len(expected_manifest_sha256) != 64 or any(
        character not in HEX_DIGEST for character in expected_manifest_sha256
    ):
        fail("expected release manifest SHA-256 is invalid")
    if set(payload_paths) != set(PAYLOAD_ARTIFACT_IDS):
        fail(
            "release payload verification requires the exact closed payload set: "
            f"missing={sorted(set(PAYLOAD_ARTIFACT_IDS) - set(payload_paths))}; "
            f"extra={sorted(set(payload_paths) - set(PAYLOAD_ARTIFACT_IDS))}"
        )
    binding_reports = {
        evidence_id: json_object(
            report_paths[evidence_id], f"payload binding report {evidence_id}"
        )
        for evidence_id, _artifact_id, _binding_id in PAYLOAD_EVIDENCE_BINDINGS
        if evidence_id in report_paths
    }
    require_contracts = {
        canonical_json(report.get("contract"))
        for report in binding_reports.values()
        if isinstance(report.get("contract"), dict)
    }
    expected_binding_report_ids = {
        evidence_id
        for evidence_id, _artifact_id, _binding_id in PAYLOAD_EVIDENCE_BINDINGS
    }
    if (
        set(binding_reports) != expected_binding_report_ids
        or len(require_contracts) != 1
    ):
        fail("platform payload reports do not share one exact release contract")
    expected_contract = next(report["contract"] for report in binding_reports.values())

    native = package_verifier()
    embedded_manifests: set[bytes] = set()
    for artifact_id in RUNTIME_PAYLOAD_ARTIFACT_IDS[:3]:
        try:
            payload = regular_file(payload_paths[artifact_id], artifact_id)
            native.verify_archive(payload)
            entries = native.archive_entries(payload)
        except native.PackageError as error:
            fail(f"release payload {artifact_id} failed deep verification: {error}")
        manifest_paths = [
            name for name in entries if name.endswith("/manifest/compatibility.json")
        ]
        if len(manifest_paths) != 1:
            fail(f"release payload {artifact_id} has no unique embedded manifest")
        manifest_bytes = entries[manifest_paths[0]]
        embedded_manifest = strict_json_bytes(
            manifest_bytes, f"release payload {artifact_id} embedded manifest"
        )
        if (
            not isinstance(embedded_manifest, dict)
            or embedded_manifest.get("contracts") != expected_contract
            or embedded_manifest.get("release_candidate")
            != expected_contract.get("product")
        ):
            fail(f"release payload {artifact_id} embedded release contract drifted")
        embedded_manifests.add(manifest_bytes)
    if len(embedded_manifests) != 1:
        fail("source, Linux, and Windows payloads embed different release manifests")
    embedded_manifest_bytes = next(iter(embedded_manifests))
    if sha256_bytes(embedded_manifest_bytes) != expected_manifest_sha256:
        fail("release payloads do not embed the exact authoritative manifest")

    oci = oci_layout_verifier()
    try:
        oci.verify_artifact_structure(
            regular_file(
                payload_paths["oci-linux-amd64-image"],
                "oci-linux-amd64-image",
            )
        )
    except oci.VerificationError as error:
        fail(f"release payload oci-linux-amd64-image failed deep verification: {error}")

    if manifest is None:
        embedded_manifest = strict_json_bytes(
            embedded_manifest_bytes, "embedded compatibility manifest"
        )
        if not isinstance(embedded_manifest, dict):
            fail("embedded compatibility manifest is not an object")
        manifest = embedded_manifest
    starter = starter_subject_verifier()
    official_rows = [
        row
        for row in manifest.get("activity_pack_bundles", [])
        if isinstance(row, dict)
        and row.get("pack_id") == "worldstream.negotiate"
        and row.get("required_for_release") is True
        and row.get("status") == "resolved"
    ]
    if len(official_rows) != 1:
        fail("release compatibility contract has no unique official Negotiate bundle")
    for artifact_id in STARTER_SUBJECT_ARTIFACT_IDS:
        path = regular_file(payload_paths[artifact_id], artifact_id)
        try:
            content = starter.read_stable(path, artifact_id)
            if artifact_id == "worldstream-negotiate-bundle":
                identity = starter.negotiate_pack_identity(
                    content, "official Negotiate release subject"
                )
                row = official_rows[0]
                if (
                    identity.get("pack_id") != row.get("pack_id")
                    or identity.get("explanatory_version")
                    != row.get("explanatory_version")
                    or identity.get("revision_digest") != row.get("revision_digest")
                    or identity.get("bundle_digest") != row.get("bundle_digest")
                ):
                    fail(
                        "official Negotiate release subject differs from the exact compatibility identity"
                    )
            else:
                starter.verify_wrapped(
                    content,
                    artifact_id,
                    str(manifest.get("release_candidate", "")),
                )
        except starter.SubjectError as error:
            fail(f"release payload {artifact_id} failed deep verification: {error}")

    for evidence_id, artifact_id, binding_id in PAYLOAD_EVIDENCE_BINDINGS:
        report_path = report_paths.get(evidence_id)
        if report_path is None:
            fail(f"payload binding report is missing: {evidence_id}")
        report = binding_reports[evidence_id]
        details = (
            report.get("producer_details")
            if report.get("schema") == NORMALIZED_EVIDENCE_SCHEMA
            else report.get("details")
        )
        artifacts = details.get("artifacts") if isinstance(details, dict) else None
        binding = artifacts.get(binding_id) if isinstance(artifacts, dict) else None
        payload = payload_paths[artifact_id]
        expected = {
            "sha256": "sha256:" + sha256_file(payload),
            "size_bytes": payload.stat().st_size,
        }
        if binding != expected:
            fail(
                f"platform evidence {evidence_id} does not bind the exact "
                f"shipped payload {artifact_id}"
            )


def expected_release_files(version: str, evidence_ids: tuple[str, ...]) -> set[str]:
    pre_sign_evidence_ids = tuple(
        evidence_id
        for evidence_id in evidence_ids
        if evidence_id != "checksums-signature-sbom-provenance"
    )
    return (
        set(payload_names(version).values())
        | set(SIDECAR_PATHS.values())
        | {"release-manifest.json"}
        | {f"evidence/{evidence_id}.json" for evidence_id in evidence_ids}
        | {PRE_SIGN_INVENTORY_PATH, PRE_SIGN_SIGNATURE_PATH}
        | {
            f"{PRE_SIGN_SUBJECT_DIRECTORY}/{evidence_id}.json"
            for evidence_id in pre_sign_evidence_ids
        }
    )


def validate_existing_release_dir(
    release_dir: Path, version: str, evidence_ids: tuple[str, ...]
) -> None:
    if release_dir.exists() and release_dir.is_symlink():
        fail(f"release directory must not be a symlink: {release_dir}")
    release_dir.mkdir(parents=True, exist_ok=True)
    allowed = expected_release_files(version, evidence_ids)
    for candidate in sorted(release_dir.rglob("*")):
        relative = candidate.relative_to(release_dir).as_posix()
        if candidate.is_symlink():
            fail(f"release directory contains a symlink: {relative}")
        if candidate.is_dir():
            if relative not in {
                "evidence",
                "supply-chain",
                PRE_SIGN_SUBJECT_DIRECTORY,
            }:
                fail(f"release directory contains an unexpected directory: {relative}")
            continue
        if not candidate.is_file():
            fail(f"release directory contains a non-regular entry: {relative}")
        if relative not in allowed:
            fail(f"release directory contains an extra file: {relative}")


def atomic_write_bytes(path: Path, content: bytes, label: str) -> None:
    if path.is_symlink():
        fail(f"{label} must not be a symlink: {path}")
    if path.exists() and not path.is_file():
        fail(f"{label} must be a regular file: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def copy_atomic(
    source: Path,
    destination: Path,
    label: str,
    *,
    maximum: int,
) -> tuple[str, int]:
    """Copy one bounded regular file without materializing it in memory."""

    if maximum <= 0:
        fail(f"{label} has an invalid byte limit")
    source_metadata = regular_file(source, label).lstat()
    if not (0 < source_metadata.st_size <= maximum):
        fail(f"{label} is empty or exceeds its {maximum}-byte limit")
    if destination.is_symlink():
        fail(f"{label} destination must not be a symlink: {destination}")
    if destination.exists() and not destination.is_file():
        fail(f"{label} destination must be a regular file: {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{destination.name}.", dir=destination.parent
    )
    temporary = Path(temporary_name)
    digest = hashlib.sha256()
    copied = 0
    descriptor_open = True
    try:
        # Enter separately so a failed source open leaves the temporary
        # descriptor under the cleanup path below.
        with source.open("rb") as input_stream:  # noqa: SIM117
            with os.fdopen(descriptor, "wb") as output_stream:
                descriptor_open = False
                opened = os.fstat(input_stream.fileno())
                if not stat.S_ISREG(opened.st_mode) or (
                    opened.st_dev,
                    opened.st_ino,
                    opened.st_size,
                    opened.st_mtime_ns,
                    opened.st_ctime_ns,
                ) != (
                    source_metadata.st_dev,
                    source_metadata.st_ino,
                    source_metadata.st_size,
                    source_metadata.st_mtime_ns,
                    source_metadata.st_ctime_ns,
                ):
                    fail(f"{label} changed before it could be copied")
                while chunk := input_stream.read(COPY_CHUNK_BYTES):
                    copied += len(chunk)
                    if copied > maximum:
                        fail(f"{label} exceeded its byte limit while being copied")
                    digest.update(chunk)
                    output_stream.write(chunk)
                finished = os.fstat(input_stream.fileno())
                if (
                    copied != opened.st_size
                    or (finished.st_dev, finished.st_ino, finished.st_size)
                    != (opened.st_dev, opened.st_ino, opened.st_size)
                    or finished.st_mtime_ns != opened.st_mtime_ns
                    or finished.st_ctime_ns != opened.st_ctime_ns
                ):
                    fail(f"{label} changed while it was being copied")
                output_stream.flush()
                os.fsync(output_stream.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, destination)
    except BaseException:
        if descriptor_open:
            try:
                os.close(descriptor)
            except OSError:
                pass
        temporary.unlink(missing_ok=True)
        raise
    return digest.hexdigest(), copied


def stage_release_inputs(
    release_dir: Path,
    payload_dir: Path,
    reports_dir: Path,
    version: str,
    evidence_ids: tuple[str, ...],
    contract: dict[str, Any],
    manifest_sha256: str,
) -> tuple[dict[str, Path], dict[str, Path]]:
    payloads = validate_payload_inputs(payload_dir, version)
    reports = validate_evidence_inputs(reports_dir, evidence_ids, version, contract)
    validate_release_payloads(payloads, reports, manifest_sha256)
    validate_existing_release_dir(release_dir, version, evidence_ids)
    payload_paths = payload_names(version)
    for artifact_id, source in payloads.items():
        copy_atomic(
            source,
            release_dir / payload_paths[artifact_id],
            f"payload {artifact_id}",
            maximum=MAX_RELEASE_PAYLOAD_BYTES,
        )
    evidence_paths: dict[str, Path] = {}
    for evidence_id, source in reports.items():
        destination = release_dir / "evidence" / f"{evidence_id}.json"
        copy_atomic(
            source,
            destination,
            f"evidence report {evidence_id}",
            maximum=evidence_collector().BUILD_IDENTITY.MAX_RELEASE_JSON_BYTES,
        )
        evidence_paths[evidence_id] = destination
    return (
        {
            artifact_id: release_dir / relative
            for artifact_id, relative in payload_paths.items()
        },
        evidence_paths,
    )


def subject_paths(
    payload_paths: dict[str, Path], evidence_paths: dict[str, Path], release_dir: Path
) -> dict[str, Path]:
    subjects = {
        path.relative_to(release_dir).as_posix(): path
        for path in payload_paths.values()
    }
    subjects.update(
        {
            path.relative_to(release_dir).as_posix(): path
            for path in evidence_paths.values()
        }
    )
    if len(subjects) != len(payload_paths) + len(evidence_paths):
        fail("payload and evidence subject paths are not unique")
    if "release-manifest.json" in subjects:
        fail("release-manifest.json cannot be a release subject")
    return dict(sorted(subjects.items()))


def checksums_bytes(subjects: dict[str, Path]) -> bytes:
    return "".join(
        f"{sha256_file(path)}  {relative}\n"
        for relative, path in sorted(subjects.items())
    ).encode("utf-8")


def json_object(path: Path, label: str) -> dict[str, Any]:
    return strict_json_bytes(bounded_regular_bytes(path, label), label)


def validate_pre_sign_material(
    release_dir: Path,
    payload_paths: dict[str, Path],
    evidence_paths: dict[str, Path],
    evidence_ids: tuple[str, ...],
    version: str,
    contract: dict[str, Any],
    manifest_sha256: str,
) -> dict[str, Path]:
    """Validate the signed pre-sign subject set used by final assembly."""

    supply_chain_evidence_id = "checksums-signature-sbom-provenance"
    pre_sign_evidence_ids = tuple(
        evidence_id
        for evidence_id in evidence_ids
        if evidence_id != supply_chain_evidence_id
    )
    expected_pre_sign_reports = REQUIRED_RELEASE_EVIDENCE_COUNT - 1
    if len(pre_sign_evidence_ids) != expected_pre_sign_reports:
        fail(
            "pre-sign subject inventory requires exactly "
            f"{expected_pre_sign_reports} non-supply-chain reports"
        )
    validate_release_payloads(payload_paths, evidence_paths, manifest_sha256)
    inventory = json_object(
        release_dir / PRE_SIGN_INVENTORY_PATH, "pre-sign subject inventory"
    )
    if set(inventory) != {"schema", "phase", "product", "subjects"}:
        fail("pre-sign subject inventory has wrong fields")
    if (
        inventory["schema"] != "worldstream/release-subject-inventory/v1"
        or inventory["phase"] != "pre-sign"
        or inventory["product"] != version
        or not isinstance(inventory["subjects"], list)
    ):
        fail("pre-sign subject inventory has wrong identity or shape")
    expected: dict[tuple[str, str], str] = {}
    for artifact_id, path in payload_paths.items():
        expected[("payload", artifact_id)] = path.relative_to(release_dir).as_posix()
    for evidence_id in pre_sign_evidence_ids:
        expected[("source-report", evidence_id)] = (
            f"{PRE_SIGN_SUBJECT_DIRECTORY}/{evidence_id}.json"
        )
    subjects = inventory["subjects"]
    if len(subjects) != len(expected):
        fail(
            "pre-sign subject inventory does not contain the closed "
            f"{len(expected)}-subject set"
        )
    observed: dict[tuple[str, str], dict[str, Any]] = {}
    for item in subjects:
        if not isinstance(item, dict) or set(item) != {
            "kind",
            "id",
            "path",
            "sha256",
            "size_bytes",
        }:
            fail("pre-sign subject inventory contains an invalid subject")
        kind = item["kind"]
        identity = item["id"]
        if not isinstance(kind, str) or not isinstance(identity, str):
            fail("pre-sign subject inventory subject identity is not textual")
        key = (kind, identity)
        if key in observed or key not in expected:
            fail(f"pre-sign subject inventory contains unexpected subject: {key!r}")
        path = safe_relative(item["path"], "pre-sign subject path")
        if path != expected[key]:
            fail(f"pre-sign subject path is not deterministic for {identity}")
        digest = item["sha256"]
        if (
            not isinstance(digest, str)
            or len(digest) != len("sha256:") + 64
            or not digest.startswith("sha256:")
            or any(character not in HEX_DIGEST for character in digest[7:])
        ):
            fail(f"pre-sign subject digest is invalid: {identity}")
        if type(item["size_bytes"]) is not int or item["size_bytes"] < 0:
            fail(f"pre-sign subject size is invalid: {identity}")
        subject_path = release_dir / path
        regular_file(subject_path, f"pre-sign subject {identity}")
        observed_digest = "sha256:" + sha256_file(subject_path)
        if (
            observed_digest != digest
            or subject_path.stat().st_size != item["size_bytes"]
        ):
            fail(f"pre-sign subject bytes changed: {identity}")
        observed[key] = item
    if set(observed) != set(expected):
        fail("pre-sign subject inventory does not cover the closed subject set")
    if subjects != sorted(subjects, key=lambda item: (item["kind"], item["id"])):
        fail("pre-sign subject inventory is not deterministically ordered")

    collector = evidence_collector()
    collector_manifest = {"release_candidate": version, "contracts": contract}
    for evidence_id in pre_sign_evidence_ids:
        report = json_object(
            evidence_paths[evidence_id], f"normalized evidence {evidence_id}"
        )
        citation = report.get("source_report")
        expected_digest = observed[("source-report", evidence_id)]["sha256"]
        if (
            not isinstance(citation, dict)
            or citation.get("evidence_id") != evidence_id
            or citation.get("sha256") != expected_digest
        ):
            fail(
                f"normalized evidence {evidence_id} is not bound to the signed source inventory"
            )
        spec = collector.EVIDENCE_BY_ID[evidence_id]
        source_path = release_dir / expected[("source-report", evidence_id)]
        try:
            loaded = collector.source_report(source_path, spec, collector_manifest)
        except collector.CollectionError as error:
            fail(f"signed source report {evidence_id} is invalid: {error}")
        expected_report = collector.normalized_report(spec, collector_manifest, loaded)
        if report != expected_report:
            fail(
                f"normalized evidence {evidence_id} is not the exact projection "
                "of its signed source report"
            )

    supply_report = json_object(
        evidence_paths[supply_chain_evidence_id], "normalized supply-chain evidence"
    )
    details = supply_report.get("producer_details")
    if (
        not isinstance(details, dict)
        or set(details)
        != {
            "producer_id",
            "phase",
            "outcomes",
            "artifacts",
        }
        or details.get("phase") != "pre-sign"
    ):
        fail("normalized supply-chain evidence is not a pre-sign producer report")
    artifact_paths = {
        "subject-inventory": PRE_SIGN_INVENTORY_PATH,
        "subject-signature": PRE_SIGN_SIGNATURE_PATH,
        "checksums": SIDECAR_PATHS["checksums"],
        "spdx-sbom": SIDECAR_PATHS["spdx-sbom"],
        "slsa-provenance": SIDECAR_PATHS["slsa-provenance"],
    }
    artifacts = details.get("artifacts")
    if not isinstance(artifacts, dict) or set(artifacts) != set(artifact_paths):
        fail("normalized supply-chain evidence has incomplete artifact bindings")
    for binding_id, relative in artifact_paths.items():
        artifact = artifacts[binding_id]
        artifact_path = release_dir / relative
        regular_file(artifact_path, f"supply-chain artifact {binding_id}")
        if (
            not isinstance(artifact, dict)
            or set(artifact) != {"sha256", "size_bytes"}
            or artifact.get("sha256") != "sha256:" + sha256_file(artifact_path)
            or artifact.get("size_bytes") != artifact_path.stat().st_size
        ):
            fail(f"supply-chain artifact binding does not match bytes: {binding_id}")

    checksums_path = release_dir / SIDECAR_PATHS["checksums"]
    if bounded_regular_bytes(checksums_path, "SHA256SUMS") != checksums_bytes(
        {path: release_dir / path for path in expected.values()}
    ):
        fail("SHA256SUMS does not match the signed pre-sign subject inventory")
    subjects_by_path = {path: release_dir / path for path in expected.values()}
    validate_spdx_subjects(
        release_dir / SIDECAR_PATHS["spdx-sbom"],
        subjects_by_path,
        version=version,
        manifest_sha256=manifest_sha256,
    )
    validate_provenance_subjects(
        release_dir / SIDECAR_PATHS["slsa-provenance"], subjects_by_path
    )
    return subjects_by_path


def validate_spdx_subjects(
    path: Path,
    subjects: dict[str, Path],
    *,
    version: str,
    manifest_sha256: str,
) -> None:
    value = json_object(path, "SPDX SBOM")
    namespace = value.get("documentNamespace")
    parsed_namespace = (
        urllib.parse.urlparse(namespace) if isinstance(namespace, str) else None
    )
    creation = value.get("creationInfo")
    if (
        value.get("spdxVersion") != "SPDX-2.3"
        or value.get("SPDXID") != "SPDXRef-DOCUMENT"
        or value.get("dataLicense") != "CC0-1.0"
        or parsed_namespace is None
        or not parsed_namespace.scheme
        or bool(parsed_namespace.fragment)
        or not isinstance(creation, dict)
        or not isinstance(creation.get("created"), str)
        or SPDX_CREATED.fullmatch(creation["created"]) is None
        or not isinstance(creation.get("creators"), list)
        or not any(
            isinstance(creator, str)
            and SPDX_TOOL_CREATOR.fullmatch(creator) is not None
            for creator in creation["creators"]
        )
        or not isinstance(value.get("files"), list)
        or not isinstance(value.get("packages"), list)
        or not isinstance(value.get("relationships"), list)
    ):
        fail("SPDX SBOM is not a valid SPDX-2.3 JSON document")
    validate_spdx_created(creation["created"])
    identity = release_build_identity_verifier()
    try:
        expected_namespace = identity.spdx_document_namespace(
            version, manifest_sha256, subjects, creation["created"]
        )
    except identity.IdentityError as error:
        fail(f"SPDX namespace inputs are invalid: {error}")
    if namespace != expected_namespace:
        fail("SPDX document namespace does not bind the exact document version")
    observed_sha1: dict[str, str] = {}
    observed_sha256: dict[str, str] = {}
    element_ids = {"SPDXRef-DOCUMENT"}
    for package in value["packages"]:
        identifier = package.get("SPDXID") if isinstance(package, dict) else None
        if not isinstance(identifier, str) or identifier in element_ids:
            fail("SPDX contains a missing or duplicate package identifier")
        element_ids.add(identifier)
    for item in value["files"]:
        if (
            not isinstance(item, dict)
            or set(item)
            != {
                "SPDXID",
                "fileName",
                "checksums",
                "copyrightText",
                "licenseConcluded",
            }
            or not isinstance(item.get("fileName"), str)
            or item.get("SPDXID")
            != identity._spdx_id("ReleaseSubject", item["fileName"])
            or item.get("copyrightText") != "NOASSERTION"
            or item.get("licenseConcluded") != "NOASSERTION"
            or item["SPDXID"] in element_ids
        ):
            fail("SPDX SBOM contains an invalid file entry")
        element_ids.add(item["SPDXID"])
        checksums = item.get("checksums")
        if not isinstance(checksums, list) or any(
            not isinstance(checksum, dict)
            or set(checksum) != {"algorithm", "checksumValue"}
            for checksum in checksums
        ):
            fail(f"SPDX file has no checksums: {item['fileName']}")
        by_algorithm = {
            str(checksum["algorithm"]).replace("-", "").upper(): checksum[
                "checksumValue"
            ]
            for checksum in checksums
        }
        if set(by_algorithm) != {"SHA1", "SHA256"} or len(checksums) != 2:
            fail(
                "SPDX file must have exactly one SHA-1 and SHA-256 checksum: "
                + item["fileName"]
            )
        sha1 = by_algorithm["SHA1"]
        sha256 = by_algorithm["SHA256"]
        if (
            not isinstance(sha1, str)
            or len(sha1) != 40
            or any(character not in HEX_DIGEST for character in sha1)
            or not isinstance(sha256, str)
            or len(sha256) != 64
            or any(character not in HEX_DIGEST for character in sha256)
        ):
            fail(f"SPDX file has an invalid checksum: {item['fileName']}")
        if item["fileName"] in observed_sha256:
            fail(f"SPDX contains duplicate file subject: {item['fileName']}")
        observed_sha1[item["fileName"]] = sha1
        observed_sha256[item["fileName"]] = sha256
    expected_sha1 = {relative: sha1_file(file) for relative, file in subjects.items()}
    expected_sha256 = {
        relative: sha256_file(file) for relative, file in subjects.items()
    }
    if set(observed_sha256) != set(expected_sha256):
        fail(
            "SPDX subject coverage does not match payload+evidence: "
            f"missing={sorted(set(expected_sha256) - set(observed_sha256))}; "
            f"extra={sorted(set(observed_sha256) - set(expected_sha256))}"
        )
    mismatches = sorted(
        relative
        for relative in expected_sha256
        if observed_sha1[relative] != expected_sha1[relative]
        or observed_sha256[relative] != expected_sha256[relative]
    )
    if mismatches:
        fail("SPDX subject checksum mismatch: " + ", ".join(mismatches))
    for relationship in value["relationships"]:
        if not isinstance(relationship, dict) or set(relationship) != {
            "spdxElementId",
            "relationshipType",
            "relatedSpdxElement",
        }:
            fail("SPDX contains a malformed relationship")
        for endpoint in ("spdxElementId", "relatedSpdxElement"):
            identifier = relationship[endpoint]
            if (
                identifier not in {"NONE", "NOASSERTION"}
                and identifier not in element_ids
            ):
                fail("SPDX relationship contains a dangling element identifier")
    if any(
        identifier not in element_ids
        for identifier in value.get("documentDescribes", [])
    ):
        fail("SPDX documentDescribes contains a dangling element identifier")


def validate_provenance_subjects(path: Path, subjects: dict[str, Path]) -> None:
    value = json_object(path, "SLSA provenance")
    if (
        value.get("_type") != "https://in-toto.io/Statement/v1"
        or value.get("predicateType") != "https://slsa.dev/provenance/v1"
        or not isinstance(value.get("predicate"), dict)
        or not isinstance(value["predicate"].get("buildDefinition"), dict)
        or not isinstance(value["predicate"].get("runDetails"), dict)
    ):
        fail("provenance is not an in-toto v1/SLSA v1 statement")
    observed: dict[str, str] = {}
    for item in value.get("subject", []):
        if not isinstance(item, dict) or not isinstance(item.get("name"), str):
            fail("provenance contains an invalid subject")
        digest = item.get("digest")
        if (
            not isinstance(digest, dict)
            or not isinstance(digest.get("sha256"), str)
            or len(digest["sha256"]) != 64
            or any(
                character not in HEX_DIGEST for character in digest["sha256"].lower()
            )
        ):
            fail(f"provenance subject has an invalid SHA-256: {item.get('name')}")
        if item["name"] in observed:
            fail(f"provenance has duplicate subject: {item['name']}")
        observed[item["name"]] = digest["sha256"].lower()
    expected = {relative: sha256_file(path) for relative, path in subjects.items()}
    if set(observed) != set(expected):
        fail(
            "provenance subject coverage does not match payload+evidence: "
            f"missing={sorted(set(expected) - set(observed))}; "
            f"extra={sorted(set(observed) - set(expected))}"
        )
    mismatches = sorted(
        relative for relative in expected if observed[relative] != expected[relative]
    )
    if mismatches:
        fail("provenance subject SHA-256 mismatch: " + ", ".join(mismatches))


def build_release_manifest(
    manifest: dict[str, Any],
    mirror_bytes: bytes,
    release_dir: Path,
    payload_paths: dict[str, Path],
    evidence_paths: dict[str, Path],
) -> dict[str, Any]:
    version = str(manifest["release_candidate"])
    artifact_paths = {
        artifact_id: path.relative_to(release_dir).as_posix()
        for artifact_id, path in payload_paths.items()
    }
    artifact_paths.update(SIDECAR_PATHS)
    artifact_paths["sigstore-bundle"] = SIDECAR_PATHS["sigstore-bundle"]
    artifact_digests = {
        artifact_id: "sha256:" + sha256_file(release_dir / relative)
        for artifact_id, relative in artifact_paths.items()
        if artifact_id != "sigstore-bundle"
    }
    evidence = {
        evidence_id: path.relative_to(release_dir).as_posix()
        for evidence_id, path in evidence_paths.items()
    }
    evidence_digests = {
        evidence_id: "sha256:" + sha256_file(path)
        for evidence_id, path in evidence_paths.items()
    }
    if "sigstore-bundle" in artifact_digests:
        fail("Sigstore bundle must never have a digest in release-manifest.json")
    return {
        "schema": SCHEMA,
        "product": version,
        "source_version": version,
        "manifest": {
            "source": "compatibility.toml",
            "mirror": "compatibility.json",
            "sha256": sha256_bytes(mirror_bytes),
        },
        "artifacts": dict(sorted(artifact_paths.items())),
        "artifact_digests": dict(sorted(artifact_digests.items())),
        "evidence": dict(sorted(evidence.items())),
        "evidence_digests": dict(sorted(evidence_digests.items())),
        "verification_material": {
            "sigstore-bundle": {"path": SIDECAR_PATHS["sigstore-bundle"]}
        },
    }


def prepare_inputs(
    release_dir: Path,
    payload_dir: Path,
    reports_dir: Path,
    manifest_toml: Path = DEFAULT_MANIFEST_TOML,
    manifest_json: Path = DEFAULT_MANIFEST_JSON,
) -> tuple[
    dict[str, Any], bytes, bytes, tuple[str, ...], dict[str, Path], dict[str, Path]
]:
    manifest, _authored_bytes, mirror_bytes = load_manifest(
        manifest_toml, manifest_json
    )
    version, evidence_ids = validate_release_contract(manifest)
    payload_paths, evidence_paths = stage_release_inputs(
        release_dir,
        payload_dir,
        reports_dir,
        version,
        evidence_ids,
        manifest["contracts"],
        sha256_bytes(mirror_bytes),
    )
    return (
        manifest,
        mirror_bytes,
        mirror_bytes,
        evidence_ids,
        payload_paths,
        evidence_paths,
    )


def assemble(
    release_dir: Path,
    payload_dir: Path,
    reports_dir: Path,
    manifest_toml: Path = DEFAULT_MANIFEST_TOML,
    manifest_json: Path = DEFAULT_MANIFEST_JSON,
    output: Path | None = None,
) -> Path:
    manifest, _authored, mirror_bytes = load_manifest(manifest_toml, manifest_json)
    version, evidence_ids = validate_release_contract(manifest)
    payload_paths, evidence_paths = stage_release_inputs(
        release_dir,
        payload_dir,
        reports_dir,
        version,
        evidence_ids,
        manifest["contracts"],
        sha256_bytes(mirror_bytes),
    )
    validate_pre_sign_material(
        release_dir,
        payload_paths,
        evidence_paths,
        evidence_ids,
        version,
        manifest["contracts"],
        sha256_bytes(mirror_bytes),
    )
    metadata = build_release_manifest(
        manifest, mirror_bytes, release_dir, payload_paths, evidence_paths
    )
    destination = output or release_dir / "release-manifest.json"
    if destination.resolve().parent != release_dir.resolve():
        fail("release manifest output must be directly inside release-dir")
    atomic_write_bytes(destination, canonical_json(metadata), "release-manifest.json")
    return destination


def check_inputs(
    payload_dir: Path,
    reports_dir: Path,
    manifest_toml: Path = DEFAULT_MANIFEST_TOML,
    manifest_json: Path = DEFAULT_MANIFEST_JSON,
) -> tuple[str, tuple[str, ...]]:
    manifest, _authored, mirror = load_manifest(manifest_toml, manifest_json)
    version, evidence_ids = validate_release_contract(manifest)
    payload_paths = validate_payload_inputs(payload_dir, version)
    report_paths = validate_evidence_inputs(
        reports_dir, evidence_ids, version, manifest["contracts"]
    )
    validate_release_payloads(payload_paths, report_paths, sha256_bytes(mirror))
    return version, evidence_ids


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--release-dir", type=Path, required=True)
    command.add_argument("--payload-dir", type=Path, required=True)
    command.add_argument("--reports-dir", type=Path, required=True)
    command.add_argument("--manifest-toml", type=Path, default=DEFAULT_MANIFEST_TOML)
    command.add_argument("--manifest-json", type=Path, default=DEFAULT_MANIFEST_JSON)
    command.add_argument("--output", type=Path)
    command.add_argument(
        "--check-inputs",
        action="store_true",
        help="validate the exact payload/report inputs without writing release files",
    )
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.check_inputs:
            version, evidence_ids = check_inputs(
                args.payload_dir,
                args.reports_dir,
                args.manifest_toml,
                args.manifest_json,
            )
            print(
                f"release inputs valid: version={version}; "
                f"payloads={len(PAYLOAD_ARTIFACT_IDS)}; evidence={len(evidence_ids)}"
            )
        else:
            destination = assemble(
                args.release_dir,
                args.payload_dir,
                args.reports_dir,
                args.manifest_toml,
                args.manifest_json,
                args.output,
            )
            print(f"assembled detached release manifest: {destination}")
    except AssemblyError as error:
        print(f"release assembly failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

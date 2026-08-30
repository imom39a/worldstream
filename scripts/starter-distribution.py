#!/usr/bin/env python3
"""Build and verify deterministic WorldStream Starter Distribution carriers.

The carrier is not a second release authority.  Official payload subjects are
authenticated by the exact detached ``release-manifest.json`` and its Sigstore
bundle.  A custom candidate may additionally carry exact, proved ``.wspack``
bytes, but those bytes remain unapproved until the destination Host Operator
approves their physical digest.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import importlib.util
import io
import json
import os
import re
import stat
import sys
import tarfile
import tempfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
CANDIDATE_SCHEMA = "worldstream/starter-candidate/v1"
MANIFEST_SCHEMA = "worldstream/starter-distribution-manifest/v1"
RECEIPT_SCHEMA = "worldstream/starter-distribution-verification/v1"
RELEASE_MANIFEST_SCHEMA = "worldstream/release-artifact-manifest/v2"
PACK_BUNDLE_SCHEMA = "worldstream/activity-pack-bundle/v1"

MAX_INVENTORY_BYTES = 1024 * 1024
MAX_TRUST_BYTES = 16 * 1024 * 1024
MAX_SUBJECT_BYTES = 2 * 1024 * 1024 * 1024
MAX_ARCHIVE_BYTES = 8 * 1024 * 1024 * 1024
MAX_ARCHIVE_MEMBERS = 256
MAX_ARCHIVE_UNPACKED_BYTES = 16 * 1024 * 1024 * 1024
MAX_PACK_MEMBERS = 512
MAX_PACK_UNPACKED_BYTES = 512 * 1024 * 1024

SAFE_ID = re.compile(r"[a-z][a-z0-9._-]{0,127}\Z")
SAFE_FILENAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+-]{0,159}\Z")
VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?\Z")
SHA256_REFERENCE = re.compile(r"sha256:[0-9a-f]{64}\Z")
BLAKE3_REFERENCE = re.compile(r"blake3:[0-9a-f]{64}\Z")

PROFILES = frozenset(
    {
        "native-linux-x86_64",
        "native-windows-x64",
        "oci-linux-amd64",
        "source",
    }
)

BASE_ROLES = frozenset(
    {
        "runtime-distribution",
        "participant-console",
        "studio",
        "typescript-pack-sdk",
        "pack-toolchain",
        "a202-adapter",
        "negotiate-evidence-verifier",
        "deterministic-agents",
        "examples",
        "documentation",
        "licenses",
        "release-metadata",
    }
)
OFFICIAL_ROLES = BASE_ROLES | {"negotiate-bundle", "negotiate-evidence"}
CUSTOM_PACK_ROLES = frozenset({"activity-pack-bundle", "activity-pack-evidence"})
ALL_ROLES = OFFICIAL_ROLES | CUSTOM_PACK_ROLES
PROFILE_RUNTIME_ARTIFACTS = {
    "native-linux-x86_64": "native-linux-x86_64-archive",
    "native-windows-x64": "native-windows-x64-archive",
    "oci-linux-amd64": "oci-linux-amd64-image",
    "source": "source-archive",
}
RELEASE_ROLE_IDS = {
    "a202-adapter": "worldstream-a202-adapter",
    "deterministic-agents": "worldstream-deterministic-agents",
    "documentation": "worldstream-documentation",
    "examples": "worldstream-examples",
    "licenses": "worldstream-licenses",
    "negotiate-bundle": "worldstream-negotiate-bundle",
    "negotiate-evidence": "worldstream-negotiate-evidence",
    "negotiate-evidence-verifier": "worldstream-negotiate-evidence-verifier",
    "pack-toolchain": "worldstream-pack-toolchain",
    "participant-console": "worldstream-participant-console",
    "release-metadata": "worldstream-release-metadata",
    "studio": "worldstream-studio",
    "typescript-pack-sdk": "worldstream-typescript-pack-sdk",
}
RELEASE_EVIDENCE_ROLES = frozenset({"negotiate-evidence"})

FORBIDDEN_PATH_PARTS = frozenset(
    {
        ".env",
        "approval",
        "approvals",
        "backup",
        "backups",
        "credential",
        "credentials",
        "database",
        "databases",
        "private-key",
        "private-keys",
        "runtime-state",
        "secret",
        "secrets",
    }
)
FORBIDDEN_SUFFIXES = (
    ".db",
    ".env",
    ".key",
    ".p12",
    ".pfx",
    ".pem",
    ".sqlite",
    ".sqlite3",
)


class StarterError(RuntimeError):
    """A fail-closed Starter Distribution validation failure."""


@dataclass(frozen=True)
class PreparedSubject:
    subject_id: str
    role: str
    media_type: str
    license_expression: str
    archive_path: str
    content: bytes
    authentication: str
    release_binding: dict[str, str] | None


def fail(message: str) -> None:
    raise StarterError(message)


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def compact_canonical_json(value: object) -> bytes:
    return json.dumps(
        value, ensure_ascii=False, separators=(",", ":"), sort_keys=True
    ).encode("utf-8")


def strict_json(content: bytes, label: str) -> dict[str, Any]:
    def reject_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        value: dict[str, Any] = {}
        for key, item in pairs:
            if key in value:
                fail(f"{label} contains duplicate JSON key {key!r}")
            value[key] = item
        return value

    try:
        value = json.loads(content, object_pairs_hook=reject_duplicates)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise StarterError(f"{label} is not strict JSON") from error
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object")
    return value


def exact_fields(value: dict[str, Any], fields: set[str], label: str) -> None:
    if set(value) != fields:
        missing = sorted(fields - set(value))
        extra = sorted(set(value) - fields)
        fail(f"{label} field inventory differs: missing={missing}; extra={extra}")


def object_value(value: object, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{label} must be an object")
    return value


def list_value(
    value: object, label: str, maximum: int = MAX_ARCHIVE_MEMBERS
) -> list[Any]:
    if not isinstance(value, list) or not (1 <= len(value) <= maximum):
        fail(f"{label} must be a bounded non-empty array")
    return value


def safe_id(value: object, label: str) -> str:
    if not isinstance(value, str) or SAFE_ID.fullmatch(value) is None:
        fail(f"{label} is not a safe identifier")
    return value


def safe_text(value: object, label: str, maximum: int = 256) -> str:
    if (
        not isinstance(value, str)
        or not value
        or value != value.strip()
        or len(value.encode("utf-8")) > maximum
        or any(ord(character) < 0x20 for character in value)
    ):
        fail(f"{label} is not bounded text")
    return value


def safe_relative(value: object, label: str) -> str:
    if (
        not isinstance(value, str)
        or not value
        or value != value.strip()
        or "\\" in value
        or PurePosixPath(value).is_absolute()
        or any(part in {"", ".", ".."} for part in PurePosixPath(value).parts)
    ):
        fail(f"{label} is not a safe relative path")
    return value


def regular_size(
    path: Path,
    label: str,
    maximum: int,
    *,
    allow_empty: bool = False,
) -> int:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise StarterError(f"{label} is unavailable") from error
    minimum = 0 if allow_empty else 1
    if (
        stat.S_ISLNK(metadata.st_mode)
        or not stat.S_ISREG(metadata.st_mode)
        or not (minimum <= metadata.st_size <= maximum)
    ):
        fail(f"{label} must be a bounded regular non-symlink file")
    return metadata.st_size


def regular_bytes(
    path: Path,
    label: str,
    maximum: int,
    *,
    allow_empty: bool = False,
) -> bytes:
    size = regular_size(path, label, maximum, allow_empty=allow_empty)
    try:
        content = path.read_bytes()
    except OSError as error:
        raise StarterError(f"{label} could not be read") from error
    if len(content) != size:
        fail(f"{label} changed while being read")
    return content


def sha256_reference(content: bytes) -> str:
    return "sha256:" + hashlib.sha256(content).hexdigest()


def sha256_file_reference(path: Path, label: str, maximum: int) -> str:
    expected_size = regular_size(path, label, maximum)
    digest = hashlib.sha256()
    observed_size = 0
    try:
        with path.open("rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                observed_size += len(chunk)
                if observed_size > expected_size:
                    fail(f"{label} changed while being hashed")
                digest.update(chunk)
    except OSError as error:
        raise StarterError(f"{label} could not be hashed") from error
    if observed_size != expected_size:
        fail(f"{label} changed while being hashed")
    return "sha256:" + digest.hexdigest()


def load_blake3():
    try:
        import blake3 as native_blake3

        return lambda content: native_blake3.blake3(content).digest()
    except ImportError:
        pass
    name = "worldstream_starter_blake3"
    existing = sys.modules.get(name)
    if existing is not None:
        return existing.blake3
    path = ROOT / "scripts/manifest-evidence-wave6.py"
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load BLAKE3 implementation: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module.blake3


BLAKE3 = load_blake3()


def blake3_reference(content: bytes) -> str:
    return "blake3:" + BLAKE3(content).hex()


def resolve_input(base: Path, value: object, label: str) -> Path:
    if not isinstance(value, str) or not value or "\0" in value:
        fail(f"{label} path is invalid")
    candidate = Path(value)
    if not candidate.is_absolute():
        candidate = base / candidate
    return candidate


def validate_no_sensitive_path(path: str, label: str) -> None:
    parts = tuple(part.casefold() for part in PurePosixPath(path).parts)
    if any(part in FORBIDDEN_PATH_PARTS for part in parts) or parts[-1].endswith(
        FORBIDDEN_SUFFIXES
    ):
        fail(f"{label} names approval, credential, secret, or mutable state material")


def expected_release_binding(role: str, profile: str) -> tuple[str, str] | None:
    if role == "runtime-distribution":
        return "artifact", PROFILE_RUNTIME_ARTIFACTS[profile]
    release_id = RELEASE_ROLE_IDS.get(role)
    if release_id is None:
        return None
    inventory = "evidence" if role in RELEASE_EVIDENCE_ROLES else "artifact"
    return inventory, release_id


def validate_release_manifest(value: dict[str, Any]) -> None:
    exact_fields(
        value,
        {
            "schema",
            "product",
            "source_version",
            "manifest",
            "artifacts",
            "artifact_digests",
            "evidence",
            "evidence_digests",
            "verification_material",
        },
        "release manifest",
    )
    if value["schema"] != RELEASE_MANIFEST_SCHEMA:
        fail("release manifest schema is unsupported")
    product = value["product"]
    if not isinstance(product, str) or VERSION.fullmatch(product) is None:
        fail("release manifest product version is invalid")
    if value["source_version"] != product:
        fail("release manifest source version differs from product")
    identity = object_value(value["manifest"], "release manifest identity")
    exact_fields(identity, {"source", "mirror", "sha256"}, "release manifest identity")
    if (
        identity["source"] != "compatibility.toml"
        or identity["mirror"] != "compatibility.json"
        or not isinstance(identity["sha256"], str)
        or re.fullmatch(r"[0-9a-f]{64}", identity["sha256"]) is None
    ):
        fail("release manifest compatibility identity is invalid")
    artifacts = object_value(value["artifacts"], "release artifact paths")
    artifact_digests = object_value(
        value["artifact_digests"], "release artifact digests"
    )
    evidence = object_value(value["evidence"], "release evidence paths")
    evidence_digests = object_value(
        value["evidence_digests"], "release evidence digests"
    )
    if not artifacts or not evidence:
        fail("release manifest has an empty artifact or evidence inventory")
    for inventory_name, paths, digests in (
        ("artifact", artifacts, artifact_digests),
        ("evidence", evidence, evidence_digests),
    ):
        for subject_id, relative in paths.items():
            safe_id(subject_id, f"release {inventory_name} id")
            safe_relative(relative, f"release {inventory_name} path")
        expected_digests = set(paths)
        if inventory_name == "artifact":
            expected_digests.discard("sigstore-bundle")
        if set(digests) != expected_digests:
            fail(f"release {inventory_name} digest inventory is not closed")
        if any(
            not isinstance(digest, str) or SHA256_REFERENCE.fullmatch(digest) is None
            for digest in digests.values()
        ):
            fail(f"release {inventory_name} digest is invalid")
    verification = object_value(
        value["verification_material"], "release verification material"
    )
    if set(verification) != {"sigstore-bundle"}:
        fail("release verification material inventory is not closed")
    sigstore = object_value(verification["sigstore-bundle"], "Sigstore material")
    exact_fields(sigstore, {"path"}, "Sigstore material")
    if artifacts.get("sigstore-bundle") != sigstore["path"]:
        fail("release manifest Sigstore paths disagree")


def validate_compatibility_manifest(
    value: dict[str, Any], version: str
) -> dict[str, Any]:
    if value.get("schema") != "worldstream/storage-compatibility-manifest/v1":
        fail("compatibility manifest schema is unsupported")
    if (
        value.get("manifest_kind") != "release"
        or value.get("release_ready") is not True
    ):
        fail("Starter input compatibility contract is not release-ready")
    if value.get("unresolved_required_fields") != []:
        fail("Starter input compatibility contract has unresolved release fields")
    contracts = object_value(value.get("contracts"), "compatibility contracts")
    if contracts.get("product") != version or value.get("release_candidate") != version:
        fail("Starter version differs from the compatibility contract")
    rows = value.get("activity_pack_bundles")
    if not isinstance(rows, list):
        fail("compatibility manifest has no Activity Pack Bundle inventory")
    official = [
        row
        for row in rows
        if isinstance(row, dict)
        and row.get("pack_id") == "worldstream.negotiate"
        and row.get("status") == "resolved"
        and row.get("required_for_release") is True
    ]
    if len(official) != 1:
        fail("compatibility manifest has no unique official Negotiate bundle")
    return official[0]


def release_subject(
    release_dir: Path,
    release_manifest: dict[str, Any],
    inventory: str,
    subject_id: str,
) -> tuple[Path, str, str]:
    if inventory not in {"artifact", "evidence"}:
        fail("release binding inventory must be artifact or evidence")
    paths_key = "artifacts" if inventory == "artifact" else "evidence"
    digests_key = "artifact_digests" if inventory == "artifact" else "evidence_digests"
    relative = release_manifest[paths_key].get(subject_id)
    digest = release_manifest[digests_key].get(subject_id)
    if not isinstance(relative, str) or not isinstance(digest, str):
        fail(f"release binding is absent: {inventory}/{subject_id}")
    relative = safe_relative(relative, f"release binding {subject_id}")
    candidate = release_dir.joinpath(*PurePosixPath(relative).parts)
    try:
        resolved_root = release_dir.resolve(strict=True)
        resolved_candidate = candidate.resolve(strict=False)
    except OSError as error:
        raise StarterError("release binding path could not be resolved") from error
    if not resolved_candidate.is_relative_to(resolved_root):
        fail(f"release binding escapes the release directory: {subject_id}")
    return candidate, relative, digest


def pack_entries(content: bytes, label: str) -> dict[str, bytes]:
    try:
        with tarfile.open(fileobj=io.BytesIO(content), mode="r:") as archive:
            members = archive.getmembers()
            if not (1 <= len(members) <= MAX_PACK_MEMBERS):
                fail(f"{label} member count is outside the bound")
            entries: dict[str, bytes] = {}
            unpacked = 0
            for member in members:
                name = safe_relative(member.name, f"{label} member")
                if name in entries:
                    fail(f"{label} contains duplicate member {name}")
                if (
                    not member.isfile()
                    or member.pax_headers
                    or member.mode != 0o644
                    or member.uid != 0
                    or member.gid != 0
                    or member.uname
                    or member.gname
                    or member.mtime != 0
                    or not (0 <= member.size <= MAX_SUBJECT_BYTES)
                ):
                    fail(f"{label} contains a non-canonical member: {name}")
                unpacked += member.size
                if unpacked > MAX_PACK_UNPACKED_BYTES:
                    fail(f"{label} unpacked bytes exceed the bound")
                source = archive.extractfile(member)
                if source is None:
                    fail(f"{label} member cannot be read: {name}")
                value = source.read(MAX_SUBJECT_BYTES + 1)
                if len(value) != member.size:
                    fail(f"{label} member changed while being read: {name}")
                entries[name] = value
    except (OSError, EOFError, tarfile.TarError) as error:
        raise StarterError(f"{label} is not an uncompressed tar archive") from error
    return entries


def pack_identity(content: bytes, label: str) -> dict[str, str]:
    entries = pack_entries(content, label)
    required = {
        "bundle-manifest.json",
        "codec-bundle.json",
        "conformance.json",
        "dependency-lock.json",
        "descriptor.json",
        "executor.component.wasm",
        "golden-corpus.json",
        "revision-lock.json",
        "schemas.json",
    }
    if not required.issubset(entries):
        fail(f"{label} is missing required Activity Pack Bundle members")
    bundle = strict_json(entries["bundle-manifest.json"], f"{label} bundle manifest")
    if entries["bundle-manifest.json"] != compact_canonical_json(bundle):
        fail(f"{label} bundle manifest is not canonical JSON")
    if bundle.get("bundle_format_id") != PACK_BUNDLE_SCHEMA:
        fail(f"{label} bundle format is unsupported")
    revision_digest = bundle.get("revision_digest")
    if (
        not isinstance(revision_digest, str)
        or BLAKE3_REFERENCE.fullmatch(revision_digest) is None
    ):
        fail(f"{label} revision digest is invalid")
    if revision_digest != blake3_reference(entries["revision-lock.json"]):
        fail(f"{label} revision digest differs from revision-lock.json")
    rows = bundle.get("members")
    static_members = bundle.get("static_members")
    if not isinstance(rows, list) or not isinstance(static_members, list):
        fail(f"{label} bundle inventory is invalid")
    observed: dict[str, tuple[str, int]] = {}
    for row in [*rows, *static_members]:
        if not isinstance(row, dict) or set(row) != {"name", "blake3", "size"}:
            fail(f"{label} bundle member row is invalid")
        name = safe_relative(row["name"], f"{label} bundle member row")
        if (
            name == "bundle-manifest.json"
            or name in observed
            or not isinstance(row["blake3"], str)
            or BLAKE3_REFERENCE.fullmatch(row["blake3"]) is None
            or isinstance(row["size"], bool)
            or not isinstance(row["size"], int)
            or row["size"] < 0
        ):
            fail(f"{label} bundle member row is invalid")
        observed[name] = (row["blake3"], row["size"])
    expected_names = set(entries) - {"bundle-manifest.json"}
    if set(observed) != expected_names:
        fail(f"{label} bundle member inventory is not closed")
    for name, value in entries.items():
        if name == "bundle-manifest.json":
            continue
        if observed[name] != (blake3_reference(value), len(value)):
            fail(f"{label} bundle member identity differs: {name}")
    descriptor = strict_json(entries["descriptor.json"], f"{label} descriptor")
    pack_id = safe_id(descriptor.get("pack_id"), f"{label} pack_id")
    version = descriptor.get("explanatory_version")
    if not isinstance(version, str) or VERSION.fullmatch(version) is None:
        fail(f"{label} explanatory version is invalid")
    return {
        "pack_id": pack_id,
        "explanatory_version": version,
        "revision_digest": revision_digest,
        "bundle_digest": blake3_reference(content),
    }


def validate_official_evidence(
    content: bytes,
    pack: dict[str, Any],
) -> None:
    evidence = strict_json(content, "official Negotiate evidence")
    bundle = object_value(evidence.get("bundle"), "official Negotiate evidence bundle")
    proof = object_value(
        evidence.get("production_proof"), "official Negotiate production proof"
    )
    complete = object_value(proof.get("complete"), "official Negotiate complete proof")
    if (
        evidence.get("evidence_id") != "worldstream/negotiate-official-pack-evidence/v1"
        or evidence.get("result") != "passed"
        or evidence.get("pack_id") != pack["pack_id"]
        or evidence.get("version") != pack["explanatory_version"]
        or bundle.get("bundle_digest") != pack["bundle_digest"]
        or bundle.get("revision_digest") != pack["revision_digest"]
        or complete.get("status") != "passed"
        or complete.get("proof_type") != "complete"
        or complete.get("bundle_digest") != pack["bundle_digest"]
        or complete.get("revision_digest") != pack["revision_digest"]
    ):
        fail("official Negotiate evidence does not bind the exact proved bundle")


def signature_verifier():
    name = "worldstream_starter_release_signature"
    existing = sys.modules.get(name)
    if existing is not None:
        return existing
    path = ROOT / "scripts/release-evidence-assemble.py"
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load release signature verifier: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def verify_release_signature(manifest: bytes, sigstore: bytes) -> None:
    verifier = signature_verifier()
    with tempfile.TemporaryDirectory(prefix="worldstream-starter-trust-") as directory:
        root = Path(directory)
        manifest_path = root / "release-manifest.json"
        bundle_path = root / "sigstore.bundle.json"
        manifest_path.write_bytes(manifest)
        bundle_path.write_bytes(sigstore)
        try:
            verifier.verify_sigstore_signature(
                manifest_path, bundle_path, "Starter release manifest"
            )
        except verifier.AssemblyError as error:
            raise StarterError(str(error)) from error


def trust_inputs(
    inventory_path: Path,
    trust: dict[str, Any],
) -> tuple[Path, bytes, dict[str, Any], bytes, dict[str, Any], bytes]:
    exact_fields(
        trust,
        {"release_manifest", "sigstore_bundle", "compatibility_manifest"},
        "Starter trust",
    )
    base = inventory_path.parent
    release_path = resolve_input(base, trust["release_manifest"], "release manifest")
    release_bytes = regular_bytes(release_path, "release manifest", MAX_INVENTORY_BYTES)
    release = strict_json(release_bytes, "release manifest")
    if release_bytes != canonical_json(release):
        fail("release manifest is not canonical JSON")
    validate_release_manifest(release)
    sigstore_path = resolve_input(base, trust["sigstore_bundle"], "Sigstore bundle")
    sigstore_bytes = regular_bytes(sigstore_path, "Sigstore bundle", MAX_TRUST_BYTES)
    expected_sigstore = release_path.parent.joinpath(
        *PurePosixPath(
            release["verification_material"]["sigstore-bundle"]["path"]
        ).parts
    )
    if sigstore_path.resolve() != expected_sigstore.resolve():
        fail("configured Sigstore bundle differs from release-manifest.json")
    compatibility_path = resolve_input(
        base, trust["compatibility_manifest"], "compatibility manifest"
    )
    compatibility_bytes = regular_bytes(
        compatibility_path, "compatibility manifest", MAX_INVENTORY_BYTES
    )
    compatibility = strict_json(compatibility_bytes, "compatibility manifest")
    if compatibility_bytes != canonical_json(compatibility):
        fail("compatibility manifest is not canonical JSON")
    if hashlib.sha256(compatibility_bytes).hexdigest() != release["manifest"]["sha256"]:
        fail("compatibility manifest differs from the signed release identity")
    return (
        release_path.parent,
        release_bytes,
        release,
        sigstore_bytes,
        compatibility,
        compatibility_bytes,
    )


def prepare_subjects(
    inventory_path: Path,
    rows: list[Any],
    mode: str,
    profile: str,
    release_dir: Path,
    release_manifest: dict[str, Any],
) -> list[PreparedSubject]:
    prepared: list[PreparedSubject] = []
    ids: set[str] = set()
    release_references: set[tuple[str, str]] = set()
    roles: list[str] = []
    for raw in rows:
        row = object_value(raw, "Starter subject")
        exact_fields(
            row,
            {"id", "role", "media_type", "license_expression", "binding"},
            "Starter subject",
        )
        subject_id = safe_id(row["id"], "Starter subject id")
        if subject_id in ids:
            fail(f"duplicate Starter subject id: {subject_id}")
        ids.add(subject_id)
        role = row["role"]
        if not isinstance(role, str) or role not in ALL_ROLES:
            fail(f"unsupported Starter subject role: {role!r}")
        roles.append(role)
        media_type = safe_text(row["media_type"], f"{subject_id} media type", 160)
        license_expression = safe_text(
            row["license_expression"], f"{subject_id} license expression", 160
        )
        binding = object_value(row["binding"], f"{subject_id} binding")
        kind = binding.get("kind")
        release_binding: dict[str, str] | None
        if kind == "release":
            exact_fields(binding, {"kind", "inventory", "id"}, f"{subject_id} binding")
            release_id = safe_id(binding["id"], f"{subject_id} release id")
            inventory = binding["inventory"]
            if not isinstance(inventory, str):
                fail(f"{subject_id} release inventory is invalid")
            reference = (inventory, release_id)
            expected_binding = expected_release_binding(role, profile)
            if expected_binding is not None and reference != expected_binding:
                fail(
                    f"{subject_id} release binding differs from the canonical "
                    f"{role} subject {expected_binding}"
                )
            if reference in release_references:
                fail(
                    f"release subject is aliased by more than one Starter role: {reference}"
                )
            release_references.add(reference)
            source, relative, expected_digest = release_subject(
                release_dir, release_manifest, inventory, release_id
            )
            content = regular_bytes(
                source, f"release subject {release_id}", MAX_SUBJECT_BYTES
            )
            observed_digest = sha256_reference(content)
            if observed_digest != expected_digest:
                fail(f"release subject was substituted: {release_id}")
            basename = PurePosixPath(relative).name
            release_binding = {
                "inventory": inventory,
                "id": release_id,
                "path": relative,
                "sha256": observed_digest,
            }
            authentication = "release-manifest"
        elif kind == "candidate":
            exact_fields(binding, {"kind", "path"}, f"{subject_id} binding")
            if mode != "custom" or role not in CUSTOM_PACK_ROLES:
                fail("only custom Pack bundle/evidence subjects may be candidate-bound")
            source = resolve_input(
                inventory_path.parent, binding["path"], f"{subject_id} candidate"
            )
            content = regular_bytes(
                source, f"candidate subject {subject_id}", MAX_SUBJECT_BYTES
            )
            basename = source.name
            release_binding = None
            authentication = "exact-digest-pending-local-approval"
        else:
            fail(f"{subject_id} binding kind is unsupported")
        if SAFE_FILENAME.fullmatch(basename) is None:
            fail(f"{subject_id} source filename is unsafe")
        validate_no_sensitive_path(basename, f"{subject_id} source")
        archive_path = f"payload/{role}/{subject_id}/{basename}"
        safe_relative(archive_path, f"{subject_id} archive path")
        prepared.append(
            PreparedSubject(
                subject_id=subject_id,
                role=role,
                media_type=media_type,
                license_expression=license_expression,
                archive_path=archive_path,
                content=content,
                authentication=authentication,
                release_binding=release_binding,
            )
        )
    counts = {role: roles.count(role) for role in set(roles)}
    if mode == "official":
        if set(roles) != OFFICIAL_ROLES or any(count != 1 for count in counts.values()):
            fail(
                "official Starter must contain exactly one subject for every official role"
            )
        if any(subject.release_binding is None for subject in prepared):
            fail(
                "official Starter contains a subject outside the signed release inventory"
            )
    else:
        if not set(roles).issubset(BASE_ROLES | CUSTOM_PACK_ROLES):
            fail("custom Starter contains an unsupported official-only role")
        if not BASE_ROLES.issubset(roles) or any(
            counts.get(role) != 1 for role in BASE_ROLES
        ):
            fail("custom Starter must contain exactly one subject for every base role")
        if counts.get("activity-pack-bundle", 0) < 1:
            fail(
                "custom Starter must contain at least one candidate Activity Pack Bundle"
            )
        if counts.get("activity-pack-bundle") != counts.get("activity-pack-evidence"):
            fail("custom Starter Pack bundle/evidence counts differ")
        if any(
            subject.role in BASE_ROLES and subject.release_binding is None
            for subject in prepared
        ):
            fail("custom Starter base subjects must remain release-bound")
    return sorted(prepared, key=lambda subject: subject.subject_id)


def validate_pack_rows(
    rows: list[Any],
    mode: str,
    subjects: list[PreparedSubject],
    official_compatibility: dict[str, Any],
) -> list[dict[str, Any]]:
    by_id = {subject.subject_id: subject for subject in subjects}
    result: list[dict[str, Any]] = []
    pack_ids: set[str] = set()
    subject_ids: set[str] = set()
    for raw in rows:
        row = object_value(raw, "Starter Activity Pack")
        exact_fields(
            row,
            {
                "pack_id",
                "explanatory_version",
                "revision_digest",
                "bundle_digest",
                "bundle_subject_id",
                "evidence_subject_id",
                "official",
            },
            "Starter Activity Pack",
        )
        pack_id = safe_id(row["pack_id"], "Starter Activity Pack id")
        if pack_id in pack_ids:
            fail(f"duplicate Starter Activity Pack id: {pack_id}")
        pack_ids.add(pack_id)
        version = row["explanatory_version"]
        if not isinstance(version, str) or VERSION.fullmatch(version) is None:
            fail(f"{pack_id} explanatory version is invalid")
        revision = row["revision_digest"]
        bundle_digest = row["bundle_digest"]
        if (
            not isinstance(revision, str)
            or BLAKE3_REFERENCE.fullmatch(revision) is None
        ):
            fail(f"{pack_id} revision digest is invalid")
        if (
            not isinstance(bundle_digest, str)
            or BLAKE3_REFERENCE.fullmatch(bundle_digest) is None
        ):
            fail(f"{pack_id} bundle digest is invalid")
        bundle_id = safe_id(row["bundle_subject_id"], f"{pack_id} bundle subject")
        evidence_id = safe_id(row["evidence_subject_id"], f"{pack_id} evidence subject")
        if (
            bundle_id in subject_ids
            or evidence_id in subject_ids
            or bundle_id == evidence_id
        ):
            fail("Starter Activity Pack subjects cannot be reused")
        subject_ids.update({bundle_id, evidence_id})
        bundle_subject = by_id.get(bundle_id)
        evidence_subject = by_id.get(evidence_id)
        expected_bundle_role = (
            "negotiate-bundle" if mode == "official" else "activity-pack-bundle"
        )
        expected_evidence_role = (
            "negotiate-evidence" if mode == "official" else "activity-pack-evidence"
        )
        if bundle_subject is None or bundle_subject.role != expected_bundle_role:
            fail(f"{pack_id} bundle subject has the wrong role")
        if evidence_subject is None or evidence_subject.role != expected_evidence_role:
            fail(f"{pack_id} evidence subject has the wrong role")
        observed = pack_identity(bundle_subject.content, f"{pack_id} bundle")
        expected = {
            "pack_id": pack_id,
            "explanatory_version": version,
            "revision_digest": revision,
            "bundle_digest": bundle_digest,
        }
        if observed != expected:
            fail(f"{pack_id} declared identity differs from exact bundle bytes")
        official = row["official"]
        if not isinstance(official, bool) or official is not (mode == "official"):
            fail(f"{pack_id} official status is invalid for Starter mode {mode}")
        if mode == "official":
            compatibility_expected = {
                "pack_id": official_compatibility.get("pack_id"),
                "explanatory_version": official_compatibility.get(
                    "explanatory_version"
                ),
                "revision_digest": official_compatibility.get("revision_digest"),
                "bundle_digest": official_compatibility.get("bundle_digest"),
            }
            if expected != compatibility_expected:
                fail("official Negotiate bytes differ from the compatibility inventory")
            validate_official_evidence(evidence_subject.content, expected)
        result.append(
            {
                **expected,
                "bundle_subject_id": bundle_id,
                "evidence_subject_id": evidence_id,
                "official": official,
            }
        )
    if mode == "official" and len(result) != 1:
        fail("official Starter must contain exactly one Activity Pack")
    bundle_count = sum(subject.role == "activity-pack-bundle" for subject in subjects)
    if mode == "custom" and len(result) != bundle_count:
        fail("custom Starter Activity Pack rows do not cover every candidate bundle")
    return sorted(result, key=lambda row: row["pack_id"])


def subject_manifest_row(subject: PreparedSubject) -> dict[str, Any]:
    return {
        "authentication": subject.authentication,
        "id": subject.subject_id,
        "license_expression": subject.license_expression,
        "media_type": subject.media_type,
        "path": subject.archive_path,
        "release_binding": subject.release_binding,
        "role": subject.role,
        "sha256": sha256_reference(subject.content),
        "size_bytes": len(subject.content),
    }


def load_candidate(
    inventory_path: Path,
) -> tuple[dict[str, Any], list[PreparedSubject], dict[str, bytes]]:
    content = regular_bytes(inventory_path, "Starter candidate", MAX_INVENTORY_BYTES)
    candidate = strict_json(content, "Starter candidate")
    exact_fields(
        candidate,
        {"schema", "distribution", "trust", "subjects", "activity_packs"},
        "Starter candidate",
    )
    if candidate["schema"] != CANDIDATE_SCHEMA:
        fail("Starter candidate schema is unsupported")
    distribution = object_value(candidate["distribution"], "Starter distribution")
    exact_fields(
        distribution,
        {"id", "version", "profile", "mode"},
        "Starter distribution",
    )
    distribution_id = safe_id(distribution["id"], "Starter distribution id")
    version = distribution["version"]
    profile = distribution["profile"]
    mode = distribution["mode"]
    if not isinstance(version, str) or VERSION.fullmatch(version) is None:
        fail("Starter distribution version is invalid")
    if profile not in PROFILES:
        fail("Starter distribution profile is unsupported")
    if mode not in {"official", "custom"}:
        fail("Starter distribution mode is unsupported")
    if "approval" in distribution_id or "secret" in distribution_id:
        fail("Starter distribution id names forbidden authority material")
    trust = object_value(candidate["trust"], "Starter trust")
    (
        release_dir,
        release_bytes,
        release_manifest,
        sigstore_bytes,
        compatibility,
        compatibility_bytes,
    ) = trust_inputs(inventory_path, trust)
    if release_manifest["product"] != version:
        fail("Starter version differs from the signed release manifest")
    official_compatibility = validate_compatibility_manifest(compatibility, version)
    subjects = prepare_subjects(
        inventory_path,
        list_value(candidate["subjects"], "Starter subjects"),
        mode,
        profile,
        release_dir,
        release_manifest,
    )
    packs = validate_pack_rows(
        list_value(candidate["activity_packs"], "Starter Activity Packs", 64),
        mode,
        subjects,
        official_compatibility,
    )
    manifest = {
        "activity_packs": packs,
        "distribution": {
            "id": distribution_id,
            "mode": mode,
            "profile": profile,
            "version": version,
        },
        "policy": {
            "approval_state_carried": False,
            "credentials_carried": False,
            "mutable_database_state_carried": False,
            "network_download_required": False,
            "registry_required": False,
            "runtime_rebuild_required": False,
            "target_local_exact_digest_approval_required": True,
        },
        "schema": MANIFEST_SCHEMA,
        "subjects": [subject_manifest_row(subject) for subject in subjects],
        "trust": {
            "compatibility_manifest_path": "trust/compatibility.json",
            "compatibility_manifest_sha256": sha256_reference(compatibility_bytes),
            "release_manifest_path": "trust/release-manifest.json",
            "release_manifest_sha256": sha256_reference(release_bytes),
            "sigstore_bundle_path": "trust/sigstore.bundle.json",
            "sigstore_bundle_sha256": sha256_reference(sigstore_bytes),
        },
    }
    files = {
        "starter-manifest.json": canonical_json(manifest),
        "trust/compatibility.json": compatibility_bytes,
        "trust/release-manifest.json": release_bytes,
        "trust/sigstore.bundle.json": sigstore_bytes,
        **{subject.archive_path: subject.content for subject in subjects},
    }
    return manifest, subjects, dict(sorted(files.items()))


def archive_root(manifest: dict[str, Any]) -> str:
    distribution = manifest["distribution"]
    return f"worldstream-starter-{distribution['version']}-{distribution['profile']}"


def validate_epoch(value: int) -> int:
    if (
        isinstance(value, bool)
        or not isinstance(value, int)
        or not (0 <= value <= 0xFFFFFFFF)
    ):
        fail("SOURCE_DATE_EPOCH is outside the supported range")
    return value


def write_archive(
    path: Path, root_name: str, files: dict[str, bytes], epoch: int
) -> None:
    safe_relative(root_name, "Starter archive root")
    validate_epoch(epoch)
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists() or path.is_symlink():
        fail(f"refusing to overwrite Starter archive: {path}")
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    os.close(descriptor)
    temporary = Path(temporary_name)
    try:
        with (
            temporary.open("wb") as output,
            gzip.GzipFile(
                fileobj=output, mode="wb", filename="", mtime=epoch
            ) as compressed,
            tarfile.open(
                fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT
            ) as archive,
        ):
            for relative, content in sorted(files.items()):
                safe_relative(relative, "Starter archive member")
                info = tarfile.TarInfo(f"{root_name}/{relative}")
                info.size = len(content)
                info.mode = 0o644
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                info.mtime = epoch
                archive.addfile(info, io.BytesIO(content))
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def archive_entries(path: Path) -> tuple[str, dict[str, bytes], int]:
    regular_size(path, "Starter archive", MAX_ARCHIVE_BYTES)
    try:
        with tarfile.open(path, mode="r:gz") as archive:
            members = archive.getmembers()
            if not (1 <= len(members) <= MAX_ARCHIVE_MEMBERS):
                fail("Starter archive member count is outside the bound")
            entries: dict[str, bytes] = {}
            roots: set[str] = set()
            epoch: int | None = None
            unpacked = 0
            for member in members:
                name = safe_relative(member.name, "Starter archive member")
                parts = PurePosixPath(name).parts
                if len(parts) < 2:
                    fail("Starter archive member is outside the package root")
                roots.add(parts[0])
                relative = PurePosixPath(*parts[1:]).as_posix()
                if relative in entries:
                    fail(f"Starter archive contains duplicate member: {relative}")
                if (
                    not member.isfile()
                    or member.pax_headers
                    or member.mode != 0o644
                    or member.uid != 0
                    or member.gid != 0
                    or member.uname
                    or member.gname
                    or not (0 <= member.size <= MAX_SUBJECT_BYTES)
                ):
                    fail(f"Starter archive contains a non-canonical member: {relative}")
                if epoch is None:
                    epoch = member.mtime
                elif member.mtime != epoch:
                    fail("Starter archive member timestamps differ")
                validate_epoch(member.mtime)
                unpacked += member.size
                if unpacked > MAX_ARCHIVE_UNPACKED_BYTES:
                    fail("Starter archive unpacked bytes exceed the bound")
                source = archive.extractfile(member)
                if source is None:
                    fail(f"Starter archive member cannot be read: {relative}")
                value = source.read(MAX_SUBJECT_BYTES + 1)
                if len(value) != member.size:
                    fail(f"Starter archive member changed while being read: {relative}")
                entries[relative] = value
    except (OSError, EOFError, tarfile.TarError) as error:
        raise StarterError(
            f"Starter archive container is malformed: {error}"
        ) from error
    if len(roots) != 1 or epoch is None:
        fail("Starter archive does not have one canonical package root")
    return next(iter(roots)), entries, epoch


def verify_manifest_and_entries(
    manifest: dict[str, Any],
    manifest_bytes: bytes,
    entries: dict[str, bytes],
) -> None:
    exact_fields(
        manifest,
        {"schema", "distribution", "policy", "subjects", "activity_packs", "trust"},
        "Starter manifest",
    )
    if manifest["schema"] != MANIFEST_SCHEMA or manifest_bytes != canonical_json(
        manifest
    ):
        fail("Starter manifest schema or canonical bytes are invalid")
    distribution = object_value(manifest["distribution"], "Starter distribution")
    exact_fields(
        distribution,
        {"id", "mode", "profile", "version"},
        "Starter distribution",
    )
    safe_id(distribution["id"], "Starter distribution id")
    if distribution["mode"] not in {"official", "custom"}:
        fail("Starter distribution mode is invalid")
    if distribution["profile"] not in PROFILES:
        fail("Starter distribution profile is invalid")
    if (
        not isinstance(distribution["version"], str)
        or VERSION.fullmatch(distribution["version"]) is None
    ):
        fail("Starter distribution version is invalid")
    policy = object_value(manifest["policy"], "Starter policy")
    expected_policy = {
        "approval_state_carried": False,
        "credentials_carried": False,
        "mutable_database_state_carried": False,
        "network_download_required": False,
        "registry_required": False,
        "runtime_rebuild_required": False,
        "target_local_exact_digest_approval_required": True,
    }
    if policy != expected_policy:
        fail("Starter policy would transport authority or require unsupported behavior")
    trust = object_value(manifest["trust"], "Starter trust")
    exact_fields(
        trust,
        {
            "compatibility_manifest_path",
            "compatibility_manifest_sha256",
            "release_manifest_path",
            "release_manifest_sha256",
            "sigstore_bundle_path",
            "sigstore_bundle_sha256",
        },
        "Starter trust",
    )
    for stem in ("compatibility_manifest", "release_manifest", "sigstore_bundle"):
        relative = safe_relative(trust[f"{stem}_path"], f"Starter {stem} path")
        expected_digest = trust[f"{stem}_sha256"]
        if (
            not isinstance(expected_digest, str)
            or SHA256_REFERENCE.fullmatch(expected_digest) is None
        ):
            fail(f"Starter {stem} digest is invalid")
        content = entries.get(relative)
        if content is None or sha256_reference(content) != expected_digest:
            fail(f"Starter {stem} bytes differ from the manifest")
    release = strict_json(entries[trust["release_manifest_path"]], "release manifest")
    validate_release_manifest(release)
    compatibility = strict_json(
        entries[trust["compatibility_manifest_path"]], "compatibility manifest"
    )
    official_compatibility = validate_compatibility_manifest(
        compatibility, distribution["version"]
    )
    if (
        hashlib.sha256(entries[trust["compatibility_manifest_path"]]).hexdigest()
        != release["manifest"]["sha256"]
    ):
        fail("Starter compatibility bytes differ from the signed release identity")
    subject_rows = list_value(manifest["subjects"], "Starter subjects")
    subjects: list[PreparedSubject] = []
    expected_paths = {
        "starter-manifest.json",
        trust["compatibility_manifest_path"],
        trust["release_manifest_path"],
        trust["sigstore_bundle_path"],
    }
    ids: set[str] = set()
    release_references: set[tuple[str, str]] = set()
    for raw in subject_rows:
        row = object_value(raw, "Starter subject")
        exact_fields(
            row,
            {
                "authentication",
                "id",
                "license_expression",
                "media_type",
                "path",
                "release_binding",
                "role",
                "sha256",
                "size_bytes",
            },
            "Starter subject",
        )
        subject_id = safe_id(row["id"], "Starter subject id")
        if subject_id in ids:
            fail(f"duplicate Starter subject id: {subject_id}")
        ids.add(subject_id)
        role = row["role"]
        if role not in ALL_ROLES:
            fail(f"unsupported Starter role: {role!r}")
        path = safe_relative(row["path"], f"{subject_id} path")
        if not path.startswith(f"payload/{role}/{subject_id}/"):
            fail(f"{subject_id} path is outside its canonical role directory")
        validate_no_sensitive_path(path, f"{subject_id} path")
        if path in expected_paths:
            fail(f"duplicate Starter payload path: {path}")
        expected_paths.add(path)
        content = entries.get(path)
        if content is None:
            fail(f"Starter subject is missing: {subject_id}")
        if row["sha256"] != sha256_reference(content) or row["size_bytes"] != len(
            content
        ):
            fail(f"Starter subject bytes were substituted: {subject_id}")
        authentication = row["authentication"]
        binding = row["release_binding"]
        if authentication == "release-manifest":
            binding = object_value(binding, f"{subject_id} release binding")
            exact_fields(
                binding,
                {"inventory", "id", "path", "sha256"},
                f"{subject_id} release binding",
            )
            inventory = binding["inventory"]
            release_id = safe_id(binding["id"], f"{subject_id} release id")
            paths_key = "artifacts" if inventory == "artifact" else "evidence"
            digests_key = (
                "artifact_digests" if inventory == "artifact" else "evidence_digests"
            )
            if inventory not in {"artifact", "evidence"}:
                fail(f"{subject_id} release inventory is invalid")
            reference = (inventory, release_id)
            expected_binding = expected_release_binding(role, distribution["profile"])
            if expected_binding is not None and reference != expected_binding:
                fail(f"{subject_id} release binding differs from its canonical role")
            if reference in release_references:
                fail(f"release subject is aliased by more than one role: {reference}")
            release_references.add(reference)
            if (
                release[paths_key].get(release_id) != binding["path"]
                or release[digests_key].get(release_id) != binding["sha256"]
                or binding["sha256"] != row["sha256"]
            ):
                fail(f"{subject_id} differs from the signed release inventory")
        elif authentication == "exact-digest-pending-local-approval":
            if distribution["mode"] != "custom" or role not in CUSTOM_PACK_ROLES:
                fail(f"{subject_id} has invalid candidate authentication")
            if binding is not None:
                fail(f"{subject_id} candidate must not claim a release binding")
        else:
            fail(f"{subject_id} authentication mode is invalid")
        subjects.append(
            PreparedSubject(
                subject_id=subject_id,
                role=role,
                media_type=safe_text(row["media_type"], f"{subject_id} media type"),
                license_expression=safe_text(
                    row["license_expression"], f"{subject_id} license expression"
                ),
                archive_path=path,
                content=content,
                authentication=authentication,
                release_binding=binding,
            )
        )
    if set(entries) != expected_paths:
        fail(
            "Starter archive subject inventory is not closed: "
            f"missing={sorted(expected_paths - set(entries))}; "
            f"extra={sorted(set(entries) - expected_paths)}"
        )
    mode = distribution["mode"]
    roles = [subject.role for subject in subjects]
    if mode == "official":
        if set(roles) != OFFICIAL_ROLES or len(roles) != len(OFFICIAL_ROLES):
            fail("official Starter role inventory is not closed")
        if any(subject.authentication != "release-manifest" for subject in subjects):
            fail("official Starter contains unsigned candidate material")
    else:
        counts = {role: roles.count(role) for role in set(roles)}
        if not set(roles).issubset(BASE_ROLES | CUSTOM_PACK_ROLES):
            fail("custom Starter contains an unsupported official-only role")
        if not BASE_ROLES.issubset(roles) or any(
            counts.get(role) != 1 for role in BASE_ROLES
        ):
            fail("custom Starter base role inventory is incomplete")
        if counts.get("activity-pack-bundle", 0) < 1 or counts.get(
            "activity-pack-bundle"
        ) != counts.get("activity-pack-evidence"):
            fail("custom Starter Pack bundle/evidence role inventory is invalid")
    validate_pack_rows(
        list_value(manifest["activity_packs"], "Starter Activity Packs", 64),
        mode,
        subjects,
        official_compatibility,
    )


def verify_archive(path: Path, *, structural_only: bool = False) -> dict[str, Any]:
    root_name, entries, epoch = archive_entries(path)
    manifest_bytes = entries.get("starter-manifest.json")
    if manifest_bytes is None:
        fail("Starter archive has no starter-manifest.json")
    manifest = strict_json(manifest_bytes, "Starter manifest")
    verify_manifest_and_entries(manifest, manifest_bytes, entries)
    if root_name != archive_root(manifest):
        fail("Starter archive root differs from its manifest identity")
    trust = manifest["trust"]
    if not structural_only:
        verify_release_signature(
            entries[trust["release_manifest_path"]],
            entries[trust["sigstore_bundle_path"]],
        )
    return {
        "archive_sha256": sha256_file_reference(
            path, "Starter archive", MAX_ARCHIVE_BYTES
        ),
        "authentication": (
            "structural-only" if structural_only else "sigstore-identity-verified"
        ),
        "distribution": manifest["distribution"],
        "manifest_sha256": sha256_reference(manifest_bytes),
        "schema": RECEIPT_SCHEMA,
        "source_date_epoch": epoch,
        "status": "passed",
        "subject_count": len(manifest["subjects"]),
    }


def build_archive(
    inventory_path: Path,
    output: Path,
    *,
    source_date_epoch: int,
    structural_only: bool = False,
) -> dict[str, Any]:
    manifest, _subjects, files = load_candidate(inventory_path)
    if not structural_only:
        trust = manifest["trust"]
        verify_release_signature(
            files[trust["release_manifest_path"]], files[trust["sigstore_bundle_path"]]
        )
    write_archive(
        output, archive_root(manifest), files, validate_epoch(source_date_epoch)
    )
    return verify_archive(output, structural_only=structural_only)


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    subcommands = command.add_subparsers(dest="command", required=True)
    build = subcommands.add_parser(
        "build", help="build one deterministic Starter archive"
    )
    build.add_argument("--inventory", required=True, type=Path)
    build.add_argument("--output", required=True, type=Path)
    build.add_argument(
        "--source-date-epoch",
        type=int,
        default=os.environ.get("SOURCE_DATE_EPOCH", "0"),
    )
    build.add_argument(
        "--structural-only",
        action="store_true",
        help="skip Sigstore identity verification and make no release claim",
    )
    verify = subcommands.add_parser("verify", help="verify a Starter archive offline")
    verify.add_argument("archive", type=Path)
    verify.add_argument(
        "--structural-only",
        action="store_true",
        help="skip Sigstore identity verification and make no release claim",
    )
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "build":
            receipt = build_archive(
                args.inventory,
                args.output,
                source_date_epoch=args.source_date_epoch,
                structural_only=args.structural_only,
            )
        else:
            receipt = verify_archive(args.archive, structural_only=args.structural_only)
        print(canonical_json(receipt).decode("utf-8"), end="")
        return 11 if args.structural_only else 0
    except StarterError as error:
        print(f"Starter Distribution rejected: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

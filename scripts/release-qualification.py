#!/usr/bin/env python3
"""Assemble and verify post-sign Starter and outside-adopter qualification.

The primary detached release manifest must be signed before a Starter can
embed it and before outside adopters can use it.  Those facts therefore cannot
be inserted back into that same manifest without a hash/signature cycle.  This
second, separately signed manifest binds the exact primary manifest, verified
Starter carriers, and the two adopter-journey projections.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import re
import stat
import sys
import tempfile
from pathlib import Path, PurePosixPath
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = "worldstream/release-qualification-manifest/v1"
EVIDENCE_SCHEMA = "worldstream/release-qualification-evidence/v1"
SUMMARY_SCHEMA = "worldstream/outside-adopter-qualification/v1"
RELEASE_SCHEMA = "worldstream/release-artifact-manifest/v2"
MAX_JSON_BYTES = 16 * 1024 * 1024
MAX_ARTIFACT_BYTES = 8 * 1024 * 1024 * 1024
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
SAFE_ID = re.compile(r"[a-z][a-z0-9._-]{0,127}\Z")
QUALIFICATION_IDS = (
    "starter-distribution-and-custom-pack-recovery",
    "outside-adopter-pack-author-journey",
    "outside-adopter-application-integrator-journey",
)
ADOPTER_INSTALLATION_PROFILES = frozenset(
    {"native-linux-x86_64", "native-windows-x64", "oci-linux-amd64"}
)


class QualificationError(RuntimeError):
    """Post-sign qualification material is incomplete or substituted."""


def fail(message: str) -> None:
    raise QualificationError(message)


def load_module(name: str, path: Path):
    existing = sys.modules.get(name)
    if existing is not None:
        return existing
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


ASSEMBLER = load_module(
    "worldstream_qualification_release_verifier",
    ROOT / "scripts/release-evidence-assemble.py",
)
STARTER = load_module(
    "worldstream_qualification_starter_verifier",
    ROOT / "scripts/starter-distribution.py",
)


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode()


def strict_json(content: bytes, label: str) -> dict[str, Any]:
    def pairs(items: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in items:
            if key in result:
                fail(f"{label} contains duplicate JSON key {key!r}")
            result[key] = value
        return result

    try:
        value = json.loads(content, object_pairs_hook=pairs)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise QualificationError(f"{label} is not strict JSON") from error
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object")
    return value


def regular_bytes(path: Path, label: str, maximum: int) -> bytes:
    try:
        admitted = path.lstat()
    except OSError as error:
        raise QualificationError(f"{label} is unavailable") from error
    if (
        stat.S_ISLNK(admitted.st_mode)
        or not stat.S_ISREG(admitted.st_mode)
        or not (0 < admitted.st_size <= maximum)
    ):
        fail(f"{label} must be a bounded regular non-symlink file")
    try:
        content = path.read_bytes()
        finished = path.lstat()
    except OSError as error:
        raise QualificationError(f"{label} could not be read") from error
    if len(content) != admitted.st_size or (
        admitted.st_dev,
        admitted.st_ino,
        admitted.st_size,
        admitted.st_mtime_ns,
    ) != (finished.st_dev, finished.st_ino, finished.st_size, finished.st_mtime_ns):
        fail(f"{label} changed while being read")
    return content


def sha256(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def sha256_ref(content: bytes) -> str:
    return "sha256:" + sha256(content)


def safe_id(value: str, label: str) -> str:
    if not isinstance(value, str) or SAFE_ID.fullmatch(value) is None:
        fail(f"{label} is not a safe identifier")
    return value


def safe_relative(value: str, label: str) -> str:
    if not isinstance(value, str):
        fail(f"{label} is not a path")
    path = PurePosixPath(value)
    if (
        not value
        or value != value.strip()
        or "\\" in value
        or path.is_absolute()
        or any(part in {"", ".", ".."} for part in path.parts)
    ):
        fail(f"{label} is not a safe relative path")
    return value


def parse_starters(values: list[str]) -> dict[str, Path]:
    parsed: dict[str, Path] = {}
    for value in values:
        if "=" not in value:
            fail(f"Starter must use ID=PATH syntax: {value!r}")
        identity, raw_path = value.split("=", 1)
        safe_id(identity, "Starter id")
        if identity in parsed:
            fail(f"duplicate Starter id: {identity}")
        parsed[identity] = Path(raw_path)
    if not parsed:
        fail("qualification requires at least one official and one custom Starter")
    return parsed


def validate_release_inputs(release_dir: Path) -> tuple[dict[str, Any], bytes, bytes]:
    manifest_path = release_dir / "release-manifest.json"
    signature_path = release_dir / "sigstore.bundle.json"
    manifest_bytes = regular_bytes(
        manifest_path, "signed release manifest", MAX_JSON_BYTES
    )
    signature_bytes = regular_bytes(
        signature_path, "release Sigstore bundle", MAX_JSON_BYTES
    )
    manifest = strict_json(manifest_bytes, "signed release manifest")
    if manifest.get("schema") != RELEASE_SCHEMA:
        fail("signed release manifest schema is unsupported")
    try:
        ASSEMBLER.verify_sigstore_signature(
            manifest_path, signature_path, "primary release manifest"
        )
    except ASSEMBLER.AssemblyError as error:
        raise QualificationError(str(error)) from error
    return manifest, manifest_bytes, signature_bytes


def validate_starters(
    starters: dict[str, Path],
    release_manifest_sha256: str,
    *,
    authenticate: bool = True,
) -> tuple[dict[str, dict[str, Any]], dict[str, bytes]]:
    receipts: dict[str, dict[str, Any]] = {}
    contents: dict[str, bytes] = {}
    modes: set[str] = set()
    profiles: set[str] = set()
    for identity, path in sorted(starters.items()):
        content = regular_bytes(path, f"Starter {identity}", MAX_ARTIFACT_BYTES)
        try:
            receipt = STARTER.verify_archive(path, structural_only=not authenticate)
            _root, entries, _epoch = STARTER.archive_entries(path)
        except STARTER.StarterError as error:
            raise QualificationError(
                f"Starter {identity} failed verification: {error}"
            ) from error
        manifest = STARTER.strict_json(
            entries["starter-manifest.json"], "Starter manifest"
        )
        embedded_release = entries[manifest["trust"]["release_manifest_path"]]
        distribution = receipt.get("distribution")
        mode = distribution.get("mode") if isinstance(distribution, dict) else None
        profile = (
            distribution.get("profile") if isinstance(distribution, dict) else None
        )
        if (
            receipt.get("schema") != STARTER.RECEIPT_SCHEMA
            or receipt.get("status") != "passed"
            or receipt.get("authentication")
            != ("sigstore-identity-verified" if authenticate else "structural-only")
            or sha256(embedded_release) != release_manifest_sha256
            or mode not in {"official", "custom"}
            or not isinstance(profile, str)
        ):
            fail(f"Starter {identity} does not bind the exact signed release")
        modes.add(mode)
        profiles.add(profile)
        receipts[identity] = receipt
        contents[identity] = content
    if modes != {"official", "custom"}:
        fail("qualification requires both an official and custom verified Starter")
    if not profiles:
        fail("qualification has no Starter installation profile")
    return receipts, contents


def validate_adopters(
    path: Path, release_manifest_sha256: str
) -> tuple[dict[str, Any], bytes]:
    content = regular_bytes(path, "outside-adopter qualification", MAX_JSON_BYTES)
    value = strict_json(content, "outside-adopter qualification")
    expected_fields = {
        "schema",
        "status",
        "release_manifest_sha256",
        "trial_count",
        "pack_author_count",
        "application_integrator_count",
        "installation_profiles",
        "maximum_pack_author_seconds",
        "maximum_application_integrator_seconds",
        "receipts",
    }
    if (
        set(value) != expected_fields
        or value.get("schema") != SUMMARY_SCHEMA
        or value.get("status") != "qualified"
        or value.get("release_manifest_sha256") != release_manifest_sha256
        or value.get("trial_count") != 6
        or value.get("pack_author_count") != 3
        or value.get("application_integrator_count") != 3
        or not isinstance(value.get("installation_profiles"), list)
        or len(value["installation_profiles"]) < 2
        or not isinstance(value.get("receipts"), list)
        or len(value["receipts"]) != 6
        or type(value.get("maximum_pack_author_seconds")) is not int
        or not 0 <= value["maximum_pack_author_seconds"] <= 3600
        or type(value.get("maximum_application_integrator_seconds")) is not int
        or not 0 <= value["maximum_application_integrator_seconds"] <= 1800
    ):
        fail("outside-adopter qualification is incomplete or refers to another release")
    profiles = value["installation_profiles"]
    if (
        profiles != sorted(set(profiles))
        or any(not isinstance(profile, str) or not profile for profile in profiles)
        or not set(profiles).issubset(ADOPTER_INSTALLATION_PROFILES)
    ):
        fail("outside-adopter installation profiles are invalid")
    trials: set[str] = set()
    pack_routes: set[str] = set()
    journey_counts = {"pack_author": 0, "application_integrator": 0}
    for row in value["receipts"]:
        if not isinstance(row, dict) or set(row) != {
            "trial_id",
            "journey",
            "route",
            "installation_profile",
            "receipt_sha256",
        }:
            fail("outside-adopter receipt summary has an invalid shape")
        trial_id = row.get("trial_id")
        journey = row.get("journey")
        route = row.get("route")
        profile = row.get("installation_profile")
        if (
            not isinstance(trial_id, str)
            or trial_id in trials
            or journey not in journey_counts
            or profile not in profiles
            or SHA256.fullmatch(str(row.get("receipt_sha256"))) is None
            or (
                journey == "pack_author"
                and route not in {"deterministic", "prompt_assisted"}
            )
            or (journey == "application_integrator" and route != "official_negotiate")
        ):
            fail("outside-adopter receipt summary is inconsistent")
        trials.add(trial_id)
        journey_counts[journey] += 1
        if journey == "pack_author":
            pack_routes.add(route)
    if journey_counts != {
        "pack_author": 3,
        "application_integrator": 3,
    } or pack_routes != {
        "deterministic",
        "prompt_assisted",
    }:
        fail("outside-adopter journey coverage is incomplete")
    return value, content


def evidence_reports(
    release_manifest_sha256: str,
    starter_receipts: dict[str, dict[str, Any]],
    adopter: dict[str, Any],
) -> dict[str, bytes]:
    starter_modes = sorted(
        {receipt["distribution"]["mode"] for receipt in starter_receipts.values()}
    )
    starter_profiles = sorted(
        {receipt["distribution"]["profile"] for receipt in starter_receipts.values()}
    )
    rows = {
        "starter-distribution-and-custom-pack-recovery": {
            "checks": {
                "custom_pack_exact_recovery": True,
                "no_authority_or_secret_transfer": True,
                "offline_sigstore_verification": True,
                "official_subject_inventory": True,
            },
            "facts": {
                "modes": starter_modes,
                "profiles": starter_profiles,
                "starter_count": len(starter_receipts),
            },
        },
        "outside-adopter-pack-author-journey": {
            "checks": {
                "deterministic_authoring_route": True,
                "prompt_assisted_authoring_route": True,
                "released_artifact_time_bound": adopter["maximum_pack_author_seconds"]
                <= 3600,
                "three_distinct_non_contributors": adopter["pack_author_count"] == 3,
            },
            "facts": {
                "profiles": adopter["installation_profiles"],
                "trial_count": adopter["pack_author_count"],
            },
        },
        "outside-adopter-application-integrator-journey": {
            "checks": {
                "independently_controlled_agents": True,
                "official_negotiate_unchanged": True,
                "released_artifact_time_bound": adopter[
                    "maximum_application_integrator_seconds"
                ]
                <= 1800,
                "three_distinct_non_contributors": adopter[
                    "application_integrator_count"
                ]
                == 3,
            },
            "facts": {
                "profiles": adopter["installation_profiles"],
                "trial_count": adopter["application_integrator_count"],
            },
        },
    }
    reports: dict[str, bytes] = {}
    for evidence_id in QUALIFICATION_IDS:
        row = rows[evidence_id]
        if any(value is not True for value in row["checks"].values()):
            fail(f"qualification evidence did not pass: {evidence_id}")
        reports[evidence_id] = canonical_json(
            {
                "schema": EVIDENCE_SCHEMA,
                "evidence_id": evidence_id,
                "status": "passed",
                "release_manifest_sha256": release_manifest_sha256,
                **row,
            }
        )
    return reports


def copy_atomic(path: Path, destination: Path, label: str, maximum: int) -> bytes:
    content = regular_bytes(path, label, maximum)
    atomic_write(destination, content)
    return content


def atomic_write(path: Path, content: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists() or path.is_symlink():
        fail(f"refusing to overwrite qualification material: {path}")
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


def assemble(args: argparse.Namespace) -> Path:
    release, release_bytes, release_signature = validate_release_inputs(
        args.release_dir
    )
    release_digest = sha256(release_bytes)
    starters = parse_starters(args.starter)
    receipts, starter_contents = validate_starters(
        starters, release_digest, authenticate=True
    )
    adopter, adopter_bytes = validate_adopters(
        args.adopter_qualification, release_digest
    )
    reports = evidence_reports(release_digest, receipts, adopter)
    if args.output_dir.exists() and args.output_dir.is_symlink():
        fail("qualification output directory must not be a symlink")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    if any(args.output_dir.iterdir()):
        fail("qualification output directory must be empty")
    atomic_write(args.output_dir / "release/release-manifest.json", release_bytes)
    atomic_write(args.output_dir / "release/sigstore.bundle.json", release_signature)
    artifact_paths: dict[str, str] = {}
    artifact_digests: dict[str, str] = {}
    for identity, source in sorted(starters.items()):
        filename = source.name
        safe_relative(filename, f"Starter filename {identity}")
        relative = f"starters/{identity}/{filename}"
        atomic_write(args.output_dir / relative, starter_contents[identity])
        artifact_paths[identity] = relative
        artifact_digests[identity] = sha256_ref(starter_contents[identity])
    atomic_write(
        args.output_dir / "inputs/outside-adopter-qualification.json", adopter_bytes
    )
    evidence_paths: dict[str, str] = {}
    evidence_digests: dict[str, str] = {}
    for evidence_id, content in reports.items():
        relative = f"evidence/{evidence_id}.json"
        atomic_write(args.output_dir / relative, content)
        evidence_paths[evidence_id] = relative
        evidence_digests[evidence_id] = sha256_ref(content)
    value = {
        "schema": SCHEMA,
        "product": release.get("product"),
        "release_manifest": {
            "path": "release/release-manifest.json",
            "sha256": sha256_ref(release_bytes),
            "signature_path": "release/sigstore.bundle.json",
        },
        "artifacts": artifact_paths,
        "artifact_digests": artifact_digests,
        "inputs": {
            "outside-adopter-qualification": "inputs/outside-adopter-qualification.json"
        },
        "input_digests": {"outside-adopter-qualification": sha256_ref(adopter_bytes)},
        "evidence": evidence_paths,
        "evidence_digests": evidence_digests,
        "verification_material": {
            "qualification-sigstore-bundle": {
                "path": "release-qualification-manifest.bundle.json"
            }
        },
    }
    destination = args.output_dir / "release-qualification-manifest.json"
    atomic_write(destination, canonical_json(value))
    validate_layout(args.output_dir, require_signature=False)
    return destination


def validate_layout(root: Path, *, require_signature: bool) -> dict[str, Any]:
    manifest_bytes = regular_bytes(
        root / "release-qualification-manifest.json",
        "qualification manifest",
        MAX_JSON_BYTES,
    )
    value = strict_json(manifest_bytes, "qualification manifest")
    if (
        set(value)
        != {
            "schema",
            "product",
            "release_manifest",
            "artifacts",
            "artifact_digests",
            "inputs",
            "input_digests",
            "evidence",
            "evidence_digests",
            "verification_material",
        }
        or value.get("schema") != SCHEMA
    ):
        fail("qualification manifest shape is invalid")
    release = value["release_manifest"]
    if not isinstance(release, dict) or set(release) != {
        "path",
        "sha256",
        "signature_path",
    }:
        fail("qualification release-manifest binding is invalid")
    release_path = root / safe_relative(release["path"], "release manifest path")
    release_signature = root / safe_relative(
        release["signature_path"], "release signature path"
    )
    release_bytes = regular_bytes(
        release_path, "bound release manifest", MAX_JSON_BYTES
    )
    if release.get("sha256") != sha256_ref(release_bytes):
        fail("bound release manifest bytes were substituted")
    if require_signature:
        try:
            ASSEMBLER.verify_sigstore_signature(
                release_path, release_signature, "primary release manifest"
            )
        except ASSEMBLER.AssemblyError as error:
            raise QualificationError(str(error)) from error
    for paths_key, digests_key in (
        ("artifacts", "artifact_digests"),
        ("inputs", "input_digests"),
        ("evidence", "evidence_digests"),
    ):
        paths = value[paths_key]
        digests = value[digests_key]
        if (
            not isinstance(paths, dict)
            or not isinstance(digests, dict)
            or set(paths) != set(digests)
        ):
            fail(f"qualification {paths_key} inventory is not closed")
        for identity, relative in paths.items():
            safe_id(identity, f"qualification {paths_key} id")
            content = regular_bytes(
                root / safe_relative(relative, f"{identity} path"),
                identity,
                MAX_ARTIFACT_BYTES,
            )
            if digests[identity] != sha256_ref(content):
                fail(f"qualification subject bytes were substituted: {identity}")
    if set(value["evidence"]) != set(QUALIFICATION_IDS):
        fail("qualification evidence inventory is incomplete")
    starter_paths = {
        identity: root / safe_relative(relative, f"Starter {identity} path")
        for identity, relative in value["artifacts"].items()
    }
    starter_receipts, _starter_contents = validate_starters(
        starter_paths, sha256(release_bytes), authenticate=require_signature
    )
    adopter_path = root / safe_relative(
        value["inputs"].get("outside-adopter-qualification"),
        "outside-adopter qualification path",
    )
    adopter, _adopter_bytes = validate_adopters(adopter_path, sha256(release_bytes))
    expected_reports = evidence_reports(
        sha256(release_bytes), starter_receipts, adopter
    )
    for evidence_id, relative in value["evidence"].items():
        report_bytes = regular_bytes(root / relative, evidence_id, MAX_JSON_BYTES)
        report = strict_json(report_bytes, evidence_id)
        if (
            report.get("schema") != EVIDENCE_SCHEMA
            or report.get("evidence_id") != evidence_id
            or report.get("status") != "passed"
            or report.get("release_manifest_sha256") != sha256(release_bytes)
            or not isinstance(report.get("checks"), dict)
            or any(result is not True for result in report["checks"].values())
            or report_bytes != expected_reports[evidence_id]
        ):
            fail(f"qualification evidence is invalid: {evidence_id}")
    signature = value.get("verification_material", {}).get(
        "qualification-sigstore-bundle"
    )
    if not isinstance(signature, dict) or set(signature) != {"path"}:
        fail("qualification signature declaration is invalid")
    signature_path = root / safe_relative(
        signature["path"], "qualification signature path"
    )
    if require_signature:
        try:
            ASSEMBLER.verify_sigstore_signature(
                root / "release-qualification-manifest.json",
                signature_path,
                "release qualification manifest",
            )
        except ASSEMBLER.AssemblyError as error:
            raise QualificationError(str(error)) from error
    elif signature_path.exists() or signature_path.is_symlink():
        fail("unsigned qualification assembly must not contain a signature")
    expected_files = {
        "release-qualification-manifest.json",
        release["path"],
        release["signature_path"],
        *value["artifacts"].values(),
        *value["inputs"].values(),
        *value["evidence"].values(),
    }
    if require_signature:
        expected_files.add(signature["path"])
    observed_files: set[str] = set()
    allowed_directories = {
        str(PurePosixPath(relative).parent)
        for relative in expected_files
        if str(PurePosixPath(relative).parent) != "."
    }
    allowed_directories |= {
        str(parent)
        for relative in tuple(allowed_directories)
        for parent in PurePosixPath(relative).parents
        if str(parent) != "."
    }
    for candidate in root.rglob("*"):
        relative = candidate.relative_to(root).as_posix()
        if candidate.is_symlink():
            fail(f"qualification layout contains a symlink: {relative}")
        if candidate.is_dir():
            if relative not in allowed_directories:
                fail(f"qualification layout contains an extra directory: {relative}")
        elif candidate.is_file():
            observed_files.add(relative)
        else:
            fail(f"qualification layout contains a special entry: {relative}")
    if observed_files != expected_files:
        fail(
            "qualification file inventory is not closed: "
            f"missing={sorted(expected_files - observed_files)}; "
            f"extra={sorted(observed_files - expected_files)}"
        )
    return value


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    subcommands = command.add_subparsers(dest="command", required=True)
    assemble_command = subcommands.add_parser("assemble")
    assemble_command.add_argument("--release-dir", type=Path, required=True)
    assemble_command.add_argument("--starter", action="append", default=[])
    assemble_command.add_argument("--adopter-qualification", type=Path, required=True)
    assemble_command.add_argument("--output-dir", type=Path, required=True)
    verify_command = subcommands.add_parser("verify")
    verify_command.add_argument("--qualification-dir", type=Path, required=True)
    verify_command.add_argument("--structural-only", action="store_true")
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "assemble":
            destination = assemble(args)
            print(f"assembled unsigned post-sign qualification manifest: {destination}")
            return 0
        validate_layout(
            args.qualification_dir, require_signature=not args.structural_only
        )
        if args.structural_only:
            print(
                "qualification structure verified; signature verification intentionally incomplete"
            )
            return 11
        print("release qualification manifest and both signature levels verified")
        return 0
    except (QualificationError, OSError, KeyError, TypeError, ValueError) as error:
        print(f"release qualification failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

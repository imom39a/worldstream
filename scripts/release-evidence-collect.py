#!/usr/bin/env python3
"""Collect and normalize explicitly produced release evidence reports.

The compatibility pair owns the release-gated evidence IDs.  This command
binds each ID to one explicitly supplied source report; it never discovers
reports by filename, infers success from a process exit code, or turns an
existing file into evidence.  All source reports are validated before any
normalized report is written.

The named checks are producer attestations.  The collector validates their
schema, exact keys, truth values, and binding to the release contract; it does
not independently establish the truth of a generic boolean check.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import stat
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import tomllib

ROOT = Path(__file__).resolve().parents[1]
BUILD_IDENTITY_PATH = ROOT / "scripts/release_build_identity.py"
DEFAULT_MANIFEST_TOML = ROOT / "compatibility.toml"
DEFAULT_MANIFEST_JSON = ROOT / "compatibility.json"
SOURCE_SCHEMA_PREFIX = "worldstream/release-evidence/"
NORMALIZED_SCHEMA = "worldstream/release-evidence-report/v1"
PRE_SIGN_PHASE = "pre-sign"
REQUIRED_RELEASE_EVIDENCE_COUNT = 14
STATUS_VALUES = frozenset({"pass", "passed", "failed", "unavailable", "incomplete"})
EXPECTED_SOURCE_FIELDS = {
    "schema",
    "source_id",
    "evidence_id",
    "status",
    "release_evidence",
    "fail_closed",
    "version",
    "platform",
    "contract",
    "checks",
}
OPTIONAL_SOURCE_FIELDS = {"details"}
PRODUCER_DETAIL_FIELDS = {"producer_id", "phase", "outcomes", "artifacts"}
OPTIONAL_PRODUCER_DETAIL_FIELDS = {"missing_producer"}
REQUIRED_ARTIFACT_BINDINGS = {
    "manifest-contract": ("compatibility-pair",),
    "sqlite-conformance": ("sqlite-conformance-result",),
    "postgres-conformance": ("postgres-conformance-result",),
    "migration-history": ("migration-history-result",),
    "transfer": ("transfer-result",),
    "restore": ("restore-result",),
    "native-linux": ("linux-release-profile",),
    "native-windows": ("windows-release-profile",),
    "oci-linux": ("oci-release-profile",),
    "macos-source": ("macos-quickstart",),
    "security-observability": ("security-observability",),
    "supply-chain": (
        "subject-inventory",
        "subject-signature",
        "checksums",
        "spdx-sbom",
        "slsa-provenance",
    ),
    "failure-soak": ("failure-soak", "linux-release-profile"),
    "reference-performance": ("reference-performance", "linux-release-profile"),
}
EXPECTED_PRODUCER_IDS = {
    "manifest-contract": "conformance/manifest-contract/v1",
    "sqlite-conformance": "conformance/sqlite-conformance/v1",
    "postgres-conformance": "conformance/postgres-conformance/v1",
    "migration-history": "conformance/migration-history/v1",
    "transfer": "conformance/transfer/v1",
    "restore": "conformance/restore/v1",
    "native-linux": "platform-security/native-linux/v1",
    "native-windows": "platform-security/native-windows/v1",
    "oci-linux": "platform-security/oci-linux/v1",
    "macos-source": "platform-security/macos-source/v1",
    "security-observability": "platform-security/security-observability/v1",
    "supply-chain": "release-supply-chain-pre-sign-v1",
    "failure-soak": "linux-failure-soak-release-v1",
    "reference-performance": "reference-performance-publication/v1",
}


@dataclass(frozen=True)
class SourceSpec:
    source_id: str
    evidence_id: str
    platform: str
    checks: tuple[str, ...]

    @property
    def schema(self) -> str:
        return f"{SOURCE_SCHEMA_PREFIX}{self.evidence_id}/v1"


# This is deliberately code-owned rather than inferred from report filenames.
# A release workflow must name every source with --source SOURCE_ID=PATH.
SOURCE_SPECS = (
    SourceSpec(
        "manifest-contract",
        "manifest-syntax-parity",
        "all-supported-platforms",
        (
            "toml_json_semantic_equal",
            "canonical_json_mirror",
            "release_contract_complete",
        ),
    ),
    SourceSpec(
        "sqlite-conformance",
        "sqlite-conformance-migration-backup-restore-crash",
        "native-linux-x86_64",
        (
            "migration_history",
            "backup_restore",
            "crash_recovery",
            "canonical_hash_parity",
        ),
    ),
    SourceSpec(
        "postgres-conformance",
        "postgresql-direct-and-transaction-pooler-conformance",
        "native-linux-x86_64",
        (
            "postgres_version",
            "direct_runtime",
            "transaction_pooler",
            "adapter_conformance",
        ),
    ),
    SourceSpec(
        "migration-history",
        "all-prior-forward-migrations-both-backends",
        "native-linux-x86_64",
        ("sqlite_forward_history", "postgresql_forward_history", "migration_checksums"),
    ),
    SourceSpec(
        "transfer",
        "sqlite-postgresql-transfer-byte-parity-and-epoch-fencing",
        "native-linux-x86_64",
        ("byte_parity", "checkpoint_resume", "epoch_fencing", "finalization"),
    ),
    SourceSpec(
        "restore",
        "backend-native-isolated-restore-and-bounded-semantic-verifier",
        "native-linux-x86_64",
        (
            "sqlite_isolated_restore",
            "postgresql_isolated_restore",
            "bounded_fixture_semantic_verifier",
        ),
    ),
    SourceSpec(
        "native-linux",
        "native-linux-release-profile",
        "native-linux-x86_64",
        ("archive_identity", "runtime_smoke", "filesystem_policy"),
    ),
    SourceSpec(
        "native-windows",
        "native-windows-release-profile",
        "native-windows-x64",
        ("archive_identity", "runtime_smoke", "windows_acl_and_reparse_policy"),
    ),
    SourceSpec(
        "oci-linux",
        "oci-linux-amd64-release-profile",
        "oci-linux-amd64",
        ("pinned_base_image", "image_digest", "runtime_smoke", "filesystem_policy"),
    ),
    SourceSpec(
        "macos-source",
        "macos-source-quickstart",
        "macos-source",
        ("pinned_toolchain", "source_build", "quickstart"),
    ),
    SourceSpec(
        "security-observability",
        "config-secrets-probes-observability-security",
        "all-supported-platforms",
        (
            "config_validation",
            "secret_redaction",
            "observability_bounds",
            "security_probes",
        ),
    ),
    SourceSpec(
        "supply-chain",
        "checksums-signature-sbom-provenance",
        "release-assembly-linux-x86_64",
        ("checksums", "sigstore_identity", "spdx_subjects", "slsa_subjects"),
    ),
    SourceSpec(
        "failure-soak",
        "failure-fuzz-resource-and-one-hour-sqlite-soak",
        "native-linux-x86_64",
        ("failure_matrix", "resource_bounds", "one_hour_soak", "retained_logs"),
    ),
    SourceSpec(
        "reference-performance",
        "reference-performance-per-backend",
        "native-linux-x86_64",
        (
            "packaged_workload_identity",
            "sqlite_measurements",
            "postgresql_measurements",
            "non_sla_publication",
        ),
    ),
)
SOURCE_BY_ID = {spec.source_id: spec for spec in SOURCE_SPECS}
EVIDENCE_BY_ID = {spec.evidence_id: spec for spec in SOURCE_SPECS}
SOURCE_TO_EVIDENCE = {spec.source_id: spec.evidence_id for spec in SOURCE_SPECS}


class CollectionError(RuntimeError):
    """A fail-closed collector input or output error."""


def fail(message: str) -> None:
    raise CollectionError(message)


def load_build_identity():
    """Load the repository-wide strict JSON and release identity helpers."""

    name = "worldstream_release_evidence_strict_json"
    existing = sys.modules.get(name)
    if existing is not None:
        return existing
    spec = importlib.util.spec_from_file_location(name, BUILD_IDENTITY_PATH)
    if spec is None or spec.loader is None:  # pragma: no cover
        fail(f"cannot load strict release JSON parser: {BUILD_IDENTITY_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


BUILD_IDENTITY = load_build_identity()


def strict_json_object(raw: bytes, label: str) -> dict[str, Any]:
    """Parse a release JSON object while rejecting every duplicate key."""

    try:
        return BUILD_IDENTITY.strict_json(raw, label)
    except BUILD_IDENTITY.IdentityError as error:
        fail(str(error))


def bounded_regular_bytes(path: Path, label: str) -> bytes:
    """Read a release control document without permitting unbounded allocation."""

    try:
        return BUILD_IDENTITY.regular_bytes(
            path,
            label,
            maximum=BUILD_IDENTITY.MAX_RELEASE_JSON_BYTES,
        )
    except BUILD_IDENTITY.IdentityError as error:
        fail(str(error))


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


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


def load_manifest(toml_path: Path, json_path: Path) -> dict[str, Any]:
    if toml_path.name != "compatibility.toml" or json_path.name != "compatibility.json":
        fail("collector requires compatibility.toml and compatibility.json filenames")
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
    mirror = strict_json_object(mirror_bytes, "compatibility.json")
    if not isinstance(authored, dict) or not isinstance(mirror, dict):
        fail("compatibility manifest pair must contain objects")
    if authored != mirror:
        fail("compatibility.toml and compatibility.json differ semantically")
    if mirror_bytes != canonical_json(authored):
        fail("compatibility.json is not the deterministic sorted-key mirror")
    return authored


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
        ids.append(evidence_id)
    if len(ids) != REQUIRED_RELEASE_EVIDENCE_COUNT:
        fail(
            "collector requires exactly "
            f"{REQUIRED_RELEASE_EVIDENCE_COUNT} release-gated evidence IDs; "
            f"found {len(ids)}"
        )
    return tuple(sorted(ids))


def validate_mapping(manifest_ids: tuple[str, ...]) -> tuple[SourceSpec, ...]:
    mapped_ids = tuple(sorted(EVIDENCE_BY_ID))
    if manifest_ids != mapped_ids:
        missing = sorted(set(manifest_ids) - set(mapped_ids))
        extra = sorted(set(mapped_ids) - set(manifest_ids))
        fail(
            f"source-to-evidence mapping does not match manifest: missing={missing}; extra={extra}"
        )
    return tuple(EVIDENCE_BY_ID[evidence_id] for evidence_id in manifest_ids)


def parse_sources(values: list[str], specs: tuple[SourceSpec, ...]) -> dict[str, Path]:
    expected = {spec.source_id for spec in specs}
    parsed: dict[str, Path] = {}
    resolved_paths: dict[Path, str] = {}
    for value in values:
        if "=" not in value:
            fail(f"source must use SOURCE_ID=PATH syntax: {value!r}")
        source_id, raw_path = value.split("=", 1)
        if source_id not in expected:
            fail(f"unknown source ID: {source_id}")
        if source_id in parsed:
            fail(f"duplicate source ID: {source_id}")
        path = Path(raw_path)
        regular_file(path, f"source report {source_id}")
        resolved = path.resolve()
        if resolved in resolved_paths:
            fail(
                f"duplicate source report: {source_id} and {resolved_paths[resolved]} both use {path}"
            )
        parsed[source_id] = path
        resolved_paths[resolved] = source_id
    missing = sorted(expected - set(parsed))
    if missing:
        fail("missing explicit source reports: " + ", ".join(missing))
    if len(parsed) != len(expected):
        fail(
            f"expected {len(expected)} explicit source reports; received {len(parsed)}"
        )
    return parsed


def checked_status(value: object, label: str) -> str:
    if not isinstance(value, str):
        fail(f"{label} status must be a string")
    if value not in STATUS_VALUES:
        fail(f"{label} status is invalid: {value!r}")
    return value


def validate_producer_details(value: dict[str, Any], spec: SourceSpec) -> None:
    details = value.get("details")
    if not isinstance(details, dict):
        fail(f"source report {spec.source_id} requires typed producer details")
    unknown = sorted(
        set(details) - PRODUCER_DETAIL_FIELDS - OPTIONAL_PRODUCER_DETAIL_FIELDS
    )
    missing = sorted(PRODUCER_DETAIL_FIELDS - set(details))
    if missing or unknown:
        fail(
            f"source report {spec.source_id} has wrong producer detail fields: "
            f"missing={missing}; extra={unknown}"
        )
    producer_id = details.get("producer_id")
    expected_producer_id = EXPECTED_PRODUCER_IDS[spec.source_id]
    if producer_id != expected_producer_id:
        fail(
            f"source report {spec.source_id} producer_id must be "
            f"{expected_producer_id!r}; observed={producer_id!r}"
        )
    if details.get("phase") != PRE_SIGN_PHASE:
        fail(
            f"source report {spec.source_id} must use phase={PRE_SIGN_PHASE!r}; "
            f"observed={details.get('phase')!r}"
        )
    outcomes = details.get("outcomes")
    if not isinstance(outcomes, dict) or set(outcomes) != set(spec.checks):
        fail(
            f"source report {spec.source_id} producer outcomes must exactly match checks"
        )
    for check in spec.checks:
        outcome = outcomes[check]
        if not isinstance(outcome, dict) or set(outcome) != {"status", "observations"}:
            fail(f"source report {spec.source_id} outcome {check} has wrong fields")
        if (
            checked_status(
                outcome.get("status"), f"source report {spec.source_id} outcome {check}"
            )
            != "passed"
        ):
            fail(f"source report {spec.source_id} outcome {check} is not passed")
        observations = outcome.get("observations")
        if not isinstance(observations, list) or not observations:
            fail(f"source report {spec.source_id} outcome {check} has no observations")
        for observation in observations:
            if (
                not isinstance(observation, dict)
                or set(observation) != {"kind", "value"}
                or not isinstance(observation["kind"], str)
                or not observation["kind"]
                or not isinstance(observation["value"], str)
                or not observation["value"]
            ):
                fail(
                    f"source report {spec.source_id} outcome {check} has invalid observations"
                )
    artifacts = details.get("artifacts")
    required = set(REQUIRED_ARTIFACT_BINDINGS[spec.source_id])
    if not isinstance(artifacts, dict) or set(artifacts) != required:
        fail(
            f"source report {spec.source_id} artifact bindings must exactly be "
            f"{sorted(required)}"
        )
    for binding_id, artifact in artifacts.items():
        if not isinstance(artifact, dict) or set(artifact) != {"sha256", "size_bytes"}:
            fail(
                f"source report {spec.source_id} artifact {binding_id} has wrong fields"
            )
        digest = artifact.get("sha256")
        if (
            not isinstance(digest, str)
            or len(digest) != len("sha256:") + 64
            or not digest.startswith("sha256:")
            or any(character not in "0123456789abcdef" for character in digest[7:])
        ):
            fail(
                f"source report {spec.source_id} artifact {binding_id} has invalid SHA-256"
            )
        if type(artifact.get("size_bytes")) is not int or artifact["size_bytes"] < 0:
            fail(
                f"source report {spec.source_id} artifact {binding_id} has invalid size_bytes"
            )


def source_report(
    path: Path, spec: SourceSpec, manifest: dict[str, Any]
) -> dict[str, Any]:
    raw = bounded_regular_bytes(path, f"source report {spec.source_id}")
    value = strict_json_object(raw, f"source report {spec.source_id}")
    unknown = sorted(set(value) - EXPECTED_SOURCE_FIELDS - OPTIONAL_SOURCE_FIELDS)
    missing = sorted(EXPECTED_SOURCE_FIELDS - set(value))
    if missing or unknown:
        fail(
            f"source report {spec.source_id} has wrong fields: missing={missing}; extra={unknown}"
        )
    if value.get("schema") != spec.schema:
        fail(
            f"source report {spec.source_id} has wrong schema: "
            f"expected={spec.schema!r}; observed={value.get('schema')!r}"
        )
    if value.get("source_id") != spec.source_id:
        fail(f"source report {spec.source_id} has mismatched source_id")
    if value.get("evidence_id") != spec.evidence_id:
        fail(f"source report {spec.source_id} has mismatched evidence_id")
    status = checked_status(value.get("status"), f"source report {spec.source_id}")
    if status != "passed":
        fail(
            f"source report {spec.source_id} is not passed: status={value.get('status')!r}"
        )
    if value.get("release_evidence") is not True:
        fail(
            f"source report {spec.source_id} does not explicitly claim release_evidence=true"
        )
    if value.get("fail_closed") is not False:
        fail(f"source report {spec.source_id} must explicitly set fail_closed=false")
    version = manifest.get("release_candidate")
    if value.get("version") != version:
        fail(
            f"source report {spec.source_id} has wrong version: "
            f"expected={version!r}; observed={value.get('version')!r}"
        )
    if value.get("platform") != spec.platform:
        fail(
            f"source report {spec.source_id} has wrong platform: "
            f"expected={spec.platform!r}; observed={value.get('platform')!r}"
        )
    contract = manifest.get("contracts")
    if not isinstance(contract, dict) or value.get("contract") != contract:
        fail(f"source report {spec.source_id} has the wrong compatibility contract")
    checks = value.get("checks")
    if not isinstance(checks, dict):
        fail(f"source report {spec.source_id} checks must be an object")
    if set(checks) != set(spec.checks):
        fail(
            f"source report {spec.source_id} has wrong checks: "
            f"expected={sorted(spec.checks)}; observed={sorted(checks)}"
        )
    if any(result is not True for result in checks.values()):
        fail(f"source report {spec.source_id} contains a failed or incomplete check")
    validate_producer_details(value, spec)
    if value["checks"] != {
        check: value["details"]["outcomes"][check]["status"] == "passed"
        for check in spec.checks
    }:
        fail(
            f"source report {spec.source_id} checks are not derived from producer outcomes"
        )
    return {"raw": raw, "value": value}


def normalized_report(
    spec: SourceSpec, manifest: dict[str, Any], loaded: dict[str, Any]
) -> dict[str, Any]:
    value = loaded["value"]
    result: dict[str, Any] = {
        "schema": NORMALIZED_SCHEMA,
        "evidence_id": spec.evidence_id,
        "release_gate": True,
        "release_evidence": True,
        "fail_closed": False,
        "status": "passed",
        "summary": {"failures": 0, "incomplete_skips": 0},
        "version": manifest["release_candidate"],
        "platform": spec.platform,
        "contract": manifest["contracts"],
        "checks": value["checks"],
        "producer_details": value["details"],
        "source_report": {
            "source_id": spec.source_id,
            "evidence_id": spec.evidence_id,
            "schema": value["schema"],
            "sha256": "sha256:" + sha256_bytes(loaded["raw"]),
        },
    }
    return result


def validate_output_dir(output_dir: Path, evidence_ids: tuple[str, ...]) -> None:
    if output_dir.exists() and output_dir.is_symlink():
        fail(f"output directory must not be a symlink: {output_dir}")
    output_dir.mkdir(parents=True, exist_ok=True)
    expected = {evidence_id + ".json" for evidence_id in evidence_ids}
    for candidate in sorted(output_dir.rglob("*")):
        relative = candidate.relative_to(output_dir).as_posix()
        if candidate.is_symlink():
            fail(f"output directory contains a symlink: {relative}")
        if candidate.is_dir():
            fail(f"output directory contains an unexpected directory: {relative}")
        if not candidate.is_file() or relative not in expected:
            fail(f"output directory contains an unexpected file: {relative}")


def atomic_write(path: Path, content: bytes) -> None:
    if path.is_symlink():
        fail(f"output report must not be a symlink: {path}")
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


def collect(
    output_dir: Path,
    sources: dict[str, Path],
    manifest_toml: Path = DEFAULT_MANIFEST_TOML,
    manifest_json: Path = DEFAULT_MANIFEST_JSON,
) -> tuple[str, ...]:
    manifest = load_manifest(manifest_toml, manifest_json)
    evidence_ids = release_evidence_ids(manifest)
    specs = validate_mapping(evidence_ids)
    expected_sources = {spec.source_id for spec in specs}
    if set(sources) != expected_sources:
        fail(
            "source map must contain exactly the mapped source IDs: "
            f"missing={sorted(expected_sources - set(sources))}; "
            f"extra={sorted(set(sources) - expected_sources)}"
        )
    resolved: dict[Path, str] = {}
    source_paths: dict[str, Path] = {}
    for spec in specs:
        path = regular_file(
            Path(sources[spec.source_id]), f"source report {spec.source_id}"
        )
        real_path = path.resolve()
        if real_path in resolved:
            fail(
                f"duplicate source report: {spec.source_id} and {resolved[real_path]} both use {path}"
            )
        resolved[real_path] = spec.source_id
        source_paths[spec.source_id] = path
    normalized: dict[str, bytes] = {}
    for spec in specs:
        path = source_paths[spec.source_id]
        loaded = source_report(path, spec, manifest)
        normalized[spec.evidence_id] = canonical_json(
            normalized_report(spec, manifest, loaded)
        )
    validate_output_dir(output_dir, evidence_ids)
    for evidence_id in evidence_ids:
        atomic_write(output_dir / f"{evidence_id}.json", normalized[evidence_id])
    return evidence_ids


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--output-dir", type=Path, required=True)
    command.add_argument(
        "--source", action="append", default=[], metavar="SOURCE_ID=PATH"
    )
    command.add_argument("--manifest-toml", type=Path, default=DEFAULT_MANIFEST_TOML)
    command.add_argument("--manifest-json", type=Path, default=DEFAULT_MANIFEST_JSON)
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        manifest = load_manifest(args.manifest_toml, args.manifest_json)
        specs = validate_mapping(release_evidence_ids(manifest))
        sources = parse_sources(args.source, specs)
        evidence_ids = collect(
            args.output_dir, sources, args.manifest_toml, args.manifest_json
        )
    except CollectionError as error:
        print(f"release evidence collection failed: {error}", file=sys.stderr)
        return 1
    print(
        f"collected {len(evidence_ids)} normalized release evidence reports into {args.output_dir}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

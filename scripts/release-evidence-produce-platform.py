#!/usr/bin/env python3
"""Produce strict typed evidence for the platform and security sources.

Each report and artifact is named explicitly by the caller.  The diagnostic
reports are attestations from the platform checks; this command validates
their identity and release contract, hashes the supplied artifact bytes, and
only emits a releasable producer result when every required fact is present.
Anything missing, unsupported, malformed, skipped, or contradictory becomes
an explicit fail-closed result.  No result is promoted until every source has
been checked.
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

ROOT = Path(__file__).resolve().parents[1]
COLLECTOR_PATH = ROOT / "scripts/release-evidence-collect.py"
PRODUCER_SCHEMA = "worldstream/release-evidence-producer/v1"
DIAGNOSTIC_SCHEMA = "worldstream/release-platform-diagnostic/v1"
HASH_PREFIX = "sha256:"
DIAGNOSTIC_FIELDS = {
    "schema",
    "report_id",
    "source_id",
    "platform",
    "version",
    "contract",
    "status",
    "release_evidence",
    "fail_closed",
    "facts",
}


def load_collector():
    spec = importlib.util.spec_from_file_location(
        "release_evidence_collect_platform", COLLECTOR_PATH
    )
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load collector: {COLLECTOR_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


COLLECTOR = load_collector()

# The report kinds are deliberately explicit.  They are the five platform /
# security diagnostics requested by the release contract, not filenames that
# this command discovers.  The identity, checks, and artifact binding below
# are checked against the collector at runtime so drift cannot be silently
# accepted by this producer.
PLATFORM_SPECS: dict[str, dict[str, Any]] = {
    "native-linux": {
        "evidence_id": "native-linux-release-profile",
        "platform": "native-linux-x86_64",
        "reports": (
            "package",
            "runtime",
            "filesystem",
            "native-postgres-restore",
        ),
        "checks": ("archive_identity", "runtime_smoke", "filesystem_policy"),
    },
    "native-windows": {
        "evidence_id": "native-windows-release-profile",
        "platform": "native-windows-x64",
        "reports": (
            "package",
            "runtime",
            "acl-or-reparse",
            "native-postgres-restore",
        ),
        "checks": (
            "archive_identity",
            "runtime_smoke",
            "windows_acl_and_reparse_policy",
        ),
    },
    "oci-linux": {
        "evidence_id": "oci-linux-amd64-release-profile",
        "platform": "oci-linux-amd64",
        "reports": ("oci",),
        "checks": (
            "pinned_base_image",
            "image_digest",
            "runtime_smoke",
            "filesystem_policy",
        ),
    },
    "macos-source": {
        "evidence_id": "macos-source-quickstart",
        "platform": "macos-source",
        "reports": ("source-quickstart",),
        "checks": ("pinned_toolchain", "source_build", "quickstart"),
    },
    "security-observability": {
        "evidence_id": "config-secrets-probes-observability-security",
        "platform": "all-supported-platforms",
        "reports": ("config-redaction-observability", "security"),
        "checks": (
            "config_validation",
            "secret_redaction",
            "observability_bounds",
            "security_probes",
        ),
    },
}

NATIVE_POSTGRES_RESTORE_FACTS = (
    "platform_identity",
    "release_candidate",
    "native_restore_report_sha256",
    "native_restore_report_size_bytes",
    "release_archive_sha256",
    "release_archive_size_bytes",
    "package_report_sha256",
    "package_report_size_bytes",
    "packaged_runtime_report_sha256",
    "packaged_runtime_report_size_bytes",
    "provider_identity",
    "restore_profile",
    "verified_scope",
    "target_safety",
    "source_durable_domains_digest",
    "backup_id",
    "native_point_digest",
    "native_dump_digest",
    "native_dump_size_bytes",
    "source_provider_identity",
    "target_provider_identity",
    "fixture_classification",
    "fixture_report_sha256",
    "fixture_report_size_bytes",
    "source_revision",
    "packaged_control_sha256",
    "packaged_control_size_bytes",
    "native_raw_report_sha256",
    "native_raw_report_size_bytes",
    "native_product_execution_sha256",
    "native_provider_tools_sha256",
    "native_cleanup_sha256",
)

# A report is not evidence merely because its JSON is well formed.  Every
# diagnostic must state each fact needed to explain the corresponding release
# check.  Values may be structured attestations, but empty/false/skipped
# values are never accepted.
REQUIRED_FACTS: dict[tuple[str, str], tuple[str, ...]] = {
    ("native-linux", "package"): (
        "archive_identity",
        "release_candidate",
        "platform_identity",
        "artifact_path",
    ),
    ("native-linux", "runtime"): (
        "platform_identity",
        "release_candidate",
        "runtime_smoke",
        "storage_profile",
        "runtime_report_sha256",
        "packaged_binary_sha256",
        "packaged_control_sha256",
        "postgres_admin",
    ),
    ("native-linux", "filesystem"): (
        "platform_identity",
        "filesystem_policy",
        "owner_only_paths",
        "symlink_rejection",
    ),
    ("native-linux", "native-postgres-restore"): NATIVE_POSTGRES_RESTORE_FACTS,
    ("native-windows", "package"): (
        "archive_identity",
        "release_candidate",
        "platform_identity",
        "artifact_path",
    ),
    ("native-windows", "runtime"): (
        "platform_identity",
        "release_candidate",
        "runtime_smoke",
        "storage_profile",
        "runtime_report_sha256",
        "packaged_binary_sha256",
        "packaged_control_sha256",
        "postgres_admin",
    ),
    ("native-windows", "acl-or-reparse"): (
        "platform_identity",
        "acl_policy",
        "reparse_policy",
        "broad_write_rejected",
    ),
    ("native-windows", "native-postgres-restore"): NATIVE_POSTGRES_RESTORE_FACTS,
    ("oci-linux", "oci"): (
        "platform_identity",
        "release_candidate",
        "pinned_base_image",
        "image_digest",
        "tested_image_config_digest",
        "context_inventory_sha256",
        "context_report_sha256",
        "context_metadata_sha256",
        "runtime_report_sha256",
        "runtime_smoke",
        "storage_profiles",
        "postgres_provider_image",
        "postgres_engine_identity",
        "postgres_packaged_administration",
        "postgres_runtime_role",
        "postgres_network",
        "secrets",
        "filesystem_policy",
    ),
    ("macos-source", "source-quickstart"): (
        "platform_identity",
        "release_candidate",
        "architectures",
        "pinned_toolchain",
        "source_build",
        "quickstart",
    ),
    ("security-observability", "config-redaction-observability"): (
        "platform_identity",
        "config_validation",
        "secret_redaction",
        "observability_bounds",
    ),
    ("security-observability", "security"): (
        "platform_identity",
        "security_probes",
        "remote_tls",
        "filesystem_policy",
    ),
}


@dataclass(frozen=True)
class Produced:
    results: dict[str, dict[str, Any]]
    failed: tuple[str, ...]


class ProducerError(RuntimeError):
    """Input cannot be promoted to typed release evidence."""


def regular_file(path: Path, label: str) -> Path:
    try:
        mode = path.lstat().st_mode
    except FileNotFoundError as error:
        raise ProducerError(f"missing {label}: {path}") from error
    except OSError as error:
        raise ProducerError(f"cannot inspect {label} {path}: {error}") from error
    if stat.S_ISLNK(mode):
        raise ProducerError(f"{label} must not be a symlink: {path}")
    if not stat.S_ISREG(mode):
        raise ProducerError(f"{label} must be a regular file: {path}")
    return path


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def atomic_write(path: Path, content: bytes) -> None:
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


def parse_report_assignments(values: list[str]) -> dict[tuple[str, str], Path]:
    expected = {
        (source_id, kind)
        for source_id, spec in PLATFORM_SPECS.items()
        for kind in spec["reports"]
    }
    parsed: dict[tuple[str, str], Path] = {}
    for value in values:
        if "=" not in value or "/" not in value.split("=", 1)[0]:
            raise ProducerError(
                f"report must use SOURCE_ID/REPORT_KIND=PATH syntax: {value!r}"
            )
        key_text, raw_path = value.split("=", 1)
        source_id, kind = key_text.split("/", 1)
        key = (source_id, kind)
        if key not in expected:
            raise ProducerError(f"unknown report: {key_text}")
        if key in parsed:
            raise ProducerError(f"duplicate report: {key_text}")
        if not raw_path:
            raise ProducerError(f"empty report path: {key_text}")
        parsed[key] = Path(raw_path)
    return parsed


def parse_artifact_assignments(values: list[str]) -> dict[tuple[str, str], Path]:
    expected = {
        (source_id, binding)
        for source_id in PLATFORM_SPECS
        for binding in COLLECTOR.REQUIRED_ARTIFACT_BINDINGS[source_id]
    }
    parsed: dict[tuple[str, str], Path] = {}
    for value in values:
        if "=" not in value or "/" not in value.split("=", 1)[0]:
            raise ProducerError(
                f"artifact must use SOURCE_ID/BINDING_ID=PATH syntax: {value!r}"
            )
        key_text, raw_path = value.split("=", 1)
        source_id, binding = key_text.split("/", 1)
        key = (source_id, binding)
        if key not in expected:
            raise ProducerError(f"unknown artifact binding: {key_text}")
        if key in parsed:
            raise ProducerError(f"duplicate artifact binding: {key_text}")
        if not raw_path:
            raise ProducerError(f"empty artifact path: {key_text}")
        parsed[key] = Path(raw_path)
    return parsed


def verify_collector_alignment() -> None:
    source_by_id = {spec.source_id: spec for spec in COLLECTOR.SOURCE_SPECS}
    for source_id, expected in PLATFORM_SPECS.items():
        try:
            observed = source_by_id[source_id]
        except KeyError as error:
            raise ProducerError(
                f"collector is missing platform source: {source_id}"
            ) from error
        for field in ("evidence_id", "platform", "checks"):
            observed_value = (
                tuple(observed.checks)
                if field == "checks"
                else getattr(observed, field)
            )
            if observed_value != expected[field]:
                raise ProducerError(f"collector mapping drift for {source_id}: {field}")
        if not COLLECTOR.REQUIRED_ARTIFACT_BINDINGS.get(source_id):
            raise ProducerError(f"invalid artifact binding for {source_id}")


def load_manifest(manifest_toml: Path, manifest_json: Path) -> dict[str, Any]:
    try:
        manifest = COLLECTOR.load_manifest(manifest_toml, manifest_json)
        COLLECTOR.validate_mapping(COLLECTOR.release_evidence_ids(manifest))
    except Exception as error:
        if isinstance(error, ProducerError):
            raise
        raise ProducerError(f"invalid compatibility manifest: {error}") from error
    verify_collector_alignment()
    return manifest


def read_report(
    path: Path, source_id: str, kind: str, manifest: dict[str, Any]
) -> dict[str, Any]:
    regular_file(path, f"{source_id}/{kind} diagnostic report")
    try:
        raw = path.read_bytes()
    except OSError as error:
        raise ProducerError(
            f"malformed {source_id}/{kind} diagnostic report: {error}"
        ) from error
    try:
        value = COLLECTOR.strict_json_object(raw, f"{source_id}/{kind} diagnostic")
    except COLLECTOR.CollectionError as error:
        raise ProducerError(str(error)) from error
    missing = sorted(DIAGNOSTIC_FIELDS - set(value))
    extra = sorted(set(value) - DIAGNOSTIC_FIELDS)
    if missing or extra:
        raise ProducerError(
            f"{source_id}/{kind} diagnostic has wrong fields: missing={missing}; extra={extra}"
        )
    spec = PLATFORM_SPECS[source_id]
    expected_report_id = f"{source_id}/{kind}"
    if value["schema"] != DIAGNOSTIC_SCHEMA:
        raise ProducerError(f"{expected_report_id} has unsupported diagnostic schema")
    if value["report_id"] != expected_report_id or value["source_id"] != source_id:
        raise ProducerError(f"{expected_report_id} identity mismatch")
    if value["platform"] != spec["platform"]:
        raise ProducerError(f"{expected_report_id} platform identity mismatch")
    if value["version"] != manifest["release_candidate"]:
        raise ProducerError(f"{expected_report_id} release candidate mismatch")
    if value["contract"] != manifest["contracts"]:
        raise ProducerError(f"{expected_report_id} compatibility contract mismatch")
    if value["status"] != "passed":
        raise ProducerError(f"{expected_report_id} is {value['status']!r}, not passed")
    if value["release_evidence"] is not True or value["fail_closed"] is not False:
        raise ProducerError(f"{expected_report_id} is not honest release evidence")
    facts = value["facts"]
    required = set(REQUIRED_FACTS[(source_id, kind)])
    if not isinstance(facts, dict) or set(facts) != required:
        raise ProducerError(f"{expected_report_id} is missing required facts")
    if "platform_identity" in facts and facts["platform_identity"] != spec["platform"]:
        raise ProducerError(f"{expected_report_id} platform fact mismatch")
    if (
        "release_candidate" in facts
        and facts["release_candidate"] != manifest["release_candidate"]
    ):
        raise ProducerError(f"{expected_report_id} release candidate fact mismatch")
    for fact, fact_value in facts.items():
        if fact_value is False or fact_value is None or fact_value == "":
            raise ProducerError(f"{expected_report_id} fact {fact} is empty or false")
        if isinstance(fact_value, str) and fact_value.strip().lower() in {
            "skipped",
            "unavailable",
            "incomplete",
            "unsupported",
        }:
            raise ProducerError(f"{expected_report_id} fact {fact} is not evidence")
        if isinstance(fact_value, (dict, list, tuple, set)) and not fact_value:
            raise ProducerError(f"{expected_report_id} fact {fact} is empty")
    return value


def sha256_file(path: Path) -> tuple[str, int]:
    regular_file(path, "artifact")
    try:
        data = path.read_bytes()
    except OSError as error:
        raise ProducerError(f"cannot read artifact {path}: {error}") from error
    return HASH_PREFIX + hashlib.sha256(data).hexdigest(), len(data)


def result_for_failure(
    source_id: str, manifest: dict[str, Any], reason: str
) -> dict[str, Any]:
    spec = PLATFORM_SPECS[source_id]
    return {
        "schema": PRODUCER_SCHEMA,
        "producer_id": f"platform-security/{source_id}/v1",
        "evidence_id": spec["evidence_id"],
        "status": "unavailable",
        "release_evidence": False,
        "fail_closed": True,
        "phase": COLLECTOR.PRE_SIGN_PHASE,
        "version": manifest["release_candidate"],
        "platform": spec["platform"],
        "contract": manifest["contracts"],
        "outcomes": {
            check: {
                "status": "unavailable",
                "observations": [{"kind": "producer-error", "value": reason}],
            }
            for check in spec["checks"]
        },
        "artifacts": {},
        "details": {"reason": reason},
    }


def result_for_source(
    source_id: str,
    manifest: dict[str, Any],
    reports: dict[tuple[str, str], Path],
    artifacts: dict[tuple[str, str], Path],
) -> dict[str, Any]:
    spec = PLATFORM_SPECS[source_id]
    diagnostic_reports = {
        kind: read_report(reports[(source_id, kind)], source_id, kind, manifest)
        for kind in spec["reports"]
    }
    declared_bindings = tuple(COLLECTOR.REQUIRED_ARTIFACT_BINDINGS[source_id])
    supplied_bindings = tuple(
        binding for current_source, binding in artifacts if current_source == source_id
    )
    if set(supplied_bindings) != set(declared_bindings):
        raise ProducerError(
            f"artifact bindings for {source_id} must exactly match {sorted(declared_bindings)}"
        )
    artifact_values = {}
    for binding in declared_bindings:
        digest, size = sha256_file(artifacts[(source_id, binding)])
        artifact_values[binding] = {"sha256": digest, "size_bytes": size}
    observations = []
    for kind in spec["reports"]:
        report_path = reports[(source_id, kind)]
        report_digest, report_size = sha256_file(report_path)
        observations.append(
            {
                "kind": "diagnostic-report",
                "value": (
                    f"{diagnostic_reports[kind]['report_id']};"
                    f"sha256={report_digest};size_bytes={report_size}"
                ),
            }
        )
    return {
        "schema": PRODUCER_SCHEMA,
        "producer_id": f"platform-security/{source_id}/v1",
        "evidence_id": spec["evidence_id"],
        "status": "passed",
        "release_evidence": True,
        "fail_closed": False,
        "phase": COLLECTOR.PRE_SIGN_PHASE,
        "version": manifest["release_candidate"],
        "platform": spec["platform"],
        "contract": manifest["contracts"],
        "outcomes": {
            check: {"status": "passed", "observations": observations}
            for check in spec["checks"]
        },
        "artifacts": artifact_values,
    }


def produce(
    output_dir: Path,
    reports: dict[tuple[str, str], Path],
    artifacts: dict[tuple[str, str], Path],
    manifest_toml: Path = COLLECTOR.DEFAULT_MANIFEST_TOML,
    manifest_json: Path = COLLECTOR.DEFAULT_MANIFEST_JSON,
    selected_sources: set[str] | None = None,
) -> Produced:
    manifest = load_manifest(manifest_toml, manifest_json)
    sources = set(PLATFORM_SPECS) if selected_sources is None else selected_sources
    if not sources or not sources.issubset(PLATFORM_SPECS):
        raise ProducerError("selected platform sources are empty or unknown")
    expected_reports = {
        (source_id, kind)
        for source_id, spec in PLATFORM_SPECS.items()
        if source_id in sources
        for kind in spec["reports"]
    }
    unknown_reports = sorted(set(reports) - expected_reports)
    if unknown_reports:
        raise ProducerError(
            f"unknown report: {unknown_reports[0][0]}/{unknown_reports[0][1]}"
        )
    expected_artifacts = {
        (source_id, binding)
        for source_id in PLATFORM_SPECS
        if source_id in sources
        for binding in COLLECTOR.REQUIRED_ARTIFACT_BINDINGS[source_id]
    }
    unknown_artifacts = sorted(set(artifacts) - expected_artifacts)
    if unknown_artifacts:
        raise ProducerError(
            f"unknown artifact binding: {unknown_artifacts[0][0]}/{unknown_artifacts[0][1]}"
        )
    if output_dir.exists() and output_dir.is_symlink():
        raise ProducerError(f"output directory must not be a symlink: {output_dir}")
    output_dir.mkdir(parents=True, exist_ok=True)
    expected_outputs = {
        PLATFORM_SPECS[source_id]["evidence_id"] + ".json" for source_id in sources
    }
    for candidate in output_dir.iterdir():
        if candidate.is_symlink() or candidate.name not in expected_outputs:
            raise ProducerError(
                f"output directory contains an unexpected entry: {candidate.name}"
            )

    results: dict[str, dict[str, Any]] = {}
    failed: list[str] = []
    for source_id in PLATFORM_SPECS:
        if source_id not in sources:
            continue
        try:
            missing_reports = [
                kind
                for kind in PLATFORM_SPECS[source_id]["reports"]
                if (source_id, kind) not in reports
            ]
            if missing_reports:
                raise ProducerError(
                    f"missing diagnostic report: {source_id}/{missing_reports[0]}"
                )
            results[source_id] = result_for_source(
                source_id, manifest, reports, artifacts
            )
        except ProducerError as error:
            failed.append(source_id)
            results[source_id] = result_for_failure(source_id, manifest, str(error))

    for source_id, result in results.items():
        output = {key: value for key, value in result.items() if key != "details"}
        if result["fail_closed"]:
            output["details"] = result["details"]
        atomic_write(
            output_dir / f"{result['evidence_id']}.json", canonical_json(output)
        )
    return Produced(results, tuple(failed))


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--output-dir", type=Path, required=True)
    command.add_argument(
        "--report", action="append", default=[], metavar="SOURCE_ID/REPORT_KIND=PATH"
    )
    command.add_argument(
        "--artifact", action="append", default=[], metavar="SOURCE_ID/BINDING_ID=PATH"
    )
    command.add_argument(
        "--source",
        action="append",
        choices=tuple(PLATFORM_SPECS),
        help="limit production to one or more explicitly named sources",
    )
    command.add_argument(
        "--manifest-toml", type=Path, default=COLLECTOR.DEFAULT_MANIFEST_TOML
    )
    command.add_argument(
        "--manifest-json", type=Path, default=COLLECTOR.DEFAULT_MANIFEST_JSON
    )
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        reports = parse_report_assignments(args.report)
        artifacts = parse_artifact_assignments(args.artifact)
        produced = produce(
            args.output_dir,
            reports,
            artifacts,
            args.manifest_toml,
            args.manifest_json,
            set(args.source) if args.source else None,
        )
    except ProducerError as error:
        print(f"platform evidence production failed: {error}", file=sys.stderr)
        return 1
    print(
        f"produced {len(produced.results)} platform/security results; "
        f"fail-closed sources={len(produced.failed)}: "
        f"{', '.join(produced.failed) if produced.failed else 'none'}"
    )
    return 1 if produced.failed else 0


if __name__ == "__main__":
    raise SystemExit(main())

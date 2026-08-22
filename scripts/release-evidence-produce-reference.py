#!/usr/bin/env python3
"""Promote a measured, non-SLA Linux reference report into typed release evidence.

The input remains explicitly non-SLA and diagnostic.  This verifier checks its
packaged workload identity, complete SQLite/PostgreSQL dimensions, percentile
shape, and byte inventory before emitting the release-gated publication
attestation.  No latency value is compared to a release threshold.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import importlib.util
import json
import os
import stat
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
ADAPTER_PATH = ROOT / "scripts/release-evidence-produce.py"
AGGREGATOR_PATH = ROOT / "scripts/reference-evidence.py"
PACKAGED_ACCEPTANCE_PATH = ROOT / "scripts/postgres-packaged-acceptance.py"
PROJECTOR_PATH = ROOT / "scripts/reference-evidence-project.py"
SOURCE_ID = "reference-performance"
EVIDENCE_ID = "reference-performance-per-backend"
EXPECTED_KINDS = frozenset({"counter", "heist", "sqlite", "postgres", "soak"})
EXPECTED_COVERAGE = {
    "counter": {"latency_ms", "load", "fan_out"},
    "heist": {"latency_ms", "load", "fan_out"},
    "sqlite": {"latency_ms", "memory", "database_growth", "recovery"},
    "postgres": {"latency_ms", "memory", "database_growth", "recovery"},
    "soak": {"latency_ms", "memory", "database_growth"},
}
EXPECTED_MEASUREMENT_SOURCES = {
    "latency_ms": EXPECTED_KINDS,
    "load": frozenset({"counter", "heist"}),
    "fan_out": frozenset({"counter", "heist"}),
    "memory": frozenset({"sqlite", "postgres", "soak"}),
    "database_growth": frozenset({"sqlite", "postgres", "soak"}),
    "recovery": frozenset({"sqlite", "postgres"}),
}
SHA256_PREFIX = "sha256:"


def load_adapter():
    spec = importlib.util.spec_from_file_location(
        "worldstream_reference_release_adapter", ADAPTER_PATH
    )
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {ADAPTER_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


ADAPTER = load_adapter()


def load_aggregator():
    spec = importlib.util.spec_from_file_location(
        "worldstream_reference_evidence_aggregator", AGGREGATOR_PATH
    )
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {AGGREGATOR_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


AGGREGATOR = load_aggregator()


def load_packaged_acceptance():
    spec = importlib.util.spec_from_file_location(
        "worldstream_reference_packaged_acceptance", PACKAGED_ACCEPTANCE_PATH
    )
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {PACKAGED_ACCEPTANCE_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


PACKAGED_ACCEPTANCE = load_packaged_acceptance()


class ReferenceError(RuntimeError):
    """The measured reference report cannot support publication evidence."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ReferenceError(message)


def regular_file(path: Path, label: str) -> Path:
    try:
        mode = path.lstat().st_mode
    except FileNotFoundError as error:
        raise ReferenceError(f"missing {label}: {path}") from error
    except OSError as error:
        raise ReferenceError(f"cannot inspect {label}: {error}") from error
    require(not stat.S_ISLNK(mode), f"{label} must not be a symlink")
    require(stat.S_ISREG(mode), f"{label} must be a regular file")
    return path


def sha256(path: Path) -> str:
    try:
        return SHA256_PREFIX + hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        raise ReferenceError(f"cannot hash {path}: {error}") from error


def read_json(path: Path, label: str) -> tuple[dict[str, Any], bytes]:
    regular_file(path, label)
    try:
        raw = path.read_bytes()
        value = json.loads(raw.decode("utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ReferenceError(f"{label} is not valid UTF-8 JSON: {error}") from error
    require(isinstance(value, dict), f"{label} must be a JSON object")
    return value, raw


def sha256_reference(value: object, label: str) -> str:
    require(
        isinstance(value, str)
        and len(value) == len(SHA256_PREFIX) + 64
        and value.startswith(SHA256_PREFIX)
        and all(character in "0123456789abcdef" for character in value[7:]),
        f"{label} must be an exact SHA-256 reference",
    )
    return value


def canonical_compact(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
        + "\n"
    ).encode("utf-8")


def atomic_write(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    require(not path.is_symlink() and not path.is_dir(), f"unsafe output path: {path}")
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump(value, output, ensure_ascii=False, indent=2, sort_keys=True)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def verify_packaged_distribution(
    archive: Path,
    package_report_path: Path,
    daemon_bin: Path,
    manifest_toml: Path,
    manifest_json: Path,
    manifest: dict[str, Any],
) -> tuple[dict[str, Any], dict[str, Any], bytes, dict[str, Any]]:
    regular_file(archive, "fresh Linux package archive")
    regular_file(daemon_bin, "extracted packaged daemon")
    require(
        daemon_bin.stat().st_mode & 0o111 != 0,
        "extracted packaged daemon must retain an executable mode",
    )
    package_report, package_report_raw = read_json(
        package_report_path, "fresh Linux package report"
    )
    archive_digest = sha256(archive)
    report_identity = package_report.get("identity")
    require(
        package_report.get("schema") == "worldstream/package-report/v1"
        and package_report.get("kind") == "archive"
        and package_report.get("artifact") == archive.name
        and package_report.get("path") == archive.name
        and package_report.get("sha256") == archive_digest
        and package_report.get("size_bytes") == archive.stat().st_size
        and package_report.get("inventory", {}).get("archive_verified") is True
        and package_report.get("inventory", {}).get("release_evidence") is False
        and package_report.get("inventory", {}).get("manifest_source")
        == "compatibility.toml"
        and package_report.get("inventory", {}).get("manifest_mirror")
        == "compatibility.json"
        and isinstance(report_identity, dict)
        and set(report_identity)
        == {
            "target",
            "version",
            "manifest_sha256",
            "manifest_json_sha256",
            "manifest_toml_sha256",
        }
        and report_identity.get("target") == "linux-x86_64"
        and report_identity.get("version") == manifest["release_candidate"],
        "fresh Linux package report does not exactly bind the archive identity",
    )
    regular_file(manifest_toml, "compatibility TOML")
    regular_file(manifest_json, "compatibility JSON")
    root_manifest_toml = manifest_toml.read_bytes()
    root_manifest_json = manifest_json.read_bytes()
    expected_manifest_toml = hashlib.sha256(root_manifest_toml).hexdigest()
    expected_manifest_json = hashlib.sha256(root_manifest_json).hexdigest()
    require(
        report_identity.get("manifest_sha256") == expected_manifest_json
        and report_identity.get("manifest_json_sha256") == expected_manifest_json
        and report_identity.get("manifest_toml_sha256") == expected_manifest_toml,
        "fresh package manifest pair identity mismatch",
    )
    try:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-reference-package-verify-"
        ) as directory:
            verified_daemon, verified_control, package_binding = (
                PACKAGED_ACCEPTANCE._bind_package(
                    archive,
                    package_report_path,
                    Path(directory) / "extracted",
                )
            )
            archived_daemon = verified_daemon.read_bytes()
            archived_control = verified_control.read_bytes()
    except (PACKAGED_ACCEPTANCE.LaneFailure, OSError) as error:
        raise ReferenceError(
            f"cannot independently verify fresh package archive: {error}"
        ) from error
    daemon_bytes = daemon_bin.read_bytes()
    require(
        archived_daemon == daemon_bytes,
        "extracted daemon bytes differ from the fresh package archive",
    )
    daemon_digest = SHA256_PREFIX + hashlib.sha256(daemon_bytes).hexdigest()
    control_digest = SHA256_PREFIX + hashlib.sha256(archived_control).hexdigest()
    require(
        package_binding.get("identity", {}).get("manifest_json_sha256")
        == expected_manifest_json
        and package_binding.get("identity", {}).get("manifest_toml_sha256")
        == expected_manifest_toml,
        "archived compatibility pair differs from the verified root manifests",
    )
    distribution = {
        "packaged_artifact_bound": True,
        "reference_class": "fresh_packaged_linux_x86_64",
        "target": "linux-x86_64",
        "version": manifest["release_candidate"],
        "archive_sha256": archive_digest,
        "archive_size_bytes": archive.stat().st_size,
        "package_report_sha256": sha256(package_report_path),
        "manifest_sha256": SHA256_PREFIX + expected_manifest_json,
        "manifest_json_sha256": SHA256_PREFIX + expected_manifest_json,
        "manifest_toml_sha256": SHA256_PREFIX + expected_manifest_toml,
        "binary_sha256": daemon_digest,
        "binary_size_bytes": len(daemon_bytes),
        "control_binary_sha256": control_digest,
        "control_binary_size_bytes": len(archived_control),
    }
    return distribution, package_report, package_report_raw, package_binding


def positive_story_load(value: object, label: str) -> None:
    require(
        isinstance(value, dict)
        and type(value.get("active_rooms")) in {int, float}
        and value["active_rooms"] > 0
        and type(value.get("actions_per_second")) in {int, float}
        and value["actions_per_second"] > 0,
        f"{label} must disclose positive active_rooms and actions_per_second",
    )


def positive_story_fan_out(value: object, label: str) -> None:
    require(
        isinstance(value, dict)
        and type(value.get("observation_fan_out")) in {int, float}
        and value["observation_fan_out"] > 0,
        f"{label} must disclose positive observation_fan_out",
    )


def verify_packaged_acceptance(
    path: Path,
    distribution: dict[str, Any],
    expected_binding: dict[str, Any],
) -> tuple[dict[str, Any], bytes, str]:
    report, raw = read_json(path, "packaged backend acceptance report")
    require(
        set(report)
        == {
            "schema",
            "canonical_encoding",
            "status",
            "release_evidence",
            "secrets_emitted",
            "provider",
            "reference_environment",
            "reference_workloads",
            "cells",
            "comparison",
            "package_binding",
            "performance",
            "cleanup",
        }
        and report.get("schema") == PACKAGED_ACCEPTANCE.SCHEMA
        and report.get("canonical_encoding") == "utf8-sorted-key-compact-json-lf"
        and report.get("status") == "pass"
        and report.get("release_evidence") is True
        and report.get("secrets_emitted") is False
        and report.get("cleanup") == "pass",
        "packaged backend acceptance is not a complete releasable six-cell report",
    )
    require(
        report.get("provider")
        == {
            "postgres_image": PACKAGED_ACCEPTANCE.POSTGRES_IMAGE,
            "pgbouncer_image": PACKAGED_ACCEPTANCE.PGBOUNCER_IMAGE,
            "pool_mode": "transaction",
        },
        "packaged backend acceptance provider identity drifted",
    )
    require(
        report.get("package_binding") == expected_binding,
        "packaged backend acceptance does not exactly bind the independently verified package",
    )
    binding = report["package_binding"]
    require(
        binding.get("archive_sha256") == distribution["archive_sha256"]
        and binding.get("archive_size_bytes") == distribution["archive_size_bytes"]
        and binding.get("package_report_sha256")
        == distribution["package_report_sha256"]
        and SHA256_PREFIX + binding.get("identity", {}).get("manifest_sha256", "")
        == distribution["manifest_sha256"]
        and SHA256_PREFIX + binding.get("identity", {}).get("manifest_json_sha256", "")
        == distribution["manifest_json_sha256"]
        and SHA256_PREFIX + binding.get("identity", {}).get("manifest_toml_sha256", "")
        == distribution["manifest_toml_sha256"]
        and binding.get("binaries", {}).get("worldstreamd", {}).get("sha256")
        == distribution["binary_sha256"]
        and binding.get("binaries", {}).get("worldstreamctl", {}).get("sha256")
        == distribution["control_binary_sha256"],
        "packaged backend acceptance package/report/binary/manifest identity mismatch",
    )
    environment = report.get("reference_environment")
    platform_value = (
        environment.get("platform") if isinstance(environment, dict) else None
    )
    hardware = environment.get("hardware") if isinstance(environment, dict) else None
    filesystem = (
        environment.get("filesystem") if isinstance(environment, dict) else None
    )
    engines = environment.get("engines") if isinstance(environment, dict) else None
    require(
        isinstance(environment, dict)
        and environment.get("schema") == "worldstream/reference-environment/v1"
        and isinstance(platform_value, dict)
        and platform_value.get("system") == "Linux"
        and platform_value.get("machine") in {"x86_64", "amd64"}
        and isinstance(platform_value.get("release"), str)
        and bool(platform_value["release"])
        and isinstance(hardware, dict)
        and type(hardware.get("logical_cpus")) is int
        and hardware["logical_cpus"] > 0
        and type(hardware.get("physical_memory_bytes")) is int
        and hardware["physical_memory_bytes"] > 0
        and isinstance(filesystem, dict)
        and all(
            isinstance(filesystem.get(name), str) and bool(filesystem[name])
            for name in ("repository", "temporary")
        )
        and isinstance(engines, dict)
        and engines.get("postgresql")
        == {
            "version": "17.11",
            "settings": {
                "server_version_num": "170011",
                "synchronous_commit": "on",
                "transaction_isolation": "read committed",
            },
            "connection_mode": "direct_and_transaction_pooler",
        }
        and engines.get("pgbouncer")
        == {
            "image": PACKAGED_ACCEPTANCE.PGBOUNCER_IMAGE,
            "pool_mode": "transaction",
        }
        and engines.get("sqlite")
        == {
            "version": "3.53.4",
            "settings": {"journal_mode": "wal", "synchronous": "full"},
            "connection_mode": "embedded",
        },
        "packaged backend acceptance environment disclosure is incomplete",
    )

    expected_backends = {"sqlite", "postgres_direct", "transaction_pooler"}
    cells = report.get("cells")
    comparisons = report.get("comparison")
    workloads = report.get("reference_workloads")
    performance = report.get("performance")
    require(
        isinstance(cells, dict)
        and set(cells) == {"counter", "heist"}
        and isinstance(comparisons, dict)
        and set(comparisons) == {"counter", "heist"}
        and isinstance(workloads, dict)
        and set(workloads) == {"counter", "heist"}
        and isinstance(performance, dict)
        and set(performance)
        == {"classification", "percentile_definition", "cells_ms", "cells"}
        and performance.get("classification") == "measured_non_sla"
        and performance.get("percentile_definition") == "nearest-rank"
        and isinstance(performance.get("cells_ms"), dict)
        and isinstance(performance.get("cells"), dict),
        "packaged backend acceptance six-cell inventories are incomplete",
    )
    expected_cell_names = {
        f"{story}.{backend}"
        for story in ("counter", "heist")
        for backend in expected_backends
    }
    require(
        set(performance["cells_ms"]) == expected_cell_names
        and set(performance["cells"]) == expected_cell_names,
        "packaged backend acceptance performance inventory is not the exact six cells",
    )
    for story, normalizer in (
        ("counter", PACKAGED_ACCEPTANCE._counter_normalized),
        ("heist", PACKAGED_ACCEPTANCE._heist_normalized),
    ):
        require(
            isinstance(cells[story], dict)
            and set(cells[story]) == expected_backends
            and isinstance(workloads[story], dict)
            and set(workloads[story]) == expected_backends,
            f"packaged backend acceptance {story} backend inventory is incomplete",
        )
        normalized: dict[str, Any] = {}
        for backend in sorted(expected_backends):
            cell = cells[story][backend]
            require(
                isinstance(cell, dict)
                and set(cell)
                == {
                    "exit_code",
                    "elapsed_ms",
                    "process_tree_rss_kib",
                    "report",
                    "performance",
                }
                and cell.get("exit_code") == 0
                and type(cell.get("elapsed_ms")) is int
                and cell["elapsed_ms"] > 0
                and isinstance(cell.get("report"), dict)
                and isinstance(cell.get("performance"), dict),
                f"packaged backend acceptance {story}.{backend} cell is malformed",
            )
            try:
                PACKAGED_ACCEPTANCE._validate_cell(story, backend, cell["report"])
            except PACKAGED_ACCEPTANCE.LaneFailure as error:
                raise ReferenceError(
                    f"packaged backend acceptance {story}.{backend} failed validation: {error}"
                ) from error
            cell_name = f"{story}.{backend}"
            cell_performance = cell["performance"]
            require(
                performance["cells_ms"][cell_name] == cell["elapsed_ms"]
                and performance["cells"][cell_name] == cell_performance
                and cell_performance.get("story_duration_ms") == cell["elapsed_ms"]
                and cell_performance.get("latency_ms")
                == PACKAGED_ACCEPTANCE._latency_triplet(cell["report"])
                and isinstance(cell_performance.get("memory"), dict)
                and cell_performance["memory"].get("status") == "measured"
                and type(cell_performance["memory"].get("peak_rss_bytes")) is int
                and cell_performance["memory"]["peak_rss_bytes"] > 0,
                f"packaged backend acceptance {cell_name} measurements are not byte-consistent",
            )
            positive_story_load(cell_performance.get("load"), f"{cell_name} load")
            positive_story_fan_out(
                cell_performance.get("fan_out"), f"{cell_name} fan-out"
            )
            triplet(cell_performance.get("latency_ms"), f"{cell_name} latency")
            recovery = cell_performance.get("recovery")
            require(
                isinstance(recovery, dict)
                and isinstance(recovery.get("durations_ms"), list)
                and recovery["durations_ms"]
                and all(
                    type(value) in {int, float} and value >= 0
                    for value in recovery["durations_ms"]
                ),
                f"packaged backend acceptance {cell_name} recovery is unmeasured",
            )
            if backend != "sqlite":
                growth = cell_performance.get("database_growth")
                require(
                    isinstance(growth, dict)
                    and growth.get("status") == "measured"
                    and type(growth.get("growth_bytes")) is int
                    and growth["growth_bytes"] >= 0
                    and type(growth.get("cluster_wal_growth_bytes")) is int
                    and growth["cluster_wal_growth_bytes"] >= 0,
                    f"packaged backend acceptance {cell_name} database/WAL growth is unmeasured",
                )
            disclosure = cell["report"].get("reference_workload")
            try:
                AGGREGATOR._validate_reference_workload(
                    cell_name, {"reference_workload": disclosure}
                )
            except AGGREGATOR.EvidenceError as error:
                raise ReferenceError(
                    f"packaged backend acceptance {cell_name} workload disclosure is invalid: {error}"
                ) from error
            require(
                disclosure.get("pack_id")
                == (
                    "worldstream.counter"
                    if story == "counter"
                    else "worldstream.agent-heist"
                )
                and disclosure.get("fan_out")
                == cell_performance["fan_out"].get("observation_fan_out")
                and workloads[story][backend]
                == {
                    "load": cell_performance.get("load"),
                    "fan_out": cell_performance.get("fan_out"),
                    "observed_transitions": cell["report"]
                    .get("measurements", {})
                    .get("observed_transitions"),
                    "story_duration_ms": cell["report"]
                    .get("measurements", {})
                    .get("story_duration_ms"),
                    "disclosure": disclosure,
                },
                f"packaged backend acceptance {cell_name} workload disclosure drifted",
            )
            normalized[backend] = normalizer(cell["report"])
        require(
            normalized["sqlite"]
            == normalized["postgres_direct"]
            == normalized["transaction_pooler"]
            and comparisons[story] == {"status": "pass", "normalized": normalized},
            f"packaged backend acceptance {story} normalized comparator drifted",
        )
    digest_value = SHA256_PREFIX + hashlib.sha256(raw).hexdigest()
    return report, raw, digest_value


def triplet(value: object, label: str) -> None:
    require(isinstance(value, dict), f"{label} percentile result must be an object")
    count = value.get("sample_count")
    p50 = value.get("p50_ms")
    p95 = value.get("p95_ms")
    p99 = value.get("p99_ms")
    require(
        value.get("definition") == "nearest-rank"
        and type(count) is int
        and count > 0
        and all(type(item) in {int, float} for item in (p50, p95, p99))
        and 0 <= p50 <= p95 <= p99,
        f"{label} requires measured monotonic nearest-rank p50/p95/p99",
    )


def measured_resources(value: object, label: str, field: str) -> None:
    require(
        isinstance(value, dict)
        and value.get("status") == "measured"
        and type(value.get(field)) is int
        and value[field] >= 0,
        f"{label} is not a measured non-negative resource result",
    )


def validate_report(
    report: dict[str, Any],
    manifest: dict[str, Any],
    archive_sha256: str,
    packaged_acceptance_sha256: str,
) -> dict[str, Any]:
    require(
        set(report)
        == {
            "schema",
            "status",
            "release_evidence",
            "performance_class",
            "inputs",
            "input_sha256_inventory",
            "sources",
            "identity",
            "reference_environment",
            "workloads",
            "coverage",
            "measurements",
            "gaps",
            "errors",
        },
        "reference report has wrong top-level fields",
    )
    require(
        report.get("schema") == "worldstream/imo-61-reference-evidence/v1"
        and report.get("status") == "pass"
        and report.get("release_evidence") is False
        and report.get("performance_class") == "reference_non_release"
        and report.get("gaps") == []
        and report.get("errors") == [],
        "reference report is not a complete explicit non-SLA measurement",
    )
    identity = report.get("identity")
    require(
        isinstance(identity, dict)
        and set(identity)
        == {"product", "profile", "version", "packaged_acceptance_sha256"}
        and identity.get("product") == "worldstream"
        and identity.get("profile") == "linux-reference"
        and identity.get("version") == manifest["release_candidate"],
        "reference report product/profile/version identity mismatch",
    )
    environment = report.get("reference_environment")
    try:
        AGGREGATOR._validate_reference_environment(
            "aggregate", {"reference_environment": environment}
        )
    except AGGREGATOR.EvidenceError as error:
        raise ReferenceError(
            f"reference environment disclosure is invalid: {error}"
        ) from error
    workloads = report.get("workloads")
    require(
        isinstance(workloads, dict) and set(workloads) == EXPECTED_KINDS,
        "reference report per-source workload disclosures are incomplete",
    )
    for kind, workload in workloads.items():
        try:
            AGGREGATOR._validate_reference_workload(
                kind, {"reference_workload": workload}
            )
        except AGGREGATOR.EvidenceError as error:
            raise ReferenceError(
                f"{kind} reference workload disclosure is invalid: {error}"
            ) from error

    inputs = report.get("inputs")
    require(isinstance(inputs, list) and len(inputs) == 5, "five inputs are required")
    by_kind: dict[str, dict[str, Any]] = {}
    for item in inputs:
        require(
            isinstance(item, dict)
            and set(item) == {"kind", "bytes", "sha256", "schema", "status"}
            and item.get("kind") in EXPECTED_KINDS
            and item.get("kind") not in by_kind
            and type(item.get("bytes")) is int
            and item["bytes"] > 0
            and isinstance(item.get("schema"), str)
            and item["schema"]
            and isinstance(item.get("status"), str)
            and item["status"],
            "reference report input inventory is malformed or duplicated",
        )
        sha256_reference(item.get("sha256"), f"{item['kind']} input")
        by_kind[item["kind"]] = item
    require(
        set(by_kind) == EXPECTED_KINDS, "reference report input kinds are incomplete"
    )
    expected_items = sorted(inputs, key=lambda item: item["kind"])
    inventory = report.get("input_sha256_inventory")
    require(
        isinstance(inventory, dict)
        and set(inventory) == {"definition", "sha256", "items"}
        and inventory.get("items") == expected_items,
        "reference report byte inventory does not match the five named inputs",
    )
    inventory_digest = (
        SHA256_PREFIX + hashlib.sha256(canonical_compact(expected_items)).hexdigest()
    )
    require(
        sha256_reference(inventory.get("sha256"), "reference input inventory")
        == inventory_digest,
        "reference input inventory digest mismatch",
    )

    sources = report.get("sources")
    require(
        isinstance(sources, dict) and set(sources) == EXPECTED_KINDS,
        "reference report source identities are incomplete",
    )
    artifact_identities: set[str] = set()
    acceptance_identities: set[str] = set()
    for kind, source in sources.items():
        source_identity = source.get("identity") if isinstance(source, dict) else None
        require(
            isinstance(source, dict)
            and set(source)
            == {
                "schema",
                "status",
                "identity",
                "reference_environment",
                "reference_workload",
            }
            and source.get("schema") == by_kind[kind]["schema"]
            and source.get("status") == by_kind[kind]["status"]
            and source.get("reference_environment") == environment
            and source.get("reference_workload") == workloads[kind]
            and isinstance(source_identity, dict)
            and source_identity.get("product") == "worldstream"
            and source_identity.get("profile") == "linux-reference"
            and source_identity.get("version") == manifest["release_candidate"],
            f"{kind} packaged reference source identity mismatch",
        )
        artifact_identities.add(
            sha256_reference(
                source_identity.get("artifact_sha256"), f"{kind} packaged artifact"
            )
        )
        acceptance_identities.add(
            sha256_reference(
                source_identity.get("packaged_acceptance_sha256"),
                f"{kind} packaged acceptance",
            )
        )
    require(
        artifact_identities == {archive_sha256},
        "all reference measurements must bind the verified fresh package archive",
    )
    require(
        identity.get("packaged_acceptance_sha256") == packaged_acceptance_sha256
        and acceptance_identities == {packaged_acceptance_sha256},
        "all reference measurements must bind the verified packaged six-cell acceptance",
    )

    coverage = report.get("coverage")
    require(
        isinstance(coverage, dict) and set(coverage) == EXPECTED_KINDS,
        "reference coverage map is incomplete",
    )
    for kind, names in EXPECTED_COVERAGE.items():
        require(
            isinstance(coverage[kind], dict)
            and set(coverage[kind]) == names
            and all(coverage[kind][name] is True for name in names),
            f"{kind} reference measurement dimensions are incomplete",
        )

    measurements = report.get("measurements")
    require(
        isinstance(measurements, dict)
        and set(measurements) == set(EXPECTED_MEASUREMENT_SOURCES),
        "reference measurement groups are incomplete",
    )
    for name, expected_sources in EXPECTED_MEASUREMENT_SOURCES.items():
        group = measurements[name]
        observed = group.get("sources") if isinstance(group, dict) else None
        require(
            isinstance(observed, dict) and set(observed) == expected_sources,
            f"{name} does not cover its exact reference sources",
        )
    for kind, value in measurements["latency_ms"]["sources"].items():
        triplet(value, f"{kind} latency")
    for kind, value in measurements["recovery"]["sources"].items():
        triplet(value, f"{kind} recovery")
    for kind, value in measurements["load"]["sources"].items():
        require(
            isinstance(value, dict)
            and type(value.get("active_rooms")) in {int, float}
            and value["active_rooms"] > 0
            and type(value.get("actions_per_second")) in {int, float}
            and value["actions_per_second"] > 0,
            f"{kind} load must name positive active_rooms and actions_per_second",
        )
    for kind, value in measurements["fan_out"]["sources"].items():
        require(
            isinstance(value, dict)
            and type(value.get("observation_fan_out")) in {int, float}
            and value["observation_fan_out"] > 0,
            f"{kind} fan_out must name a positive observation_fan_out",
        )
    for kind, value in measurements["memory"]["sources"].items():
        measured_resources(value, f"{kind} memory", "peak_rss_bytes")
    for kind, value in measurements["database_growth"]["sources"].items():
        measured_resources(value, f"{kind} database growth", "growth_bytes")
    return {
        "artifact_sha256": next(iter(artifact_identities)),
        "input_inventory_sha256": inventory_digest,
        "measurements": measurements,
        "workloads": workloads,
    }


def recompute_report(input_paths: dict[str, Path]) -> dict[str, Any]:
    arguments = argparse.Namespace(
        **{f"{kind}_report": str(input_paths[kind]) for kind in EXPECTED_KINDS}
    )
    try:
        return AGGREGATOR._aggregate(arguments)
    except (AGGREGATOR.EvidenceError, OSError, ValueError) as error:
        raise ReferenceError(f"cannot recompute reference report: {error}") from error


def independently_project_raw_sources(
    *,
    packaged_acceptance_path: Path,
    soak_path: Path,
    package_archive: Path,
    package_report_path: Path,
    daemon_bin: Path,
    manifest_toml: Path,
    manifest_json: Path,
) -> tuple[dict[str, bytes], bytes, bytes]:
    """Re-run the owning projection from raw acceptance and soak bytes."""

    spec = importlib.util.spec_from_file_location(
        "worldstream_reference_release_independent_projector", PROJECTOR_PATH
    )
    if spec is None or spec.loader is None:  # pragma: no cover
        raise ReferenceError(f"cannot load independent projector: {PROJECTOR_PATH}")
    projector = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = projector
    spec.loader.exec_module(projector)
    try:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-reference-independent-projection-"
        ) as directory:
            root = Path(directory)
            arguments = argparse.Namespace(
                packaged_acceptance_report=packaged_acceptance_path,
                soak_report=soak_path,
                package_archive=package_archive,
                package_report=package_report_path,
                daemon_bin=daemon_bin,
                output_dir=root / "normalized",
                aggregate_report=root / "aggregate.json",
                manifest_toml=manifest_toml,
                manifest_json=manifest_json,
            )
            paths = projector.project(arguments)
            projected = {kind: paths[kind].read_bytes() for kind in EXPECTED_KINDS}
            aggregate = arguments.aggregate_report.read_bytes()
    except (
        projector.ProjectionError,
        projector.REFERENCE_PRODUCER.ReferenceError,
        projector.FAILURE_PRODUCER.EvidenceError,
        OSError,
        KeyError,
        TypeError,
        ValueError,
    ) as error:
        raise ReferenceError(
            f"cannot independently project raw reference sources: {error}"
        ) from error
    _soak, soak_raw = read_json(soak_path, "raw one-hour process soak report")
    return projected, aggregate, soak_raw


def produce(
    output: Path,
    artifact_output: Path,
    report_path: Path,
    input_paths: dict[str, Path],
    package_archive: Path,
    package_report_path: Path,
    daemon_bin: Path,
    packaged_acceptance_path: Path,
    soak_path: Path,
    manifest_toml: Path,
    manifest_json: Path,
) -> None:
    require(
        set(input_paths) == EXPECTED_KINDS,
        "reference producer requires all five exact named input reports",
    )
    all_inputs = {
        report_path.resolve(),
        package_archive.resolve(),
        package_report_path.resolve(),
        daemon_bin.resolve(),
        packaged_acceptance_path.resolve(),
        soak_path.resolve(),
        manifest_toml.resolve(),
        manifest_json.resolve(),
        *(path.resolve() for path in input_paths.values()),
    }
    destinations = {output.resolve(), artifact_output.resolve()}
    require(len(destinations) == 2, "producer and artifact outputs must be distinct")
    require(
        destinations.isdisjoint(all_inputs),
        "reference outputs must not overwrite an evidence input",
    )
    report, report_raw = read_json(report_path, "reference performance report")
    manifest = ADAPTER.COLLECTOR.load_manifest(manifest_toml, manifest_json)
    ADAPTER.COLLECTOR.validate_mapping(ADAPTER.COLLECTOR.release_evidence_ids(manifest))
    distribution, package_report, package_report_raw, package_binding = (
        verify_packaged_distribution(
            package_archive,
            package_report_path,
            daemon_bin,
            manifest_toml,
            manifest_json,
            manifest,
        )
    )
    packaged_acceptance, packaged_acceptance_raw, packaged_acceptance_sha256 = (
        verify_packaged_acceptance(
            packaged_acceptance_path,
            distribution,
            package_binding,
        )
    )
    projected_inputs, projected_aggregate, soak_raw = independently_project_raw_sources(
        packaged_acceptance_path=packaged_acceptance_path,
        soak_path=soak_path,
        package_archive=package_archive,
        package_report_path=package_report_path,
        daemon_bin=daemon_bin,
        manifest_toml=manifest_toml,
        manifest_json=manifest_json,
    )
    for kind, path in sorted(input_paths.items()):
        _value, raw = read_json(path, f"{kind} reference input")
        require(
            raw == projected_inputs[kind],
            f"{kind} reference input is not the exact projection of raw acceptance/soak bytes",
        )
    require(
        report_raw == projected_aggregate,
        "reference aggregate is not the exact independent projection of raw acceptance/soak bytes",
    )
    recomputed = recompute_report(input_paths)
    require(
        recomputed == report,
        "reference report does not exactly match recomputation from the five named input bytes",
    )
    validated = validate_report(
        report,
        manifest,
        distribution["archive_sha256"],
        packaged_acceptance_sha256,
    )
    spec = ADAPTER.SOURCE_BY_ID[SOURCE_ID]
    require(spec.evidence_id == EVIDENCE_ID, "reference producer mapping drift")
    bundled_inputs = []
    for kind in sorted(input_paths):
        _value, raw = read_json(input_paths[kind], f"{kind} reference input")
        bundled_inputs.append(
            {
                "kind": kind,
                "sha256": SHA256_PREFIX + hashlib.sha256(raw).hexdigest(),
                "size_bytes": len(raw),
                "content_base64": base64.b64encode(raw).decode("ascii"),
            }
        )
    bundle = {
        "schema": "worldstream/reference-performance-publication-bundle/v1",
        "status": "passed",
        "release_evidence": True,
        "performance_class": "reference_non_release",
        "evidence_id": EVIDENCE_ID,
        "version": manifest["release_candidate"],
        "platform": spec.platform,
        "contract": manifest["contracts"],
        "distribution": distribution,
        "reference_environment": report["reference_environment"],
        "workloads": report["workloads"],
        "aggregate_report": {
            "sha256": SHA256_PREFIX + hashlib.sha256(report_raw).hexdigest(),
            "size_bytes": len(report_raw),
            "content_base64": base64.b64encode(report_raw).decode("ascii"),
        },
        "input_reports": bundled_inputs,
        "package_report": {
            "sha256": SHA256_PREFIX + hashlib.sha256(package_report_raw).hexdigest(),
            "size_bytes": len(package_report_raw),
            "content": package_report,
        },
        "packaged_acceptance_report": {
            "sha256": packaged_acceptance_sha256,
            "size_bytes": len(packaged_acceptance_raw),
            "content_base64": base64.b64encode(packaged_acceptance_raw).decode("ascii"),
            "schema": packaged_acceptance["schema"],
        },
        "raw_soak_report": {
            "sha256": SHA256_PREFIX + hashlib.sha256(soak_raw).hexdigest(),
            "size_bytes": len(soak_raw),
            "content_base64": base64.b64encode(soak_raw).decode("ascii"),
            "schema": "worldstream/soak-evidence/v1",
        },
        "projection": {
            "definition": "exact independent projection from signed raw acceptance and one-hour soak bytes",
            "packaged_acceptance_sha256": packaged_acceptance_sha256,
            "raw_soak_sha256": SHA256_PREFIX + hashlib.sha256(soak_raw).hexdigest(),
            "aggregate_sha256": SHA256_PREFIX
            + hashlib.sha256(projected_aggregate).hexdigest(),
            "input_sha256": {
                kind: SHA256_PREFIX + hashlib.sha256(raw).hexdigest()
                for kind, raw in sorted(projected_inputs.items())
            },
        },
    }
    atomic_write(artifact_output, bundle)
    bundle_digest = sha256(artifact_output)
    observations = {
        "packaged_workload_identity": (
            f"version={manifest['release_candidate']};"
            f"artifact={validated['artifact_sha256']};"
            f"binary={distribution['binary_sha256']};"
            f"acceptance={packaged_acceptance_sha256};"
            f"inputs={validated['input_inventory_sha256']};"
            f"bundle={bundle_digest}"
        ),
        "sqlite_measurements": json.dumps(
            {
                name: validated["measurements"][name]["sources"].get("sqlite")
                for name in ("latency_ms", "memory", "database_growth", "recovery")
            },
            sort_keys=True,
            separators=(",", ":"),
        ),
        "postgresql_measurements": json.dumps(
            {
                name: validated["measurements"][name]["sources"].get("postgres")
                for name in ("latency_ms", "memory", "database_growth", "recovery")
            },
            sort_keys=True,
            separators=(",", ":"),
        ),
        "non_sla_publication": (
            "performance_class=reference_non_release;profile=linux-reference;"
            "no threshold or SLA comparison performed"
        ),
    }
    typed = {
        "schema": ADAPTER.PRODUCER_SCHEMA,
        "producer_id": "reference-performance-publication/v1",
        "evidence_id": spec.evidence_id,
        "status": "passed",
        "release_evidence": True,
        "fail_closed": False,
        "phase": ADAPTER.COLLECTOR.PRE_SIGN_PHASE,
        "version": manifest["release_candidate"],
        "platform": spec.platform,
        "contract": manifest["contracts"],
        "outcomes": {
            check: {
                "status": "passed",
                "observations": [
                    {"kind": "measured-non-sla-reference", "value": observations[check]}
                ],
            }
            for check in spec.checks
        },
        "artifacts": {
            "reference-performance": {
                "sha256": bundle_digest,
                "size_bytes": artifact_output.stat().st_size,
            }
        },
    }
    atomic_write(output, typed)
    checked = ADAPTER.read_producer(output, spec, manifest)
    ADAPTER.verify_artifacts(
        SOURCE_ID,
        checked,
        {(SOURCE_ID, "reference-performance"): artifact_output},
    )


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--output", type=Path, required=True)
    command.add_argument("--artifact-output", type=Path, required=True)
    command.add_argument("--report", type=Path, required=True)
    for kind in sorted(EXPECTED_KINDS):
        command.add_argument(f"--{kind}-report", type=Path, required=True)
    command.add_argument("--package-archive", type=Path, required=True)
    command.add_argument("--package-report", type=Path, required=True)
    command.add_argument("--daemon-bin", type=Path, required=True)
    command.add_argument("--packaged-acceptance-report", type=Path, required=True)
    command.add_argument("--soak-report", type=Path, required=True)
    command.add_argument(
        "--manifest-toml", type=Path, default=ADAPTER.COLLECTOR.DEFAULT_MANIFEST_TOML
    )
    command.add_argument(
        "--manifest-json", type=Path, default=ADAPTER.COLLECTOR.DEFAULT_MANIFEST_JSON
    )
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        produce(
            args.output,
            args.artifact_output,
            args.report,
            {kind: getattr(args, f"{kind}_report") for kind in EXPECTED_KINDS},
            args.package_archive,
            args.package_report,
            args.daemon_bin,
            args.packaged_acceptance_report,
            args.soak_report,
            args.manifest_toml,
            args.manifest_json,
        )
    except (
        ReferenceError,
        ADAPTER.ProducerError,
        ADAPTER.COLLECTOR.CollectionError,
    ) as error:
        print(f"reference performance producer failed: {error}", file=sys.stderr)
        return 1
    print(f"wrote typed reference performance producer: {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

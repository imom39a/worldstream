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
import tarfile
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
ADAPTER_PATH = ROOT / "scripts/release-evidence-produce.py"
AGGREGATOR_PATH = ROOT / "scripts/reference-evidence.py"
PACKAGED_ACCEPTANCE_PATH = ROOT / "scripts/postgres-packaged-acceptance.py"
PROJECTOR_PATH = ROOT / "scripts/reference-evidence-project.py"
REFERENCE_TARGET_RUNNER_PATH = ROOT / "scripts/reference-target-workload.py"
REFERENCE_TARGET_FIXTURE_SOURCE_PATH = (
    ROOT / "crates/worldstream-sqlite/examples/reference_snapshot_tail_fixture.rs"
)
SOURCE_ID = "reference-performance"
EVIDENCE_ID = "reference-performance-per-backend"
EXPECTED_KINDS = frozenset({"counter", "heist", "sqlite", "postgres", "soak", "target"})
EXPECTED_COVERAGE = {
    "counter": {"latency_ms", "load", "fan_out"},
    "heist": {"latency_ms", "load", "fan_out"},
    "sqlite": {"latency_ms", "memory", "database_growth", "recovery"},
    "postgres": {"latency_ms", "memory", "database_growth", "recovery"},
    "soak": {"latency_ms", "memory", "database_growth"},
    "target": {"reference_targets"},
}
EXPECTED_MEASUREMENT_SOURCES = {
    "latency_ms": EXPECTED_KINDS,
    "load": frozenset({"counter", "heist"}),
    "fan_out": frozenset({"counter", "heist"}),
    "memory": frozenset({"sqlite", "postgres", "soak"}),
    "database_growth": frozenset({"sqlite", "postgres", "soak"}),
    "recovery": frozenset({"sqlite", "postgres"}),
    "reference_targets": frozenset({"target"}),
}
SHA256_PREFIX = "sha256:"
REFERENCE_TARGET_SCHEMA = "worldstream/reference-target-workload/v1"
REFERENCE_TARGET_PROJECTION_SCHEMA = "worldstream/reference-target-projection/v1"
REFERENCE_TARGET_PROFILE_SCHEMA = "worldstream/reference-target-profile/v1"
REFERENCE_TARGET_DIMENSIONS = (
    "stored_passivated_rooms",
    "simultaneously_loaded_rooms",
    "mostly_idle_websocket_sessions",
    "sustained_accepted_transition_rate",
    "commit_to_ack_latency",
    "one_hour_bounded_soak",
    "snapshot_tail_recovery",
    "repeated_forced_termination_no_acknowledged_loss",
)
FROZEN_REFERENCE_PROFILE = {
    "schema": REFERENCE_TARGET_PROFILE_SCHEMA,
    "name": "frozen_release",
    "publishable_candidate": True,
    "stored_room_target": 10_000,
    "loaded_room_target": 100,
    "idle_websocket_target": 1_000,
    "sustained_transition_rate_target_per_second": 100,
    "sustained_transition_window_seconds": 1_800,
    "commit_to_ack_p95_target_ms": 100,
    "history_room_transition_target": 100_000,
    "snapshot_maximum_lag_transitions": 250,
    "snapshot_tail_recovery_target_ms": 5_000,
    "one_hour_soak_target_seconds": 3_600,
    "forced_termination_minimum_count": 2,
}


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
    except OSError as error:
        raise ReferenceError(f"{label} is not valid UTF-8 JSON: {error}") from error
    try:
        value = ADAPTER.COLLECTOR.strict_json_object(raw, label)
    except ADAPTER.COLLECTOR.CollectionError as error:
        raise ReferenceError(str(error)) from error
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
            "source_revision",
            "build_identity_sha256",
            "observed_build_environment",
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
            with tarfile.open(archive, "r:gz") as package:
                build_member_name = (
                    f"{package_binding['archive_root']}/"
                    f"{PACKAGED_ACCEPTANCE.BUILD_IDENTITY.BUILD_METADATA_PATH}"
                )
                build_members = [
                    member
                    for member in package.getmembers()
                    if member.name == build_member_name and member.isfile()
                ]
                require(
                    len(build_members) == 1,
                    "fresh package must contain one exact build identity",
                )
                archived_build_raw = PACKAGED_ACCEPTANCE._member_bytes(
                    package, build_members[0]
                )
                archived_build = PACKAGED_ACCEPTANCE._strict_json(
                    archived_build_raw, "package_build_identity_invalid"
                )
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
    build_source = archived_build.get("source")
    build_target = archived_build.get("target")
    build_revision = (
        build_source.get("revision") if isinstance(build_source, dict) else None
    )
    require(
        package_binding.get("identity", {}).get("manifest_json_sha256")
        == expected_manifest_json
        and package_binding.get("identity", {}).get("manifest_toml_sha256")
        == expected_manifest_toml,
        "archived compatibility pair differs from the verified root manifests",
    )
    require(
        archived_build.get("schema")
        == PACKAGED_ACCEPTANCE.BUILD_IDENTITY.BUILD_IDENTITY_SCHEMA
        and build_source
        == {
            "repository": PACKAGED_ACCEPTANCE.BUILD_IDENTITY.REPOSITORY,
            "revision": build_revision,
        }
        and isinstance(build_revision, str)
        and len(build_revision) == 40
        and all(character in "0123456789abcdef" for character in build_revision)
        and isinstance(build_target, dict)
        and build_target.get("profile") == "linux-x86_64"
        and archived_build.get("manifest_sha256")
        == SHA256_PREFIX + expected_manifest_json
        and isinstance(archived_build.get("observed_build_environment"), dict)
        and report_identity.get("source_revision") == build_revision
        and report_identity.get("build_identity_sha256")
        == SHA256_PREFIX + hashlib.sha256(archived_build_raw).hexdigest()
        and report_identity.get("observed_build_environment")
        == archived_build["observed_build_environment"],
        "fresh package source revision or build identity is not exactly bound",
    )
    distribution = {
        "packaged_artifact_bound": True,
        "reference_class": "fresh_packaged_linux_x86_64",
        "target": "linux-x86_64",
        "version": manifest["release_candidate"],
        "source_revision": build_revision,
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


def verify_packaged_channel_classes(value: object, channel_names: set[str]) -> None:
    expected_classes = list(PACKAGED_ACCEPTANCE.REQUIRED_PRIVACY_CHANNEL_CLASSES)
    require(
        isinstance(value, dict)
        and set(value) == {"schema", "status", "required", "classes"}
        and value.get("schema") == PACKAGED_ACCEPTANCE.PRIVACY_CHANNEL_CLASS_SCHEMA
        and value.get("status") == "complete"
        and value.get("required") == expected_classes
        and isinstance(value.get("classes"), list)
        and len(value["classes"]) == len(expected_classes),
        "packaged backend acceptance privacy channel-class contract is invalid",
    )
    assigned_channels: set[str] = set()
    for expected_class, row in zip(expected_classes, value["classes"], strict=True):
        observed_channels = row.get("channels") if isinstance(row, dict) else None
        require(
            isinstance(row, dict)
            and set(row) == {"class", "channel_count", "channels"}
            and row.get("class") == expected_class
            and type(row.get("channel_count")) is int
            and isinstance(observed_channels, list)
            and 0 < len(observed_channels) <= len(channel_names)
            and row["channel_count"] == len(observed_channels)
            and all(isinstance(name, str) for name in observed_channels)
            and observed_channels == sorted(observed_channels)
            and len(set(observed_channels)) == len(observed_channels)
            and all(name in channel_names for name in observed_channels)
            and not assigned_channels.intersection(observed_channels),
            "packaged backend acceptance privacy channel-class mapping is invalid",
        )
        assigned_channels.update(observed_channels)
    require(
        assigned_channels == channel_names,
        "packaged backend acceptance privacy channel classes do not exactly cover scanned channels",
    )


def verify_packaged_privacy(value: object) -> None:
    require(
        isinstance(value, dict)
        and set(value) == {"status", "channel_contract", "secret_scan"}
        and value.get("status") == "pass"
        and value.get("channel_contract")
        == PACKAGED_ACCEPTANCE.PRIVACY_CHANNEL_CONTRACT,
        "packaged backend acceptance privacy evidence is incomplete",
    )
    scan = value["secret_scan"]
    require(
        isinstance(scan, dict)
        and set(scan)
        == {
            "schema",
            "status",
            "secrets_emitted",
            "encodings_scanned",
            "sentinels",
            "channels",
            "channel_class_inventory",
        }
        and scan.get("schema") == PACKAGED_ACCEPTANCE.SECRET_SCAN.MATRIX_SCHEMA
        and scan.get("status") == "pass"
        and scan.get("secrets_emitted") is False
        and scan.get("encodings_scanned") == ["base64", "base64url", "hex", "raw"],
        "packaged backend acceptance privacy scan matrix is invalid",
    )
    channels = scan["channels"]
    require(
        isinstance(channels, list)
        and 0 < len(channels) <= PACKAGED_ACCEPTANCE.SECRET_SCAN.MAX_CHANNELS
        and all(isinstance(item, dict) for item in channels)
        and channels == sorted(channels, key=lambda item: item.get("channel", "")),
        "packaged backend acceptance privacy channel inventory is invalid",
    )
    channel_names: set[str] = set()
    total_channel_bytes = 0
    for channel in channels:
        require(
            isinstance(channel, dict)
            and set(channel) == {"channel", "sha256", "size_bytes"}
            and isinstance(channel.get("channel"), str)
            and bool(channel["channel"])
            and channel["channel"].replace("-", "").isalnum()
            and channel["channel"] not in channel_names
            and type(channel.get("size_bytes")) is int
            and 0
            <= channel["size_bytes"]
            <= PACKAGED_ACCEPTANCE.SECRET_SCAN.MAX_CHANNEL_BYTES,
            "packaged backend acceptance privacy channel record is invalid",
        )
        sha256_reference(channel.get("sha256"), "privacy channel")
        channel_names.add(channel["channel"])
        total_channel_bytes += channel["size_bytes"]
    require(
        total_channel_bytes > 0,
        "packaged backend acceptance privacy channels contain no retained bytes",
    )
    verify_packaged_channel_classes(scan["channel_class_inventory"], channel_names)

    sentinels = scan["sentinels"]
    require(
        isinstance(sentinels, list)
        and 0 < len(sentinels) <= PACKAGED_ACCEPTANCE.SECRET_SCAN.MAX_SENTINELS
        and all(isinstance(item, dict) for item in sentinels)
        and sentinels == sorted(sentinels, key=lambda item: item.get("name", "")),
        "packaged backend acceptance privacy sentinel inventory is invalid",
    )
    sentinel_names: set[str] = set()
    for sentinel in sentinels:
        require(
            isinstance(sentinel, dict)
            and set(sentinel) == {"name", "sha256", "size_bytes"}
            and isinstance(sentinel.get("name"), str)
            and bool(sentinel["name"])
            and sentinel["name"].replace("-", "").isalnum()
            and sentinel["name"] not in sentinel_names
            and type(sentinel.get("size_bytes")) is int
            and 16
            <= sentinel["size_bytes"]
            <= PACKAGED_ACCEPTANCE.SECRET_SCAN.MAX_SENTINEL_BYTES,
            "packaged backend acceptance privacy sentinel record is invalid",
        )
        sha256_reference(sentinel.get("sha256"), "privacy sentinel")
        sentinel_names.add(sentinel["name"])
    require(
        all(
            any(name.startswith(prefix) for name in sentinel_names)
            for prefix in (
                "authority-secret-",
                "operator-capability-",
                "postgres-admin-password-",
                "postgres-runtime-password-",
                "postgres-admin-dsn-",
                "postgres-runtime-dsn-",
            )
        ),
        "packaged backend acceptance privacy sentinel classes are incomplete",
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
            "browser_story",
            "performance",
            "cleanup",
            "privacy",
        }
        and report.get("schema") == PACKAGED_ACCEPTANCE.SCHEMA
        and report.get("canonical_encoding") == "utf8-sorted-key-compact-json-lf"
        and report.get("status") == "pass"
        and report.get("release_evidence") is True
        and report.get("secrets_emitted") is False
        and report.get("cleanup") == "pass",
        "packaged backend acceptance is not a complete releasable six-cell plus browser report",
    )
    browser_story = report.get("browser_story")
    require(
        isinstance(browser_story, dict),
        "packaged backend acceptance browser story is missing",
    )
    try:
        PACKAGED_ACCEPTANCE._validate_browser_story(
            browser_story,
            expected_binding,
            PACKAGED_ACCEPTANCE.PINNED_BROWSER_IDENTITY,
        )
    except PACKAGED_ACCEPTANCE.LaneFailure as error:
        raise ReferenceError(
            f"packaged backend acceptance browser story failed validation: {error}"
        ) from error
    verify_packaged_privacy(report.get("privacy"))
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
    engines = environment.get("engines") if isinstance(environment, dict) else None
    try:
        AGGREGATOR._validate_reference_environment(
            "packaged-acceptance", {"reference_environment": environment}
        )
    except AGGREGATOR.EvidenceError as error:
        raise ReferenceError(
            f"packaged backend acceptance reference host is not certified: {error}"
        ) from error
    require(
        isinstance(environment, dict)
        and isinstance(engines, dict)
        and engines.get("postgresql")
        == {
            "version": "17.11",
            "settings": {
                "server_version_num": "170011",
                "synchronous_commit": "on",
                "transaction_isolation": "read committed",
                **PACKAGED_ACCEPTANCE.POSTGRESQL_CONTRACT_SETTINGS,
            },
            "connection_mode": "direct_and_transaction_pooler",
        }
        and engines.get("sqlite")
        == {
            "version": "3.53.4",
            "settings": PACKAGED_ACCEPTANCE.SQLITE_REFERENCE_SETTINGS,
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


def exact_binding(raw: bytes, schema: str) -> dict[str, Any]:
    return {
        "sha256": SHA256_PREFIX + hashlib.sha256(raw).hexdigest(),
        "size_bytes": len(raw),
        "schema": schema,
    }


def file_binding(path: Path, schema: str) -> dict[str, Any]:
    regular_file(path, f"{schema} binding input")
    return {
        "sha256": sha256(path),
        "size_bytes": path.stat().st_size,
        "schema": schema,
    }


def packaged_sdk_identity(archive: Path) -> dict[str, Any]:
    """Digest the exact Python SDK inputs retained in the verified native archive."""

    suffixes = {
        "pyproject_sha256": "/sdk/python/pyproject.toml",
        "lock_sha256": "/sdk/python/uv.lock",
        "module_init_sha256": "/sdk/python/src/worldstream_sdk/__init__.py",
        "client_module_sha256": "/sdk/python/src/worldstream_sdk/client.py",
        "compatibility_identity_sha256": (
            "/sdk/python/src/worldstream_sdk/compatibility_identity.json"
        ),
    }
    observed: dict[str, Any] = {}
    try:
        with tarfile.open(archive, "r:gz") as package:
            members = package.getmembers()
            for field, suffix in suffixes.items():
                matches = [
                    member
                    for member in members
                    if member.name.endswith(suffix) and member.isfile()
                ]
                require(len(matches) == 1, f"archive must contain one exact {suffix}")
                source = package.extractfile(matches[0])
                require(source is not None, f"archive {suffix} could not be read")
                raw = source.read(16 * 1024 * 1024 + 1)
                require(
                    0 < len(raw) <= 16 * 1024 * 1024,
                    f"archive {suffix} exceeded its byte bound",
                )
                observed[field] = SHA256_PREFIX + hashlib.sha256(raw).hexdigest()
                observed[field.replace("_sha256", "_size_bytes")] = len(raw)
    except (OSError, tarfile.TarError) as error:
        raise ReferenceError("packaged SDK identity could not be read") from error
    return {
        "source": "verified_native_archive",
        **observed,
        "runtime_module_under_packaged_sdk_root": True,
    }


def validate_reference_target(
    report: dict[str, Any],
    *,
    manifest: dict[str, Any],
    distribution: dict[str, Any],
    packaged_acceptance_sha256: str,
    expected_bindings: dict[str, dict[str, Any]],
    expected_external_sources: dict[str, dict[str, Any]],
    expected_packaged_sdk: dict[str, Any],
    kill_cell_count: int,
    soak_elapsed_seconds: float,
) -> dict[str, Any]:
    """Validate the exact real frozen-target attempt, including honest misses."""

    require(
        set(report)
        == {
            "schema",
            "status",
            "release_evidence",
            "performance_class",
            "execution",
            "profile",
            "identity",
            "reference_environment",
            "reference_workload",
            "runtime_observation",
            "bindings",
            "bound_external_sources",
            "dimensions",
            "correctness",
            "measurements",
            "limitations",
        }
        and report.get("schema") == REFERENCE_TARGET_SCHEMA
        and report.get("status") == "completed"
        and report.get("release_evidence") is False
        and report.get("performance_class") == "reference_non_release",
        "frozen reference-target report is incomplete",
    )
    execution = report.get("execution")
    runner_digest = sha256(REFERENCE_TARGET_RUNNER_PATH)
    require(
        isinstance(execution, dict)
        and set(execution)
        == {
            "mode",
            "workload_source",
            "storage_profile",
            "connection_mode",
            "public_api_only",
            "simulated",
            "scaled",
            "profile_args_locked",
            "runner_sha256",
        }
        and execution.get("mode") == "package_bound_linux_reference"
        and execution.get("workload_source")
        == "packaged_worldstreamd_public_api_plus_source_bound_fixture_setup"
        and execution.get("storage_profile") == "sqlite-bundled"
        and execution.get("connection_mode") == "embedded"
        and execution.get("public_api_only") is False
        and execution.get("simulated") is False
        and execution.get("scaled") is False
        and execution.get("profile_args_locked") is True
        and execution.get("runner_sha256") == runner_digest,
        "reference target was scaled, simulated, or not run by the bound runner",
    )
    require(
        report.get("profile") == FROZEN_REFERENCE_PROFILE,
        "reference target did not use the exact frozen release profile",
    )
    identity = report.get("identity")
    require(
        isinstance(identity, dict)
        and set(identity)
        == {
            "product",
            "profile",
            "version",
            "artifact_sha256",
            "binary_sha256",
            "manifest_json_sha256",
            "manifest_toml_sha256",
            "packaged_acceptance_sha256",
            "packaged_sdk",
        }
        and identity.get("product") == "worldstream"
        and identity.get("profile") == "linux-reference"
        and identity.get("version") == manifest["release_candidate"]
        and identity.get("artifact_sha256") == distribution["archive_sha256"]
        and identity.get("binary_sha256") == distribution["binary_sha256"]
        and identity.get("manifest_json_sha256") == distribution["manifest_json_sha256"]
        and identity.get("manifest_toml_sha256") == distribution["manifest_toml_sha256"]
        and identity.get("packaged_acceptance_sha256") == packaged_acceptance_sha256,
        "reference target package/manifest identity is not exact",
    )
    require(
        identity.get("packaged_sdk") == expected_packaged_sdk,
        "reference target did not execute the SDK bytes in the verified native archive",
    )
    try:
        AGGREGATOR._validate_reference_environment(
            "target", {"reference_environment": report.get("reference_environment")}
        )
        AGGREGATOR._validate_reference_workload(
            "target", {"reference_workload": report.get("reference_workload")}
        )
    except AGGREGATOR.EvidenceError as error:
        raise ReferenceError(
            f"reference target disclosure is invalid: {error}"
        ) from error
    runtime_observation = report.get("runtime_observation")
    sqlite_runtime = (
        runtime_observation.get("sqlite")
        if isinstance(runtime_observation, dict)
        else None
    )
    require(
        isinstance(runtime_observation, dict)
        and set(runtime_observation) == {"sqlite", "postgresql"}
        and isinstance(sqlite_runtime, dict)
        and set(sqlite_runtime) == {"status", "profile", "exact_identity"}
        and sqlite_runtime.get("status") == "observed_runtime_verified"
        and sqlite_runtime.get("profile") == "sqlite-bundled"
        and isinstance(sqlite_runtime.get("exact_identity"), str)
        and sqlite_runtime["exact_identity"].startswith("sqlite/3.53.4;")
        and runtime_observation.get("postgresql")
        == {"status": "not_observed_by_sqlite_target_workload"},
        "reference target runtime-engine observation is incomplete",
    )
    require(
        report.get("bindings") == expected_bindings,
        "reference target raw byte bindings differ from release inputs",
    )
    require(
        report.get("bound_external_sources") == expected_external_sources,
        "reference target merged or mislabeled external-source facts",
    )
    dimensions = report.get("dimensions")
    require(
        isinstance(dimensions, list)
        and len(dimensions) == len(REFERENCE_TARGET_DIMENSIONS),
        "reference target dimension inventory is incomplete",
    )
    by_id: dict[str, dict[str, Any]] = {}
    for row in dimensions:
        require(
            isinstance(row, dict)
            and set(row)
            == {
                "id",
                "classification",
                "attempted",
                "completed",
                "observed",
                "target",
                "target_met",
                "outcome",
                "method",
            }
            and row.get("id") in REFERENCE_TARGET_DIMENSIONS
            and row.get("id") not in by_id
            and row.get("classification")
            in {"measured_non_sla_performance", "bound_hard_gate"}
            and row.get("attempted") is True
            and type(row.get("completed")) is bool
            and isinstance(row.get("observed"), dict)
            and isinstance(row.get("target"), dict)
            and type(row.get("target_met")) is bool
            and row.get("outcome") == ("met" if row.get("target_met") else "missed")
            and isinstance(row.get("method"), str)
            and row["method"],
            "reference target contains a malformed, duplicate, or unattempted dimension",
        )
        by_id[row["id"]] = row
    require(
        tuple(row["id"] for row in dimensions) == REFERENCE_TARGET_DIMENSIONS,
        "reference target dimensions are reordered or substituted",
    )
    for identifier, row in by_id.items():
        expected_classification = (
            "bound_hard_gate"
            if identifier
            in {
                "one_hour_bounded_soak",
                "repeated_forced_termination_no_acknowledged_loss",
            }
            else "measured_non_sla_performance"
        )
        require(
            row["classification"] == expected_classification
            and row["completed"] is True,
            f"{identifier} completion/classification is inconsistent",
        )

    def integer_field(value: Any, name: str) -> int:
        require(type(value) is int and value >= 0, f"{name} must be non-negative")
        return value

    stored = by_id["stored_passivated_rooms"]
    require(
        stored["target"] == {"minimum_room_count": 10_000}
        and set(stored["observed"])
        == {"create_attempt_count", "created_room_count", "offline_stored_room_count"}
        and integer_field(
            stored["observed"].get("create_attempt_count"), "Room create attempts"
        )
        == 10_000
        and integer_field(stored["observed"].get("created_room_count"), "created Rooms")
        == integer_field(
            stored["observed"].get("offline_stored_room_count"), "stored Rooms"
        )
        and stored["target_met"]
        is (stored["observed"]["offline_stored_room_count"] >= 10_000),
        "stored/passivated Room target measurement is invalid",
    )
    loaded = by_id["simultaneously_loaded_rooms"]
    require(
        loaded["target"] == {"minimum_loaded_room_count": 100}
        and set(loaded["observed"])
        == {"open_attempt_count", "peak_simultaneously_loaded_room_count"}
        and integer_field(
            loaded["observed"].get("open_attempt_count"), "loaded Room attempts"
        )
        >= 100
        and integer_field(
            loaded["observed"].get("peak_simultaneously_loaded_room_count"),
            "loaded Rooms",
        )
        <= loaded["observed"]["open_attempt_count"]
        and loaded["target_met"]
        is (loaded["observed"]["peak_simultaneously_loaded_room_count"] >= 100),
        "simultaneously loaded Room target measurement is invalid",
    )
    idle = by_id["mostly_idle_websocket_sessions"]
    require(
        idle["target"] == {"minimum_session_count": 1_000}
        and set(idle["observed"])
        == {
            "open_attempt_count",
            "peak_connected_idle_session_count",
            "observation_window_seconds",
        }
        and integer_field(
            idle["observed"].get("open_attempt_count"), "idle Session attempts"
        )
        == 1_000
        and integer_field(
            idle["observed"].get("peak_connected_idle_session_count"),
            "idle Sessions",
        )
        <= 1_000
        and type(idle["observed"].get("observation_window_seconds")) in {int, float}
        and idle["observed"]["observation_window_seconds"] >= 0
        and idle["target_met"]
        is (idle["observed"]["peak_connected_idle_session_count"] >= 1_000),
        "mostly-idle WebSocket target measurement is invalid",
    )
    rate = by_id["sustained_accepted_transition_rate"]
    rate_observed = rate["observed"]
    require(
        rate["target"]
        == {"minimum_per_second": 100, "continuous_window_seconds": 1_800}
        and set(rate_observed)
        == {
            "configured_window_seconds",
            "bucket_definition",
            "window_start_basis",
            "elapsed_seconds",
            "full_second_bucket_count",
            "dispatch_attempt_count",
            "dispatch_queue_full_count",
            "action_attempt_count",
            "unconsumed_token_count",
            "minimum_accepted_per_full_second",
            "accepted_transition_count",
            "late_accepted_transition_count",
            "aggregate_accepted_per_second",
        }
        and rate_observed.get("configured_window_seconds") == 1_800
        and rate_observed.get("bucket_definition")
        == "1800 contiguous half-open one-second buckets from a monotonic start boundary"
        and rate_observed.get("window_start_basis")
        == "monotonic_after_worker_readiness"
        and type(rate_observed.get("elapsed_seconds")) in {int, float}
        and rate_observed["elapsed_seconds"] >= 1_800
        and integer_field(rate_observed.get("full_second_bucket_count"), "rate buckets")
        == 1_800
        and integer_field(
            rate_observed.get("dispatch_attempt_count"), "dispatch attempts"
        )
        >= integer_field(rate_observed.get("action_attempt_count"), "Action attempts")
        and integer_field(
            rate_observed.get("dispatch_queue_full_count"), "full dispatch queue"
        )
        <= rate_observed["dispatch_attempt_count"]
        and integer_field(
            rate_observed.get("unconsumed_token_count"), "unconsumed tokens"
        )
        <= 512
        and rate_observed["dispatch_attempt_count"]
        - rate_observed["dispatch_queue_full_count"]
        == rate_observed["action_attempt_count"]
        + rate_observed["unconsumed_token_count"]
        and integer_field(
            rate_observed.get("minimum_accepted_per_full_second"), "minimum rate"
        )
        >= 0
        and integer_field(
            rate_observed.get("accepted_transition_count"), "accepted transitions"
        )
        >= 0
        and integer_field(
            rate_observed.get("late_accepted_transition_count"),
            "late accepted transitions",
        )
        >= 0
        and rate_observed["action_attempt_count"]
        == rate_observed["accepted_transition_count"]
        + rate_observed["late_accepted_transition_count"]
        and rate_observed["accepted_transition_count"]
        >= rate_observed["minimum_accepted_per_full_second"] * 1_800
        and type(rate_observed.get("aggregate_accepted_per_second")) in {int, float}
        and rate_observed["aggregate_accepted_per_second"]
        == round(rate_observed["accepted_transition_count"] / 1_800, 3)
        and rate["target_met"]
        is (
            rate["completed"]
            and rate_observed["full_second_bucket_count"] == 1_800
            and rate_observed["minimum_accepted_per_full_second"] >= 100
        ),
        "sustained transition-rate measurement is invalid",
    )
    latency = by_id["commit_to_ack_latency"]
    latency_observed = latency["observed"]
    require(
        latency["target"] == {"maximum_p95_ms_exclusive": 100}
        and set(latency_observed)
        == {"definition", "sample_count", "p50_ms", "p95_ms", "p99_ms"},
        "commit-to-ack latency shape is invalid",
    )
    triplet(latency_observed, "reference target commit-to-ack latency")
    require(
        latency_observed["sample_count"] == rate_observed["accepted_transition_count"]
        and latency["target_met"] is (latency_observed["p95_ms"] < 100),
        "commit-to-ack p95 target result is inconsistent",
    )
    soak = by_id["one_hour_bounded_soak"]
    require(
        soak["classification"] == "bound_hard_gate"
        and soak["completed"] is True
        and soak["target"] == {"minimum_elapsed_seconds": 3_600}
        and soak["observed"]
        == {
            "elapsed_seconds": soak_elapsed_seconds,
            "window_completed": True,
            "report_sha256": expected_bindings["one_hour_soak"]["sha256"],
            "source_attribution": "bound_external_source",
        }
        and soak["target_met"] is True,
        "one-hour bounded soak is not exactly bound",
    )
    history = by_id["snapshot_tail_recovery"]
    require(
        history["target"]
        == {
            "minimum_room_transition_count": 100_000,
            "maximum_snapshot_lag_transitions": 250,
            "maximum_recovery_ms": 5_000,
        },
        "snapshot-tail recovery target drifted",
    )
    history_observed = history["observed"]
    require(
        set(history_observed)
        == {
            "requested_transition_count",
            "accepted_transition_count",
            "setup",
            "snapshot",
            "recovery",
        }
        and history_observed.get("requested_transition_count") == 100_000
        and history_observed.get("accepted_transition_count") == 100_000,
        "snapshot-tail recovery did not complete exactly 100,000 Transitions",
    )
    setup = history_observed["setup"]
    require(
        isinstance(setup, dict)
        and set(setup)
        == {
            "boundary",
            "mode",
            "source_revision",
            "generator_source_sha256",
            "generator_binary_sha256",
            "generator_report_sha256",
            "transition_kind",
            "setup_elapsed_ms",
        }
        and setup.get("boundary") == "excluded_before_recovery_stopwatch"
        and setup.get("mode") == "source_bound_deterministic_production_core_commits"
        and setup.get("source_revision") == distribution.get("source_revision")
        and setup.get("transition_kind")
        == "alternating_authorized_membership_suspend_resume"
        and type(setup.get("setup_elapsed_ms")) is int
        and setup["setup_elapsed_ms"] >= 0,
        "snapshot-tail setup boundary or source identity is incomplete",
    )
    for field in (
        "generator_source_sha256",
        "generator_binary_sha256",
        "generator_report_sha256",
    ):
        sha256_reference(setup.get(field), f"snapshot fixture {field}")
    require(
        setup["generator_source_sha256"]
        == expected_bindings["snapshot_fixture_source"]["sha256"]
        and setup["generator_binary_sha256"]
        == expected_bindings["snapshot_fixture_binary"]["sha256"],
        "snapshot fixture source or executable differs from its exact binding",
    )
    snapshot = history_observed["snapshot"]
    require(
        snapshot
        == {
            "head_room_seq": 100_000,
            "newest_snapshot_room_seq": 99_998,
            "snapshot_lag_transitions": 2,
        },
        "snapshot-tail fixture did not retain the exact recent non-empty tail",
    )
    recovery = history_observed["recovery"]
    require(
        isinstance(recovery, dict)
        and set(recovery)
        == {
            "boundary",
            "daemon_binary_sha256",
            "recovery_ms",
            "before_complete_head_sha256",
            "after_complete_head_sha256",
            "complete_head_equal",
            "before_projection_hash",
            "after_projection_hash",
            "projection_hash_equal",
        }
        and recovery.get("boundary")
        == "fresh_packaged_daemon_process_start_through_verified_current_projection"
        and recovery.get("daemon_binary_sha256") == distribution["binary_sha256"]
        and type(recovery.get("recovery_ms")) in {int, float}
        and recovery["recovery_ms"] >= 0
        and recovery.get("complete_head_equal") is True
        and recovery.get("projection_hash_equal") is True
        and recovery.get("before_complete_head_sha256")
        == recovery.get("after_complete_head_sha256")
        and recovery.get("before_projection_hash")
        == recovery.get("after_projection_hash"),
        "fresh packaged recovery did not preserve the exact Head and Projection",
    )
    sha256_reference(
        recovery.get("before_complete_head_sha256"), "pre-recovery complete Head"
    )
    require(
        all(
            isinstance(recovery.get(field), str)
            and recovery[field].startswith("blake3:")
            and len(recovery[field]) == len("blake3:") + 64
            for field in ("before_projection_hash", "after_projection_hash")
        ),
        "snapshot-tail Projection hashes are malformed",
    )
    expected_history_met = (
        history_observed["accepted_transition_count"] >= 100_000
        and snapshot["snapshot_lag_transitions"] <= 250
        and recovery["recovery_ms"] <= 5_000
        and recovery["complete_head_equal"] is True
        and recovery["projection_hash_equal"] is True
    )
    require(
        history["target_met"] is expected_history_met,
        "snapshot-tail target result was not computed from the observation",
    )
    forced = by_id["repeated_forced_termination_no_acknowledged_loss"]
    require(
        forced["classification"] == "bound_hard_gate"
        and forced["completed"] is True
        and forced["target"]
        == {"minimum_forced_termination_count": 2, "maximum_acknowledged_loss_count": 0}
        and forced["observed"]
        == {
            "forced_termination_count": kill_cell_count,
            "acknowledged_loss_count": 0,
            "kill_report_sha256": expected_bindings["kill_point"]["sha256"],
            "source_attribution": "bound_external_source",
        }
        and kill_cell_count >= 2
        and forced["target_met"] is True,
        "forced-termination no-acknowledged-loss result is not exactly bound",
    )
    correctness = report.get("correctness")
    require(
        correctness
        == {
            "status": "passed",
            "acknowledged_transition_ids_unique": True,
            "acknowledged_transition_receipts_verified": True,
            "acknowledged_loss_count": 0,
            "hard_gate_inputs_validated": True,
        },
        "reference target contains a correctness or acknowledged-loss failure",
    )
    measurements = report.get("measurements")
    require(
        isinstance(measurements, dict)
        and set(measurements)
        == {"commit_to_ack_latency", "transition_rate", "resource_observation"}
        and measurements.get("commit_to_ack_latency") == latency_observed
        and measurements.get("transition_rate") == rate_observed
        and isinstance(measurements.get("resource_observation"), dict)
        and set(measurements["resource_observation"])
        == {
            "peak_process_tree_rss_bytes",
            "process_tree_rss_sample_count",
            "database_bytes_after_workload",
        }
        and integer_field(
            measurements["resource_observation"].get("peak_process_tree_rss_bytes"),
            "reference target peak RSS",
        )
        > 0
        and integer_field(
            measurements["resource_observation"].get("process_tree_rss_sample_count"),
            "reference target RSS samples",
        )
        > 0
        and integer_field(
            measurements["resource_observation"].get("database_bytes_after_workload"),
            "reference target database bytes",
        )
        > 0,
        "reference target retained measurements are incomplete",
    )
    limitations = report.get("limitations")
    require(
        isinstance(limitations, list)
        and limitations
        == (
            []
            if expected_history_met
            else [
                {
                    "code": "snapshot_tail_recovery_target_missed",
                    "dimension": "snapshot_tail_recovery",
                    "publishable_non_sla_target_miss": True,
                }
            ]
        ),
        "reference target limitation disclosure is incomplete or fabricated",
    )
    return {
        "status": "completed",
        "profile": FROZEN_REFERENCE_PROFILE,
        "dimensions": dimensions,
        "target_miss_ids": [
            row["id"] for row in dimensions if row["target_met"] is False
        ],
        "measurements": measurements,
        "environment": report["reference_environment"],
        "workload": report["reference_workload"],
    }


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
            "reference_environments",
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
    environments = report.get("reference_environments")
    require(
        isinstance(environments, dict) and set(environments) == EXPECTED_KINDS,
        "reference report per-source environment disclosures are incomplete",
    )
    for kind, environment in environments.items():
        try:
            AGGREGATOR._validate_reference_environment(
                kind, {"reference_environment": environment}
            )
        except AGGREGATOR.EvidenceError as error:
            raise ReferenceError(
                f"{kind} reference environment disclosure is invalid: {error}"
            ) from error
        require(
            ("storage_bindings" in environment)
            is (kind in {"counter", "heist", "postgres"}),
            f"{kind} storage facts were merged across evidence sources",
        )
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
    require(isinstance(inputs, list) and len(inputs) == 6, "six inputs are required")
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
        "reference report byte inventory does not match the six named inputs",
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
            and source.get("reference_environment") == environments[kind]
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
    targets = measurements["reference_targets"]["sources"]["target"]
    require(
        isinstance(targets, dict)
        and set(targets)
        == {
            "schema",
            "profile",
            "dimensions",
            "target_miss_ids",
            "raw_report_sha256",
        }
        and targets.get("schema") == REFERENCE_TARGET_PROFILE_SCHEMA
        and targets.get("profile") == FROZEN_REFERENCE_PROFILE
        and isinstance(targets.get("dimensions"), list)
        and tuple(row.get("id") for row in targets["dimensions"])
        == REFERENCE_TARGET_DIMENSIONS
        and isinstance(targets.get("target_miss_ids"), list)
        and targets["target_miss_ids"]
        == [
            row["id"] for row in targets["dimensions"] if row.get("target_met") is False
        ],
        "frozen reference-target projection is incomplete",
    )
    sha256_reference(targets.get("raw_report_sha256"), "raw reference target")
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
    kill_point_path: Path,
    target_path: Path,
    package_archive: Path,
    package_report_path: Path,
    daemon_bin: Path,
    snapshot_fixture_bin: Path,
    manifest_toml: Path,
    manifest_json: Path,
) -> tuple[dict[str, bytes], bytes, bytes, bytes, bytes]:
    """Re-run the owning projection from all exact raw source bytes."""

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
                kill_point_report=kill_point_path,
                target_report=target_path,
                package_archive=package_archive,
                package_report=package_report_path,
                daemon_bin=daemon_bin,
                snapshot_fixture_bin=snapshot_fixture_bin,
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
    _kill, kill_raw = read_json(kill_point_path, "raw kill-point matrix report")
    _target, target_raw = read_json(target_path, "raw reference-target report")
    return projected, aggregate, soak_raw, kill_raw, target_raw


def produce(
    output: Path,
    artifact_output: Path,
    report_path: Path,
    input_paths: dict[str, Path],
    package_archive: Path,
    package_report_path: Path,
    daemon_bin: Path,
    snapshot_fixture_bin: Path,
    packaged_acceptance_path: Path,
    raw_soak_path: Path,
    kill_point_path: Path,
    raw_target_path: Path,
    manifest_toml: Path,
    manifest_json: Path,
) -> None:
    require(
        set(input_paths) == EXPECTED_KINDS,
        "reference producer requires all six exact named input reports",
    )
    all_inputs = {
        report_path.resolve(),
        package_archive.resolve(),
        package_report_path.resolve(),
        daemon_bin.resolve(),
        snapshot_fixture_bin.resolve(),
        packaged_acceptance_path.resolve(),
        raw_soak_path.resolve(),
        kill_point_path.resolve(),
        raw_target_path.resolve(),
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
    (
        projected_inputs,
        projected_aggregate,
        soak_raw,
        kill_raw,
        target_raw,
    ) = independently_project_raw_sources(
        packaged_acceptance_path=packaged_acceptance_path,
        soak_path=raw_soak_path,
        kill_point_path=kill_point_path,
        target_path=raw_target_path,
        package_archive=package_archive,
        package_report_path=package_report_path,
        daemon_bin=daemon_bin,
        snapshot_fixture_bin=snapshot_fixture_bin,
        manifest_toml=manifest_toml,
        manifest_json=manifest_json,
    )
    for kind, path in sorted(input_paths.items()):
        _value, raw = read_json(path, f"{kind} reference input")
        require(
            raw == projected_inputs[kind],
            f"{kind} reference input is not the exact projection of all raw source bytes",
        )
    require(
        report_raw == projected_aggregate,
        "reference aggregate is not the exact independent projection of all raw source bytes",
    )
    recomputed = recompute_report(input_paths)
    require(
        recomputed == report,
        "reference report does not exactly match recomputation from the six named input bytes",
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
        "reference_environments": report["reference_environments"],
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
        "raw_kill_point_report": {
            "sha256": SHA256_PREFIX + hashlib.sha256(kill_raw).hexdigest(),
            "size_bytes": len(kill_raw),
            "content_base64": base64.b64encode(kill_raw).decode("ascii"),
            "schema": "worldstream/kill-point-evidence/v1",
        },
        "raw_reference_target_report": {
            "sha256": SHA256_PREFIX + hashlib.sha256(target_raw).hexdigest(),
            "size_bytes": len(target_raw),
            "content_base64": base64.b64encode(target_raw).decode("ascii"),
            "schema": REFERENCE_TARGET_SCHEMA,
        },
        "projection": {
            "definition": "exact independent projection from raw acceptance, soak, kill, and target bytes",
            "packaged_acceptance_sha256": packaged_acceptance_sha256,
            "raw_soak_sha256": SHA256_PREFIX + hashlib.sha256(soak_raw).hexdigest(),
            "raw_kill_point_sha256": SHA256_PREFIX
            + hashlib.sha256(kill_raw).hexdigest(),
            "raw_reference_target_sha256": SHA256_PREFIX
            + hashlib.sha256(target_raw).hexdigest(),
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
            "target comparisons are measured non-SLA;target misses remain publishable"
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
    command.add_argument("--snapshot-fixture-bin", type=Path, required=True)
    command.add_argument("--packaged-acceptance-report", type=Path, required=True)
    command.add_argument("--raw-soak-report", type=Path, required=True)
    command.add_argument("--kill-point-report", type=Path, required=True)
    command.add_argument("--raw-target-report", type=Path, required=True)
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
            args.snapshot_fixture_bin,
            args.packaged_acceptance_report,
            args.raw_soak_report,
            args.kill_point_report,
            args.raw_target_report,
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

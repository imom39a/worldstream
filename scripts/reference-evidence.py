#!/usr/bin/env python3
"""Aggregate bounded, non-release Linux reference measurements.

This command consumes five legacy reports plus the frozen-target projection.
It never runs a
benchmark, reads a database, or turns a fixture into release evidence.  The
input reports must carry the common artifact identity and non-release labels
documented in ``docs/agents/imo-61-reference-evidence-luna.md``.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import math
import sys
from pathlib import Path
from typing import Any

SCHEMA = "worldstream/imo-61-reference-evidence/v1"
ROOT = Path(__file__).resolve().parents[1]
BUILD_IDENTITY_PATH = ROOT / "scripts/release_build_identity.py"
MAX_INPUT_BYTES = 8 * 1024 * 1024
MAX_JSON_DEPTH = 16
MAX_SAMPLES = 1_000_000
MAX_GAPS = 64
DIGEST_PREFIX = "sha256:"
SHA256_HEX_LENGTH = 64
CERTIFIED_REFERENCE_MEMORY_BYTES = 8 * 1024 * 1024 * 1024

EXPECTED_SCHEMAS: dict[str, tuple[str, ...]] = {
    "counter": ("worldstream/imo-55-live-counter-luna/v1",),
    "heist": (
        "worldstream/imo-53-57-live-wave11/v1",
        "worldstream/imo-57-absent-broker-story/v1",
        "worldstream/imo-57-absent-broker-live/v1",
    ),
    "sqlite": (
        "worldstream/soak-evidence/v1",
        "worldstream/native-sqlite-restore-evidence/v1",
        "worldstream/sqlite-native-restore-evidence/v1",
    ),
    "postgres": (
        "worldstream/postgresql-evidence/v1",
        "worldstream/postgresql-live-evidence/v1",
        "worldstream/native-postgres-restore-evidence/v2",
    ),
    "soak": ("worldstream/soak-evidence/v1",),
    "target": ("worldstream/reference-target-projection/v1",),
}

EXPECTED_STATUSES: dict[str, tuple[str, ...]] = {
    "counter": ("completed",),
    "heist": ("completed",),
    "sqlite": ("pass", "passed", "ready", "completed"),
    "postgres": ("pass", "passed", "ready", "completed"),
    "soak": ("pass",),
    "target": ("completed",),
}

MEASUREMENT_NAMES = ("measurements", "measurement", "metrics")
LATENCY_KEYS = ("latency_ms", "ack_latency_ms", "action_latency_ms")

REQUIRED_COVERAGE: dict[str, tuple[str, ...]] = {
    "counter": ("latency_ms", "load", "fan_out"),
    "heist": ("latency_ms", "load", "fan_out"),
    "sqlite": ("latency_ms", "memory", "database_growth", "recovery"),
    "postgres": ("latency_ms", "memory", "database_growth", "recovery"),
    "soak": ("latency_ms", "memory", "database_growth"),
    "target": ("reference_targets",),
}


class EvidenceError(ValueError):
    """A deterministic, user-actionable input validation failure."""


def _load_build_identity():
    name = "worldstream_reference_strict_json"
    existing = sys.modules.get(name)
    if existing is not None:
        return existing
    spec = importlib.util.spec_from_file_location(name, BUILD_IDENTITY_PATH)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {BUILD_IDENTITY_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


BUILD_IDENTITY = _load_build_identity()


def _is_number(value: Any) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
    )


def _digest(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def _sha256_reference(value: bytes) -> str:
    return f"{DIGEST_PREFIX}{_digest(value)}"


def _canonical(value: Any) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
        + "\n"
    ).encode("utf-8")


def _check_depth(value: Any, depth: int = 0) -> None:
    if depth > MAX_JSON_DEPTH:
        raise EvidenceError("input_json_depth_exceeds_bound")
    if isinstance(value, dict):
        if len(value) > 512:
            raise EvidenceError("input_object_key_count_exceeds_bound")
        for key, child in value.items():
            if not isinstance(key, str) or len(key) > 256:
                raise EvidenceError("input_object_key_invalid")
            _check_depth(child, depth + 1)
    elif isinstance(value, list):
        if len(value) > MAX_SAMPLES:
            raise EvidenceError("input_array_length_exceeds_bound")
        for child in value:
            _check_depth(child, depth + 1)
    elif isinstance(value, str) and len(value) > 4096:
        raise EvidenceError("input_string_length_exceeds_bound")


def _read_report(kind: str, path_value: str) -> tuple[dict[str, Any], dict[str, Any]]:
    path = Path(path_value)
    if path.is_symlink() or not path.is_file():
        raise EvidenceError(f"{kind}:input_not_regular_file")
    size = path.stat().st_size
    if size > MAX_INPUT_BYTES:
        raise EvidenceError(f"{kind}:input_exceeds_8_mib_bound")
    try:
        raw = path.read_bytes()
    except OSError as error:
        raise EvidenceError(f"{kind}:input_is_not_utf8_json") from error
    try:
        report = BUILD_IDENTITY.strict_json(raw, f"{kind} reference input")
    except BUILD_IDENTITY.IdentityError as error:
        raise EvidenceError(f"{kind}:input_is_not_strict_json:{error}") from error
    _check_depth(report)
    inventory = {
        "kind": kind,
        "bytes": len(raw),
        "sha256": _sha256_reference(raw),
        "schema": report.get("schema"),
        "status": report.get("status"),
    }
    return report, inventory


def _require_digest(value: Any, field: str) -> None:
    if (
        not isinstance(value, str)
        or len(value) != len(DIGEST_PREFIX) + SHA256_HEX_LENGTH
    ):
        raise EvidenceError(f"{field}:sha256_reference_required")
    if not value.startswith(DIGEST_PREFIX) or any(
        c not in "0123456789abcdef" for c in value[len(DIGEST_PREFIX) :]
    ):
        raise EvidenceError(f"{field}:sha256_reference_required")


def _validate_identity(kind: str, report: dict[str, Any]) -> dict[str, Any]:
    identity = report.get("identity")
    if not isinstance(identity, dict):
        raise EvidenceError(f"{kind}:identity_object_required")
    if identity.get("product") != "worldstream":
        raise EvidenceError(f"{kind}:identity_product_must_be_worldstream")
    if identity.get("profile") != "linux-reference":
        raise EvidenceError(f"{kind}:identity_profile_must_be_linux_reference")
    for field in ("profile", "version"):
        if not isinstance(identity.get(field), str) or not identity[field]:
            raise EvidenceError(f"{kind}:identity_{field}_required")
    _require_digest(identity.get("artifact_sha256"), f"{kind}:identity_artifact_sha256")
    _require_digest(
        identity.get("packaged_acceptance_sha256"),
        f"{kind}:identity_packaged_acceptance_sha256",
    )
    return {
        "product": "worldstream",
        "profile": identity["profile"],
        "version": identity["version"],
        "artifact_sha256": identity["artifact_sha256"],
        "packaged_acceptance_sha256": identity["packaged_acceptance_sha256"],
    }


def _validate_labels(kind: str, report: dict[str, Any]) -> None:
    if report.get("release_evidence") is not False:
        raise EvidenceError(f"{kind}:release_evidence_must_be_false")
    # Accept either spelling used by existing report producers, but require an
    # explicit negative SLA claim.  A prose limitation is not sufficient.
    if report.get("non_release_sla") is True:
        return
    if report.get("sla") is False:
        return
    performance_class = report.get("performance_class")
    if performance_class == "reference_non_release":
        return
    raise EvidenceError(f"{kind}:explicit_non_release_sla_label_required")


def _validate_reference_environment(
    kind: str, report: dict[str, Any]
) -> dict[str, Any]:
    value = report.get("reference_environment")
    if not isinstance(value, dict) or set(value) not in (
        {"platform", "hardware", "filesystem", "engines"},
        {"platform", "hardware", "filesystem", "storage_bindings", "engines"},
    ):
        raise EvidenceError(f"{kind}:reference_environment_shape_required")
    platform = value["platform"]
    if not (
        isinstance(platform, dict)
        and set(platform)
        == {"system", "distribution", "distribution_version", "machine"}
        and platform.get("system") == "Linux"
        and platform.get("distribution") == "Ubuntu"
        and platform.get("distribution_version") == "24.04"
        and platform.get("machine") == "x86_64"
    ):
        raise EvidenceError(f"{kind}:ubuntu_24_04_x86_64_reference_platform_required")
    hardware = value["hardware"]
    if not (
        isinstance(hardware, dict)
        and set(hardware) == {"cpu_model", "logical_cpu_count", "memory_bytes"}
        and isinstance(hardware.get("cpu_model"), str)
        and hardware["cpu_model"].strip()
        and type(hardware.get("logical_cpu_count")) is int
        and hardware["logical_cpu_count"] == 4
        and type(hardware.get("memory_bytes")) is int
        and hardware["memory_bytes"] == CERTIFIED_REFERENCE_MEMORY_BYTES
    ):
        raise EvidenceError(f"{kind}:reference_4vcpu_8gib_hardware_required")
    filesystem = value["filesystem"]
    if not (
        isinstance(filesystem, dict)
        and set(filesystem) == {"type", "mount_options", "storage_class"}
        and filesystem.get("type") == "ext4"
        and filesystem.get("storage_class") == "local_ssd_or_nvme"
        and isinstance(filesystem.get("mount_options"), list)
        and filesystem["mount_options"]
        and all(
            isinstance(option, str) and option.strip()
            for option in filesystem["mount_options"]
        )
    ):
        raise EvidenceError(f"{kind}:reference_ext4_local_ssd_disclosure_required")
    engines = value["engines"]
    if not isinstance(engines, dict) or set(engines) != {"sqlite", "postgresql"}:
        raise EvidenceError(f"{kind}:reference_engine_disclosure_required")
    for engine in ("sqlite", "postgresql"):
        row = engines[engine]
        if engine == "postgresql" and isinstance(row, dict) and set(row) == {"status"}:
            expected_unobserved = (
                "not_observed_by_sqlite_target_workload"
                if kind == "target"
                else "not_observed_by_sqlite_process_soak"
            )
            if row.get("status") == expected_unobserved:
                continue
            raise EvidenceError(f"{kind}:postgresql_engine_disclosure_required")
        if not (
            isinstance(row, dict)
            and set(row) == {"version", "settings", "connection_mode"}
            and isinstance(row.get("version"), str)
            and row["version"].strip()
            and isinstance(row.get("settings"), dict)
            and row["settings"]
            and all(isinstance(key, str) and key for key in row["settings"])
            and isinstance(row.get("connection_mode"), str)
            and row["connection_mode"].strip()
        ):
            raise EvidenceError(f"{kind}:{engine}_engine_disclosure_required")
    if "storage_bindings" in value:
        _validate_storage_bindings(kind, value["storage_bindings"], filesystem)
    return value


def _validate_storage_bindings(
    kind: str, value: object, workload_filesystem: dict[str, Any]
) -> None:
    """Validate the exact workload/provider storage attribution from acceptance."""

    if not (
        isinstance(value, dict)
        and set(value)
        == {
            "schema",
            "status",
            "profile",
            "reference_environment_filesystem_role",
            "layout",
            "raw_paths_retained",
            "workload",
            "postgresql_database",
        }
        and value.get("schema") == "worldstream/packaged-storage-bindings/v1"
        and value.get("status") == "verified"
        and value.get("profile") == "frozen_local_ext4"
        and value.get("reference_environment_filesystem_role")
        == "workload_owned_runtime_parent"
        and value.get("layout") in {"same_host_mount", "split_host_mounts"}
        and value.get("raw_paths_retained") is False
    ):
        raise EvidenceError(f"{kind}:packaged_storage_bindings_required")

    def mount_identity(row: object) -> bool:
        return (
            isinstance(row, dict)
            and set(row) == {"mount_id", "device_id"}
            and type(row.get("mount_id")) is int
            and row["mount_id"] > 0
            and isinstance(row.get("device_id"), str)
            and bool(row["device_id"])
        )

    def frozen_filesystem(row: object) -> bool:
        return (
            isinstance(row, dict)
            and set(row) == {"type", "mount_options", "storage_class"}
            and row.get("type") == "ext4"
            and row.get("storage_class") == "local_ssd_or_nvme"
            and isinstance(row.get("mount_options"), list)
            and bool(row["mount_options"])
            and all(
                isinstance(option, str) and option.strip()
                for option in row["mount_options"]
            )
        )

    workload = value.get("workload")
    database = value.get("postgresql_database")
    docker_volume = (
        database.get("docker_volume") if isinstance(database, dict) else None
    )
    if not (
        isinstance(workload, dict)
        and set(workload) == {"role", "filesystem", "mount_identity"}
        and workload.get("role") == "workload_owned_runtime_parent"
        and workload.get("filesystem") == workload_filesystem
        and frozen_filesystem(workload["filesystem"])
        and mount_identity(workload.get("mount_identity"))
        and isinstance(database, dict)
        and set(database) == {"role", "filesystem", "mount_identity", "docker_volume"}
        and database.get("role") == "provider_owned_database_volume"
        and frozen_filesystem(database.get("filesystem"))
        and mount_identity(database.get("mount_identity"))
        and isinstance(docker_volume, dict)
        and set(docker_volume)
        == {
            "driver",
            "scope",
            "driver_options",
            "container_destination",
            "container_mount_type",
            "read_write",
            "source_matches_volume_mountpoint",
            "run_unique_ownership_label_verified",
        }
        and docker_volume.get("driver") == "local"
        and docker_volume.get("scope") == "local"
        and docker_volume.get("driver_options") == {}
        and docker_volume.get("container_destination") == "/var/lib/postgresql/data"
        and docker_volume.get("container_mount_type") == "volume"
        and docker_volume.get("read_write") is True
        and docker_volume.get("source_matches_volume_mountpoint") is True
        and docker_volume.get("run_unique_ownership_label_verified") is True
    ):
        raise EvidenceError(f"{kind}:packaged_storage_bindings_required")


def _validate_reference_workload(kind: str, report: dict[str, Any]) -> dict[str, Any]:
    workload = report.get("reference_workload")
    if not (
        isinstance(workload, dict)
        and set(workload)
        == {
            "payload_sizes_bytes",
            "pack_id",
            "participants_per_room",
            "fan_out",
            "snapshot_cadence_transitions",
        }
        and isinstance(workload.get("payload_sizes_bytes"), list)
        and workload["payload_sizes_bytes"]
        and all(
            type(size) is int and size > 0 for size in workload["payload_sizes_bytes"]
        )
        and isinstance(workload.get("pack_id"), str)
        and workload["pack_id"].strip()
        and type(workload.get("participants_per_room")) is int
        and workload["participants_per_room"] > 0
        and type(workload.get("fan_out")) is int
        and workload["fan_out"] > 0
        and type(workload.get("snapshot_cadence_transitions")) is int
        and workload["snapshot_cadence_transitions"] > 0
    ):
        raise EvidenceError(f"{kind}:reference_workload_disclosure_required")
    return workload


def _validate_report(kind: str, report: dict[str, Any]) -> dict[str, Any]:
    schema = report.get("schema")
    if schema not in EXPECTED_SCHEMAS[kind]:
        raise EvidenceError(f"{kind}:unsupported_schema")
    status = report.get("status")
    if status not in EXPECTED_STATUSES[kind]:
        raise EvidenceError(f"{kind}:unsupported_status")
    identity = _validate_identity(kind, report)
    _validate_labels(kind, report)
    environment = _validate_reference_environment(kind, report)
    workload = _validate_reference_workload(kind, report)
    if kind == "soak" and report.get("one_hour_window_completed") is not True:
        # This is an honest gap, not a malformed report.  The report remains
        # useful for bounded smoke data, but cannot satisfy the one-hour row.
        pass
    return {
        "schema": schema,
        "status": status,
        "identity": identity,
        "reference_environment": environment,
        "reference_workload": workload,
    }


def _first_dict(
    report: dict[str, Any], names: tuple[str, ...]
) -> dict[str, Any] | None:
    for name in names:
        value = report.get(name)
        if isinstance(value, dict):
            return value
    return None


def _measurement_dict(kind: str, report: dict[str, Any]) -> dict[str, Any]:
    value = _first_dict(report, MEASUREMENT_NAMES)
    if value is None:
        return {}
    return value


def _number(value: Any, field: str) -> float:
    if not _is_number(value) or value < 0:
        raise EvidenceError(f"{field}:non_negative_number_required")
    return float(value)


def _samples(value: Any, field: str) -> list[float]:
    if not isinstance(value, list) or not value:
        raise EvidenceError(f"{field}:non_empty_numeric_samples_required")
    if len(value) > MAX_SAMPLES:
        raise EvidenceError(f"{field}:sample_count_exceeds_bound")
    return [_number(item, field) for item in value]


def _percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    index = max(1, min(len(ordered), math.ceil(len(ordered) * fraction))) - 1
    return round(ordered[index], 3)


def _percentile_triplet(values: list[float]) -> dict[str, Any]:
    return {
        "definition": "nearest-rank",
        "sample_count": len(values),
        "p50_ms": _percentile(values, 0.50),
        "p95_ms": _percentile(values, 0.95),
        "p99_ms": _percentile(values, 0.99),
    }


def _direct_triplet(value: Any, field: str) -> dict[str, Any]:
    if not isinstance(value, dict) or value.get("definition") not in (
        "nearest-rank",
        "nearest_rank",
    ):
        raise EvidenceError(f"{field}:nearest_rank_definition_required")
    result = {
        "definition": "nearest-rank",
        "sample_count": value.get("sample_count"),
        "p50_ms": _number(value.get("p50_ms"), f"{field}.p50_ms"),
        "p95_ms": _number(value.get("p95_ms"), f"{field}.p95_ms"),
        "p99_ms": _number(value.get("p99_ms"), f"{field}.p99_ms"),
    }
    if (
        not isinstance(result["sample_count"], int)
        or isinstance(result["sample_count"], bool)
        or result["sample_count"] <= 0
        or result["sample_count"] > MAX_SAMPLES
    ):
        raise EvidenceError(f"{field}.sample_count:bounded_positive_integer_required")
    if not (result["p50_ms"] <= result["p95_ms"] <= result["p99_ms"]):
        raise EvidenceError(f"{field}:percentiles_must_be_monotonic")
    return result


def _soak_duration_triplet(
    statistics: dict[str, Any], duration: dict[str, Any]
) -> dict[str, Any]:
    if {"sample_count", "p50_ms", "p95_ms", "p99_ms"}.issubset(duration):
        return _direct_triplet(duration, "soak.statistics.command_duration_ms")
    if duration.get("definition") != "nearest-rank percentile":
        raise EvidenceError(
            "soak.statistics.command_duration_ms:nearest_rank_definition_required"
        )
    return _direct_triplet(
        {
            "definition": "nearest-rank",
            "sample_count": statistics.get("matrix_run_count"),
            "p50_ms": duration.get("p50"),
            "p95_ms": duration.get("p95"),
            "p99_ms": duration.get("p99"),
        },
        "soak.statistics.command_duration_ms",
    )


def _latency_for(kind: str, report: dict[str, Any]) -> dict[str, Any] | None:
    measurements = _measurement_dict(kind, report)
    for key in LATENCY_KEYS:
        if key in measurements:
            value = measurements[key]
            if isinstance(value, list):
                return _percentile_triplet(
                    _samples(value, f"{kind}.measurements.{key}")
                )
            return _direct_triplet(value, f"{kind}.measurements.{key}")
        if key in report:
            value = report[key]
            if isinstance(value, list):
                return _percentile_triplet(_samples(value, f"{kind}.{key}"))
            return _direct_triplet(value, f"{kind}.{key}")
    if kind == "soak":
        statistics = report.get("statistics")
        if isinstance(statistics, dict):
            duration = statistics.get("command_duration_ms")
            if isinstance(duration, dict):
                return _soak_duration_triplet(statistics, duration)
    return None


def _load_for(kind: str, report: dict[str, Any]) -> dict[str, Any] | None:
    measurements = _measurement_dict(kind, report)
    value = measurements.get("load")
    if value is None:
        value = report.get("load")
    if value is None:
        return None
    if not isinstance(value, dict):
        raise EvidenceError(f"{kind}.load:object_required")
    allowed = (
        "connections",
        "active_rooms",
        "actions_per_second",
        "transition_rate_per_second",
    )
    result: dict[str, Any] = {}
    for field in allowed:
        if field in value:
            number = _number(value[field], f"{kind}.load.{field}")
            result[field] = int(number) if number.is_integer() else number
    if not result:
        raise EvidenceError(f"{kind}.load:no_supported_parameters")
    return result


def _fan_out_for(kind: str, report: dict[str, Any]) -> dict[str, Any] | None:
    measurements = _measurement_dict(kind, report)
    value = measurements.get(
        "fan_out", measurements.get("fanout", report.get("fan_out"))
    )
    if value is None:
        return None
    if not isinstance(value, dict):
        raise EvidenceError(f"{kind}.fan_out:object_required")
    allowed = ("observation_fan_out", "observers_per_room", "frames_per_transition")
    result: dict[str, Any] = {}
    for field in allowed:
        if field in value:
            number = _number(value[field], f"{kind}.fan_out.{field}")
            result[field] = int(number) if number.is_integer() else number
    if not result:
        raise EvidenceError(f"{kind}.fan_out:no_supported_parameters")
    return result


def _memory_for(kind: str, report: dict[str, Any]) -> dict[str, Any] | None:
    measurements = _measurement_dict(kind, report)
    value = measurements.get("memory")
    if value is None and kind == "soak":
        statistics = report.get("statistics")
        value = statistics.get("memory") if isinstance(statistics, dict) else None
    if value is None:
        return None
    if not isinstance(value, dict):
        raise EvidenceError(f"{kind}.memory:object_required")
    peak = value.get("peak_rss_bytes")
    if peak is None and kind == "soak":
        peaks = value.get("peak_rss_bytes_per_run")
        if peaks is not None:
            samples = _samples(peaks, "soak.statistics.memory.peak_rss_bytes_per_run")
            if any(not sample.is_integer() for sample in samples):
                raise EvidenceError(
                    "soak.statistics.memory.peak_rss_bytes_per_run:integer_bytes_required"
                )
            peak = max(samples)
    if peak is None:
        return {"status": value.get("status", "not_measured")}
    number = _number(peak, f"{kind}.memory.peak_rss_bytes")
    if not number.is_integer():
        raise EvidenceError(f"{kind}.memory.peak_rss_bytes:integer_bytes_required")
    result: dict[str, Any] = {
        "status": value.get("status", "measured"),
        "peak_rss_bytes": int(number),
    }
    if isinstance(value.get("method"), str):
        result["method"] = value["method"]
    return result


def _database_for(kind: str, report: dict[str, Any]) -> dict[str, Any] | None:
    measurements = _measurement_dict(kind, report)
    value = measurements.get("database_growth", measurements.get("database"))
    if value is None:
        value = report.get("database")
    if value is None:
        return None
    if not isinstance(value, dict):
        raise EvidenceError(f"{kind}.database:object_required")
    if value.get("status") == "not_configured" and value.get("growth_bytes") is None:
        return None
    result: dict[str, Any] = {"status": value.get("status", "unknown")}
    if value.get("growth_bytes") is not None:
        number = _number(value["growth_bytes"], f"{kind}.database.growth_bytes")
        if not number.is_integer():
            raise EvidenceError(f"{kind}.database.growth_bytes:integer_bytes_required")
        result["growth_bytes"] = int(number)
    if value.get("wal_growth_bytes") is not None:
        number = _number(value["wal_growth_bytes"], f"{kind}.database.wal_growth_bytes")
        if not number.is_integer():
            raise EvidenceError(
                f"{kind}.database.wal_growth_bytes:integer_bytes_required"
            )
        result["wal_growth_bytes"] = int(number)
    return result


def _recovery_for(kind: str, report: dict[str, Any]) -> dict[str, Any] | None:
    measurements = _measurement_dict(kind, report)
    value = measurements.get("recovery")
    if value is None:
        value = report.get("recovery")
    if value is None:
        return None
    if not isinstance(value, dict):
        raise EvidenceError(f"{kind}.recovery:object_required")
    samples = value.get("durations_ms", value.get("samples_ms"))
    if samples is not None:
        return _percentile_triplet(_samples(samples, f"{kind}.recovery.durations_ms"))
    if value.get("duration_ms") is not None:
        return {
            "duration_ms": _number(value["duration_ms"], f"{kind}.recovery.duration_ms")
        }
    raise EvidenceError(f"{kind}.recovery:duration_required")


def _reference_targets_for(kind: str, report: dict[str, Any]) -> dict[str, Any] | None:
    if kind != "target":
        return None
    value = _measurement_dict(kind, report).get("reference_targets")
    if not isinstance(value, dict) or not value:
        raise EvidenceError("target.reference_targets:non_empty_object_required")
    return value


def _coverage_present(name: str, value: Any) -> bool:
    if not isinstance(value, dict):
        return False
    if name in {"latency_ms", "recovery"}:
        return "p50_ms" in value or "duration_ms" in value
    if name == "memory":
        return (
            value.get("status") == "measured"
            and isinstance(value.get("peak_rss_bytes"), int)
            and value["peak_rss_bytes"] >= 0
        )
    if name == "database_growth":
        return (
            value.get("status") == "measured"
            and isinstance(value.get("growth_bytes"), int)
            and value["growth_bytes"] >= 0
        )
    if name == "reference_targets":
        return bool(value)
    return bool(value)


def _add_gap(gaps: list[str], value: str) -> None:
    if value not in gaps and len(gaps) < MAX_GAPS:
        gaps.append(value)


def _aggregate(args: argparse.Namespace) -> dict[str, Any]:
    paths = {
        "counter": args.counter_report,
        "heist": args.heist_report,
        "sqlite": args.sqlite_report,
        "postgres": args.postgres_report,
        "soak": args.soak_report,
    }
    paths["target"] = args.target_report
    reports: dict[str, dict[str, Any]] = {}
    inventory: list[dict[str, Any]] = []
    source_metadata: dict[str, dict[str, Any]] = {}
    errors: list[str] = []
    for kind, path_value in paths.items():
        try:
            report, item = _read_report(kind, path_value)
            inventory.append(item)
            metadata = _validate_report(kind, report)
            reports[kind] = report
            source_metadata[kind] = metadata
        except EvidenceError as error:
            errors.append(str(error))
    inventory.sort(key=lambda item: item["kind"])
    summary: dict[str, Any] = {
        "schema": SCHEMA,
        "status": "blocked" if errors else "incomplete",
        "release_evidence": False,
        "performance_class": "reference_non_release",
        "inputs": inventory,
        "input_sha256_inventory": {
            "definition": "sorted logical kind, byte count, schema, status, and exact file SHA-256",
            "sha256": _sha256_reference(_canonical(inventory)),
            "items": inventory,
        },
        "sources": source_metadata,
        "identity": None,
        "reference_environments": {},
        "workloads": {},
        "coverage": {
            kind: {name: False for name in REQUIRED_COVERAGE[kind]} for kind in paths
        },
        "measurements": {
            "latency_ms": {"sources": {}},
            "load": {"sources": {}},
            "fan_out": {"sources": {}},
            "memory": {"sources": {}},
            "database_growth": {"sources": {}},
            "recovery": {"sources": {}},
            "reference_targets": {"sources": {}},
        },
        "gaps": [],
        "errors": sorted(errors),
    }
    gaps: list[str] = []
    accepted_identities = [
        source_metadata[kind]["identity"] for kind in paths if kind in source_metadata
    ]
    if accepted_identities:
        common = accepted_identities[0]
        summary["identity"] = {
            "product": common["product"],
            "profile": common["profile"],
            "version": common["version"],
            "packaged_acceptance_sha256": common["packaged_acceptance_sha256"],
        }
        for kind in paths:
            if kind not in source_metadata:
                continue
            identity = source_metadata[kind]["identity"]
            if identity["version"] != common["version"]:
                summary["errors"].append(f"{kind}:identity_version_mismatch")
            if identity["profile"] != common["profile"]:
                summary["errors"].append(f"{kind}:identity_profile_mismatch")
            if (
                identity["packaged_acceptance_sha256"]
                != common["packaged_acceptance_sha256"]
            ):
                summary["errors"].append(
                    f"{kind}:identity_packaged_acceptance_mismatch"
                )
    if len(accepted_identities) != len(paths):
        _add_gap(gaps, "identity:all_six_reports_must_be_accepted")
    accepted_environments = {
        kind: source_metadata[kind]["reference_environment"]
        for kind in paths
        if kind in source_metadata
    }
    summary["reference_environments"] = accepted_environments
    if len(accepted_environments) != len(paths):
        _add_gap(gaps, "reference_environments:all_six_reports_must_be_disclosed")
    accepted_workloads = {
        kind: source_metadata[kind]["reference_workload"]
        for kind in paths
        if kind in source_metadata
    }
    summary["workloads"] = accepted_workloads
    if len(accepted_workloads) != len(paths):
        _add_gap(gaps, "reference_workloads:all_six_reports_must_be_disclosed")
    for kind in paths:
        if kind not in reports:
            _add_gap(gaps, f"{kind}:report_not_accepted")
            continue
        report = reports[kind]
        measurements = summary["measurements"]
        extractors = (
            ("latency_ms", _latency_for),
            ("load", _load_for),
            ("fan_out", _fan_out_for),
            ("memory", _memory_for),
            ("database_growth", _database_for),
            ("recovery", _recovery_for),
            ("reference_targets", _reference_targets_for),
        )
        for name, extractor in extractors:
            try:
                value = extractor(kind, report)
            except EvidenceError as error:
                summary["errors"].append(str(error))
                continue
            if value is not None:
                measurements[name]["sources"][kind] = value
                if name in summary["coverage"].get(kind, {}):
                    summary["coverage"][kind][name] = _coverage_present(name, value)
        if kind == "soak" and report.get("one_hour_window_completed") is not True:
            _add_gap(gaps, "soak:one_hour_window_not_completed")
        if kind == "soak":
            database = report.get("database")
            if (
                isinstance(database, dict)
                and database.get("status") == "not_configured"
            ):
                _add_gap(gaps, "soak:database_growth_not_configured")
        for name in REQUIRED_COVERAGE[kind]:
            if not summary["coverage"][kind][name]:
                _add_gap(gaps, f"{kind}:{name}_not_reported")
    latency_sources = summary["measurements"]["latency_ms"]["sources"]
    if not latency_sources:
        _add_gap(gaps, "latency:p50_p95_p99_unavailable")
    else:
        direct = list(latency_sources.values())
        summary["measurements"]["latency_ms"]["percentiles"] = (
            direct[0]
            if len(direct) == 1
            else {kind: latency_sources[kind] for kind in sorted(latency_sources)}
        )
    if not summary["measurements"]["load"]["sources"]:
        _add_gap(gaps, "load:parameters_not_reported")
    if not summary["measurements"]["fan_out"]["sources"]:
        _add_gap(gaps, "fan_out:parameters_not_reported")
    if not summary["measurements"]["memory"]["sources"]:
        _add_gap(gaps, "memory:peak_not_reported")
    if not summary["measurements"]["database_growth"]["sources"]:
        _add_gap(gaps, "database_growth:not_measured")
    if not summary["measurements"]["recovery"]["sources"]:
        _add_gap(gaps, "recovery:timing_not_reported")
    summary["errors"] = sorted(set(summary["errors"]))[:MAX_GAPS]
    summary["gaps"] = sorted(gaps)[:MAX_GAPS]
    if summary["errors"]:
        summary["status"] = "blocked"
    elif not summary["gaps"]:
        summary["status"] = "pass"
    return summary


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    for kind in ("counter", "heist", "sqlite", "postgres", "soak", "target"):
        parser.add_argument(f"--{kind}-report", required=True, metavar="PATH")
    parser.add_argument(
        "--output", metavar="PATH", help="write the same canonical JSON to PATH"
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        summary = _aggregate(args)
    except (OSError, ValueError) as error:
        summary = {
            "schema": SCHEMA,
            "status": "blocked",
            "release_evidence": False,
            "performance_class": "reference_non_release",
            "inputs": [],
            "input_sha256_inventory": {
                "definition": "no accepted inputs",
                "sha256": _sha256_reference(b"[]\n"),
                "items": [],
            },
            "measurements": {},
            "reference_environments": {},
            "workloads": {},
            "gaps": [],
            "errors": [str(error)],
        }
    encoded = (
        json.dumps(summary, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
        + "\n"
    )
    if args.output:
        output = Path(args.output)
        if output.is_symlink():
            raise SystemExit("output_must_not_be_symlink")
        output.write_text(encoded, encoding="utf-8")
    sys.stdout.write(encoded)
    return 0 if summary["status"] == "pass" else 2


if __name__ == "__main__":
    raise SystemExit(main())

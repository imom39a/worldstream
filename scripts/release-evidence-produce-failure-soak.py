#!/usr/bin/env python3
"""Produce typed release evidence from a completed Linux failure/soak campaign."""

from __future__ import annotations

import argparse
import base64
import errno
import hashlib
import importlib.util
import os
import re
import stat
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
ADAPTER_PATH = ROOT / "scripts/release-evidence-produce.py"
REFERENCE_PRODUCER_PATH = ROOT / "scripts/release-evidence-produce-reference.py"
SOURCE_ID = "failure-soak"
EVIDENCE_ID = "failure-fuzz-resource-and-one-hour-sqlite-soak"
CHECKS = ("failure_matrix", "resource_bounds", "one_hour_soak", "retained_logs")
REQUIRED_COVERAGE_GROUPS = frozenset(
    {
        "create",
        "action",
        "timer",
        "snapshot",
        "recovery",
        "resource_or_storage_fault",
        "fuzz",
        "corruption_or_quarantine",
        "concurrency",
        "authority",
        "delivery",
        "migration",
    }
)
REQUIRED_FIXTURE_HOOKS = frozenset({"resource", "fault", "corruption"})
REQUIRED_KILL_OPERATIONS = (
    "room_create",
    "action",
    "timer",
    "activation_lease",
)
REQUIRED_KILL_BOUNDARIES = (
    "before_commit",
    "after_commit_before_publication",
    "after_publication_before_reply",
)
REQUIRED_KILL_PROFILES = (
    ("sqlite", "embedded"),
    ("postgresql", "direct"),
    ("postgresql", "transaction_pool"),
)
EXPECTED_KILL_CELL_COUNT = 36
POSTGRES_KILL_IMAGE = (
    "postgres:17.11-alpine@"
    "sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
)
POSTGRES_KILL_DIGEST = (
    "postgres@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
)
PGBOUNCER_KILL_IMAGE = (
    "edoburu/pgbouncer@"
    "sha256:4c1ca296ef525f108f5d3552cc337c0c09587cf8dae7f0067fd93349e47dc1cd"
)
PGBOUNCER_KILL_DIGEST = (
    "edoburu/pgbouncer@"
    "sha256:4c1ca296ef525f108f5d3552cc337c0c09587cf8dae7f0067fd93349e47dc1cd"
)
REQUIRED_PROVIDER_CHANNEL_CLASSES = frozenset(
    {
        "process.stdout",
        "process.stderr",
        "process.config",
        "provider.postgresql.stdout",
        "provider.postgresql.stderr",
        "provider.pgbouncer.stdout",
        "provider.pgbouncer.stderr",
    }
)
OVERDUE_TIMER_PROFILE_FIELDS = frozenset(
    {
        "status",
        "storage_backend",
        "connection_mode",
        "durable_state",
        "daemon_ready",
        "ordinary_work_gated_before_drain",
        "exact_timer_retry_drained",
        "first_result_duplicate",
        "second_result_duplicate",
        "receipt_hash_equal",
        "receipt_hash",
        "projection_hash_equal_after_restart",
        "projection_hash",
        "same_data_directory",
    }
)
EXPECTED_KILL_OUTCOMES = {
    "before_commit": "no_commit",
    "after_commit_before_publication": "original_result",
    "after_publication_before_reply": "original_result",
}
RELEASE_SOAK_EVIDENCE_CLASS = "process_level_daemon_workload"
RELEASE_DATABASE_WORKLOAD_BINDING = "worldstream-daemon-transition-soak/v1"
ONE_HOUR_SECONDS = 3600.0
MAX_PEAK_RSS_BYTES = 2 * 1024 * 1024 * 1024
MAX_DATABASE_GROWTH_BYTES = 256 * 1024 * 1024
MAX_WAL_GROWTH_BYTES = 256 * 1024 * 1024
MAX_TEMP_GROWTH_BYTES = 64 * 1024 * 1024
MAX_RUNTIME_LOG_GROWTH_BYTES = 256 * 1024
MAX_AUXILIARY_ARTIFACT_GROWTH_BYTES = (
    MAX_TEMP_GROWTH_BYTES + MAX_RUNTIME_LOG_GROWTH_BYTES
)
TELEMETRY_QUEUE_HARD_LIMIT = 256
DNS_RESOLVER_QUEUE_HARD_LIMIT = 2
WEBSOCKET_LIVE_PUSH_FRAME_QUEUE_HARD_LIMIT = 256
WEBSOCKET_OUTBOUND_PAYLOAD_BYTES_HARD_LIMIT = 4 * 1024 * 1024
RSS_SAMPLE_INTERVAL_MS = 50
STORAGE_SAMPLE_INTERVAL_MS = 250
RESOURCE_SAMPLE_MAX_GAP_MS = 1000
RESOURCE_SAMPLING_SCHEMA = "worldstream/live-resource-sampling/v1"
QUEUE_OBSERVATION_SCOPE = "all_bounded_runtime_queues_public_prometheus"
QUEUE_BOUNDARY_TEST_SCHEMA = "worldstream/queue-boundary-tests/v1"
QUEUE_BOUNDARY_MAX_TOTAL_SECONDS = 300.0
QUEUE_SPECS = (
    {
        "name": "telemetry_exporter",
        "capacity_scope": "global",
        "hard_limit": TELEMETRY_QUEUE_HARD_LIMIT,
        "activity_unit": "events",
    },
    {
        "name": "telemetry_dns_resolver_queue",
        "capacity_scope": "global",
        "hard_limit": DNS_RESOLVER_QUEUE_HARD_LIMIT,
        "activity_unit": "requests",
    },
    {
        "name": "room_admission_lane",
        "capacity_scope": "per_room",
        "hard_limit": TELEMETRY_QUEUE_HARD_LIMIT,
        "activity_unit": "reservations",
    },
    {
        "name": "websocket_live_push_frame_queue",
        "capacity_scope": "per_connection",
        "hard_limit": WEBSOCKET_LIVE_PUSH_FRAME_QUEUE_HARD_LIMIT,
        "activity_unit": "frames",
    },
    {
        "name": "websocket_outbound_payload_bytes",
        "capacity_scope": "per_connection",
        "hard_limit": WEBSOCKET_OUTBOUND_PAYLOAD_BYTES_HARD_LIMIT,
        "activity_unit": "bytes",
    },
)
QUEUE_BOUNDARY_TESTS = (
    (
        "telemetry_exporter",
        "worldstream-server",
        "telemetry::tests::postgres_bridge_is_nonblocking_when_queue_is_saturated",
    ),
    (
        "telemetry_dns_resolver_queue",
        "worldstream-server",
        "telemetry::tests::dns_timeouts_use_a_fixed_worker_and_queue_bound",
    ),
    (
        "room_admission_lane",
        "worldstream-core",
        "semantic_time::tests::coordinated_lane_rejects_full_action_before_sampling",
    ),
    (
        "websocket_live_push_frame_queue",
        "worldstream-server",
        "tests::live_frame_count_overflow_closes_with_typed_error",
    ),
    (
        "websocket_outbound_payload_bytes",
        "worldstream-server",
        "tests::live_payload_byte_overflow_closes_with_typed_error",
    ),
)
MAX_LOG_BYTES = 64 * 1024 * 1024
SECRET_SCAN_MATRIX_SCHEMA = "worldstream/secret-absence-matrix/v1"
DISK_FULL_SCHEMA = "worldstream/disk-full-evidence/v1"
DISK_FULL_SCENARIO = "packaged_sqlite_bootstrap_on_full_ext4_loopback"
DISK_FULL_EVIDENCE_CLASS = "process_level_daemon_fault_injection"
DISK_FULL_CONTAINER_IMAGE = (
    "docker@sha256:12e683a161823b2a839aeea999b9d960e6e1f9a97b1679ad6b441982e2d9cf07"
)
DISK_FULL_FILESYSTEM_IMAGE_BYTES = 64 * 1024 * 1024
DISK_FULL_BLOCK_SIZE_BYTES = 4096
DISK_FULL_ATTEMPTED_WRITE_BYTES = 4096
DISK_FULL_MAX_DAEMON_SECONDS = 10.0
DISK_FULL_MAX_CONTAINER_SECONDS = 120.0
DISK_FULL_MAX_LOG_BYTES = 64 * 1024
DISK_FULL_MAX_CAPTURE_BYTES = 256 * 1024
SHA256_REFERENCE = re.compile(r"sha256:[0-9a-f]{64}\Z")
REQUIRED_CELL_VERIFICATION = {
    "room_create": frozenset(
        {
            "exact_request_retried",
            "two_retry_results_equal",
            "genesis_room_seq_zero",
            "pre_retry_room_count_matches_boundary",
            "pre_retry_genesis_count_matches_boundary",
            "pre_retry_creation_receipt_count_matches_boundary",
            "boundary_marker_precedes_signal",
        }
    ),
    "action": frozenset(
        {
            "exact_request_retried",
            "pre_retry_state_matches_boundary",
            "first_retry_duplicate_matches_boundary",
            "second_retry_is_duplicate",
            "durable_receipt_equal",
            "final_transition_applied_once",
        }
    ),
    "timer": frozenset(
        {
            "exact_request_retried",
            "pre_retry_state_matches_boundary",
            "first_retry_duplicate_matches_boundary",
            "second_retry_is_duplicate",
            "durable_receipt_equal",
            "final_transition_applied_once",
        }
    ),
    "activation_lease": frozenset(
        {
            "exact_request_retried",
            "offer_visibility_matches_boundary",
            "claim_granted",
            "duplicate_claim_result_equal",
            "claim_identity_equal",
        }
    ),
}
SECRET_PATTERNS = (
    re.compile(r"(?i)authorization:\s*bearer\s+(?!<redacted>(?:\s|$))\S+"),
    re.compile(r"(?i)authorization:\s*(?!bearer(?:\s|$)|<redacted>(?:\s|$))\S+"),
    re.compile(r"(?i)\bbearer\s+(?!<redacted>)[A-Za-z0-9._~+/=-]{8,}"),
    re.compile(r"\bwsb1:[0-9a-fA-F]{64}\b"),
    re.compile(r"(?i)\b(?:password|secret|token)\s*=\s*(?!<redacted>)\S+"),
)


def load_adapter():
    spec = importlib.util.spec_from_file_location(
        "release_evidence_produce", ADAPTER_PATH
    )
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {ADAPTER_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


PRODUCER = load_adapter()


def load_reference_producer():
    spec = importlib.util.spec_from_file_location(
        "worldstream_failure_soak_package_verifier", REFERENCE_PRODUCER_PATH
    )
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {REFERENCE_PRODUCER_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


REFERENCE_PRODUCER = load_reference_producer()


class EvidenceError(RuntimeError):
    """The supplied campaign cannot support a release attestation."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise EvidenceError(message)


def regular_file(path: Path, label: str) -> Path:
    try:
        mode = path.lstat().st_mode
    except FileNotFoundError as error:
        raise EvidenceError(f"missing {label}: {path}") from error
    except OSError as error:
        raise EvidenceError(f"cannot inspect {label}: {error}") from error
    require(not stat.S_ISLNK(mode), f"{label} must not be a symlink")
    require(stat.S_ISREG(mode), f"{label} must be a regular file")
    return path


def read_json(path: Path, label: str) -> dict[str, Any]:
    regular_file(path, label)
    try:
        raw = path.read_bytes()
        value = PRODUCER.COLLECTOR.strict_json_object(raw, label)
    except OSError as error:
        raise EvidenceError(f"{label} is not valid JSON: {error}") from error
    except PRODUCER.COLLECTOR.CollectionError as error:
        raise EvidenceError(f"{label} is not valid JSON: {error}") from error
    return value


def digest(path: Path) -> str:
    try:
        return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        raise EvidenceError(f"cannot hash evidence input: {error}") from error


def number(value: object, label: str) -> float:
    require(type(value) in {int, float}, f"{label} must be numeric")
    return float(value)


def integer(value: object, label: str, *, minimum: int = 0) -> int:
    require(type(value) is int and value >= minimum, f"{label} must be an integer")
    return value


def validate_disk_full(evidence: object, *, binary_sha256: str) -> dict[str, Any]:
    """Require exact bounded ext4 ENOSPC against the measured daemon."""

    require(isinstance(evidence, dict), "disk-full evidence is missing")
    expected_top_level = {
        "schema",
        "status",
        "release_evidence",
        "scenario",
        "evidence_class",
        "platform",
        "container",
        "bounds",
        "filesystem",
        "fault",
        "daemon",
        "cleanup",
        "limitations",
    }
    require(
        set(evidence) == expected_top_level
        and evidence.get("schema") == DISK_FULL_SCHEMA
        and evidence.get("status") == "pass"
        and evidence.get("release_evidence") is False
        and evidence.get("scenario") == DISK_FULL_SCENARIO
        and evidence.get("evidence_class") == DISK_FULL_EVIDENCE_CLASS,
        "disk-full evidence does not describe the exact release scenario",
    )
    require(
        evidence.get("platform")
        == {
            "system": "Linux",
            "machine": "x86_64",
            "filesystem": "ext4",
        },
        "disk-full evidence did not use Linux x86-64 ext4",
    )
    container = evidence.get("container")
    require(
        isinstance(container, dict)
        and set(container)
        == {
            "image",
            "platform",
            "privileged",
            "network",
            "root_filesystem_read_only",
            "daemon_mount_read_only",
            "observed_elapsed_ms",
            "captured_output_bytes",
        }
        and container.get("image") == DISK_FULL_CONTAINER_IMAGE
        and container.get("platform") == "linux/amd64"
        and container.get("privileged") is True
        and container.get("network") == "none"
        and container.get("root_filesystem_read_only") is True
        and container.get("daemon_mount_read_only") is True
        and type(container.get("observed_elapsed_ms")) in {int, float}
        and 0
        <= container["observed_elapsed_ms"]
        <= DISK_FULL_MAX_CONTAINER_SECONDS * 1000
        and type(container.get("captured_output_bytes")) is int
        and 0 < container["captured_output_bytes"] <= DISK_FULL_MAX_CAPTURE_BYTES,
        "disk-full container identity, isolation, or output bound is invalid",
    )
    require(
        evidence.get("bounds")
        == {
            "filesystem_image_bytes": DISK_FULL_FILESYSTEM_IMAGE_BYTES,
            "attempted_write_bytes": DISK_FULL_ATTEMPTED_WRITE_BYTES,
            "max_daemon_seconds": DISK_FULL_MAX_DAEMON_SECONDS,
            "max_container_seconds": DISK_FULL_MAX_CONTAINER_SECONDS,
            "max_log_bytes": DISK_FULL_MAX_LOG_BYTES,
            "max_capture_bytes": DISK_FULL_MAX_CAPTURE_BYTES,
        },
        "disk-full scenario did not use the frozen bounds",
    )
    filesystem = evidence.get("filesystem")
    require(
        isinstance(filesystem, dict)
        and set(filesystem)
        == {
            "type",
            "mount_source_class",
            "image_size_bytes",
            "block_size_bytes",
            "available_kib_after_fill",
            "database_file_type",
            "database_file_mode",
            "fill_bytes_written",
        }
        and filesystem.get("type") == "ext4"
        and filesystem.get("mount_source_class") == "loop_device"
        and filesystem.get("image_size_bytes") == DISK_FULL_FILESYSTEM_IMAGE_BYTES
        and filesystem.get("block_size_bytes") == DISK_FULL_BLOCK_SIZE_BYTES
        and filesystem.get("available_kib_after_fill") == 0
        and filesystem.get("database_file_type") == "regular"
        and filesystem.get("database_file_mode") == "0600"
        and type(filesystem.get("fill_bytes_written")) is int
        and 0 < filesystem["fill_bytes_written"] < DISK_FULL_FILESYSTEM_IMAGE_BYTES,
        "disk-full ext4 mount or regular database witness is invalid",
    )
    require(
        evidence.get("fault")
        == {
            "errno_number": errno.ENOSPC,
            "errno_name": "ENOSPC",
            "attempted_write_bytes": DISK_FULL_ATTEMPTED_WRITE_BYTES,
            "write_returned_bytes": 0,
            "database_size_before_bytes": 0,
            "database_size_after_bytes": 0,
        },
        "disk-full evidence lacks exact bounded ENOSPC",
    )
    daemon = evidence.get("daemon")
    require(
        isinstance(daemon, dict)
        and set(daemon)
        == {
            "binary_sha256",
            "started",
            "exit_observed",
            "exit_code",
            "ready_http_200_observed",
            "public_mutation_available",
            "storage_failure_observed",
            "elapsed_ms",
            "diagnostic_bytes",
            "diagnostic_sha256",
        }
        and daemon.get("binary_sha256") == binary_sha256
        and daemon.get("started") is True
        and daemon.get("exit_observed") is True
        and daemon.get("exit_code") == 1
        and daemon.get("ready_http_200_observed") is False
        and daemon.get("public_mutation_available") is False
        and daemon.get("storage_failure_observed") is True
        and type(daemon.get("elapsed_ms")) in {int, float}
        and 0 <= daemon["elapsed_ms"] <= DISK_FULL_MAX_DAEMON_SECONDS * 1000
        and type(daemon.get("diagnostic_bytes")) is int
        and 0 < daemon["diagnostic_bytes"] <= DISK_FULL_MAX_LOG_BYTES
        and isinstance(daemon.get("diagnostic_sha256"), str)
        and SHA256_REFERENCE.fullmatch(daemon["diagnostic_sha256"]) is not None,
        "daemon did not fail closed on the bounded disk-full fault",
    )
    require(
        evidence.get("cleanup")
        == {
            "internal_unmount_observed": True,
            "container_remove_requested": True,
            "container_absent_after_run": True,
        },
        "disk-full container cleanup was not proven",
    )
    require(
        evidence.get("limitations")
        == {
            "runtime_disk_exhaustion_recovery_observed": False,
            "physical_power_loss_observed": False,
        },
        "disk-full evidence overclaims the bounded startup scenario",
    )
    return evidence


def validate_distribution(
    report: dict[str, Any],
    *,
    version: str,
    manifest_json_sha256: str,
    manifest_toml_sha256: str,
    label: str,
) -> dict[str, Any]:
    distribution = report.get("distribution")
    expected_fields = {
        "binary_sha256",
        "binary_size_bytes",
        "packaged_artifact_bound",
        "reference_class",
        "archive_sha256",
        "archive_size_bytes",
        "package_report_sha256",
        "manifest_sha256",
        "manifest_json_sha256",
        "manifest_toml_sha256",
        "target",
        "version",
    }
    require(
        isinstance(distribution, dict)
        and set(distribution) == expected_fields
        and distribution.get("packaged_artifact_bound") is True
        and distribution.get("reference_class") == "fresh_packaged_linux_x86_64"
        and distribution.get("target") == "linux-x86_64"
        and distribution.get("version") == version,
        f"{label} is not bound to the fresh packaged Linux x86-64 distribution",
    )
    for field in (
        "binary_sha256",
        "archive_sha256",
        "package_report_sha256",
        "manifest_sha256",
        "manifest_json_sha256",
        "manifest_toml_sha256",
    ):
        require(
            isinstance(distribution.get(field), str)
            and SHA256_REFERENCE.fullmatch(distribution[field]) is not None,
            f"{label} packaged distribution {field} is invalid",
        )
    require(
        distribution["manifest_sha256"] == manifest_json_sha256
        and distribution["manifest_json_sha256"] == manifest_json_sha256
        and distribution["manifest_toml_sha256"] == manifest_toml_sha256,
        f"{label} packaged distribution manifest pair identity mismatch",
    )
    for field in ("binary_size_bytes", "archive_size_bytes"):
        require(
            integer(distribution.get(field), f"{label} {field}", minimum=1) > 0,
            f"{label} packaged distribution {field} is invalid",
        )
    return distribution


def validate_live_resource_sampling(
    report: dict[str, Any],
    *,
    configuration: dict[str, Any],
    elapsed_seconds: float,
    memory: dict[str, Any],
    database: dict[str, Any],
    temporary: dict[str, Any],
    logs: dict[str, Any],
    artifacts: dict[str, Any],
) -> dict[str, Any]:
    """Require complete bounded-interval peaks and bind legacy final snapshots."""

    hard_limits = {
        "process_tree_rss_bytes": integer(
            configuration.get("max_peak_rss_bytes"),
            "max_peak_rss_bytes",
            minimum=1,
        ),
        "database_bytes": integer(
            configuration.get("max_database_growth_bytes"),
            "max_database_growth_bytes",
            minimum=1,
        ),
        "wal_bytes": integer(
            configuration.get("max_wal_growth_bytes"),
            "max_wal_growth_bytes",
            minimum=1,
        ),
        "temporary_bytes": integer(
            configuration.get("max_temp_growth_bytes"),
            "max_temp_growth_bytes",
            minimum=1,
        ),
        "log_bytes": integer(
            configuration.get("max_log_growth_bytes"),
            "max_log_growth_bytes",
            minimum=1,
        ),
        "artifact_bytes": integer(
            configuration.get("max_artifact_growth_bytes"),
            "max_artifact_growth_bytes",
            minimum=1,
        ),
    }
    require(
        hard_limits
        == {
            "process_tree_rss_bytes": MAX_PEAK_RSS_BYTES,
            "database_bytes": MAX_DATABASE_GROWTH_BYTES,
            "wal_bytes": MAX_WAL_GROWTH_BYTES,
            "temporary_bytes": MAX_TEMP_GROWTH_BYTES,
            "log_bytes": MAX_RUNTIME_LOG_GROWTH_BYTES,
            "artifact_bytes": MAX_AUXILIARY_ARTIFACT_GROWTH_BYTES,
        },
        "live resource sampling does not use the frozen hard limits",
    )
    sampling = report.get("resource_sampling")
    require(
        isinstance(sampling, dict)
        and set(sampling)
        == {
            "schema",
            "status",
            "sampling_complete",
            "peak_semantics",
            "workload_elapsed_ms",
            "resources",
        }
        and sampling.get("schema") == RESOURCE_SAMPLING_SCHEMA
        and sampling.get("status") == "measured"
        and sampling.get("sampling_complete") is True
        and sampling.get("peak_semantics")
        == "maximum_observed_at_bounded_sampling_interval",
        "live resource sampling contract is missing or incomplete",
    )
    workload_elapsed_ms = number(
        sampling.get("workload_elapsed_ms"), "sampled workload_elapsed_ms"
    )
    require(
        abs(workload_elapsed_ms - elapsed_seconds * 1000) <= 1.0,
        "live resource sampling is not bound to the one-hour workload window",
    )
    resources = sampling.get("resources")
    expected_names = set(hard_limits)
    require(
        isinstance(resources, dict) and set(resources) == expected_names,
        "live resource sampling inventory is incomplete",
    )

    rss = resources["process_tree_rss_bytes"]
    rss_fields = {
        "measurement_source",
        "measurement_window",
        "sampling_interval_ms",
        "maximum_gap_ms",
        "observed_max_gap_ms",
        "coverage_duration_ms",
        "sample_count",
        "observed_peak_bytes",
        "configured_hard_limit_bytes",
        "bound_status",
    }
    require(
        isinstance(rss, dict)
        and set(rss) == rss_fields
        and rss.get("measurement_source") == "linux_proc_process_tree_vmrss"
        and rss.get("measurement_window") == "transition_workload"
        and rss.get("sampling_interval_ms") == RSS_SAMPLE_INTERVAL_MS
        and rss.get("maximum_gap_ms") == RESOURCE_SAMPLE_MAX_GAP_MS
        and rss.get("configured_hard_limit_bytes")
        == hard_limits["process_tree_rss_bytes"]
        and rss.get("bound_status") == "pass",
        "process-tree RSS live sampling metadata is not exact",
    )
    rss_gap = number(rss.get("observed_max_gap_ms"), "RSS observed_max_gap_ms")
    rss_coverage = number(rss.get("coverage_duration_ms"), "RSS coverage_duration_ms")
    rss_samples = integer(rss.get("sample_count"), "RSS sample_count", minimum=1)
    rss_peak = integer(
        rss.get("observed_peak_bytes"), "RSS observed_peak_bytes", minimum=1
    )
    require(
        0 <= rss_gap <= RESOURCE_SAMPLE_MAX_GAP_MS
        and rss_coverage >= 0
        and rss_coverage + RESOURCE_SAMPLE_MAX_GAP_MS >= workload_elapsed_ms
        and rss_samples > 0
        and rss_peak <= hard_limits["process_tree_rss_bytes"]
        and memory.get("peak_rss_bytes") == rss_peak
        and memory.get("peak_rss_bytes_per_run") == [rss_peak]
        and memory.get("sampling_interval_ms") == RSS_SAMPLE_INTERVAL_MS
        and memory.get("maximum_gap_ms") == RESOURCE_SAMPLE_MAX_GAP_MS
        and memory.get("observed_max_gap_ms") == rss_gap
        and memory.get("coverage_duration_ms") == rss_coverage
        and memory.get("sample_count") == rss_samples,
        "process-tree RSS peak, cadence, or legacy binding is invalid",
    )

    storage_sources = {
        "database_bytes": "sqlite_main_plus_wal_regular_file_sizes",
        "wal_bytes": "sqlite_wal_regular_file_size",
        "temporary_bytes": "owned_private_working_tree_regular_file_sizes",
        "log_bytes": "owned_private_working_tree_daemon_log_sizes",
        "artifact_bytes": "owned_private_working_tree_regular_file_sizes",
    }
    storage_snapshots = {
        "database_bytes": (
            database.get("initial_bytes"),
            database.get("final_bytes"),
            database.get("growth_bytes"),
        ),
        "wal_bytes": (
            database.get("wal_initial_bytes"),
            database.get("wal_final_bytes"),
            database.get("wal_growth_bytes"),
        ),
        "temporary_bytes": (
            temporary.get("initial_bytes"),
            temporary.get("final_bytes"),
            temporary.get("growth_bytes"),
        ),
        "log_bytes": (
            logs.get("initial_bytes"),
            logs.get("final_bytes"),
            logs.get("growth_bytes"),
        ),
        "artifact_bytes": (
            artifacts.get("initial_bytes"),
            artifacts.get("final_bytes"),
            artifacts.get("growth_bytes"),
        ),
    }
    storage_fields = {
        "measurement_source",
        "measurement_window",
        "sampling_interval_ms",
        "maximum_gap_ms",
        "observed_max_gap_ms",
        "coverage_duration_ms",
        "sample_count",
        "initial_bytes",
        "observed_peak_bytes",
        "observed_peak_growth_bytes",
        "configured_hard_limit_bytes",
        "bound_status",
    }
    common_storage_cadence: tuple[float, float, int, int] | None = None
    for name, measurement_source in storage_sources.items():
        row = resources[name]
        require(
            isinstance(row, dict)
            and set(row) == storage_fields
            and row.get("measurement_source") == measurement_source
            and row.get("measurement_window") == "transition_workload_through_recovery"
            and row.get("sampling_interval_ms") == STORAGE_SAMPLE_INTERVAL_MS
            and row.get("maximum_gap_ms") == RESOURCE_SAMPLE_MAX_GAP_MS
            and row.get("configured_hard_limit_bytes") == hard_limits[name]
            and row.get("bound_status") == "pass",
            f"{name} live sampling metadata is not exact",
        )
        observed_gap = number(
            row.get("observed_max_gap_ms"), f"{name} observed_max_gap_ms"
        )
        coverage = number(
            row.get("coverage_duration_ms"), f"{name} coverage_duration_ms"
        )
        sample_count = integer(
            row.get("sample_count"), f"{name} sample_count", minimum=1
        )
        initial = integer(row.get("initial_bytes"), f"{name} initial_bytes")
        peak = integer(row.get("observed_peak_bytes"), f"{name} observed_peak_bytes")
        peak_growth = integer(
            row.get("observed_peak_growth_bytes"),
            f"{name} observed_peak_growth_bytes",
        )
        legacy_initial, legacy_final, legacy_growth = storage_snapshots[name]
        require(
            0 <= observed_gap <= RESOURCE_SAMPLE_MAX_GAP_MS
            and coverage >= 0
            and coverage + RESOURCE_SAMPLE_MAX_GAP_MS >= workload_elapsed_ms
            and sample_count > 0
            and peak >= initial
            and peak_growth == peak - initial
            and peak_growth <= hard_limits[name]
            and legacy_initial == initial
            and type(legacy_final) is int
            and initial <= legacy_final <= peak
            and legacy_growth == legacy_final - initial
            and legacy_growth <= peak_growth,
            f"{name} live peak is invalid or not bound to its final snapshot",
        )
        cadence = (observed_gap, coverage, sample_count, row["sampling_interval_ms"])
        if common_storage_cadence is None:
            common_storage_cadence = cadence
        else:
            require(
                cadence == common_storage_cadence,
                "storage resources were not captured by one coherent live sampler",
            )
    measurements = report.get("measurements")
    require(
        isinstance(measurements, dict)
        and measurements.get("resource_sampling") == sampling,
        "standard measurements do not bind the exact live resource sampling object",
    )
    return sampling


def validate_queue_observation(
    report: dict[str, Any], *, configuration: dict[str, Any]
) -> dict[str, Any]:
    """Require live public measurements for every bounded runtime queue."""

    expected_limits = {
        str(spec["name"]): {
            "capacity_scope": spec["capacity_scope"],
            "hard_limit": spec["hard_limit"],
            "activity_unit": spec["activity_unit"],
        }
        for spec in QUEUE_SPECS
    }
    require(
        configuration.get("internal_queue_hard_limits") == expected_limits,
        "internal queues do not use the frozen hard limits",
    )
    observation = report.get("queue_observation")
    require(
        report.get("internal_queues") is None
        and isinstance(observation, dict)
        and set(observation)
        == {
            "status",
            "observation_scope",
            "all_internal_queues_observed",
            "observed_queues",
        }
        and observation.get("status") == "measured"
        and observation.get("observation_scope") == QUEUE_OBSERVATION_SCOPE
        and observation.get("all_internal_queues_observed") is True,
        "queue evidence is partial or overclaims its observation scope",
    )
    queues = observation.get("observed_queues")
    require(
        isinstance(queues, list)
        and len(queues) == len(QUEUE_SPECS)
        and [queue.get("name") for queue in queues if isinstance(queue, dict)]
        == [spec["name"] for spec in QUEUE_SPECS],
        "one or more bounded queue observations are missing or reordered",
    )
    expected_fields = {
        "status",
        "name",
        "measurement_source",
        "capacity_metric",
        "process_current_metric",
        "process_high_water_metric",
        "unit_high_water_metric",
        "activity_metric",
        "completion_metric",
        "backpressure_metric",
        "capacity_scope",
        "activity_unit",
        "configured_hard_limit",
        "maximum_observed_process_current",
        "maximum_reported_process_high_water",
        "maximum_reported_unit_high_water",
        "sample_count",
        "activity_total_initial",
        "activity_total_final",
        "activity_total_delta",
        "completion_total_initial",
        "completion_total_final",
        "completion_total_delta",
        "backpressure_total_initial",
        "backpressure_total_final",
        "backpressure_total_delta",
        "bound_status",
    }
    for queue, spec in zip(queues, QUEUE_SPECS, strict=True):
        name = str(spec["name"])
        hard_limit = int(spec["hard_limit"])
        require(
            isinstance(queue, dict) and set(queue) == expected_fields,
            f"{name} queue observation shape is invalid",
        )
        process_current = integer(
            queue.get("maximum_observed_process_current"),
            f"{name} maximum process current",
        )
        process_high_water = integer(
            queue.get("maximum_reported_process_high_water"),
            f"{name} maximum process high-water",
        )
        unit_high_water = integer(
            queue.get("maximum_reported_unit_high_water"),
            f"{name} maximum unit high-water",
            minimum=1,
        )
        activity_initial = integer(
            queue.get("activity_total_initial"), f"{name} initial activity"
        )
        activity_final = integer(
            queue.get("activity_total_final"), f"{name} final activity"
        )
        completion_initial = integer(
            queue.get("completion_total_initial"), f"{name} initial completions"
        )
        completion_final = integer(
            queue.get("completion_total_final"), f"{name} final completions"
        )
        backpressure_initial = integer(
            queue.get("backpressure_total_initial"), f"{name} initial backpressure"
        )
        backpressure_final = integer(
            queue.get("backpressure_total_final"), f"{name} final backpressure"
        )
        require(
            queue.get("status") == "measured"
            and queue.get("name") == name
            and queue.get("measurement_source") == "public_prometheus_metrics"
            and queue.get("capacity_metric") == "worldstream_internal_queue_capacity"
            and queue.get("process_current_metric")
            == "worldstream_internal_queue_process_current"
            and queue.get("process_high_water_metric")
            == "worldstream_internal_queue_process_high_water"
            and queue.get("unit_high_water_metric")
            == "worldstream_internal_queue_unit_high_water"
            and queue.get("activity_metric")
            == "worldstream_internal_queue_activity_total"
            and queue.get("completion_metric")
            == "worldstream_internal_queue_completion_total"
            and queue.get("backpressure_metric")
            == "worldstream_internal_queue_backpressure_total"
            and queue.get("capacity_scope") == spec["capacity_scope"]
            and queue.get("activity_unit") == spec["activity_unit"]
            and queue.get("configured_hard_limit") == hard_limit
            and process_current <= process_high_water
            and unit_high_water <= hard_limit
            and process_high_water >= unit_high_water
            and (spec["capacity_scope"] != "global" or process_high_water <= hard_limit)
            and integer(queue.get("sample_count"), f"{name} sample_count", minimum=1)
            > 0
            and integer(
                queue.get("activity_total_delta"), f"{name} activity delta", minimum=1
            )
            == activity_final - activity_initial
            and integer(
                queue.get("completion_total_delta"),
                f"{name} completion delta",
                minimum=1,
            )
            == completion_final - completion_initial
            and integer(
                queue.get("backpressure_total_delta"),
                f"{name} backpressure delta",
            )
            == backpressure_final - backpressure_initial
            and queue.get("bound_status") == "pass",
            f"{name} queue evidence is inactive, inconsistent, or exceeds its hard limit",
        )
    measurements = report.get("measurements")
    require(
        isinstance(measurements, dict)
        and measurements.get("queue_observation") == observation,
        "standard measurements do not bind the exact queue observation object",
    )
    return observation


def validate_queue_boundary_tests(
    report: dict[str, Any], *, configuration: dict[str, Any]
) -> dict[str, Any]:
    """Bind exact same-production-path saturation tests to the typed gate."""

    require(
        number(
            configuration.get("queue_boundary_test_max_total_seconds"),
            "queue boundary max_total_seconds",
        )
        == QUEUE_BOUNDARY_MAX_TOTAL_SECONDS
        and integer(
            configuration.get("queue_boundary_test_max_output_bytes"),
            "queue boundary max_output_bytes",
            minimum=1,
        )
        == MAX_RUNTIME_LOG_GROWTH_BYTES,
        "queue boundary tests do not use the frozen execution bounds",
    )
    evidence = report.get("queue_boundary_tests")
    expected_fields = {
        "schema",
        "status",
        "execution_scope",
        "total_duration_ms",
        "max_total_seconds",
        "max_output_bytes_per_test",
        "tests",
    }
    require(
        isinstance(evidence, dict)
        and set(evidence) == expected_fields
        and evidence.get("schema") == QUEUE_BOUNDARY_TEST_SCHEMA
        and evidence.get("status") == "pass"
        and evidence.get("execution_scope") == "same_production_queue_paths"
        and number(evidence.get("max_total_seconds"), "queue boundary total bound")
        == QUEUE_BOUNDARY_MAX_TOTAL_SECONDS
        and integer(
            evidence.get("max_output_bytes_per_test"),
            "queue boundary output bound",
            minimum=1,
        )
        == MAX_RUNTIME_LOG_GROWTH_BYTES,
        "queue boundary hard-gate evidence is missing or malformed",
    )
    total_duration_ms = number(
        evidence.get("total_duration_ms"), "queue boundary total duration"
    )
    require(
        0 <= total_duration_ms <= QUEUE_BOUNDARY_MAX_TOTAL_SECONDS * 1000,
        "queue boundary tests exceeded their frozen execution bound",
    )
    tests = evidence.get("tests")
    require(
        isinstance(tests, list)
        and len(tests) == len(QUEUE_BOUNDARY_TESTS)
        and [
            (
                row.get("queue_class"),
                row.get("package"),
                row.get("test_name"),
            )
            for row in tests
            if isinstance(row, dict)
        ]
        == list(QUEUE_BOUNDARY_TESTS),
        "one or more exact production queue boundary tests are missing or reordered",
    )
    row_fields = {
        "queue_class",
        "package",
        "test_name",
        "status",
        "exact_test_count",
        "duration_ms",
        "output_bytes",
        "output_sha256",
    }
    for row, (queue_class, package, test_name) in zip(
        tests, QUEUE_BOUNDARY_TESTS, strict=True
    ):
        require(
            isinstance(row, dict)
            and set(row) == row_fields
            and row.get("queue_class") == queue_class
            and row.get("package") == package
            and row.get("test_name") == test_name
            and row.get("status") == "passed"
            and integer(row.get("exact_test_count"), f"{queue_class} exact test count")
            == 1
            and 0
            <= number(row.get("duration_ms"), f"{queue_class} test duration")
            <= total_duration_ms
            and integer(row.get("output_bytes"), f"{queue_class} output bytes")
            <= MAX_RUNTIME_LOG_GROWTH_BYTES
            and isinstance(row.get("output_sha256"), str)
            and SHA256_REFERENCE.fullmatch(row["output_sha256"]) is not None,
            f"{queue_class} production queue boundary test evidence is invalid",
        )
    measurements = report.get("measurements")
    require(
        isinstance(measurements, dict)
        and measurements.get("queue_boundary_tests") == evidence,
        "standard measurements do not bind the exact queue boundary tests",
    )
    return evidence


def validate_soak(
    report: dict[str, Any],
    *,
    version: str,
    manifest_json_sha256: str,
    manifest_toml_sha256: str,
    packaged_acceptance_sha256: str,
) -> dict[str, Any]:
    require(report.get("schema") == "worldstream/soak-evidence/v1", "wrong soak schema")
    require(report.get("status") == "pass", "soak status must be pass")
    require(
        report.get("release_evidence") is False, "diagnostic soak claim is malformed"
    )
    require(report.get("mode") == "one_hour", "one-hour soak mode was not used")
    identity = report.get("identity")
    require(
        report.get("release_candidate_input") is True
        and isinstance(identity, dict)
        and identity.get("product") == "worldstream"
        and identity.get("profile") == "linux-reference"
        and identity.get("version") == version
        and identity.get("packaged_acceptance_sha256") == packaged_acceptance_sha256,
        "one-hour soak is not bound to the exact packaged backend acceptance bytes",
    )
    scope = report.get("evidence_scope")
    require(
        report.get("evidence_class") == RELEASE_SOAK_EVIDENCE_CLASS
        and isinstance(scope, dict)
        and scope.get("fixture_only") is False
        and scope.get("process_level") is True
        and scope.get("database_workload_bound") is True
        and scope.get("disk_full_fault_injection") is True,
        "release soak requires a process-level database-bound daemon workload",
    )
    platform = report.get("platform")
    require(
        isinstance(platform, dict)
        and platform.get("system") == "Linux"
        and platform.get("machine") in {"x86_64", "amd64"},
        "release soak requires Linux x86-64",
    )
    distribution = validate_distribution(
        report,
        version=version,
        manifest_json_sha256=manifest_json_sha256,
        manifest_toml_sha256=manifest_toml_sha256,
        label="one-hour soak",
    )
    disk_full = validate_disk_full(
        report.get("disk_full"), binary_sha256=distribution["binary_sha256"]
    )
    gate = report.get("named_gate_evidence")
    require(
        isinstance(gate, dict)
        and gate.get("manifest_evidence_id") == EVIDENCE_ID
        and gate.get("release_gate") is True,
        "soak report is not bound to the failure/soak gate",
    )
    configuration = report.get("configuration")
    require(isinstance(configuration, dict), "soak configuration is missing")
    require(
        number(configuration.get("max_total_seconds"), "max_total_seconds")
        == ONE_HOUR_SECONDS
        and number(
            configuration.get("one_hour_target_seconds"), "one_hour_target_seconds"
        )
        == ONE_HOUR_SECONDS,
        "soak must use the exact 3600-second window",
    )
    require(
        configuration.get("database_workload_binding")
        == RELEASE_DATABASE_WORKLOAD_BINDING,
        "soak database measurement is not bound to the workload",
    )
    require(
        report.get("one_hour_window_completed") is True, "one-hour window is incomplete"
    )
    elapsed = number(report.get("elapsed_seconds"), "elapsed_seconds")
    require(
        elapsed >= ONE_HOUR_SECONDS, "soak elapsed time must be at least 3600 seconds"
    )

    preflight = report.get("preflight")
    parsed = preflight.get("test_list_parse") if isinstance(preflight, dict) else None
    coverage = parsed.get("coverage_groups") if isinstance(parsed, dict) else None
    require(
        isinstance(parsed, dict)
        and parsed.get("status") == "pass"
        and parsed.get("missing_groups") == []
        and isinstance(coverage, dict),
        "failure/fuzz coverage preflight did not pass",
    )
    require(
        REQUIRED_COVERAGE_GROUPS.issubset(coverage)
        and all(
            coverage[name].get("status") == "covered"
            for name in REQUIRED_COVERAGE_GROUPS
        ),
        "failure/fuzz coverage matrix is incomplete",
    )
    hooks = report.get("fixture_hooks")
    require(isinstance(hooks, list), "fixture hook results are missing")
    by_category = {
        item.get("category"): item
        for item in hooks
        if isinstance(item, dict) and isinstance(item.get("category"), str)
    }
    require(
        REQUIRED_FIXTURE_HOOKS.issubset(by_category)
        and all(
            by_category[name].get("status") == "passed"
            and integer(
                by_category[name].get("candidate_count"),
                f"{name} candidate_count",
                minimum=1,
            )
            > 0
            for name in REQUIRED_FIXTURE_HOOKS
        ),
        "required resource/fault/corruption fixture hooks did not pass",
    )
    runs = report.get("matrix_runs")
    require(isinstance(runs, list) and runs, "one-hour soak has no matrix runs")
    output_bound = integer(
        configuration.get("max_output_bytes"), "max_output_bytes", minimum=1024
    )
    require(output_bound >= 1024, "output bound is invalid")
    for run in runs:
        require(
            isinstance(run, dict)
            and run.get("status") == "passed"
            and run.get("failure_class") == "none"
            and run.get("output_truncated") is False
            and integer(run.get("output_bytes"), "matrix output_bytes") <= output_bound
            and isinstance(run.get("test_count_validation"), dict)
            and run["test_count_validation"].get("status") == "pass",
            "one-hour matrix contains a failed, truncated, or incomplete run",
        )
    statistics = report.get("statistics")
    memory = statistics.get("memory") if isinstance(statistics, dict) else None
    peaks = memory.get("peak_rss_bytes_per_run") if isinstance(memory, dict) else None
    require(
        isinstance(statistics, dict)
        and integer(statistics.get("matrix_run_count"), "matrix_run_count", minimum=1)
        == len(runs)
        and isinstance(memory, dict)
        and memory.get("status") == "measured"
        and memory.get("scope") == "worldstreamd_process_tree"
        and isinstance(peaks, list)
        and len(peaks) == len(runs)
        and all(
            type(value) is int and 0 < value <= MAX_PEAK_RSS_BYTES for value in peaks
        ),
        "memory resource bounds are missing or exceeded",
    )
    duration = statistics.get("command_duration_ms")
    require(
        isinstance(duration, dict)
        and all(
            type(duration.get(name)) in {int, float} for name in ("p50", "p95", "p99")
        ),
        "duration percentiles are missing",
    )
    database = report.get("database")
    growth = database.get("growth_bytes") if isinstance(database, dict) else None
    wal_growth = (
        database.get("wal_growth_bytes") if isinstance(database, dict) else None
    )
    initial_bytes = (
        database.get("initial_bytes") if isinstance(database, dict) else None
    )
    final_bytes = database.get("final_bytes") if isinstance(database, dict) else None
    workload = report.get("workload")
    require(
        isinstance(database, dict)
        and database.get("status") == "measured"
        and database.get("workload_binding") == RELEASE_DATABASE_WORKLOAD_BINDING
        and type(initial_bytes) is int
        and initial_bytes >= 0
        and type(final_bytes) is int
        and final_bytes >= 0
        and type(growth) is int
        and growth >= 0
        and growth == final_bytes - initial_bytes
        and type(wal_growth) is int
        and wal_growth >= 0,
        "database growth or WAL growth was not measured against the bound workload",
    )
    require(
        isinstance(workload, dict)
        and workload.get("kind") == "worldstreamd_transition_workload"
        and workload.get("database_binding") == RELEASE_DATABASE_WORKLOAD_BINDING
        and integer(
            workload.get("accepted_transition_count"),
            "accepted_transition_count",
            minimum=1,
        )
        > 0,
        "database measurement has no bound transition workload",
    )
    require(
        growth <= MAX_DATABASE_GROWTH_BYTES
        and wal_growth <= MAX_WAL_GROWTH_BYTES
        and database.get("growth_bound_status") == "pass"
        and database.get("wal_growth_bound_status") == "pass",
        "database/WAL growth resource bound was exceeded",
    )
    temp_limit = integer(
        configuration.get("max_temp_growth_bytes"),
        "max_temp_growth_bytes",
        minimum=1,
    )
    log_limit = integer(
        configuration.get("max_log_growth_bytes"),
        "max_log_growth_bytes",
        minimum=1,
    )
    artifact_limit = integer(
        configuration.get("max_artifact_growth_bytes"),
        "max_artifact_growth_bytes",
        minimum=1,
    )
    require(
        temp_limit == MAX_TEMP_GROWTH_BYTES
        and log_limit == MAX_RUNTIME_LOG_GROWTH_BYTES
        and artifact_limit == MAX_AUXILIARY_ARTIFACT_GROWTH_BYTES
        and artifact_limit == temp_limit + log_limit,
        "auxiliary resource configuration does not use the frozen hard limits",
    )
    auxiliary = report.get("temp_and_artifacts")
    temporary = auxiliary.get("temporary") if isinstance(auxiliary, dict) else None
    logs = auxiliary.get("logs") if isinstance(auxiliary, dict) else None
    artifacts = auxiliary.get("artifacts") if isinstance(auxiliary, dict) else None

    def measured_growth(
        value: Any, *, label: str, hard_limit: int
    ) -> tuple[int, int, int]:
        require(isinstance(value, dict), f"{label} growth measurement is missing")
        initial = integer(value.get("initial_bytes"), f"{label} initial_bytes")
        final = integer(value.get("final_bytes"), f"{label} final_bytes")
        measured = integer(value.get("growth_bytes"), f"{label} growth_bytes")
        require(
            measured == final - initial
            and measured >= 0
            and measured <= hard_limit
            and value.get("configured_hard_limit_bytes") == hard_limit
            and value.get("bound_status") == "pass",
            f"{label} growth did not satisfy its exact configured hard limit",
        )
        return initial, final, measured

    temp_initial, temp_final, temp_growth = measured_growth(
        temporary, label="temporary", hard_limit=temp_limit
    )
    log_initial, log_final, log_growth = measured_growth(
        logs, label="daemon log", hard_limit=log_limit
    )
    artifact_initial, artifact_final, artifact_growth = measured_growth(
        artifacts, label="auxiliary artifact", hard_limit=artifact_limit
    )
    require(
        artifact_initial == temp_initial + log_initial
        and artifact_final == temp_final + log_final
        and artifact_growth == temp_growth + log_growth
        and auxiliary.get("initial_bytes") == artifact_initial
        and auxiliary.get("final_bytes") == artifact_final
        and auxiliary.get("growth_bytes") == artifact_growth
        and auxiliary.get("growth_bound_status") == "pass",
        "temporary/log/artifact measurements do not reconcile exactly",
    )
    log_hashes = logs.get("sha256") if isinstance(logs, dict) else None
    log_count = logs.get("file_count") if isinstance(logs, dict) else None
    require(
        type(log_count) is int
        and log_count >= 2
        and auxiliary.get("daemon_log_count") == log_count
        and isinstance(log_hashes, list)
        and len(log_hashes) == log_count
        and auxiliary.get("daemon_log_sha256") == log_hashes
        and all(
            isinstance(value, str) and SHA256_REFERENCE.fullmatch(value) is not None
            for value in log_hashes
        )
        and runs[0].get("output_bytes") == log_final,
        "daemon log inventory is incomplete or not bound to measured output",
    )
    resource_sampling = validate_live_resource_sampling(
        report,
        configuration=configuration,
        elapsed_seconds=elapsed,
        memory=memory,
        database=database,
        temporary=temporary,
        logs=logs,
        artifacts=artifacts,
    )
    queue_observation = validate_queue_observation(report, configuration=configuration)
    queue_boundary_tests = validate_queue_boundary_tests(
        report, configuration=configuration
    )
    privacy = report.get("privacy")
    secret_scan = privacy.get("secret_scan") if isinstance(privacy, dict) else None
    scan_channels = (
        secret_scan.get("channels") if isinstance(secret_scan, dict) else None
    )
    scan_sentinels = (
        secret_scan.get("sentinels") if isinstance(secret_scan, dict) else None
    )
    require(
        isinstance(privacy, dict)
        and privacy.get("status") == "pass"
        and isinstance(secret_scan, dict)
        and secret_scan.get("schema") == SECRET_SCAN_MATRIX_SCHEMA
        and secret_scan.get("status") == "pass"
        and secret_scan.get("secrets_emitted") is False
        and secret_scan.get("encodings_scanned")
        == ["base64", "base64url", "hex", "raw"]
        and isinstance(scan_channels, list)
        and len(scan_channels) == log_count
        and all(
            isinstance(item, dict)
            and set(item) == {"channel", "sha256", "size_bytes"}
            and item.get("channel") == f"daemon-log-{index:02d}"
            and type(item.get("size_bytes")) is int
            and item["size_bytes"] >= 0
            for index, item in enumerate(scan_channels, start=1)
        )
        and [item["sha256"] for item in scan_channels] == log_hashes
        and sum(item["size_bytes"] for item in scan_channels) == log_final
        and isinstance(scan_sentinels, list)
        and all(
            isinstance(item, dict)
            and set(item) == {"name", "sha256", "size_bytes"}
            and isinstance(item.get("name"), str)
            and isinstance(item.get("sha256"), str)
            and SHA256_REFERENCE.fullmatch(item["sha256"]) is not None
            and type(item.get("size_bytes")) is int
            and 16 <= item["size_bytes"] <= 128
            for item in scan_sentinels
        )
        and {item["name"] for item in scan_sentinels}.issuperset(
            {"authority-secret", "operator-capability", "member-capability-01"}
        ),
        "daemon logs lack exact multi-encoding secret-absence evidence",
    )
    return {
        "elapsed_seconds": elapsed,
        "matrix_run_count": len(runs),
        "listed_test_count": parsed.get("listed_test_count"),
        "coverage_groups": sorted(REQUIRED_COVERAGE_GROUPS),
        "peak_rss_bytes": max(peaks),
        "database_growth_bytes": growth,
        "wal_growth_bytes": wal_growth,
        "temp_growth_bytes": temp_growth,
        "log_growth_bytes": log_growth,
        "artifact_growth_bytes": artifact_growth,
        "resource_sampling": resource_sampling,
        "queue_observation": queue_observation,
        "queue_boundary_tests": queue_boundary_tests,
        "secret_scan": secret_scan,
        "accepted_transition_count": workload["accepted_transition_count"],
        "database_workload_binding": RELEASE_DATABASE_WORKLOAD_BINDING,
        "duration_ms": {name: duration[name] for name in ("p50", "p95", "p99")},
        "distribution": distribution,
        "disk_full": disk_full,
    }


def validate_kill_backend_matrix(report: dict[str, Any]) -> dict[str, Any]:
    matrix = report.get("backend_matrix")
    require(
        isinstance(matrix, dict)
        and set(matrix)
        == {
            "status",
            "storage_backend_and_connection_mode_separate",
            "profiles",
            "postgres_provider",
        }
        and matrix.get("status") == "covered"
        and matrix.get("storage_backend_and_connection_mode_separate") is True,
        "kill-point backend matrix is missing or not dimensionally exact",
    )
    profiles = matrix.get("profiles")
    require(
        isinstance(profiles, list) and len(profiles) == len(REQUIRED_KILL_PROFILES),
        "kill-point backend profile inventory is incomplete",
    )
    observed_profiles: list[tuple[str, str]] = []
    for item in profiles:
        require(
            isinstance(item, dict)
            and set(item)
            == {
                "storage_backend",
                "connection_mode",
                "cell_count",
                "overdue_timer_restart",
            }
            and item.get("cell_count") == 12
            and item.get("overdue_timer_restart") == "passed",
            "kill-point backend profile row is malformed",
        )
        observed_profiles.append(
            (item.get("storage_backend"), item.get("connection_mode"))
        )
    require(
        tuple(observed_profiles) == REQUIRED_KILL_PROFILES,
        "kill-point storage backend/connection mode inventory is not frozen",
    )

    provider = matrix.get("postgres_provider")
    require(
        isinstance(provider, dict)
        and set(provider)
        == {
            "status",
            "postgres_image",
            "postgres_digest",
            "pgbouncer_image",
            "pgbouncer_digest",
            "engine_identity",
            "server_version_num",
            "pool_mode",
            "migration_connection_mode",
            "observation_connection_mode",
            "control_binary_sha256",
            "runtime_dsn_sentinel_coverage",
            "cleanup",
            "privacy",
        }
        and provider.get("status") == "passed"
        and provider.get("postgres_image") == POSTGRES_KILL_IMAGE
        and provider.get("postgres_digest") == POSTGRES_KILL_DIGEST
        and provider.get("pgbouncer_image") == PGBOUNCER_KILL_IMAGE
        and provider.get("pgbouncer_digest") == PGBOUNCER_KILL_DIGEST
        and provider.get("engine_identity")
        == "postgresql/17.11; server_version_num=170011"
        and provider.get("server_version_num") == "170011"
        and provider.get("pool_mode") == "transaction"
        and provider.get("migration_connection_mode") == "direct_admin_offline"
        and provider.get("observation_connection_mode") == "direct_admin"
        and isinstance(provider.get("control_binary_sha256"), str)
        and SHA256_REFERENCE.fullmatch(provider["control_binary_sha256"]) is not None
        and provider.get("cleanup") == "pass",
        "kill-point PostgreSQL provider binding is incomplete or used a pooler admin path",
    )
    dsn_coverage = provider.get("runtime_dsn_sentinel_coverage")
    require(
        isinstance(dsn_coverage, dict)
        and set(dsn_coverage)
        == {"status", "expected_count", "observed_count", "profiles"}
        and dsn_coverage.get("status") == "complete"
        and dsn_coverage.get("expected_count") == 32
        and dsn_coverage.get("observed_count") == 32
        and dsn_coverage.get("profiles")
        == [
            {
                "storage_backend": "postgresql",
                "connection_mode": "direct",
                "dsn_count": 16,
            },
            {
                "storage_backend": "postgresql",
                "connection_mode": "transaction_pool",
                "dsn_count": 16,
            },
        ],
        "kill-point PostgreSQL runtime DSN sentinel coverage is incomplete",
    )
    privacy = provider.get("privacy")
    provider_scan = privacy.get("secret_scan") if isinstance(privacy, dict) else None
    provider_channels = (
        provider_scan.get("channels") if isinstance(provider_scan, dict) else None
    )
    provider_sentinels = (
        provider_scan.get("sentinels") if isinstance(provider_scan, dict) else None
    )
    class_rows = privacy.get("channel_classes") if isinstance(privacy, dict) else None
    require(
        isinstance(privacy, dict)
        and set(privacy) == {"status", "secret_scan", "channel_classes"}
        and privacy.get("status") == "pass"
        and isinstance(provider_scan, dict)
        and provider_scan.get("schema") == SECRET_SCAN_MATRIX_SCHEMA
        and provider_scan.get("status") == "pass"
        and provider_scan.get("secrets_emitted") is False
        and provider_scan.get("encodings_scanned")
        == ["base64", "base64url", "hex", "raw"]
        and isinstance(provider_channels, list)
        and provider_channels
        and isinstance(provider_sentinels, list)
        and provider_sentinels
        and isinstance(class_rows, list)
        and class_rows,
        "kill-point PostgreSQL provider privacy evidence is incomplete",
    )
    channel_names = {
        item.get("channel")
        for item in provider_channels
        if isinstance(item, dict)
        and set(item) == {"channel", "sha256", "size_bytes"}
        and isinstance(item.get("channel"), str)
        and isinstance(item.get("sha256"), str)
        and SHA256_REFERENCE.fullmatch(item["sha256"]) is not None
        and type(item.get("size_bytes")) is int
        and item["size_bytes"] >= 0
    }
    classes_by_channel = {
        item.get("channel"): item.get("class")
        for item in class_rows
        if isinstance(item, dict)
        and set(item) == {"channel", "class"}
        and isinstance(item.get("channel"), str)
        and isinstance(item.get("class"), str)
    }
    require(
        len(channel_names) == len(provider_channels)
        and len(classes_by_channel) == len(class_rows)
        and set(classes_by_channel) == channel_names
        and REQUIRED_PROVIDER_CHANNEL_CLASSES.issubset(
            set(classes_by_channel.values())
        ),
        "kill-point PostgreSQL provider channel inventory is not closed",
    )
    sentinel_names = {
        item.get("name")
        for item in provider_sentinels
        if isinstance(item, dict)
        and set(item) == {"name", "sha256", "size_bytes"}
        and isinstance(item.get("name"), str)
        and isinstance(item.get("sha256"), str)
        and SHA256_REFERENCE.fullmatch(item["sha256"]) is not None
        and type(item.get("size_bytes")) is int
        and 16 <= item["size_bytes"] <= 512
    }
    require(
        len(sentinel_names) == len(provider_sentinels)
        and all(
            any(name.startswith(prefix) for name in sentinel_names)
            for prefix in (
                "postgres-admin-password-",
                "postgres-runtime-password-",
                "postgres-admin-dsn-",
                "postgres-runtime-dsn-",
            )
        ),
        "kill-point PostgreSQL provider secret sentinel inventory is incomplete",
    )
    expected_runtime_dsn_sentinels = {
        f"postgres-runtime-dsn-postgresql_{connection_mode}-{sequence:03d}-01"
        for connection_mode in ("direct", "transaction_pool")
        for sequence in range(1, 17)
    }
    require(
        {
            name
            for name in sentinel_names
            if name.startswith("postgres-runtime-dsn-postgresql_")
        }
        == expected_runtime_dsn_sentinels,
        "kill-point PostgreSQL runtime DSN sentinels do not cover every cloned store",
    )
    return provider


def validate_overdue_timer_restart(report: dict[str, Any]) -> dict[str, Any]:
    overdue = report.get("overdue_timer_restart_regression")
    profiles = overdue.get("profiles") if isinstance(overdue, dict) else None
    require(
        isinstance(overdue, dict)
        and set(overdue) == {"status", "profiles"}
        and overdue.get("status") == "passed"
        and isinstance(profiles, list)
        and len(profiles) == len(REQUIRED_KILL_PROFILES),
        "overdue Timer restart regression is missing or incomplete",
    )
    observed: dict[tuple[str, str], dict[str, Any]] = {}
    for item in profiles:
        require(
            isinstance(item, dict) and set(item) == OVERDUE_TIMER_PROFILE_FIELDS,
            "overdue Timer restart profile schema is not closed",
        )
        key = (item.get("storage_backend"), item.get("connection_mode"))
        require(
            key in REQUIRED_KILL_PROFILES and key not in observed,
            "overdue Timer restart profiles contain an unexpected or duplicate mode",
        )
        require(
            item.get("status") == "passed"
            and item.get("durable_state") == "catching_up_with_overdue_timer"
            and item.get("daemon_ready") is True
            and item.get("ordinary_work_gated_before_drain") is True
            and item.get("exact_timer_retry_drained") is True
            and item.get("first_result_duplicate") is False
            and item.get("second_result_duplicate") is True
            and item.get("receipt_hash_equal") is True
            and isinstance(item.get("receipt_hash"), str)
            and SHA256_REFERENCE.fullmatch(item["receipt_hash"]) is not None
            and item.get("projection_hash_equal_after_restart") is True
            and isinstance(item.get("projection_hash"), str)
            and SHA256_REFERENCE.fullmatch(item["projection_hash"]) is not None
            and item.get("same_data_directory") is True,
            "overdue Timer restart regression did not preserve its exact outcome",
        )
        observed[key] = item
    require(
        set(observed) == set(REQUIRED_KILL_PROFILES),
        "overdue Timer restart regression is missing a backend runtime profile",
    )
    return {"status": "passed", "profiles": list(observed.values())}


def validate_kill(
    report: dict[str, Any],
    *,
    version: str,
    manifest_json_sha256: str,
    manifest_toml_sha256: str,
) -> dict[str, Any]:
    require(
        report.get("schema") == "worldstream/kill-point-evidence/v1",
        "wrong kill-point schema",
    )
    require(
        report.get("status") == "passed"
        and report.get("release_evidence") is False
        and report.get("evidence_class") == "process_level"
        and report.get("linux") is True,
        "process-level Linux kill-point evidence did not pass",
    )
    platform = report.get("platform")
    require(
        isinstance(platform, dict)
        and platform.get("system") == "Linux"
        and platform.get("machine") in {"x86_64", "amd64"},
        "kill-point evidence requires Linux x86-64",
    )
    distribution = validate_distribution(
        report,
        version=version,
        manifest_json_sha256=manifest_json_sha256,
        manifest_toml_sha256=manifest_toml_sha256,
        label="kill-point matrix",
    )
    gate = report.get("named_gate_evidence")
    require(
        isinstance(gate, dict)
        and gate.get("manifest_evidence_id") == EVIDENCE_ID
        and gate.get("release_gate") is True,
        "kill-point report is not bound to the failure/soak gate",
    )
    provider = validate_kill_backend_matrix(report)
    overdue_timer_restart = validate_overdue_timer_restart(report)
    kill = report.get("kill_points")
    boundaries = kill.get("boundaries") if isinstance(kill, dict) else None
    require(
        isinstance(kill, dict)
        and kill.get("status") == "covered"
        and kill.get("process_kill_claim") is True
        and kill.get("power_loss_claim") is False
        and kill.get("matrix_complete") is True,
        "documented process-level SIGKILL matrix is incomplete",
    )
    require(isinstance(boundaries, list), "documented kill-point cells are missing")
    required_cells = {
        (storage_backend, connection_mode, operation, boundary)
        for storage_backend, connection_mode in REQUIRED_KILL_PROFILES
        for operation in REQUIRED_KILL_OPERATIONS
        for boundary in REQUIRED_KILL_BOUNDARIES
    }
    observed_cells: dict[tuple[str, str, str, str], dict[str, Any]] = {}
    for item in boundaries:
        if not isinstance(item, dict):
            raise EvidenceError("documented kill-point matrix contains an invalid cell")
        require(
            all(
                isinstance(item.get(field), str)
                for field in (
                    "storage_backend",
                    "connection_mode",
                    "operation",
                    "name",
                )
            ),
            "documented kill-point matrix contains an invalid cell identity",
        )
        key = (
            item.get("storage_backend"),
            item.get("connection_mode"),
            item.get("operation"),
            item.get("name"),
        )
        require(
            key in required_cells and key not in observed_cells,
            "documented kill-point matrix contains an unexpected or duplicate cell",
        )
        expected_outcome = EXPECTED_KILL_OUTCOMES[key[3]]
        verification = item.get("verification")
        required_verification = REQUIRED_CELL_VERIFICATION[key[2]]
        require(
            item.get("signal") == "SIGKILL"
            and item.get("status") == "passed"
            and item.get("signal_sent") is True
            and item.get("process_exit_observed") is True
            and item.get("restart_status") == "passed"
            and item.get("same_data_directory") is True
            and item.get("expected_outcome") == expected_outcome
            and item.get("outcome_verified") is True
            and item.get("reply_resolution_status") == "passed",
            "documented kill-point matrix cell did not preserve its frozen outcome",
        )
        require(
            item.get("marker_verified") is True,
            "documented kill-point matrix cell has no verified boundary marker",
        )
        require(
            isinstance(verification, dict)
            and required_verification.issubset(verification)
            and all(verification[name] is True for name in required_verification),
            f"documented {key[2]} kill-point cell lacks operation-specific outcome witnesses",
        )
        observed_cells[key] = item
    require(
        len(observed_cells) == EXPECTED_KILL_CELL_COUNT
        and set(observed_cells) == required_cells,
        "documented kill-point matrix is missing required operation/boundary cells",
    )
    privacy = report.get("privacy")
    secret_scan = privacy.get("secret_scan") if isinstance(privacy, dict) else None
    channels = secret_scan.get("channels") if isinstance(secret_scan, dict) else None
    sentinels = secret_scan.get("sentinels") if isinstance(secret_scan, dict) else None
    require(
        isinstance(privacy, dict)
        and privacy.get("status") == "pass"
        and isinstance(secret_scan, dict)
        and secret_scan.get("schema") == SECRET_SCAN_MATRIX_SCHEMA
        and secret_scan.get("status") == "pass"
        and secret_scan.get("secrets_emitted") is False
        and secret_scan.get("encodings_scanned")
        == ["base64", "base64url", "hex", "raw"]
        and isinstance(channels, list)
        and channels
        and all(
            isinstance(item, dict)
            and set(item) == {"channel", "sha256", "size_bytes"}
            and item.get("channel") == f"daemon-log-{index:03d}"
            and isinstance(item.get("sha256"), str)
            and SHA256_REFERENCE.fullmatch(item["sha256"]) is not None
            and type(item.get("size_bytes")) is int
            and item["size_bytes"] >= 0
            for index, item in enumerate(channels, start=1)
        )
        and isinstance(sentinels, list)
        and all(
            isinstance(item, dict)
            and set(item) == {"name", "sha256", "size_bytes"}
            and isinstance(item.get("name"), str)
            and isinstance(item.get("sha256"), str)
            and SHA256_REFERENCE.fullmatch(item["sha256"]) is not None
            and type(item.get("size_bytes")) is int
            and 16 <= item["size_bytes"] <= 512
            for item in sentinels
        )
        and any(
            isinstance(item, dict)
            and isinstance(item.get("name"), str)
            and item["name"].startswith("authority-secret-")
            for item in sentinels
        )
        and any(
            isinstance(item, dict)
            and isinstance(item.get("name"), str)
            and item["name"].startswith("operator-capability-")
            for item in sentinels
        ),
        "kill-point daemon logs lack exact multi-encoding secret-absence evidence",
    )
    return {
        "signal": "SIGKILL",
        "operations": list(REQUIRED_KILL_OPERATIONS),
        "boundaries": list(REQUIRED_KILL_BOUNDARIES),
        "cell_count": len(observed_cells),
        "profiles": [
            {
                "storage_backend": storage_backend,
                "connection_mode": connection_mode,
            }
            for storage_backend, connection_mode in REQUIRED_KILL_PROFILES
        ],
        "restart": "same_data_directory",
        "power_loss_claim": False,
        "distribution": distribution,
        "secret_scan": secret_scan,
        "postgres_provider": provider,
        "overdue_timer_restart_regression": overdue_timer_restart,
    }


def validate_logs(paths: list[Path], excluded: set[Path]) -> list[dict[str, Any]]:
    require(len(paths) >= 2, "at least two retained logs are required")
    require(len(set(paths)) == len(paths), "retained logs must be unique")
    values: list[dict[str, Any]] = []
    for index, path in enumerate(paths, start=1):
        regular_file(path, f"retained log {index}")
        require(
            path not in excluded, "retained logs must be distinct from JSON reports"
        )
        size = path.stat().st_size
        require(0 < size <= MAX_LOG_BYTES, "retained log is empty or exceeds its bound")
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            raise EvidenceError(
                f"retained log is not bounded UTF-8: {error}"
            ) from error
        require(
            not any(pattern.search(text) for pattern in SECRET_PATTERNS),
            "retained log contains secret-like material",
        )
        values.append(
            {
                "id": f"retained-log-{index}",
                "sha256": digest(path),
                "size_bytes": size,
            }
        )
    return values


def atomic_write(path: Path, value: dict[str, Any], label: str) -> None:
    if path.exists() and path.is_symlink():
        raise EvidenceError(f"{label} must not be a symlink")
    if path.parent.exists() and path.parent.is_symlink():
        raise EvidenceError(f"{label} parent must not be a symlink")
    path.parent.mkdir(parents=True, exist_ok=True)
    data = PRODUCER.canonical_json(value)
    descriptor, raw_temp = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(raw_temp)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    except OSError as error:
        temporary.unlink(missing_ok=True)
        raise EvidenceError(f"cannot write {label}: {error}") from error


def produce(
    output: Path,
    artifact_output: Path,
    soak_path: Path,
    kill_path: Path,
    log_paths: list[Path],
    package_archive: Path,
    package_report_path: Path,
    daemon_bin: Path,
    packaged_acceptance_path: Path,
    manifest_toml: Path,
    manifest_json: Path,
) -> None:
    inputs = {
        soak_path.resolve(),
        kill_path.resolve(),
        *(path.resolve() for path in log_paths),
        package_archive.resolve(),
        package_report_path.resolve(),
        daemon_bin.resolve(),
        packaged_acceptance_path.resolve(),
    }
    destinations = {output.resolve(), artifact_output.resolve()}
    require(len(destinations) == 2, "producer and artifact outputs must be distinct")
    require(
        inputs.isdisjoint(destinations),
        "producer outputs must not overwrite evidence inputs",
    )
    manifest = PRODUCER.COLLECTOR.load_manifest(manifest_toml, manifest_json)
    expected_manifest_toml_sha256 = digest(manifest_toml)
    expected_manifest_json_sha256 = digest(manifest_json)
    try:
        verified_distribution, _package_report, _package_raw, package_binding = (
            REFERENCE_PRODUCER.verify_packaged_distribution(
                package_archive,
                package_report_path,
                daemon_bin,
                manifest_toml,
                manifest_json,
                manifest,
            )
        )
        packaged_acceptance, packaged_acceptance_raw, acceptance_sha256 = (
            REFERENCE_PRODUCER.verify_packaged_acceptance(
                packaged_acceptance_path,
                verified_distribution,
                package_binding,
            )
        )
    except REFERENCE_PRODUCER.ReferenceError as error:
        raise EvidenceError(
            f"cannot verify packaged acceptance boundary: {error}"
        ) from error
    soak = validate_soak(
        read_json(soak_path, "one-hour soak report"),
        version=manifest["release_candidate"],
        manifest_json_sha256=expected_manifest_json_sha256,
        manifest_toml_sha256=expected_manifest_toml_sha256,
        packaged_acceptance_sha256=acceptance_sha256,
    )
    kill = validate_kill(
        read_json(kill_path, "kill-point report"),
        version=manifest["release_candidate"],
        manifest_json_sha256=expected_manifest_json_sha256,
        manifest_toml_sha256=expected_manifest_toml_sha256,
    )
    require(
        soak["distribution"] == kill["distribution"],
        "kill-point and one-hour soak distribution identities differ",
    )
    runtime_distribution_fields = set(soak["distribution"])
    require(
        soak["distribution"]
        == {
            field: verified_distribution[field] for field in runtime_distribution_fields
        },
        "failure/soak reports do not bind the independently verified package bytes",
    )
    require(
        kill["postgres_provider"]["control_binary_sha256"]
        == verified_distribution.get("control_binary_sha256"),
        "kill-point PostgreSQL direct-admin binary differs from the verified package",
    )
    logs = validate_logs(log_paths, {soak_path, kill_path})
    input_reports = [
        {
            "id": "one-hour-soak",
            "sha256": digest(soak_path),
            "size_bytes": soak_path.stat().st_size,
        },
        {
            "id": "process-kill",
            "sha256": digest(kill_path),
            "size_bytes": kill_path.stat().st_size,
        },
        {
            "id": "packaged-backend-acceptance",
            "sha256": acceptance_sha256,
            "size_bytes": len(packaged_acceptance_raw),
        },
    ]
    live_resources = soak["resource_sampling"]["resources"]
    observed_queues = soak["queue_observation"]["observed_queues"]
    artifact = {
        "schema": "worldstream/failure-soak-release-evidence/v1",
        "status": "pass",
        "release_evidence": True,
        "evidence_id": EVIDENCE_ID,
        "version": manifest["release_candidate"],
        "platform": {"system": "Linux", "machine": "x86_64"},
        "distribution": soak["distribution"],
        "packaged_acceptance": {
            "schema": packaged_acceptance["schema"],
            "sha256": acceptance_sha256,
            "size_bytes": len(packaged_acceptance_raw),
            "content_base64": base64.b64encode(packaged_acceptance_raw).decode("ascii"),
        },
        "contract": manifest["contracts"],
        "failure_matrix": {
            "coverage_groups": soak["coverage_groups"],
            "listed_test_count": soak["listed_test_count"],
            "fixture_hooks": sorted(REQUIRED_FIXTURE_HOOKS),
            "disk_full": soak["disk_full"],
            "process_kill": kill,
            "overdue_timer_restart_regression": kill[
                "overdue_timer_restart_regression"
            ],
        },
        "resource_bounds": {
            "live_sampling": soak["resource_sampling"],
            "final_snapshots": {
                "database_growth_bytes": soak["database_growth_bytes"],
                "wal_growth_bytes": soak["wal_growth_bytes"],
                "temporary_growth_bytes": soak["temp_growth_bytes"],
                "log_growth_bytes": soak["log_growth_bytes"],
                "artifact_growth_bytes": soak["artifact_growth_bytes"],
            },
            "queue_observation": soak["queue_observation"],
            "queue_boundary_tests": soak["queue_boundary_tests"],
            "database_workload_binding": soak["database_workload_binding"],
            "accepted_transition_count": soak["accepted_transition_count"],
            "command_duration_ms": soak["duration_ms"],
        },
        "one_hour_soak": {
            "elapsed_seconds": soak["elapsed_seconds"],
            "matrix_run_count": soak["matrix_run_count"],
            "window_completed": True,
        },
        "inputs": input_reports,
        "retained_logs": logs,
        "privacy": {
            "daemon_log_secret_absence": soak["secret_scan"],
            "kill_point_daemon_log_secret_absence": kill["secret_scan"],
        },
    }
    atomic_write(artifact_output, artifact, "failure/soak release artifact")
    binding = {
        "sha256": digest(artifact_output),
        "size_bytes": artifact_output.stat().st_size,
    }
    package_binding = {
        "sha256": verified_distribution["archive_sha256"],
        "size_bytes": verified_distribution["archive_size_bytes"],
    }
    observations = {
        "failure_matrix": (
            f"coverage_groups={len(soak['coverage_groups'])};"
            f"fixture_hooks={len(REQUIRED_FIXTURE_HOOKS)};"
            f"disk_full_errno={soak['disk_full']['fault']['errno_name']};"
            f"process_kill_cells={kill['cell_count']}"
            f";process_kill_profiles={len(kill['profiles'])}"
            ";overdue_timer_restart_profiles="
            f"{len(kill['overdue_timer_restart_regression']['profiles'])}"
            f";packaged_acceptance={acceptance_sha256}"
        ),
        "resource_bounds": (
            f"observed_peak_rss_bytes="
            f"{live_resources['process_tree_rss_bytes']['observed_peak_bytes']};"
            f"database_observed_peak_growth_bytes="
            f"{live_resources['database_bytes']['observed_peak_growth_bytes']};"
            f"wal_observed_peak_growth_bytes="
            f"{live_resources['wal_bytes']['observed_peak_growth_bytes']};"
            f"temp_observed_peak_growth_bytes="
            f"{live_resources['temporary_bytes']['observed_peak_growth_bytes']};"
            f"log_observed_peak_growth_bytes="
            f"{live_resources['log_bytes']['observed_peak_growth_bytes']};"
            f"artifact_observed_peak_growth_bytes="
            f"{live_resources['artifact_bytes']['observed_peak_growth_bytes']};"
            f"observed_internal_queues={len(observed_queues)};"
            f"queue_boundary_tests={len(soak['queue_boundary_tests']['tests'])};"
            f"maximum_unit_high_water="
            f"{max(queue['maximum_reported_unit_high_water'] for queue in observed_queues)};"
            "all_internal_queues_observed=true;"
            f"accepted_transitions={soak['accepted_transition_count']}"
        ),
        "one_hour_soak": (
            f"elapsed_seconds={soak['elapsed_seconds']};"
            f"matrix_runs={soak['matrix_run_count']}"
        ),
        "retained_logs": (
            f"count={len(logs)};total_bytes={sum(item['size_bytes'] for item in logs)}"
        ),
    }
    typed = {
        "schema": PRODUCER.PRODUCER_SCHEMA,
        "producer_id": "linux-failure-soak-release-v1",
        "evidence_id": EVIDENCE_ID,
        "status": "passed",
        "release_evidence": True,
        "fail_closed": False,
        "phase": PRODUCER.COLLECTOR.PRE_SIGN_PHASE,
        "version": manifest["release_candidate"],
        "platform": PRODUCER.SOURCE_BY_ID[SOURCE_ID].platform,
        "contract": manifest["contracts"],
        "outcomes": {
            check: {
                "status": "passed",
                "observations": [{"kind": "verified", "value": observations[check]}],
            }
            for check in CHECKS
        },
        "artifacts": {
            "failure-soak": binding,
            "linux-release-profile": package_binding,
        },
    }
    atomic_write(output, typed, "failure/soak typed producer")
    checked = PRODUCER.read_producer(output, PRODUCER.SOURCE_BY_ID[SOURCE_ID], manifest)
    PRODUCER.verify_artifacts(
        SOURCE_ID,
        checked,
        {
            (SOURCE_ID, "failure-soak"): artifact_output,
            (SOURCE_ID, "linux-release-profile"): package_archive,
        },
    )


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--output", type=Path, required=True)
    command.add_argument("--artifact-output", type=Path, required=True)
    command.add_argument("--soak-report", type=Path, required=True)
    command.add_argument("--kill-point-report", type=Path, required=True)
    command.add_argument("--package-archive", type=Path, required=True)
    command.add_argument("--package-report", type=Path, required=True)
    command.add_argument("--daemon-bin", type=Path, required=True)
    command.add_argument("--packaged-acceptance-report", type=Path, required=True)
    command.add_argument("--retained-log", type=Path, action="append", default=[])
    command.add_argument(
        "--manifest-toml", type=Path, default=PRODUCER.COLLECTOR.DEFAULT_MANIFEST_TOML
    )
    command.add_argument(
        "--manifest-json", type=Path, default=PRODUCER.COLLECTOR.DEFAULT_MANIFEST_JSON
    )
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        produce(
            args.output,
            args.artifact_output,
            args.soak_report,
            args.kill_point_report,
            args.retained_log,
            args.package_archive,
            args.package_report,
            args.daemon_bin,
            args.packaged_acceptance_report,
            args.manifest_toml,
            args.manifest_json,
        )
    except (EvidenceError, PRODUCER.COLLECTOR.CollectionError) as error:
        print(f"failure/soak producer failed: {error}", file=sys.stderr)
        return 1
    print(f"wrote typed failure/soak producer: {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

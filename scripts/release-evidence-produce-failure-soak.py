#!/usr/bin/env python3
"""Produce typed release evidence from a completed Linux failure/soak campaign."""

from __future__ import annotations

import argparse
import base64
import hashlib
import importlib.util
import json
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
MAX_LOG_BYTES = 64 * 1024 * 1024
SECRET_SCAN_MATRIX_SCHEMA = "worldstream/secret-absence-matrix/v1"
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
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise EvidenceError(f"{label} is not valid JSON: {error}") from error
    require(isinstance(value, dict), f"{label} must be a JSON object")
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
        and scope.get("database_workload_bound") is True,
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
    queue_limit = integer(
        configuration.get("max_internal_queue_depth"),
        "max_internal_queue_depth",
        minimum=1,
    )
    require(
        temp_limit == MAX_TEMP_GROWTH_BYTES
        and log_limit == MAX_RUNTIME_LOG_GROWTH_BYTES
        and artifact_limit == MAX_AUXILIARY_ARTIFACT_GROWTH_BYTES
        and artifact_limit == temp_limit + log_limit
        and queue_limit == TELEMETRY_QUEUE_HARD_LIMIT,
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
    queue_evidence = report.get("internal_queues")
    queues = queue_evidence.get("queues") if isinstance(queue_evidence, dict) else None
    require(
        isinstance(queue_evidence, dict)
        and queue_evidence.get("status") == "measured"
        and queue_evidence.get("configured_hard_limits_enforced") is True
        and isinstance(queues, list)
        and len(queues) == 1,
        "internal queue measurement is missing",
    )
    queue = queues[0]
    require(
        isinstance(queue, dict)
        and queue.get("status") == "measured"
        and queue.get("name") == "telemetry_exporter"
        and queue.get("measurement_source") == "public_prometheus_metrics"
        and queue.get("depth_metric") == "worldstream_telemetry_queued"
        and queue.get("capacity_metric") == "worldstream_telemetry_queue_capacity"
        and queue.get("configured_hard_limit") == queue_limit
        and 0
        <= integer(queue.get("maximum_observed_depth"), "maximum queue depth")
        <= queue_limit
        and integer(queue.get("sample_count"), "queue sample_count", minimum=1) > 0
        and integer(queue.get("dropped_total_initial"), "initial queue drops") >= 0
        and integer(queue.get("dropped_total_final"), "final queue drops") >= 0
        and integer(queue.get("dropped_total_delta"), "queue drop delta")
        == queue["dropped_total_final"] - queue["dropped_total_initial"]
        and queue.get("bound_status") == "pass",
        "internal queue exceeded or did not expose its configured hard limit",
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
        "internal_queue": queue,
        "secret_scan": secret_scan,
        "accepted_transition_count": workload["accepted_transition_count"],
        "database_workload_binding": RELEASE_DATABASE_WORKLOAD_BINDING,
        "duration_ms": {name: duration[name] for name in ("p50", "p95", "p99")},
        "distribution": distribution,
    }


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
        (operation, boundary)
        for operation in REQUIRED_KILL_OPERATIONS
        for boundary in REQUIRED_KILL_BOUNDARIES
    }
    observed_cells: dict[tuple[str, str], dict[str, Any]] = {}
    for item in boundaries:
        if not isinstance(item, dict):
            raise EvidenceError("documented kill-point matrix contains an invalid cell")
        key = (item.get("operation"), item.get("name"))
        require(
            key in required_cells and key not in observed_cells,
            "documented kill-point matrix contains an unexpected or duplicate cell",
        )
        expected_outcome = EXPECTED_KILL_OUTCOMES[key[1]]
        verification = item.get("verification")
        required_verification = REQUIRED_CELL_VERIFICATION[key[0]]
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
            f"documented {key[0]} kill-point cell lacks operation-specific outcome witnesses",
        )
        observed_cells[key] = item
    require(
        set(observed_cells) == required_cells,
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
        "restart": "same_data_directory",
        "power_loss_claim": False,
        "distribution": distribution,
        "secret_scan": secret_scan,
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
            "process_kill": kill,
        },
        "resource_bounds": {
            "maximum_peak_rss_bytes": MAX_PEAK_RSS_BYTES,
            "observed_peak_rss_bytes": soak["peak_rss_bytes"],
            "maximum_database_growth_bytes": MAX_DATABASE_GROWTH_BYTES,
            "observed_database_growth_bytes": soak["database_growth_bytes"],
            "maximum_wal_growth_bytes": MAX_WAL_GROWTH_BYTES,
            "observed_wal_growth_bytes": soak["wal_growth_bytes"],
            "maximum_temp_growth_bytes": MAX_TEMP_GROWTH_BYTES,
            "observed_temp_growth_bytes": soak["temp_growth_bytes"],
            "maximum_log_growth_bytes": MAX_RUNTIME_LOG_GROWTH_BYTES,
            "observed_log_growth_bytes": soak["log_growth_bytes"],
            "maximum_auxiliary_artifact_growth_bytes": (
                MAX_AUXILIARY_ARTIFACT_GROWTH_BYTES
            ),
            "observed_auxiliary_artifact_growth_bytes": soak["artifact_growth_bytes"],
            "internal_queues": {
                "maximum_depth": TELEMETRY_QUEUE_HARD_LIMIT,
                "observed": soak["internal_queue"],
            },
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
    observations = {
        "failure_matrix": (
            f"coverage_groups={len(soak['coverage_groups'])};"
            f"fixture_hooks={len(REQUIRED_FIXTURE_HOOKS)};"
            f"process_kill_cells={kill['cell_count']}"
            f";packaged_acceptance={acceptance_sha256}"
        ),
        "resource_bounds": (
            f"peak_rss_bytes={soak['peak_rss_bytes']};"
            f"database_growth_bytes={soak['database_growth_bytes']};"
            f"wal_growth_bytes={soak['wal_growth_bytes']};"
            f"temp_growth_bytes={soak['temp_growth_bytes']};"
            f"log_growth_bytes={soak['log_growth_bytes']};"
            f"artifact_growth_bytes={soak['artifact_growth_bytes']};"
            f"internal_queue_max_depth={soak['internal_queue']['maximum_observed_depth']};"
            f"internal_queue_hard_limit={soak['internal_queue']['configured_hard_limit']};"
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
        "artifacts": {"failure-soak": binding},
    }
    atomic_write(output, typed, "failure/soak typed producer")
    checked = PRODUCER.read_producer(output, PRODUCER.SOURCE_BY_ID[SOURCE_ID], manifest)
    PRODUCER.verify_artifacts(
        SOURCE_ID, checked, {(SOURCE_ID, "failure-soak"): artifact_output}
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

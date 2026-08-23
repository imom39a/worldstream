"""Focused tests for the Linux x86-64 failure/soak release producer."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import shutil
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/release-evidence-produce-failure-soak.py"


def load_module():
    spec = importlib.util.spec_from_file_location("failure_soak_producer", SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture()
def fixture(tmp_path: Path, monkeypatch):
    module = load_module()
    toml = tmp_path / "compatibility.toml"
    mirror = tmp_path / "compatibility.json"
    shutil.copy2(ROOT / "compatibility.toml", toml)
    shutil.copy2(ROOT / "compatibility.json", mirror)
    soak = tmp_path / "soak.json"
    kill = tmp_path / "kill.json"
    logs = [tmp_path / "soak.log", tmp_path / "kill.log"]
    manifest = json.loads(mirror.read_text(encoding="utf-8"))
    distribution = {
        "packaged_artifact_bound": True,
        "reference_class": "fresh_packaged_linux_x86_64",
        "target": "linux-x86_64",
        "version": manifest["release_candidate"],
        "binary_sha256": "sha256:" + "1" * 64,
        "archive_sha256": "sha256:" + "2" * 64,
        "package_report_sha256": "sha256:" + "3" * 64,
        "manifest_sha256": "sha256:" + hashlib.sha256(mirror.read_bytes()).hexdigest(),
        "manifest_json_sha256": "sha256:"
        + hashlib.sha256(mirror.read_bytes()).hexdigest(),
        "manifest_toml_sha256": "sha256:"
        + hashlib.sha256(toml.read_bytes()).hexdigest(),
        "binary_size_bytes": 8_388_608,
        "archive_size_bytes": 16_777_216,
    }
    package_archive = tmp_path / "worldstream-package.tar.gz"
    package_archive.write_bytes(b"verified package bytes")
    distribution["archive_sha256"] = (
        "sha256:" + hashlib.sha256(package_archive.read_bytes()).hexdigest()
    )
    distribution["archive_size_bytes"] = package_archive.stat().st_size
    package_report = tmp_path / "package-report.json"
    package_report.write_text("{}\n", encoding="utf-8")
    daemon = tmp_path / "worldstreamd"
    daemon.write_bytes(b"verified packaged daemon")
    daemon.chmod(0o700)
    acceptance = tmp_path / "packaged-acceptance.json"
    acceptance_value = {
        "schema": "worldstream/packaged-backend-parity/v1",
        "status": "pass",
    }
    acceptance.write_text(
        json.dumps(acceptance_value, sort_keys=True) + "\n", encoding="utf-8"
    )
    acceptance_raw = acceptance.read_bytes()
    acceptance_sha256 = "sha256:" + hashlib.sha256(acceptance_raw).hexdigest()
    verified_distribution = {
        **distribution,
        "control_binary_sha256": "sha256:" + "4" * 64,
        "control_binary_size_bytes": 4_194_304,
    }
    monkeypatch.setattr(
        module.REFERENCE_PRODUCER,
        "verify_packaged_distribution",
        lambda *_args: (verified_distribution, {}, b"{}\n", {"status": "pass"}),
    )
    monkeypatch.setattr(
        module.REFERENCE_PRODUCER,
        "verify_packaged_acceptance",
        lambda *_args: (acceptance_value, acceptance_raw, acceptance_sha256),
    )
    unbound_produce = module.produce

    def produce_with_package(
        output,
        artifact_output,
        soak_path,
        kill_path,
        log_paths,
        manifest_toml,
        manifest_json,
    ):
        return unbound_produce(
            output,
            artifact_output,
            soak_path,
            kill_path,
            log_paths,
            package_archive,
            package_report,
            daemon,
            acceptance,
            manifest_toml,
            manifest_json,
        )

    monkeypatch.setattr(module, "produce", produce_with_package)
    coverage = {
        name: {"status": "covered", "matched_tests": [f"tests::{name}"]}
        for name in module.REQUIRED_COVERAGE_GROUPS
    }

    def observed_queue(spec):
        unit_high_water = min(4096, spec["hard_limit"])
        return {
            "status": "measured",
            "name": spec["name"],
            "measurement_source": "public_prometheus_metrics",
            "capacity_metric": "worldstream_internal_queue_capacity",
            "process_current_metric": "worldstream_internal_queue_process_current",
            "process_high_water_metric": (
                "worldstream_internal_queue_process_high_water"
            ),
            "unit_high_water_metric": "worldstream_internal_queue_unit_high_water",
            "activity_metric": "worldstream_internal_queue_activity_total",
            "completion_metric": "worldstream_internal_queue_completion_total",
            "backpressure_metric": ("worldstream_internal_queue_backpressure_total"),
            "capacity_scope": spec["capacity_scope"],
            "activity_unit": spec["activity_unit"],
            "configured_hard_limit": spec["hard_limit"],
            "maximum_observed_process_current": unit_high_water,
            "maximum_reported_process_high_water": unit_high_water,
            "maximum_reported_unit_high_water": unit_high_water,
            "sample_count": 72000,
            "activity_total_initial": 10,
            "activity_total_final": 20,
            "activity_total_delta": 10,
            "completion_total_initial": 8,
            "completion_total_final": 18,
            "completion_total_delta": 10,
            "backpressure_total_initial": 0,
            "backpressure_total_final": 0,
            "backpressure_total_delta": 0,
            "bound_status": "pass",
        }

    queue_observation = {
        "status": "measured",
        "observation_scope": module.QUEUE_OBSERVATION_SCOPE,
        "all_internal_queues_observed": True,
        "observed_queues": [observed_queue(spec) for spec in module.QUEUE_SPECS],
    }
    queue_boundary_tests = {
        "schema": module.QUEUE_BOUNDARY_TEST_SCHEMA,
        "status": "pass",
        "execution_scope": "same_production_queue_paths",
        "total_duration_ms": 5000.0,
        "max_total_seconds": module.QUEUE_BOUNDARY_MAX_TOTAL_SECONDS,
        "max_output_bytes_per_test": module.MAX_RUNTIME_LOG_GROWTH_BYTES,
        "tests": [
            {
                "queue_class": queue_class,
                "package": package,
                "test_name": test_name,
                "status": "passed",
                "exact_test_count": 1,
                "duration_ms": 500.0,
                "output_bytes": 1024,
                "output_sha256": "sha256:" + str(index) * 64,
            }
            for index, (queue_class, package, test_name) in enumerate(
                module.QUEUE_BOUNDARY_TESTS, start=1
            )
        ],
    }
    rss_sampling = {
        "measurement_source": "linux_proc_process_tree_vmrss",
        "measurement_window": "transition_workload",
        "sampling_interval_ms": module.RSS_SAMPLE_INTERVAL_MS,
        "maximum_gap_ms": module.RESOURCE_SAMPLE_MAX_GAP_MS,
        "observed_max_gap_ms": 50,
        "coverage_duration_ms": 3_600_010,
        "sample_count": 72001,
        "observed_peak_bytes": 134217728,
        "configured_hard_limit_bytes": module.MAX_PEAK_RSS_BYTES,
        "bound_status": "pass",
    }

    def storage_sampling(
        source: str, initial: int, peak: int, hard_limit: int
    ) -> dict[str, object]:
        return {
            "measurement_source": source,
            "measurement_window": "transition_workload_through_recovery",
            "sampling_interval_ms": module.STORAGE_SAMPLE_INTERVAL_MS,
            "maximum_gap_ms": module.RESOURCE_SAMPLE_MAX_GAP_MS,
            "observed_max_gap_ms": 250,
            "coverage_duration_ms": 3_600_500,
            "sample_count": 14402,
            "initial_bytes": initial,
            "observed_peak_bytes": peak,
            "observed_peak_growth_bytes": peak - initial,
            "configured_hard_limit_bytes": hard_limit,
            "bound_status": "pass",
        }

    resource_sampling = {
        "schema": module.RESOURCE_SAMPLING_SCHEMA,
        "status": "measured",
        "sampling_complete": True,
        "peak_semantics": "maximum_observed_at_bounded_sampling_interval",
        "workload_elapsed_ms": 3_600_010,
        "resources": {
            "process_tree_rss_bytes": rss_sampling,
            "database_bytes": storage_sampling(
                "sqlite_main_plus_wal_regular_file_sizes",
                4096,
                6144,
                module.MAX_DATABASE_GROWTH_BYTES,
            ),
            "wal_bytes": storage_sampling(
                "sqlite_wal_regular_file_size",
                0,
                1024,
                module.MAX_WAL_GROWTH_BYTES,
            ),
            "temporary_bytes": storage_sampling(
                "owned_private_working_tree_regular_file_sizes",
                0,
                4096,
                module.MAX_TEMP_GROWTH_BYTES,
            ),
            "log_bytes": storage_sampling(
                "owned_private_working_tree_daemon_log_sizes",
                100,
                13000,
                module.MAX_RUNTIME_LOG_GROWTH_BYTES,
            ),
            "artifact_bytes": storage_sampling(
                "owned_private_working_tree_regular_file_sizes",
                100,
                17000,
                module.MAX_AUXILIARY_ARTIFACT_GROWTH_BYTES,
            ),
        },
    }
    soak.write_text(
        json.dumps(
            {
                "schema": "worldstream/soak-evidence/v1",
                "status": "pass",
                "release_evidence": False,
                "release_candidate_input": True,
                "identity": {
                    "product": "worldstream",
                    "profile": "linux-reference",
                    "version": manifest["release_candidate"],
                    "artifact_sha256": distribution["archive_sha256"],
                    "packaged_acceptance_sha256": acceptance_sha256,
                },
                "evidence_class": module.RELEASE_SOAK_EVIDENCE_CLASS,
                "evidence_scope": {
                    "fixture_only": False,
                    "process_level": True,
                    "database_workload_bound": True,
                    "disk_full_fault_injection": True,
                },
                "mode": "one_hour",
                "platform": {"system": "Linux", "machine": "x86_64"},
                "distribution": distribution,
                "configuration": {
                    "max_total_seconds": 3600,
                    "one_hour_target_seconds": 3600,
                    "max_output_bytes": 262144,
                    "max_database_growth_bytes": 268435456,
                    "max_wal_growth_bytes": module.MAX_WAL_GROWTH_BYTES,
                    "max_temp_growth_bytes": module.MAX_TEMP_GROWTH_BYTES,
                    "max_log_growth_bytes": module.MAX_RUNTIME_LOG_GROWTH_BYTES,
                    "max_artifact_growth_bytes": (
                        module.MAX_AUXILIARY_ARTIFACT_GROWTH_BYTES
                    ),
                    "internal_queue_hard_limits": {
                        spec["name"]: {
                            "capacity_scope": spec["capacity_scope"],
                            "hard_limit": spec["hard_limit"],
                            "activity_unit": spec["activity_unit"],
                        }
                        for spec in module.QUEUE_SPECS
                    },
                    "queue_boundary_test_max_total_seconds": (
                        module.QUEUE_BOUNDARY_MAX_TOTAL_SECONDS
                    ),
                    "queue_boundary_test_max_output_bytes": (
                        module.MAX_RUNTIME_LOG_GROWTH_BYTES
                    ),
                    "max_peak_rss_bytes": module.MAX_PEAK_RSS_BYTES,
                    "database_workload_binding": (
                        module.RELEASE_DATABASE_WORKLOAD_BINDING
                    ),
                },
                "named_gate_evidence": {
                    "manifest_evidence_id": module.EVIDENCE_ID,
                    "release_gate": True,
                    "release_evidence": False,
                },
                "disk_full": {
                    "schema": module.DISK_FULL_SCHEMA,
                    "status": "pass",
                    "release_evidence": False,
                    "scenario": module.DISK_FULL_SCENARIO,
                    "evidence_class": module.DISK_FULL_EVIDENCE_CLASS,
                    "platform": {
                        "system": "Linux",
                        "machine": "x86_64",
                        "filesystem": "ext4",
                    },
                    "container": {
                        "image": module.DISK_FULL_CONTAINER_IMAGE,
                        "platform": "linux/amd64",
                        "privileged": True,
                        "network": "none",
                        "root_filesystem_read_only": True,
                        "daemon_mount_read_only": True,
                        "observed_elapsed_ms": 2100.5,
                        "captured_output_bytes": 2048,
                    },
                    "bounds": {
                        "filesystem_image_bytes": (
                            module.DISK_FULL_FILESYSTEM_IMAGE_BYTES
                        ),
                        "attempted_write_bytes": (
                            module.DISK_FULL_ATTEMPTED_WRITE_BYTES
                        ),
                        "max_daemon_seconds": module.DISK_FULL_MAX_DAEMON_SECONDS,
                        "max_container_seconds": (
                            module.DISK_FULL_MAX_CONTAINER_SECONDS
                        ),
                        "max_log_bytes": module.DISK_FULL_MAX_LOG_BYTES,
                        "max_capture_bytes": module.DISK_FULL_MAX_CAPTURE_BYTES,
                    },
                    "filesystem": {
                        "type": "ext4",
                        "mount_source_class": "loop_device",
                        "image_size_bytes": (module.DISK_FULL_FILESYSTEM_IMAGE_BYTES),
                        "block_size_bytes": module.DISK_FULL_BLOCK_SIZE_BYTES,
                        "available_kib_after_fill": 0,
                        "database_file_type": "regular",
                        "database_file_mode": "0600",
                        "fill_bytes_written": 63 * 1024 * 1024,
                    },
                    "fault": {
                        "errno_number": 28,
                        "errno_name": "ENOSPC",
                        "attempted_write_bytes": (
                            module.DISK_FULL_ATTEMPTED_WRITE_BYTES
                        ),
                        "write_returned_bytes": 0,
                        "database_size_before_bytes": 0,
                        "database_size_after_bytes": 0,
                    },
                    "daemon": {
                        "binary_sha256": distribution["binary_sha256"],
                        "started": True,
                        "exit_observed": True,
                        "exit_code": 1,
                        "ready_http_200_observed": False,
                        "public_mutation_available": False,
                        "storage_failure_observed": True,
                        "elapsed_ms": 125.5,
                        "diagnostic_bytes": 512,
                        "diagnostic_sha256": "sha256:" + "6" * 64,
                    },
                    "cleanup": {
                        "internal_unmount_observed": True,
                        "container_remove_requested": True,
                        "container_absent_after_run": True,
                    },
                    "limitations": {
                        "runtime_disk_exhaustion_recovery_observed": False,
                        "physical_power_loss_observed": False,
                    },
                },
                "preflight": {
                    "test_list_parse": {
                        "status": "pass",
                        "listed_test_count": 100,
                        "coverage_groups": coverage,
                        "missing_groups": [],
                    }
                },
                "fixture_hooks": [
                    {"category": name, "status": "passed", "candidate_count": 1}
                    for name in module.REQUIRED_FIXTURE_HOOKS
                ],
                "matrix_runs": [
                    {
                        "status": "passed",
                        "failure_class": "none",
                        "output_bytes": 12000,
                        "output_truncated": False,
                        "test_count_validation": {"status": "pass"},
                    }
                ],
                "statistics": {
                    "matrix_run_count": 1,
                    "command_duration_ms": {"p50": 1000, "p95": 1100, "p99": 1200},
                    "memory": {
                        "status": "measured",
                        "scope": "worldstreamd_process_tree",
                        "peak_rss_bytes": 134217728,
                        "peak_rss_bytes_per_run": [134217728],
                        "observed_peak_delta_bytes": 0,
                        "sampling_interval_ms": module.RSS_SAMPLE_INTERVAL_MS,
                        "maximum_gap_ms": module.RESOURCE_SAMPLE_MAX_GAP_MS,
                        "observed_max_gap_ms": 50,
                        "coverage_duration_ms": 3_600_010,
                        "sample_count": 72001,
                    },
                },
                "measurements": {
                    "resource_sampling": resource_sampling,
                    "queue_observation": queue_observation,
                    "queue_boundary_tests": queue_boundary_tests,
                },
                "resource_sampling": resource_sampling,
                "database": {
                    "status": "measured",
                    "workload_binding": module.RELEASE_DATABASE_WORKLOAD_BINDING,
                    "initial_bytes": 4096,
                    "final_bytes": 5120,
                    "growth_bytes": 1024,
                    "wal_initial_bytes": 0,
                    "wal_final_bytes": 512,
                    "wal_growth_bytes": 512,
                    "growth_bound_status": "pass",
                    "wal_growth_bound_status": "pass",
                },
                "temp_and_artifacts": {
                    "status": "measured",
                    "temporary": {
                        "initial_bytes": 0,
                        "final_bytes": 1024,
                        "growth_bytes": 1024,
                        "configured_hard_limit_bytes": module.MAX_TEMP_GROWTH_BYTES,
                        "bound_status": "pass",
                    },
                    "logs": {
                        "initial_bytes": 100,
                        "final_bytes": 12000,
                        "growth_bytes": 11900,
                        "configured_hard_limit_bytes": (
                            module.MAX_RUNTIME_LOG_GROWTH_BYTES
                        ),
                        "bound_status": "pass",
                        "file_count": 2,
                        "sha256": ["sha256:" + "a" * 64, "sha256:" + "b" * 64],
                    },
                    "artifacts": {
                        "definition": (
                            "all non-database temporary-workspace and daemon-log bytes"
                        ),
                        "initial_bytes": 100,
                        "final_bytes": 13024,
                        "growth_bytes": 12924,
                        "configured_hard_limit_bytes": (
                            module.MAX_AUXILIARY_ARTIFACT_GROWTH_BYTES
                        ),
                        "bound_status": "pass",
                    },
                    "initial_bytes": 100,
                    "final_bytes": 13024,
                    "growth_bytes": 12924,
                    "growth_bound_status": "pass",
                    "daemon_log_count": 2,
                    "daemon_log_sha256": [
                        "sha256:" + "a" * 64,
                        "sha256:" + "b" * 64,
                    ],
                },
                "queue_observation": queue_observation,
                "queue_boundary_tests": queue_boundary_tests,
                "privacy": {
                    "status": "pass",
                    "secret_scan": {
                        "schema": module.SECRET_SCAN_MATRIX_SCHEMA,
                        "status": "pass",
                        "secrets_emitted": False,
                        "encodings_scanned": [
                            "base64",
                            "base64url",
                            "hex",
                            "raw",
                        ],
                        "sentinels": [
                            {
                                "name": "authority-secret",
                                "sha256": "sha256:" + "c" * 64,
                                "size_bytes": 32,
                            },
                            {
                                "name": "member-capability-01",
                                "sha256": "sha256:" + "d" * 64,
                                "size_bytes": 69,
                            },
                            {
                                "name": "operator-capability",
                                "sha256": "sha256:" + "e" * 64,
                                "size_bytes": 69,
                            },
                        ],
                        "channels": [
                            {
                                "channel": "daemon-log-01",
                                "sha256": "sha256:" + "a" * 64,
                                "size_bytes": 6000,
                            },
                            {
                                "channel": "daemon-log-02",
                                "sha256": "sha256:" + "b" * 64,
                                "size_bytes": 6000,
                            },
                        ],
                    },
                },
                "workload": {
                    "kind": "worldstreamd_transition_workload",
                    "database_binding": module.RELEASE_DATABASE_WORKLOAD_BINDING,
                    "accepted_transition_count": 1000,
                },
                "elapsed_seconds": 3600.01,
                "one_hour_window_completed": True,
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    kill_cells = [
        {
            "storage_backend": storage_backend,
            "connection_mode": connection_mode,
            "operation": operation,
            "name": boundary,
            "signal": "SIGKILL",
            "status": "passed",
            "signal_sent": True,
            "process_exit_observed": True,
            "restart_status": "passed",
            "same_data_directory": True,
            "expected_outcome": module.EXPECTED_KILL_OUTCOMES[boundary],
            "outcome_verified": True,
            "reply_resolution_status": "passed",
            "marker_verified": True,
            "verification": {
                name: True for name in module.REQUIRED_CELL_VERIFICATION[operation]
            },
        }
        for storage_backend, connection_mode in module.REQUIRED_KILL_PROFILES
        for operation in module.REQUIRED_KILL_OPERATIONS
        for boundary in module.REQUIRED_KILL_BOUNDARIES
    ]
    provider_channel_classes = sorted(module.REQUIRED_PROVIDER_CHANNEL_CLASSES)
    provider_channels = [
        {
            "channel": f"child-{index:04d}",
            "sha256": "sha256:" + f"{index:x}"[-1] * 64,
            "size_bytes": 128,
        }
        for index, _channel_class in enumerate(provider_channel_classes, start=1)
    ]
    provider_privacy = {
        "status": "pass",
        "secret_scan": {
            "schema": module.SECRET_SCAN_MATRIX_SCHEMA,
            "status": "pass",
            "secrets_emitted": False,
            "encodings_scanned": ["base64", "base64url", "hex", "raw"],
            "sentinels": [
                {
                    "name": name,
                    "sha256": "sha256:" + f"{index:x}"[-1] * 64,
                    "size_bytes": 48,
                }
                for index, name in enumerate(
                    (
                        "postgres-admin-password-01",
                        "postgres-runtime-password-01",
                        "postgres-admin-dsn-01",
                    )
                    + tuple(
                        f"postgres-runtime-dsn-postgresql_{connection_mode}-"
                        f"{sequence:03d}-01"
                        for connection_mode in ("direct", "transaction_pool")
                        for sequence in range(1, 17)
                    ),
                    start=9,
                )
            ],
            "channels": provider_channels,
        },
        "channel_classes": [
            {"channel": channel["channel"], "class": channel_class}
            for channel, channel_class in zip(
                provider_channels, provider_channel_classes, strict=True
            )
        ],
    }
    overdue_profiles = [
        {
            "status": "passed",
            "storage_backend": storage_backend,
            "connection_mode": connection_mode,
            "durable_state": "catching_up_with_overdue_timer",
            "daemon_ready": True,
            "ordinary_work_gated_before_drain": True,
            "exact_timer_retry_drained": True,
            "first_result_duplicate": False,
            "second_result_duplicate": True,
            "receipt_hash_equal": True,
            "receipt_hash": "sha256:" + "a" * 64,
            "projection_hash_equal_after_restart": True,
            "projection_hash": "sha256:" + "b" * 64,
            "same_data_directory": True,
        }
        for storage_backend, connection_mode in module.REQUIRED_KILL_PROFILES
    ]
    kill.write_text(
        json.dumps(
            {
                "schema": "worldstream/kill-point-evidence/v1",
                "status": "passed",
                "release_evidence": False,
                "evidence_class": "process_level",
                "linux": True,
                "platform": {"system": "Linux", "machine": "x86_64"},
                "distribution": distribution,
                "named_gate_evidence": {
                    "manifest_evidence_id": module.EVIDENCE_ID,
                    "release_gate": True,
                    "release_evidence": False,
                },
                "backend_matrix": {
                    "status": "covered",
                    "storage_backend_and_connection_mode_separate": True,
                    "profiles": [
                        {
                            "storage_backend": storage_backend,
                            "connection_mode": connection_mode,
                            "cell_count": 12,
                            "overdue_timer_restart": "passed",
                        }
                        for storage_backend, connection_mode in module.REQUIRED_KILL_PROFILES
                    ],
                    "postgres_provider": {
                        "status": "passed",
                        "postgres_image": module.POSTGRES_KILL_IMAGE,
                        "postgres_digest": module.POSTGRES_KILL_DIGEST,
                        "pgbouncer_image": module.PGBOUNCER_KILL_IMAGE,
                        "pgbouncer_digest": module.PGBOUNCER_KILL_DIGEST,
                        "engine_identity": (
                            "postgresql/17.11; server_version_num=170011"
                        ),
                        "server_version_num": "170011",
                        "pool_mode": "transaction",
                        "migration_connection_mode": "direct_admin_offline",
                        "observation_connection_mode": "direct_admin",
                        "control_binary_sha256": verified_distribution[
                            "control_binary_sha256"
                        ],
                        "runtime_dsn_sentinel_coverage": {
                            "status": "complete",
                            "expected_count": 32,
                            "observed_count": 32,
                            "profiles": [
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
                        },
                        "cleanup": "pass",
                        "privacy": provider_privacy,
                    },
                },
                "operation": {
                    "status": "committed",
                    "commit_boundary": "observable_http_2xx_response",
                    "retry": {"attempted": True, "status": "passed"},
                },
                "kill_points": {
                    "status": "covered",
                    "process_kill_claim": True,
                    "power_loss_claim": False,
                    "matrix_complete": True,
                    "boundaries": kill_cells,
                },
                "restart": {
                    "status": "passed",
                    "same_data_directory": True,
                    "health_observed": True,
                },
                "overdue_timer_restart_regression": {
                    "status": "passed",
                    "profiles": overdue_profiles,
                },
                "privacy": {
                    "status": "pass",
                    "secret_scan": {
                        "schema": module.SECRET_SCAN_MATRIX_SCHEMA,
                        "status": "pass",
                        "secrets_emitted": False,
                        "encodings_scanned": [
                            "base64",
                            "base64url",
                            "hex",
                            "raw",
                        ],
                        "sentinels": [
                            {
                                "name": "authority-secret-01",
                                "sha256": "sha256:" + "f" * 64,
                                "size_bytes": 32,
                            },
                            {
                                "name": "operator-capability-01",
                                "sha256": "sha256:" + "e" * 64,
                                "size_bytes": 69,
                            },
                        ],
                        "channels": [
                            {
                                "channel": "daemon-log-001",
                                "sha256": "sha256:" + "d" * 64,
                                "size_bytes": 128,
                            }
                        ],
                    },
                },
                "comparison": {
                    "status": "passed",
                    "method": "canonical-json-sha256",
                    "before": "a" * 64,
                    "after": "a" * 64,
                    "equal": True,
                    "retry_response_hash_equal": True,
                },
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    for path in logs:
        path.write_text("bounded retained release evidence log\n", encoding="utf-8")
    return module, toml, mirror, soak, kill, logs, tmp_path


def produce(fixture):
    module, toml, mirror, soak, kill, logs, root = fixture
    output = root / "producer.json"
    artifact = root / "failure-soak.json"
    module.produce(output, artifact, soak, kill, logs, toml, mirror)
    return (
        module,
        json.loads(output.read_text()),
        json.loads(artifact.read_text()),
        artifact,
    )


def test_happy_path_emits_strict_byte_bound_typed_producer(fixture) -> None:
    module, producer, artifact, artifact_path = produce(fixture)
    assert producer["schema"] == module.PRODUCER.PRODUCER_SCHEMA
    assert producer["evidence_id"] == module.EVIDENCE_ID
    assert producer["status"] == "passed"
    assert set(producer["outcomes"]) == set(module.CHECKS)
    assert artifact["status"] == "pass"
    assert artifact["release_evidence"] is True
    assert artifact["platform"] == {"system": "Linux", "machine": "x86_64"}
    process_kill = artifact["failure_matrix"]["process_kill"]
    assert process_kill["cell_count"] == module.EXPECTED_KILL_CELL_COUNT
    assert [
        (item["storage_backend"], item["connection_mode"])
        for item in process_kill["profiles"]
    ] == list(module.REQUIRED_KILL_PROFILES)
    assert (
        artifact["failure_matrix"]["overdue_timer_restart_regression"]["status"]
        == "passed"
    )
    assert artifact["packaged_acceptance"]["sha256"].startswith("sha256:")
    assert artifact["packaged_acceptance"]["content_base64"]
    assert artifact["failure_matrix"]["disk_full"]["fault"] == {
        "errno_number": 28,
        "errno_name": "ENOSPC",
        "attempted_write_bytes": module.DISK_FULL_ATTEMPTED_WRITE_BYTES,
        "write_returned_bytes": 0,
        "database_size_before_bytes": 0,
        "database_size_after_bytes": 0,
    }
    assert artifact["failure_matrix"]["disk_full"]["container"]["image"] == (
        module.DISK_FULL_CONTAINER_IMAGE
    )
    assert artifact["failure_matrix"]["disk_full"]["filesystem"] == {
        "type": "ext4",
        "mount_source_class": "loop_device",
        "image_size_bytes": module.DISK_FULL_FILESYSTEM_IMAGE_BYTES,
        "block_size_bytes": module.DISK_FULL_BLOCK_SIZE_BYTES,
        "available_kib_after_fill": 0,
        "database_file_type": "regular",
        "database_file_mode": "0600",
        "fill_bytes_written": 63 * 1024 * 1024,
    }
    assert (
        artifact["failure_matrix"]["disk_full"]["daemon"]["ready_http_200_observed"]
        is False
    )
    assert artifact["failure_matrix"]["disk_full"]["cleanup"] == {
        "internal_unmount_observed": True,
        "container_remove_requested": True,
        "container_absent_after_run": True,
    }
    sampling = artifact["resource_bounds"]["live_sampling"]
    assert sampling["sampling_complete"] is True
    assert sampling["resources"]["wal_bytes"]["observed_peak_growth_bytes"] == 1024
    queue_observation = artifact["resource_bounds"]["queue_observation"]
    assert queue_observation["all_internal_queues_observed"] is True
    assert [queue["name"] for queue in queue_observation["observed_queues"]] == [
        spec["name"] for spec in module.QUEUE_SPECS
    ]
    boundary_tests = artifact["resource_bounds"]["queue_boundary_tests"]
    assert boundary_tests["status"] == "pass"
    assert [row["queue_class"] for row in boundary_tests["tests"]] == [
        spec[0] for spec in module.QUEUE_BOUNDARY_TESTS
    ]
    declared = producer["artifacts"]["failure-soak"]
    assert declared["size_bytes"] == artifact_path.stat().st_size
    package = producer["artifacts"]["linux-release-profile"]
    package_archive = fixture[6] / "worldstream-package.tar.gz"
    assert package == {
        "sha256": module.digest(package_archive),
        "size_bytes": package_archive.stat().st_size,
    }
    module.PRODUCER.read_producer(
        artifact_path.with_name("producer.json"),
        module.PRODUCER.SOURCE_BY_ID[module.SOURCE_ID],
        module.PRODUCER.COLLECTOR.load_manifest(fixture[1], fixture[2]),
    )


def test_soak_must_bind_exact_raw_packaged_acceptance_bytes(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(soak.read_text(encoding="utf-8"))
    report["identity"]["packaged_acceptance_sha256"] = "sha256:" + "0" * 64
    soak.write_text(json.dumps(report) + "\n", encoding="utf-8")

    with pytest.raises(module.EvidenceError, match="exact packaged backend acceptance"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


def test_reported_distribution_must_match_independently_verified_package_bytes(
    fixture, monkeypatch
) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    original = module.REFERENCE_PRODUCER.verify_packaged_distribution

    def stale_archive(*args):
        distribution, report, raw, binding = original(*args)
        return (
            {**distribution, "archive_sha256": "sha256:" + "9" * 64},
            report,
            raw,
            binding,
        )

    monkeypatch.setattr(
        module.REFERENCE_PRODUCER, "verify_packaged_distribution", stale_archive
    )

    with pytest.raises(module.EvidenceError, match="independently verified package"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


def test_package_swap_after_verification_cannot_be_rebound(
    fixture, monkeypatch
) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    package_archive = root / "worldstream-package.tar.gz"
    original = module.REFERENCE_PRODUCER.verify_packaged_distribution

    def swap_after_verification(*args):
        result = original(*args)
        package_archive.write_bytes(b"different package bytes after verification")
        return result

    monkeypatch.setattr(
        module.REFERENCE_PRODUCER,
        "verify_packaged_distribution",
        swap_after_verification,
    )

    with pytest.raises(
        module.PRODUCER.ProducerError,
        match="linux-release-profile digest mismatch",
    ):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        (("platform", "machine"), "aarch64", "Linux x86-64"),
        (("one_hour_window_completed",), False, "one-hour window"),
        (("elapsed_seconds",), 3599, "at least 3600"),
        (("statistics", "memory", "status"), "not_measured", "memory"),
        (("database", "growth_bytes"), 268435457, "database growth"),
    ],
)
def test_soak_gaps_fail_closed(fixture, field, value, message) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(soak.read_text())
    target = report
    for name in field[:-1]:
        target = target[name]
    target[field[-1]] = value
    soak.write_text(json.dumps(report) + "\n")
    with pytest.raises(module.EvidenceError, match=message):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


def test_missing_disk_full_evidence_fails_closed(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(soak.read_text())
    report.pop("disk_full")
    soak.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="disk-full evidence is missing"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        (("schema",), "worldstream/disk-full-evidence/v0", "exact release scenario"),
        (("status",), "fail", "exact release scenario"),
        (("release_evidence",), True, "exact release scenario"),
        (("scenario",), "synthetic_disk_full", "exact release scenario"),
        (("evidence_class",), "unit_test", "exact release scenario"),
        (("platform", "system"), "Darwin", "Linux x86-64 ext4"),
        (("platform", "machine"), "aarch64", "Linux x86-64 ext4"),
        (("platform", "filesystem"), "tmpfs", "Linux x86-64 ext4"),
        (("container", "image"), "docker:latest", "container identity"),
        (("container", "platform"), "linux/arm64", "container identity"),
        (("container", "privileged"), False, "container identity"),
        (("container", "network"), "bridge", "container identity"),
        (
            ("container", "root_filesystem_read_only"),
            False,
            "container identity",
        ),
        (("container", "daemon_mount_read_only"), False, "container identity"),
        (("container", "observed_elapsed_ms"), 120001, "container identity"),
        (("container", "captured_output_bytes"), 262145, "container identity"),
        (("bounds", "filesystem_image_bytes"), 1, "frozen bounds"),
        (("bounds", "attempted_write_bytes"), 8192, "frozen bounds"),
        (("bounds", "max_daemon_seconds"), 11.0, "frozen bounds"),
        (("bounds", "max_container_seconds"), 121.0, "frozen bounds"),
        (("bounds", "max_log_bytes"), 1, "frozen bounds"),
        (("bounds", "max_capture_bytes"), 1, "frozen bounds"),
        (("filesystem", "type"), "tmpfs", "ext4 mount"),
        (("filesystem", "mount_source_class"), "host_path", "ext4 mount"),
        (("filesystem", "image_size_bytes"), 1, "ext4 mount"),
        (("filesystem", "block_size_bytes"), 1024, "ext4 mount"),
        (("filesystem", "available_kib_after_fill"), 1, "ext4 mount"),
        (("filesystem", "database_file_type"), "character", "ext4 mount"),
        (("filesystem", "database_file_mode"), "0644", "ext4 mount"),
        (("filesystem", "fill_bytes_written"), 0, "ext4 mount"),
        (("fault", "errno_number"), 5, "bounded ENOSPC"),
        (("fault", "errno_name"), "EIO", "bounded ENOSPC"),
        (("fault", "attempted_write_bytes"), 8192, "bounded ENOSPC"),
        (("fault", "write_returned_bytes"), 1, "bounded ENOSPC"),
        (("fault", "database_size_before_bytes"), 1, "bounded ENOSPC"),
        (("fault", "database_size_after_bytes"), 1, "bounded ENOSPC"),
        (("daemon", "started"), False, "did not fail closed"),
        (("daemon", "exit_observed"), False, "did not fail closed"),
        (("daemon", "exit_code"), 2, "did not fail closed"),
        (("daemon", "ready_http_200_observed"), True, "did not fail closed"),
        (("daemon", "public_mutation_available"), True, "did not fail closed"),
        (("daemon", "storage_failure_observed"), False, "did not fail closed"),
        (("daemon", "binary_sha256"), "sha256:" + "9" * 64, "did not fail closed"),
        (("daemon", "elapsed_ms"), 10001, "did not fail closed"),
        (("daemon", "diagnostic_bytes"), 65537, "did not fail closed"),
        (("daemon", "diagnostic_sha256"), "sha256:bad", "did not fail closed"),
        (("cleanup", "internal_unmount_observed"), False, "cleanup"),
        (("cleanup", "container_remove_requested"), False, "cleanup"),
        (("cleanup", "container_absent_after_run"), False, "cleanup"),
        (
            ("limitations", "runtime_disk_exhaustion_recovery_observed"),
            True,
            "overclaims",
        ),
        (("limitations", "physical_power_loss_observed"), True, "overclaims"),
    ],
)
def test_disk_full_witness_must_be_exact_and_bound_to_the_packaged_daemon(
    fixture, field, value, message
) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(soak.read_text())
    target = report["disk_full"]
    for name in field[:-1]:
        target = target[name]
    target[field[-1]] = value
    soak.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match=message):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


@pytest.mark.parametrize(
    ("report_name", "field", "value", "message"),
    [
        (
            "soak",
            "packaged_artifact_bound",
            False,
            "fresh packaged Linux x86-64",
        ),
        ("soak", "version", "0.0.0-stale", "fresh packaged Linux x86-64"),
        (
            "soak",
            "manifest_sha256",
            "sha256:" + "0" * 64,
            "manifest pair identity mismatch",
        ),
        (
            "soak",
            "manifest_json_sha256",
            "sha256:" + "0" * 64,
            "manifest pair identity mismatch",
        ),
        (
            "kill",
            "manifest_toml_sha256",
            "sha256:" + "0" * 64,
            "manifest pair identity mismatch",
        ),
        (
            "kill",
            "reference_class",
            "source_tree_debug_binary",
            "fresh packaged Linux x86-64",
        ),
        ("kill", "binary_size_bytes", 0, "binary_size_bytes"),
    ],
)
def test_unpackaged_or_stale_distribution_cannot_be_promoted(
    fixture, report_name, field, value, message
) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    path = soak if report_name == "soak" else kill
    report = json.loads(path.read_text())
    report["distribution"][field] = value
    path.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match=message):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


def test_kill_and_soak_must_reference_the_identical_packaged_binary(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(kill.read_text())
    report["distribution"]["binary_sha256"] = "sha256:" + "9" * 64
    kill.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="distribution identities differ"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


def test_kill_report_cannot_claim_release_evidence_or_wrong_native_platform(
    fixture,
) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(kill.read_text())
    report["release_evidence"] = True
    kill.write_text(json.dumps(report) + "\n")
    with pytest.raises(module.EvidenceError, match="did not pass"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )

    report["release_evidence"] = False
    report["platform"]["machine"] = "aarch64"
    kill.write_text(json.dumps(report) + "\n")
    with pytest.raises(module.EvidenceError, match="Linux x86-64"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


@pytest.mark.parametrize(
    ("operation", "witness"),
    [
        ("room_create", "pre_retry_creation_receipt_count_matches_boundary"),
        ("action", "final_transition_applied_once"),
        ("timer", "durable_receipt_equal"),
        ("activation_lease", "claim_identity_equal"),
    ],
)
def test_each_operation_requires_its_observed_outcome_witness(
    fixture, operation, witness
) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(kill.read_text())
    cell = next(
        item
        for item in report["kill_points"]["boundaries"]
        if item["operation"] == operation
    )
    cell["verification"][witness] = False
    kill.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="outcome witnesses"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


def test_each_kill_cell_requires_a_verified_boundary_marker(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(kill.read_text())
    report["kill_points"]["boundaries"][0]["marker_verified"] = False
    kill.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="verified boundary marker"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


@pytest.mark.parametrize(
    "tamper",
    ["remove_cell", "duplicate_substitution", "mislabel_connection_mode"],
)
def test_exact_36_cell_backend_runtime_set_fails_closed_on_substitution(
    fixture, tamper: str
) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(kill.read_text())
    cells = report["kill_points"]["boundaries"]
    target = next(
        index
        for index, item in enumerate(cells)
        if item["storage_backend"] == "postgresql"
        and item["connection_mode"] == "transaction_pool"
        and item["operation"] == "action"
        and item["name"] == "after_commit_before_publication"
    )
    if tamper == "remove_cell":
        cells.pop(target)
    elif tamper == "duplicate_substitution":
        cells[target] = dict(cells[0])
    else:
        cells[target]["connection_mode"] = "direct"
    kill.write_text(json.dumps(report) + "\n")

    with pytest.raises(
        module.EvidenceError, match="unexpected or duplicate|missing required"
    ):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("migration_connection_mode", "transaction_pool"),
        ("observation_connection_mode", "transaction_pool"),
        ("pool_mode", "session"),
        ("cleanup", "failed"),
    ],
)
def test_postgres_provider_rejects_pooler_admin_or_incomplete_cleanup(
    fixture, field: str, value: str
) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(kill.read_text())
    report["backend_matrix"]["postgres_provider"][field] = value
    kill.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="pooler admin path"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


def test_postgres_admin_binary_must_match_verified_package(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(kill.read_text())
    report["backend_matrix"]["postgres_provider"]["control_binary_sha256"] = (
        "sha256:" + "9" * 64
    )
    kill.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="direct-admin binary differs"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


@pytest.mark.parametrize("tamper", ["count", "missing_store_sentinel"])
def test_every_postgres_clone_has_unique_runtime_dsn_scan_coverage(
    fixture, tamper: str
) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(kill.read_text())
    provider = report["backend_matrix"]["postgres_provider"]
    if tamper == "count":
        provider["runtime_dsn_sentinel_coverage"]["observed_count"] = 31
    else:
        provider["privacy"]["secret_scan"]["sentinels"].pop()
    kill.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="runtime DSN sentinel"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


@pytest.mark.parametrize(
    "tamper",
    ["missing_object", "missing_profile", "false_witness", "schema_extension"],
)
def test_overdue_timer_restart_regression_is_closed_and_tamper_evident(
    fixture, tamper: str
) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(kill.read_text())
    overdue = report["overdue_timer_restart_regression"]
    if tamper == "missing_object":
        report.pop("overdue_timer_restart_regression")
    elif tamper == "missing_profile":
        overdue["profiles"].pop()
    elif tamper == "false_witness":
        overdue["profiles"][0]["ordinary_work_gated_before_drain"] = False
    else:
        overdue["profiles"][0]["unbound_claim"] = True
    kill.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="overdue Timer restart"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


def test_observer_only_database_file_cannot_be_promoted(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(soak.read_text())
    report["evidence_class"] = "bounded_fixture_only"
    report["evidence_scope"] = {
        "fixture_only": True,
        "process_level": False,
        "database_workload_bound": False,
    }
    report["configuration"]["database_workload_binding"] = (
        "observer_only_not_bound_to_cargo_test_databases"
    )
    report["database"]["workload_binding"] = (
        "observer_only_not_bound_to_cargo_test_databases"
    )
    report["database"]["growth_bytes"] = 0
    report["database"]["initial_bytes"] = 4096
    report["database"]["final_bytes"] = 4096
    soak.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="database-bound daemon workload"):
        module.produce(
            root / "out.json",
            root / "artifact.json",
            soak,
            kill,
            logs,
            toml,
            mirror,
        )


def test_temp_log_artifact_growth_must_reconcile_to_exact_limits(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(soak.read_text())
    report["temp_and_artifacts"]["artifacts"]["growth_bytes"] += 1
    soak.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="auxiliary artifact growth"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


def test_each_queue_must_expose_and_obey_its_frozen_capacity(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(soak.read_text())
    report["queue_observation"]["observed_queues"][1][
        "maximum_reported_unit_high_water"
    ] = 3
    soak.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="telemetry_dns_resolver_queue"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        ("partial", "partial"),
        ("inactive", "activity delta"),
        ("negative_backpressure", "backpressure delta"),
        ("overclaim_process", "inactive, inconsistent"),
    ],
)
def test_queue_observation_overclaims_and_tampering_fail_closed(
    fixture, mutation, message
) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(soak.read_text())
    observation = report["queue_observation"]
    if mutation == "partial":
        observation["all_internal_queues_observed"] = False
    elif mutation == "inactive":
        queue = observation["observed_queues"][2]
        queue["activity_total_final"] = queue["activity_total_initial"]
        queue["activity_total_delta"] = 0
    elif mutation == "negative_backpressure":
        queue = observation["observed_queues"][3]
        queue["backpressure_total_final"] = 0
        queue["backpressure_total_initial"] = 1
        queue["backpressure_total_delta"] = -1
    else:
        queue = observation["observed_queues"][4]
        queue["maximum_observed_process_current"] = (
            queue["maximum_reported_process_high_water"] + 1
        )
    soak.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match=message):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


def test_transient_live_peak_cannot_hide_behind_small_final_growth(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(soak.read_text())
    temporary = report["resource_sampling"]["resources"]["temporary_bytes"]
    temporary["observed_peak_bytes"] = module.MAX_TEMP_GROWTH_BYTES + 1
    temporary["observed_peak_growth_bytes"] = module.MAX_TEMP_GROWTH_BYTES + 1
    soak.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="temporary_bytes live peak"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        ("missing_sampling", "sampling contract"),
        ("rss_gap", "RSS peak, cadence"),
        ("storage_gap", "database_bytes live peak"),
        ("rss_coverage", "RSS peak, cadence"),
    ],
)
def test_live_sampling_gaps_and_missing_evidence_fail_closed(
    fixture, mutation, message
) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(soak.read_text())
    if mutation == "missing_sampling":
        report.pop("resource_sampling")
    elif mutation == "rss_gap":
        report["resource_sampling"]["resources"]["process_tree_rss_bytes"][
            "observed_max_gap_ms"
        ] = module.RESOURCE_SAMPLE_MAX_GAP_MS + 1
    elif mutation == "storage_gap":
        report["resource_sampling"]["resources"]["database_bytes"][
            "observed_max_gap_ms"
        ] = module.RESOURCE_SAMPLE_MAX_GAP_MS + 1
    else:
        report["resource_sampling"]["resources"]["process_tree_rss_bytes"][
            "coverage_duration_ms"
        ] = 1
    soak.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match=message):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


@pytest.mark.parametrize(
    "mutation",
    ["missing_queue", "reordered", "unknown_field"],
)
def test_all_queue_classes_and_the_closed_shape_are_required(fixture, mutation) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(soak.read_text())
    observation = report["queue_observation"]
    if mutation == "missing_queue":
        observation["observed_queues"].pop()
    elif mutation == "reordered":
        observation["observed_queues"].reverse()
    else:
        observation["unobserved_queue_bounds_claimed"] = False
    soak.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="partial|missing|reordered"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        ("missing", "missing or reordered"),
        ("wrong_test", "missing or reordered"),
        ("failed", "production queue boundary test evidence"),
        ("over_output", "production queue boundary test evidence"),
        ("bad_digest", "production queue boundary test evidence"),
        ("unbound", "do not bind"),
    ],
)
def test_exact_production_queue_boundary_gate_fails_closed(
    fixture, mutation, message
) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(soak.read_text())
    evidence = report["queue_boundary_tests"]
    if mutation == "missing":
        evidence["tests"].pop()
    elif mutation == "wrong_test":
        evidence["tests"][0]["test_name"] = "tests::lookalike"
    elif mutation == "failed":
        evidence["tests"][1]["status"] = "failed"
    elif mutation == "over_output":
        evidence["tests"][2]["output_bytes"] = module.MAX_RUNTIME_LOG_GROWTH_BYTES + 1
    elif mutation == "bad_digest":
        evidence["tests"][3]["output_sha256"] = "sha256:not-a-digest"
    else:
        report["measurements"]["queue_boundary_tests"] = {
            **evidence,
            "status": "missing",
        }
    soak.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match=message):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


def test_duplicate_json_keys_fail_closed_before_evidence_validation(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    raw = soak.read_text(encoding="utf-8")
    soak.write_text(
        raw.replace('"status": "pass",', '"status": "pass", "status": "pass",', 1),
        encoding="utf-8",
    )

    with pytest.raises(module.EvidenceError, match="duplicate key"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


def test_both_process_lanes_require_retained_secret_scan_evidence(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(kill.read_text())
    report["privacy"]["secret_scan"]["channels"] = []
    kill.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="secret-absence evidence"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


def test_missing_process_kill_or_hash_parity_fails_closed(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(kill.read_text())
    report["kill_points"]["process_kill_claim"] = False
    report["kill_points"]["boundaries"][0]["outcome_verified"] = False
    kill.write_text(json.dumps(report) + "\n")
    with pytest.raises(module.EvidenceError, match="SIGKILL matrix"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


def test_single_after_http_kill_is_diagnostic_not_a_release_matrix(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(kill.read_text())
    report["kill_points"]["matrix_complete"] = False
    report["kill_points"]["boundaries"] = [
        {
            "name": "after_http_2xx_response",
            "signal": "SIGKILL",
            "status": "passed",
            "signal_sent": True,
            "process_exit_observed": True,
        }
    ]
    kill.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="SIGKILL matrix is incomplete"):
        module.produce(
            root / "out.json",
            root / "artifact.json",
            soak,
            kill,
            logs,
            toml,
            mirror,
        )


def test_logs_are_regular_nonempty_bounded_and_secret_free(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    logs[0].write_text("Authorization: Bearer leaked-value\n")
    with pytest.raises(module.EvidenceError, match="secret-like"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )
    logs[0].unlink()
    logs[0].symlink_to(logs[1])
    with pytest.raises(module.EvidenceError, match="symlink"):
        module.produce(
            root / "out.json", root / "artifact.json", soak, kill, logs, toml, mirror
        )


def test_outputs_cannot_overwrite_each_other_or_evidence_inputs(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    with pytest.raises(module.EvidenceError, match="outputs must be distinct"):
        module.produce(
            root / "same.json", root / "same.json", soak, kill, logs, toml, mirror
        )
    with pytest.raises(module.EvidenceError, match="must not overwrite"):
        module.produce(soak, root / "artifact.json", soak, kill, logs, toml, mirror)

"""Shared exact frozen-target fixtures for reference evidence tests."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]


def storage_bindings(filesystem: dict[str, Any]) -> dict[str, Any]:
    database_filesystem = json.loads(json.dumps(filesystem))
    return {
        "schema": "worldstream/packaged-storage-bindings/v1",
        "status": "verified",
        "profile": "frozen_local_ext4",
        "reference_environment_filesystem_role": "workload_owned_runtime_parent",
        "layout": "split_host_mounts",
        "raw_paths_retained": False,
        "workload": {
            "role": "workload_owned_runtime_parent",
            "filesystem": filesystem,
            "mount_identity": {"mount_id": 41, "device_id": "8:1"},
        },
        "postgresql_database": {
            "role": "provider_owned_database_volume",
            "filesystem": database_filesystem,
            "mount_identity": {"mount_id": 52, "device_id": "8:2"},
            "docker_volume": {
                "driver": "local",
                "scope": "local",
                "driver_options": {},
                "container_destination": "/var/lib/postgresql/data",
                "container_mount_type": "volume",
                "read_write": True,
                "source_matches_volume_mountpoint": True,
                "run_unique_ownership_label_verified": True,
            },
        },
    }


def valid_kill_report(
    tmp_path: Path,
    monkeypatch: Any,
    *,
    distribution: dict[str, Any],
) -> dict[str, Any]:
    """Reuse the owning failure-soak fixture and rebind its exact distribution."""

    support_root = tmp_path / "failure-support"
    support_root.mkdir()
    path = ROOT / "tests/release_evidence_produce_failure_soak.py"
    spec = importlib.util.spec_from_file_location(
        "worldstream_reference_failure_fixture_support", path
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    fixture_value = module.fixture.__wrapped__(support_root, monkeypatch)
    kill_path = fixture_value[4]
    value = json.loads(kill_path.read_text(encoding="utf-8"))
    runtime_fields = set(value["distribution"])
    value["distribution"] = {field: distribution[field] for field in runtime_fields}
    value["backend_matrix"]["postgres_provider"]["control_binary_sha256"] = (
        distribution["control_binary_sha256"]
    )
    return value


def target_report(
    producer: Any,
    *,
    manifest: dict[str, Any],
    distribution: dict[str, Any],
    acceptance: dict[str, Any],
    acceptance_raw: bytes,
    soak: dict[str, Any],
    soak_raw: bytes,
    kill: dict[str, Any],
    kill_raw: bytes,
    package_archive: Path,
    package_report: Path,
    daemon: Path,
    manifest_toml: Path,
    manifest_json: Path,
) -> dict[str, Any]:
    bindings = {
        "package_archive": producer.file_binding(
            package_archive, "worldstream/linux-release-archive/v1"
        ),
        "package_report": producer.file_binding(
            package_report, "worldstream/package-report/v1"
        ),
        "daemon": producer.file_binding(daemon, "worldstream/worldstreamd-elf/v1"),
        "packaged_acceptance": producer.exact_binding(
            acceptance_raw, acceptance["schema"]
        ),
        "one_hour_soak": producer.exact_binding(soak_raw, soak["schema"]),
        "kill_point": producer.exact_binding(kill_raw, kill["schema"]),
        "manifest_toml": producer.file_binding(
            manifest_toml, "worldstream/storage-compatibility-manifest/toml"
        ),
        "manifest_json": producer.file_binding(manifest_json, manifest["schema"]),
    }
    target_environment = {
        "platform": {
            "system": "Linux",
            "distribution": "Ubuntu",
            "distribution_version": "24.04",
            "machine": "x86_64",
        },
        "hardware": {
            "cpu_model": "target fixture CPU",
            "logical_cpu_count": 4,
            "memory_bytes": 8 * 1024**3,
        },
        "filesystem": {
            "type": "ext4",
            "mount_options": ["rw", "relatime"],
            "storage_class": "local_ssd_or_nvme",
        },
        "engines": {
            "sqlite": {
                "version": "3.53.4",
                "settings": {
                    "journal_mode": "wal",
                    "synchronous": "full",
                    "foreign_keys": "on",
                    "busy_timeout_ms": 5000,
                    "reader_query_only": "on",
                },
                "connection_mode": "embedded",
            },
            "postgresql": {"status": "not_observed_by_sqlite_target_workload"},
        },
    }
    external_sources = {
        "packaged_acceptance": {
            "source_attribution": "bound_external_source",
            "report_sha256": bindings["packaged_acceptance"]["sha256"],
            "source_environment": acceptance["reference_environment"],
        },
        "one_hour_soak": {
            "source_attribution": "bound_external_source",
            "report_sha256": bindings["one_hour_soak"]["sha256"],
            "source_environment": soak["environment_observation"],
        },
        "kill_point": {
            "source_attribution": "bound_external_source",
            "report_sha256": bindings["kill_point"]["sha256"],
            "source_environment": {"platform": kill["platform"]},
        },
    }
    latency = {
        "definition": "nearest-rank",
        "sample_count": 180_000,
        "p50_ms": 4.0,
        "p95_ms": 8.0,
        "p99_ms": 12.0,
    }
    rate = {
        "configured_window_seconds": 1_800,
        "bucket_definition": (
            "1800 contiguous half-open one-second buckets from a monotonic start boundary"
        ),
        "window_start_basis": "monotonic_after_worker_readiness",
        "elapsed_seconds": 1_800,
        "full_second_bucket_count": 1_800,
        "dispatch_attempt_count": 225_000,
        "dispatch_queue_full_count": 44_488,
        "action_attempt_count": 180_000,
        "unconsumed_token_count": 512,
        "minimum_accepted_per_full_second": 100,
        "accepted_transition_count": 180_000,
        "late_accepted_transition_count": 0,
        "aggregate_accepted_per_second": 100.0,
    }

    def row(
        identifier: str,
        observed: dict[str, Any],
        target: dict[str, Any],
        *,
        completed: bool = True,
        met: bool = True,
        hard: bool = False,
    ) -> dict[str, Any]:
        return {
            "id": identifier,
            "classification": (
                "bound_hard_gate" if hard else "measured_non_sla_performance"
            ),
            "attempted": True,
            "completed": completed,
            "observed": observed,
            "target": target,
            "target_met": met,
            "outcome": "met" if met else "missed",
            "method": f"fixture exact {identifier} measurement",
        }

    dimensions = [
        row(
            "stored_passivated_rooms",
            {
                "create_attempt_count": 10_000,
                "created_room_count": 10_000,
                "offline_stored_room_count": 10_000,
            },
            {"minimum_room_count": 10_000},
        ),
        row(
            "simultaneously_loaded_rooms",
            {
                "open_attempt_count": 1_000,
                "peak_simultaneously_loaded_room_count": 1_000,
            },
            {"minimum_loaded_room_count": 100},
        ),
        row(
            "mostly_idle_websocket_sessions",
            {
                "open_attempt_count": 1_000,
                "peak_connected_idle_session_count": 1_000,
                "observation_window_seconds": 1_800,
            },
            {"minimum_session_count": 1_000},
        ),
        row(
            "sustained_accepted_transition_rate",
            rate,
            {"minimum_per_second": 100, "continuous_window_seconds": 1_800},
        ),
        row(
            "commit_to_ack_latency",
            latency,
            {"maximum_p95_ms_exclusive": 100},
        ),
        row(
            "one_hour_bounded_soak",
            {
                "elapsed_seconds": 3600.01,
                "window_completed": True,
                "report_sha256": bindings["one_hour_soak"]["sha256"],
                "source_attribution": "bound_external_source",
            },
            {"minimum_elapsed_seconds": 3_600},
            hard=True,
        ),
        row(
            "snapshot_tail_recovery",
            {
                "requested_transition_count": 100_000,
                "action_attempt_count": 33,
                "accepted_transition_count": 32,
                "newest_snapshot_room_seq": None,
                "snapshot_lag_transitions": None,
                "recovery_ms": None,
                "projection_hash_equal": None,
                "pack": {
                    "id": "worldstream.counter",
                    "version": "2.0.0",
                    "digest": "blake3:1c5f75068220f65f9017a062dbe40203c540108b572284d9446a329914008a92",
                    "configuration": {"initial_value": 0, "maximum_value": 16},
                    "accepted_action_sequence": {
                        "increment": 16,
                        "private_ack": 16,
                    },
                },
                "terminal_rejection_code": "counter_limit_reached",
                "recovery_measurement_status": (
                    "not_reachable_due_to_frozen_pack_semantics"
                ),
            },
            {
                "minimum_room_transition_count": 100_000,
                "maximum_snapshot_lag_transitions": 250,
                "maximum_recovery_ms": 5_000,
            },
            completed=False,
            met=False,
        ),
        row(
            "repeated_forced_termination_no_acknowledged_loss",
            {
                "forced_termination_count": 36,
                "acknowledged_loss_count": 0,
                "kill_report_sha256": bindings["kill_point"]["sha256"],
                "source_attribution": "bound_external_source",
            },
            {
                "minimum_forced_termination_count": 2,
                "maximum_acknowledged_loss_count": 0,
            },
            hard=True,
        ),
    ]
    return {
        "schema": producer.REFERENCE_TARGET_SCHEMA,
        "status": "completed",
        "release_evidence": False,
        "performance_class": "reference_non_release",
        "execution": {
            "mode": "package_bound_linux_reference",
            "workload_source": "packaged_worldstreamd_public_api",
            "storage_profile": "sqlite-bundled",
            "connection_mode": "embedded",
            "public_api_only": True,
            "simulated": False,
            "scaled": False,
            "profile_args_locked": True,
            "runner_sha256": producer.sha256(producer.REFERENCE_TARGET_RUNNER_PATH),
        },
        "profile": producer.FROZEN_REFERENCE_PROFILE,
        "identity": {
            "product": "worldstream",
            "profile": "linux-reference",
            "version": manifest["release_candidate"],
            "artifact_sha256": distribution["archive_sha256"],
            "binary_sha256": distribution["binary_sha256"],
            "manifest_json_sha256": distribution["manifest_json_sha256"],
            "manifest_toml_sha256": distribution["manifest_toml_sha256"],
            "packaged_acceptance_sha256": bindings["packaged_acceptance"]["sha256"],
            "packaged_sdk": producer.packaged_sdk_identity(package_archive),
        },
        "reference_environment": target_environment,
        "reference_workload": {
            "payload_sizes_bytes": [2],
            "pack_id": "worldstream.counter",
            "participants_per_room": 1,
            "fan_out": 1,
            "snapshot_cadence_transitions": 1,
        },
        "runtime_observation": {
            "sqlite": {
                "status": "observed_runtime_verified",
                "profile": "sqlite-bundled",
                "exact_identity": "sqlite/3.53.4; source_id=fixture",
            },
            "postgresql": {"status": "not_observed_by_sqlite_target_workload"},
        },
        "bindings": bindings,
        "bound_external_sources": external_sources,
        "dimensions": dimensions,
        "correctness": {
            "status": "passed",
            "acknowledged_transition_ids_unique": True,
            "acknowledged_transition_receipts_verified": True,
            "acknowledged_loss_count": 0,
            "hard_gate_inputs_validated": True,
        },
        "measurements": {
            "commit_to_ack_latency": latency,
            "transition_rate": rate,
            "resource_observation": {
                "peak_process_tree_rss_bytes": 512 * 1024**2,
                "process_tree_rss_sample_count": 18_000,
                "database_bytes_after_workload": 64 * 1024**2,
            },
        },
        "limitations": [
            {
                "code": "frozen_counter_v2_semantic_ceiling",
                "dimension": "snapshot_tail_recovery",
                "publishable_non_sla_target_miss": True,
            }
        ],
    }

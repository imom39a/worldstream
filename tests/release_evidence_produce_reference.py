"""Fail-closed tests for measured reference-performance publication evidence."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import io
import json
import shutil
import sys
import tarfile
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/release-evidence-produce-reference.py"
PROJECTOR = ROOT / "scripts/reference-evidence-project.py"


def load_module():
    spec = importlib.util.spec_from_file_location(
        "worldstream_reference_release_producer", SCRIPT
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def load_projector():
    spec = importlib.util.spec_from_file_location(
        "worldstream_reference_release_projector_test", PROJECTOR
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def sha_bytes(value: bytes) -> str:
    return "sha256:" + hashlib.sha256(value).hexdigest()


def reference_environment() -> dict:
    return {
        "platform": {
            "system": "Linux",
            "distribution": "Ubuntu",
            "distribution_version": "24.04",
            "machine": "x86_64",
        },
        "hardware": {
            "cpu_model": "fixture cpu",
            "logical_cpu_count": 4,
            "memory_bytes": 8_589_934_592,
        },
        "filesystem": {
            "type": "ext4",
            "mount_options": ["rw", "relatime"],
            "storage_class": "local_ssd_or_nvme",
        },
        "engines": {
            "sqlite": {
                "version": "3.51.3",
                "settings": {"journal_mode": "wal", "synchronous": "full"},
                "connection_mode": "embedded",
            },
            "postgresql": {
                "version": "17.11",
                "settings": {
                    "synchronous_commit": "on",
                    "transaction_isolation": "read committed",
                },
                "connection_mode": "direct",
            },
        },
    }


def reference_workload(kind: str) -> dict:
    index = ("counter", "heist", "sqlite", "postgres", "soak").index(kind) + 1
    return {
        "payload_sizes_bytes": [256 * index, 1024 * index],
        "pack_id": {
            "counter": "worldstream.counter",
            "heist": "worldstream.agent-heist",
            "sqlite": "worldstream.sqlite-reference",
            "postgres": "worldstream.postgresql-reference",
            "soak": "worldstream.counter",
        }[kind],
        "participants_per_room": index + 1,
        "fan_out": index,
        "snapshot_cadence_transitions": index,
    }


def raw_report(
    kind: str,
    version: str,
    archive_sha256: str,
    packaged_acceptance_sha256: str,
) -> dict:
    schemas = {
        "counter": "worldstream/imo-55-live-counter-luna/v1",
        "heist": "worldstream/imo-57-absent-broker-live/v1",
        "sqlite": "worldstream/soak-evidence/v1",
        "postgres": "worldstream/postgresql-live-evidence/v1",
        "soak": "worldstream/soak-evidence/v1",
    }
    measurements = {"latency_ms": [1, 2, 3, 4, 5]}
    if kind in {"counter", "heist"}:
        measurements.update(
            {
                "load": {
                    "connections": 8,
                    "active_rooms": 4,
                    "actions_per_second": 40,
                },
                "fan_out": {"observation_fan_out": 10},
            }
        )
    if kind in {"sqlite", "postgres", "soak"}:
        measurements.update(
            {
                "memory": {"status": "measured", "peak_rss_bytes": 10_000_000},
                "database_growth": {"status": "measured", "growth_bytes": 4096},
            }
        )
    if kind in {"sqlite", "postgres"}:
        measurements["recovery"] = {"durations_ms": [10, 20, 30]}
    value = {
        "schema": schemas[kind],
        "status": "completed" if kind in {"counter", "heist"} else "pass",
        "release_evidence": False,
        "performance_class": "reference_non_release",
        "identity": {
            "product": "worldstream",
            "profile": "linux-reference",
            "version": version,
            "artifact_sha256": archive_sha256,
            "packaged_acceptance_sha256": packaged_acceptance_sha256,
        },
        "reference_environment": reference_environment(),
        "reference_workload": reference_workload(kind),
        "measurements": measurements,
    }
    if kind == "soak":
        value["one_hour_window_completed"] = True
        value["database"] = {"status": "measured", "growth_bytes": 4096}
    return value


def write_json(path: Path, value: object) -> Path:
    path.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    return path


def build_archive(
    path: Path,
    daemon_bytes: bytes,
    control_bytes: bytes,
    manifest_toml: bytes,
    manifest_json: bytes,
    version: str,
    *,
    add_symlink: bool = False,
) -> None:
    archive_root = f"worldstream-{version}-linux-x86_64"
    manifest_json_digest = hashlib.sha256(manifest_json).hexdigest()
    metadata = (
        json.dumps(
            {
                "manifest": {
                    "file": "manifest/compatibility.json",
                    "schema": "worldstream/storage-compatibility-manifest/v1",
                    "sha256": manifest_json_digest,
                },
                "target": "linux-x86_64",
                "version": version,
            },
            sort_keys=True,
        )
        + "\n"
    ).encode()
    files = {
        "bin/worldstreamd": daemon_bytes,
        "bin/worldstreamctl": control_bytes,
        "manifest/compatibility.toml": manifest_toml,
        "manifest/compatibility.json": manifest_json,
        "metadata/release.json": metadata,
    }
    files["checksums.sha256"] = "".join(
        f"{hashlib.sha256(content).hexdigest()}  {name}\n"
        for name, content in sorted(files.items())
    ).encode()
    with tarfile.open(path, "w:gz") as archive:
        for name, content in sorted(files.items()):
            info = tarfile.TarInfo(f"{archive_root}/{name}")
            info.mode = 0o755 if name.startswith("bin/") else 0o644
            info.size = len(content)
            archive.addfile(info, io.BytesIO(content))
        if add_symlink:
            link = tarfile.TarInfo(f"{archive_root}/unsafe-link")
            link.type = tarfile.SYMTYPE
            link.linkname = "bin/worldstreamd"
            archive.addfile(link)


def story_report(story: str, backend: str) -> dict:
    storage = {
        "status": "verified",
        "profile": "sqlite-bundled" if backend == "sqlite" else "postgres-primary",
        "exact_identity": (
            "sqlite/3.53.4; journal_mode=WAL; synchronous=FULL"
            if backend == "sqlite"
            else "postgresql/17.11; server_version_num=170011"
        ),
    }
    measurements = {
        "latency_ms": [1, 2, 3, 4, 5],
        "load": {
            "connections": 8,
            "active_rooms": 4,
            "actions_per_second": 40,
        },
        "fan_out": {"observation_fan_out": 10},
        "observed_transitions": 12,
        "story_duration_ms": 250,
        "recovery": {"durations_ms": [10, 20, 30]},
    }
    disclosure = {
        "payload_sizes_bytes": [128, 512],
        "pack_id": (
            "worldstream.counter" if story == "counter" else "worldstream.agent-heist"
        ),
        "participants_per_room": 2 if story == "counter" else 3,
        "fan_out": 10,
        "snapshot_cadence_transitions": 4,
    }
    if story == "counter":
        return {
            "status": "completed",
            "storage": storage,
            "secrets": "not_emitted",
            "criteria": {"exact_story": {"status": "passed"}},
            "measurements": measurements,
            "reference_workload": disclosure,
        }
    parity = {
        "verified": True,
        "fields": [
            "pack",
            "core",
            "activity",
            "aggregate_authoritative",
            "transition",
            "room_id",
            "room_seq",
        ],
        "expected": {"room_seq": 6, "transition": "sha256:fixture"},
        "replayed": {"room_seq": 6, "transition": "sha256:fixture"},
    }
    return {
        "status": "completed",
        "storage": storage,
        "secrets": "not_emitted",
        "six_phase_order": True,
        "private_contexts_emitted": False,
        "phase_path": ["setup", "complete"],
        "final": {
            "phase": "complete",
            "outcome": {
                "outcome": "success",
                "score": 5,
                "reason": "scored_selected_plan",
                "missing_roles": ["broker"],
                "vote_counts": {"plan": 2},
                "checks": {
                    "route": True,
                    "entry_window": True,
                    "required_tool": True,
                    "extraction": True,
                    "resource_contributed": True,
                },
            },
            "commitment_count": 2,
            "broker_seat_present": True,
            "replay_verified": True,
            "replay_hash_parity": parity,
        },
        "lost_claim_reply": {
            "status": "completed",
            "transport_loss_observed": True,
            "durable_duplicate_result_matches": True,
        },
        "timers": [
            {
                "phase": f"phase-{index}",
                "timer_id": f"timer-{index}",
                "generation": 1,
                "duplicate": False,
                "transition_id_present": True,
                "room_seq": index,
            }
            for index in range(5)
        ],
        "privacy": {"private_contexts_emitted": False},
        "duplicate_action": {"status": "passed"},
        "activation": {},
        "measurements": measurements,
        "reference_workload": disclosure,
    }


def packaged_acceptance_report(module, package_binding: dict) -> dict:
    cells: dict[str, dict] = {"counter": {}, "heist": {}}
    top_performance = {
        "classification": "measured_non_sla",
        "percentile_definition": "nearest-rank",
        "cells_ms": {},
        "cells": {},
    }
    workloads: dict[str, dict] = {"counter": {}, "heist": {}}
    comparisons = {}
    for story, normalizer in (
        ("counter", module.PACKAGED_ACCEPTANCE._counter_normalized),
        ("heist", module.PACKAGED_ACCEPTANCE._heist_normalized),
    ):
        normalized = {}
        for index, backend in enumerate(
            ("sqlite", "postgres_direct", "transaction_pooler"), start=1
        ):
            report = story_report(story, backend)
            elapsed = 100 + index
            performance = {
                "latency_ms": module.PACKAGED_ACCEPTANCE._latency_triplet(report),
                "memory": {
                    "status": "measured",
                    "peak_rss_bytes": 1_000_000 + index,
                    "process_tree_rss_kib": {
                        "sampling_interval_ms": 250,
                        "sample_count": 2,
                        "samples": [900, 1000],
                        "p50": 900,
                        "p95": 1000,
                        "p99": 1000,
                        "peak": 1000,
                    },
                },
                "story_duration_ms": elapsed,
                "load": report["measurements"]["load"],
                "fan_out": report["measurements"]["fan_out"],
                "recovery": report["measurements"]["recovery"],
            }
            if backend != "sqlite":
                performance["database_growth"] = {
                    "status": "measured",
                    "database": f"{story}_{backend}",
                    "before": {},
                    "after": {},
                    "growth_bytes": 4096,
                    "cluster_wal_growth_bytes": 8192,
                    "transition_delta": 12,
                    "active_rooms_delta": 1,
                    "observation_frames_delta": 20,
                    "activation_receipts_delta": 1,
                }
            cell = {
                "exit_code": 0,
                "elapsed_ms": elapsed,
                "process_tree_rss_kib": performance["memory"]["process_tree_rss_kib"],
                "report": report,
                "performance": performance,
            }
            cells[story][backend] = cell
            name = f"{story}.{backend}"
            top_performance["cells_ms"][name] = elapsed
            top_performance["cells"][name] = performance
            workloads[story][backend] = {
                "load": performance["load"],
                "fan_out": performance["fan_out"],
                "observed_transitions": report["measurements"]["observed_transitions"],
                "story_duration_ms": report["measurements"]["story_duration_ms"],
                "disclosure": report["reference_workload"],
            }
            normalized[backend] = normalizer(report)
        comparisons[story] = {"status": "pass", "normalized": normalized}
    return {
        "schema": module.PACKAGED_ACCEPTANCE.SCHEMA,
        "canonical_encoding": "utf8-sorted-key-compact-json-lf",
        "status": "pass",
        "release_evidence": True,
        "secrets_emitted": False,
        "provider": {
            "postgres_image": module.PACKAGED_ACCEPTANCE.POSTGRES_IMAGE,
            "pgbouncer_image": module.PACKAGED_ACCEPTANCE.PGBOUNCER_IMAGE,
            "pool_mode": "transaction",
        },
        "reference_environment": {
            "schema": "worldstream/reference-environment/v1",
            "platform": {
                "system": "Linux",
                "release": "6.8.0-fixture",
                "machine": "x86_64",
            },
            "hardware": {"logical_cpus": 8, "physical_memory_bytes": 17_179_869_184},
            "filesystem": {"repository": "ext4", "temporary": "ext4"},
            "engines": {
                "postgresql": {
                    "version": "17.11",
                    "settings": {
                        "server_version_num": "170011",
                        "synchronous_commit": "on",
                        "transaction_isolation": "read committed",
                    },
                    "connection_mode": "direct_and_transaction_pooler",
                },
                "pgbouncer": {
                    "image": module.PACKAGED_ACCEPTANCE.PGBOUNCER_IMAGE,
                    "pool_mode": "transaction",
                },
                "sqlite": {
                    "version": "3.53.4",
                    "settings": {"journal_mode": "wal", "synchronous": "full"},
                    "connection_mode": "embedded",
                },
            },
        },
        "reference_workloads": workloads,
        "cells": cells,
        "comparison": comparisons,
        "package_binding": package_binding,
        "performance": top_performance,
        "cleanup": "pass",
    }


@pytest.fixture()
def fixture(tmp_path: Path):
    module = load_module()
    manifest_toml = tmp_path / "compatibility.toml"
    manifest_json = tmp_path / "compatibility.json"
    shutil.copy2(ROOT / "compatibility.toml", manifest_toml)
    shutil.copy2(ROOT / "compatibility.json", manifest_json)
    version = json.loads(manifest_json.read_text())["release_candidate"]
    daemon = tmp_path / "worldstreamd"
    daemon.write_bytes(b"packaged-worldstreamd-fixture\n")
    daemon.chmod(0o755)
    control_bytes = b"packaged-worldstreamctl-fixture\n"
    archive = tmp_path / f"worldstream-{version}-linux-x86_64.tar.gz"
    build_archive(
        archive,
        daemon.read_bytes(),
        control_bytes,
        manifest_toml.read_bytes(),
        manifest_json.read_bytes(),
        version,
    )
    archive_sha256 = module.sha256(archive)
    package_report = write_json(
        tmp_path / "package-report.json",
        {
            "schema": "worldstream/package-report/v1",
            "artifact": archive.name,
            "kind": "archive",
            "path": archive.name,
            "sha256": archive_sha256,
            "size_bytes": archive.stat().st_size,
            "inventory": {
                "archive_verified": True,
                "manifest_source": "compatibility.toml",
                "manifest_mirror": "compatibility.json",
                "release_evidence": False,
            },
            "identity": {
                "target": "linux-x86_64",
                "version": version,
                "manifest_sha256": hashlib.sha256(
                    manifest_json.read_bytes()
                ).hexdigest(),
                "manifest_json_sha256": hashlib.sha256(
                    manifest_json.read_bytes()
                ).hexdigest(),
                "manifest_toml_sha256": hashlib.sha256(
                    manifest_toml.read_bytes()
                ).hexdigest(),
            },
        },
    )
    distribution, _package_value, _package_raw, package_binding = (
        module.verify_packaged_distribution(
            archive,
            package_report,
            daemon,
            manifest_toml,
            manifest_json,
            module.ADAPTER.COLLECTOR.load_manifest(manifest_toml, manifest_json),
        )
    )
    packaged_acceptance = write_json(
        tmp_path / "packaged-acceptance.json",
        packaged_acceptance_report(module, package_binding),
    )
    packaged_acceptance_sha256 = module.sha256(packaged_acceptance)
    projector = load_projector()
    failure = projector.FAILURE_PRODUCER
    soak_distribution_fields = {
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
    coverage = {
        name: {"status": "covered", "matched_tests": [f"tests::{name}"]}
        for name in failure.REQUIRED_COVERAGE_GROUPS
    }
    soak = write_json(
        tmp_path / "raw-soak.json",
        {
            "schema": "worldstream/soak-evidence/v1",
            "status": "pass",
            "release_evidence": False,
            "release_candidate_input": True,
            "identity": {
                "product": "worldstream",
                "profile": "linux-reference",
                "version": version,
                "artifact_sha256": archive_sha256,
                "packaged_acceptance_sha256": packaged_acceptance_sha256,
            },
            "evidence_class": failure.RELEASE_SOAK_EVIDENCE_CLASS,
            "evidence_scope": {
                "fixture_only": False,
                "process_level": True,
                "database_workload_bound": True,
            },
            "mode": "one_hour",
            "platform": {"system": "Linux", "machine": "x86_64"},
            "distribution": {
                key: distribution[key] for key in soak_distribution_fields
            },
            "configuration": {
                "max_total_seconds": 3600,
                "one_hour_target_seconds": 3600,
                "max_output_bytes": 262144,
                "max_database_growth_bytes": 268435456,
                "max_temp_growth_bytes": failure.MAX_TEMP_GROWTH_BYTES,
                "max_log_growth_bytes": failure.MAX_RUNTIME_LOG_GROWTH_BYTES,
                "max_artifact_growth_bytes": (
                    failure.MAX_AUXILIARY_ARTIFACT_GROWTH_BYTES
                ),
                "max_internal_queue_depth": failure.TELEMETRY_QUEUE_HARD_LIMIT,
                "database_workload_binding": failure.RELEASE_DATABASE_WORKLOAD_BINDING,
            },
            "named_gate_evidence": {
                "manifest_evidence_id": failure.EVIDENCE_ID,
                "release_gate": True,
                "release_evidence": False,
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
                for name in failure.REQUIRED_FIXTURE_HOOKS
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
                "command_duration_ms": {"p50": 2.0, "p95": 3.0, "p99": 3.0},
                "memory": {
                    "status": "measured",
                    "scope": "worldstreamd_process_tree",
                    "peak_rss_bytes_per_run": [10_000_000],
                    "observed_peak_delta_bytes": 0,
                },
            },
            "database": {
                "status": "measured",
                "workload_binding": failure.RELEASE_DATABASE_WORKLOAD_BINDING,
                "initial_bytes": 4096,
                "final_bytes": 8192,
                "growth_bytes": 4096,
                "wal_growth_bytes": 2048,
                "growth_bound_status": "pass",
                "wal_growth_bound_status": "pass",
            },
            "workload": {
                "kind": "worldstreamd_transition_workload",
                "database_binding": failure.RELEASE_DATABASE_WORKLOAD_BINDING,
                "accepted_transition_count": 1000,
            },
            "temp_and_artifacts": {
                "temporary": {
                    "initial_bytes": 1024,
                    "final_bytes": 2048,
                    "growth_bytes": 1024,
                    "configured_hard_limit_bytes": failure.MAX_TEMP_GROWTH_BYTES,
                    "bound_status": "pass",
                },
                "logs": {
                    "initial_bytes": 0,
                    "final_bytes": 12000,
                    "growth_bytes": 12000,
                    "configured_hard_limit_bytes": (
                        failure.MAX_RUNTIME_LOG_GROWTH_BYTES
                    ),
                    "bound_status": "pass",
                    "file_count": 2,
                    "sha256": ["sha256:" + "a" * 64, "sha256:" + "b" * 64],
                },
                "artifacts": {
                    "initial_bytes": 1024,
                    "final_bytes": 14048,
                    "growth_bytes": 13024,
                    "configured_hard_limit_bytes": (
                        failure.MAX_AUXILIARY_ARTIFACT_GROWTH_BYTES
                    ),
                    "bound_status": "pass",
                },
                "initial_bytes": 1024,
                "final_bytes": 14048,
                "growth_bytes": 13024,
                "growth_bound_status": "pass",
                "daemon_log_count": 2,
                "daemon_log_sha256": [
                    "sha256:" + "a" * 64,
                    "sha256:" + "b" * 64,
                ],
            },
            "internal_queues": {
                "status": "measured",
                "configured_hard_limits_enforced": True,
                "queues": [
                    {
                        "status": "measured",
                        "name": "telemetry_exporter",
                        "measurement_source": "public_prometheus_metrics",
                        "depth_metric": "worldstream_telemetry_queued",
                        "capacity_metric": "worldstream_telemetry_queue_capacity",
                        "configured_hard_limit": failure.TELEMETRY_QUEUE_HARD_LIMIT,
                        "maximum_observed_depth": 4,
                        "sample_count": 10,
                        "dropped_total_initial": 0,
                        "dropped_total_final": 0,
                        "dropped_total_delta": 0,
                        "bound_status": "pass",
                    }
                ],
            },
            "privacy": {
                "status": "pass",
                "secret_scan": {
                    "schema": failure.SECRET_SCAN_MATRIX_SCHEMA,
                    "status": "pass",
                    "secrets_emitted": False,
                    "encodings_scanned": ["base64", "base64url", "hex", "raw"],
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
                    "sentinels": [
                        {
                            "name": name,
                            "sha256": "sha256:" + digest * 64,
                            "size_bytes": 32,
                        }
                        for name, digest in (
                            ("authority-secret", "c"),
                            ("operator-capability", "d"),
                            ("member-capability-01", "e"),
                        )
                    ],
                },
            },
            "environment_observation": {
                "platform": {
                    "system": "Linux",
                    "distribution": "Ubuntu",
                    "distribution_version": "24.04",
                    "machine": "x86_64",
                },
                "hardware": {
                    "cpu_model": "fixture cpu",
                    "logical_cpu_count": 4,
                    "memory_bytes": 8_589_934_592,
                },
                "filesystem": {
                    "type": "ext4",
                    "mount_options": ["rw", "relatime"],
                    "storage_class": "local_ssd_or_nvme",
                },
                "engines": {
                    "sqlite": {
                        "status": "observed_runtime_verified",
                        "profile": "sqlite-bundled",
                        "exact_identity": "sqlite/3.53.4; source_id=fixture",
                    },
                    "postgresql": {"status": "not_observed_by_sqlite_process_soak"},
                },
            },
            "reference_workload": reference_workload("soak"),
            "measurements": {
                "latency_ms": {
                    "definition": "nearest-rank",
                    "sample_count": 3,
                    "p50_ms": 2.0,
                    "p95_ms": 3.0,
                    "p99_ms": 3.0,
                },
                "load": {"active_rooms": 1, "actions_per_second": 2.0},
                "fan_out": {"observation_fan_out": 3},
                "memory": {"status": "measured", "peak_rss_bytes": 10_000_000},
                "database_growth": {
                    "status": "measured",
                    "growth_bytes": 4096,
                    "wal_growth_bytes": 2048,
                },
                "recovery": {"durations_ms": [10.0, 20.0, 30.0]},
            },
            "elapsed_seconds": 3600.01,
            "one_hour_window_completed": True,
        },
    )
    normalized_dir = tmp_path / "normalized"
    aggregate = tmp_path / "reference.json"
    inputs = projector.project(
        argparse.Namespace(
            packaged_acceptance_report=packaged_acceptance,
            soak_report=soak,
            package_archive=archive,
            package_report=package_report,
            daemon_bin=daemon,
            output_dir=normalized_dir,
            aggregate_report=aggregate,
            manifest_toml=manifest_toml,
            manifest_json=manifest_json,
        )
    )
    return {
        "module": module,
        "manifest_toml": manifest_toml,
        "manifest_json": manifest_json,
        "daemon": daemon,
        "control_bytes": control_bytes,
        "version": version,
        "archive": archive,
        "package_report": package_report,
        "packaged_acceptance": packaged_acceptance,
        "soak": soak,
        "inputs": inputs,
        "aggregate": aggregate,
        "root": tmp_path,
    }


def produce(value: dict) -> tuple[dict, dict]:
    module = value["module"]
    output = value["root"] / "producer.json"
    artifact = value["root"] / "reference-bundle.json"
    module.produce(
        output,
        artifact,
        value["aggregate"],
        value["inputs"],
        value["archive"],
        value["package_report"],
        value["daemon"],
        value["packaged_acceptance"],
        value["soak"],
        value["manifest_toml"],
        value["manifest_json"],
    )
    return json.loads(output.read_text()), json.loads(artifact.read_text())


def refresh_aggregate(value: dict) -> None:
    write_json(value["aggregate"], value["module"].recompute_report(value["inputs"]))


def test_complete_measurements_emit_closed_byte_bound_non_sla_publication(fixture):
    producer, bundle = produce(fixture)

    assert producer["evidence_id"] == fixture["module"].EVIDENCE_ID
    assert producer["status"] == "passed"
    assert producer["release_evidence"] is True
    assert set(producer["outcomes"]) == {
        "packaged_workload_identity",
        "sqlite_measurements",
        "postgresql_measurements",
        "non_sla_publication",
    }
    assert (
        "no threshold or SLA comparison"
        in producer["outcomes"]["non_sla_publication"]["observations"][0]["value"]
    )
    assert bundle["performance_class"] == "reference_non_release"
    assert bundle["distribution"]["archive_sha256"] == fixture["module"].sha256(
        fixture["archive"]
    )
    assert {item["kind"] for item in bundle["input_reports"]} == set(fixture["inputs"])
    assert all(item["content_base64"] for item in bundle["input_reports"])
    assert bundle["packaged_acceptance_report"]["sha256"] == fixture["module"].sha256(
        fixture["packaged_acceptance"]
    )
    assert bundle["raw_soak_report"]["sha256"] == fixture["module"].sha256(
        fixture["soak"]
    )
    assert (
        bundle["projection"]["raw_soak_sha256"] == bundle["raw_soak_report"]["sha256"]
    )
    assert set(bundle["projection"]["input_sha256"]) == set(fixture["inputs"])


def test_hand_authored_aggregate_cannot_invent_input_hashes(fixture):
    report = json.loads(fixture["aggregate"].read_text())
    report["inputs"][0]["sha256"] = sha_bytes(b"invented")
    write_json(fixture["aggregate"], report)

    with pytest.raises(fixture["module"].ReferenceError, match="exact independent"):
        produce(fixture)


def test_fresh_archive_report_and_extracted_binary_are_verified(fixture):
    fixture["daemon"].write_bytes(b"different-extracted-binary\n")
    with pytest.raises(fixture["module"].ReferenceError, match="daemon bytes differ"):
        produce(fixture)

    fixture["daemon"].write_bytes(b"packaged-worldstreamd-fixture\n")
    package = json.loads(fixture["package_report"].read_text())
    package["sha256"] = sha_bytes(b"stale archive")
    write_json(fixture["package_report"], package)
    with pytest.raises(fixture["module"].ReferenceError, match="package report"):
        produce(fixture)


def test_package_report_uses_portable_artifact_basename(fixture):
    package = json.loads(fixture["package_report"].read_text())
    package["path"] = f"/ephemeral-runner/{fixture['archive'].name}"
    write_json(fixture["package_report"], package)

    with pytest.raises(fixture["module"].ReferenceError, match="package report"):
        produce(fixture)


def test_archived_manifests_are_verified_as_exact_root_pair(fixture):
    build_archive(
        fixture["archive"],
        fixture["daemon"].read_bytes(),
        fixture["control_bytes"],
        fixture["manifest_toml"].read_bytes() + b"\n# stale\n",
        fixture["manifest_json"].read_bytes(),
        fixture["version"],
    )
    package = json.loads(fixture["package_report"].read_text())
    package["sha256"] = fixture["module"].sha256(fixture["archive"])
    package["size_bytes"] = fixture["archive"].stat().st_size
    write_json(fixture["package_report"], package)

    with pytest.raises(fixture["module"].ReferenceError, match="independently verify"):
        produce(fixture)


@pytest.mark.parametrize("field", ["manifest_json_sha256", "manifest_toml_sha256"])
def test_package_report_requires_both_explicit_manifest_digests(fixture, field):
    package = json.loads(fixture["package_report"].read_text())
    package["identity"][field] = "0" * 64
    write_json(fixture["package_report"], package)

    with pytest.raises(
        fixture["module"].ReferenceError, match="manifest pair identity mismatch"
    ):
        produce(fixture)


def test_archive_links_and_special_members_are_rejected(fixture):
    build_archive(
        fixture["archive"],
        fixture["daemon"].read_bytes(),
        fixture["control_bytes"],
        fixture["manifest_toml"].read_bytes(),
        fixture["manifest_json"].read_bytes(),
        fixture["version"],
        add_symlink=True,
    )
    package = json.loads(fixture["package_report"].read_text())
    package["sha256"] = fixture["module"].sha256(fixture["archive"])
    package["size_bytes"] = fixture["archive"].stat().st_size
    write_json(fixture["package_report"], package)

    with pytest.raises(fixture["module"].ReferenceError, match="independently verify"):
        produce(fixture)


def test_all_measurements_must_bind_the_verified_package_archive(fixture):
    postgres = json.loads(fixture["inputs"]["postgres"].read_text())
    postgres["identity"]["artifact_sha256"] = sha_bytes(b"different package")
    write_json(fixture["inputs"]["postgres"], postgres)
    refresh_aggregate(fixture)

    with pytest.raises(fixture["module"].ReferenceError, match="exact projection"):
        produce(fixture)


def test_packaged_acceptance_must_exactly_bind_independently_verified_package(fixture):
    acceptance = json.loads(fixture["packaged_acceptance"].read_text())
    acceptance["package_binding"]["archive_sha256"] = sha_bytes(b"other archive")
    write_json(fixture["packaged_acceptance"], acceptance)

    with pytest.raises(
        fixture["module"].ReferenceError,
        match="exactly bind the independently verified package",
    ):
        produce(fixture)


def test_packaged_acceptance_revalidates_cell_contract_and_normalized_comparator(
    fixture,
):
    acceptance = json.loads(fixture["packaged_acceptance"].read_text())
    acceptance["cells"]["counter"]["sqlite"]["report"]["criteria"]["exact_story"][
        "status"
    ] = "failed"
    write_json(fixture["packaged_acceptance"], acceptance)

    with pytest.raises(fixture["module"].ReferenceError, match="failed validation"):
        produce(fixture)

    acceptance = packaged_acceptance_report(
        fixture["module"],
        json.loads(fixture["packaged_acceptance"].read_text())["package_binding"],
    )
    acceptance["comparison"]["heist"]["normalized"]["sqlite"]["phase_path"] = ["forged"]
    write_json(fixture["packaged_acceptance"], acceptance)
    with pytest.raises(
        fixture["module"].ReferenceError, match="normalized comparator drifted"
    ):
        produce(fixture)


def test_all_normalized_sources_bind_exact_raw_packaged_acceptance_bytes(fixture):
    invented = sha_bytes(b"invented packaged acceptance")
    for path in fixture["inputs"].values():
        report = json.loads(path.read_text())
        report["identity"]["packaged_acceptance_sha256"] = invented
        write_json(path, report)
    refresh_aggregate(fixture)

    with pytest.raises(fixture["module"].ReferenceError, match="exact projection"):
        produce(fixture)


def test_reference_environment_disclosure_is_required_and_common(fixture):
    postgres = json.loads(fixture["inputs"]["postgres"].read_text())
    postgres["reference_environment"]["filesystem"]["type"] = "tmpfs"
    write_json(fixture["inputs"]["postgres"], postgres)
    refresh_aggregate(fixture)

    with pytest.raises(fixture["module"].ReferenceError, match="exact projection"):
        produce(fixture)


@pytest.mark.parametrize(
    ("group", "field", "message"),
    [
        ("load", "active_rooms", "positive active_rooms"),
        ("load", "actions_per_second", "positive active_rooms"),
        ("fan_out", "observation_fan_out", "positive observation_fan_out"),
    ],
)
def test_named_load_and_fan_out_values_must_be_positive(fixture, group, field, message):
    counter = json.loads(fixture["inputs"]["counter"].read_text())
    counter["measurements"][group][field] = 0
    write_json(fixture["inputs"]["counter"], counter)
    refresh_aggregate(fixture)

    with pytest.raises(fixture["module"].ReferenceError, match="exact projection"):
        produce(fixture)


@pytest.mark.parametrize(
    ("kind", "measurement", "field"),
    [
        ("counter", "latency_ms", "p50_ms"),
        ("counter", "load", "active_rooms"),
        ("counter", "fan_out", "observation_fan_out"),
        ("heist", "latency_ms", "p50_ms"),
        ("heist", "load", "active_rooms"),
        ("heist", "fan_out", "observation_fan_out"),
        ("sqlite", "latency_ms", "p50_ms"),
        ("sqlite", "memory", "peak_rss_bytes"),
        ("sqlite", "database_growth", "growth_bytes"),
        ("sqlite", "recovery", "durations_ms"),
        ("postgres", "latency_ms", "p50_ms"),
        ("postgres", "memory", "peak_rss_bytes"),
        ("postgres", "database_growth", "growth_bytes"),
        ("postgres", "recovery", "durations_ms"),
        ("soak", "latency_ms", "p50_ms"),
        ("soak", "memory", "peak_rss_bytes"),
        ("soak", "database_growth", "growth_bytes"),
    ],
)
def test_every_published_metric_must_equal_its_raw_projection(
    fixture, kind, measurement, field
):
    path = fixture["inputs"][kind]
    value = json.loads(path.read_text())
    target = value["measurements"][measurement]
    if field == "durations_ms":
        target[field][0] += 0.125
    else:
        target[field] += 0.125
    write_json(path, value)
    refresh_aggregate(fixture)

    with pytest.raises(fixture["module"].ReferenceError, match="exact projection"):
        produce(fixture)


def test_stale_normalized_inputs_reject_changed_raw_acceptance_or_soak(fixture):
    module = fixture["module"]
    acceptance = json.loads(fixture["packaged_acceptance"].read_text())
    report = acceptance["cells"]["counter"]["sqlite"]["report"]
    report["measurements"]["latency_ms"] = [1, 2, 3, 4, 6]
    latency = module.PACKAGED_ACCEPTANCE._latency_triplet(report)
    acceptance["cells"]["counter"]["sqlite"]["performance"]["latency_ms"] = latency
    acceptance["performance"]["cells"]["counter.sqlite"]["latency_ms"] = latency
    write_json(fixture["packaged_acceptance"], acceptance)
    soak = json.loads(fixture["soak"].read_text())
    soak["identity"]["packaged_acceptance_sha256"] = module.sha256(
        fixture["packaged_acceptance"]
    )
    write_json(fixture["soak"], soak)
    with pytest.raises(module.ReferenceError, match="exact projection"):
        produce(fixture)

    # Restore the acceptance bytes, then alter an independently valid raw soak metric.
    fixture["packaged_acceptance"].write_text(
        json.dumps(
            packaged_acceptance_report(
                module,
                json.loads(fixture["packaged_acceptance"].read_text())[
                    "package_binding"
                ],
            ),
            sort_keys=True,
        )
        + "\n"
    )
    soak = json.loads(fixture["soak"].read_text())
    soak["identity"]["packaged_acceptance_sha256"] = module.sha256(
        fixture["packaged_acceptance"]
    )
    soak["measurements"]["latency_ms"]["p50_ms"] += 0.125
    write_json(fixture["soak"], soak)
    with pytest.raises(module.ReferenceError, match="exact projection"):
        produce(fixture)

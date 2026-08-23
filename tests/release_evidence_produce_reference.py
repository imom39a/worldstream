"""Fail-closed tests for measured reference-performance publication evidence."""

from __future__ import annotations

import argparse
import base64
import hashlib
import importlib.util
import io
import json
import shutil
import sys
import tarfile
from pathlib import Path

import pytest
from reference_target_support import (
    storage_bindings,
    target_report,
    valid_kill_report,
)

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


def fixture_build_identity(manifest_json: bytes) -> bytes:
    return (
        json.dumps(
            {
                "schema": "worldstream/release-build-identity/v2",
                "source": {
                    "repository": "https://github.com/imom39a/worldstream",
                    "revision": "1" * 40,
                },
                "target": {"profile": "linux-x86_64"},
                "observed_build_environment": {"fixture": "reference-target"},
                "manifest_sha256": "sha256:"
                + hashlib.sha256(manifest_json).hexdigest(),
            },
            sort_keys=True,
        )
        + "\n"
    ).encode()


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
        "sdk/python/pyproject.toml": (ROOT / "sdk/python/pyproject.toml").read_bytes(),
        "sdk/python/uv.lock": (ROOT / "sdk/python/uv.lock").read_bytes(),
        "sdk/python/src/worldstream_sdk/__init__.py": (
            ROOT / "sdk/python/src/worldstream_sdk/__init__.py"
        ).read_bytes(),
        "sdk/python/src/worldstream_sdk/client.py": (
            ROOT / "sdk/python/src/worldstream_sdk/client.py"
        ).read_bytes(),
        "sdk/python/src/worldstream_sdk/compatibility_identity.json": (
            ROOT / "sdk/python/src/worldstream_sdk/compatibility_identity.json"
        ).read_bytes(),
        "manifest/compatibility.toml": manifest_toml,
        "manifest/compatibility.json": manifest_json,
        "metadata/build.json": fixture_build_identity(manifest_json),
        "metadata/release.json": metadata,
        "ui/index.html": (ROOT / "web/console/dist/index.html").read_bytes(),
        "ui/compatibility-identity.json": (
            ROOT / "web/console/dist/compatibility-identity.json"
        ).read_bytes(),
    }
    for name in (
        "seed_browser_room.py",
        "run_browser_story.py",
        "run_absent_broker_live.py",
        "browser_trace_init.js",
    ):
        files[f"examples/heist/wave10_live/{name}"] = (
            ROOT / "examples/heist/wave10_live" / name
        ).read_bytes()
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


def packaged_browser_story(module, package_binding: dict) -> dict:
    packaged = module.PACKAGED_ACCEPTANCE
    digest = "sha256:" + "a" * 64
    assets = package_binding["runtime_assets"]
    binary = package_binding["binaries"]["worldstreamd"]
    return {
        "schema": packaged.BROWSER_STORY_SCHEMA,
        "canonical_encoding": "utf8-sorted-key-compact-json-lf",
        "status": "pass",
        "release_evidence": True,
        "source_mode": "package-extracted",
        "elapsed_ms": 1000,
        "browser": packaged.PINNED_BROWSER_IDENTITY,
        "tools": {
            "adapter": {
                "name": "worldstream-cdp-browser",
                "protocol": "Chrome DevTools Protocol",
                "sha256": "sha256:"
                + packaged._sha256_file(ROOT / "scripts/cdp-browser.py"),
                "size_bytes": (ROOT / "scripts/cdp-browser.py").stat().st_size,
            },
            "python": {"implementation": "cpython", "version": "3.14.7"},
        },
        "runtime": {
            "worldstreamd": {
                "sha256": binary["sha256"],
                "size_bytes": binary["size_bytes"],
                "origin": "package:bin/worldstreamd",
            },
            "ui": {**assets["ui"], "origin": "package:ui"},
            "sdk": {
                **assets["sdk_python_source"],
                "origin": "package:sdk/python/src",
            },
            "heist_reference_clients": {
                **assets["heist_reference_clients"],
                "origin": "package:examples/heist",
            },
        },
        "story": {
            "phase_path": [
                "Briefing",
                "Negotiation",
                "Commitment",
                "Resolution",
                "Result",
                "Complete",
            ],
            "public_projection": {
                "broker_present": True,
                "commitment_count": 2,
                "aggregate_outcome_present": True,
            },
            "final_replay": {"verified": True, "hash_parity": {"verified": True}},
        },
        "dom_evidence": {
            key: digest
            for key in (
                "stale_rejection",
                "precomplete_reveal",
                "public_final",
                "participant_final",
                "operator_final",
                "replay_final",
                "briefing",
                "negotiation",
                "commitment",
                "result",
                "complete",
                "resync",
                "browser_diagnostics",
            )
        },
        "typed_actions": {
            key: digest
            for key in (
                "inspect_clue",
                "publish_clue",
                "propose_plan",
                "commit_move",
                "acknowledge_result",
            )
        },
        "checks": {
            key: True
            for key in (
                "browser_identity_verified",
                "catch_up_or_reset_installed",
                "embedded_ui_loaded",
                "final_reveal_dom_visible",
                "new_session_resynchronized",
                "package_bound_reference_clients",
                "package_bound_runtime",
                "precomplete_reveal_locked",
                "privacy_negative_dom_and_browser_channels",
                "replay_hashes_verified",
                "six_phase_story_complete",
                "stale_head_rejected",
                "typed_actions_accepted_in_dom",
            )
        },
        "privacy": {
            "status": "pass",
            "private_canary_absent": True,
            "credentials_absent": True,
            "private_claim_absent_from_retained_evidence": True,
        },
    }


def packaged_acceptance_report(module, package_binding: dict) -> dict:
    privacy_channels = []
    privacy_classes = []
    for index, channel_class in enumerate(
        module.PACKAGED_ACCEPTANCE.REQUIRED_PRIVACY_CHANNEL_CLASSES, start=1
    ):
        channel_name = f"child-{index:04d}-{channel_class.replace('.', '-')}"
        privacy_channels.append(
            {
                "channel": channel_name,
                "sha256": "sha256:" + format(index, "x")[-1] * 64,
                "size_bytes": 11 + index,
            }
        )
        privacy_classes.append(
            {
                "class": channel_class,
                "channel_count": 1,
                "channels": [channel_name],
            }
        )
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
            "platform": {
                "system": "Linux",
                "distribution": "Ubuntu",
                "distribution_version": "24.04",
                "machine": "x86_64",
            },
            "hardware": {
                "cpu_model": "acceptance fixture cpu",
                "logical_cpu_count": 4,
                "memory_bytes": 8_589_934_592,
            },
            "filesystem": {
                "type": "ext4",
                "mount_options": ["rw", "relatime"],
                "storage_class": "local_ssd_or_nvme",
            },
            "storage_bindings": storage_bindings(
                {
                    "type": "ext4",
                    "mount_options": ["rw", "relatime"],
                    "storage_class": "local_ssd_or_nvme",
                }
            ),
            "engines": {
                "postgresql": {
                    "version": "17.11",
                    "settings": {
                        "server_version_num": "170011",
                        "synchronous_commit": "on",
                        "transaction_isolation": "read committed",
                        **module.PACKAGED_ACCEPTANCE.POSTGRESQL_CONTRACT_SETTINGS,
                    },
                    "connection_mode": "direct_and_transaction_pooler",
                },
                "sqlite": {
                    "version": "3.53.4",
                    "settings": module.PACKAGED_ACCEPTANCE.SQLITE_REFERENCE_SETTINGS,
                    "connection_mode": "embedded",
                },
            },
        },
        "reference_workloads": workloads,
        "cells": cells,
        "comparison": comparisons,
        "package_binding": package_binding,
        "browser_story": packaged_browser_story(module, package_binding),
        "performance": top_performance,
        "privacy": {
            "status": "pass",
            "channel_contract": module.PACKAGED_ACCEPTANCE.PRIVACY_CHANNEL_CONTRACT,
            "secret_scan": {
                "schema": module.PACKAGED_ACCEPTANCE.SECRET_SCAN.MATRIX_SCHEMA,
                "status": "pass",
                "secrets_emitted": False,
                "encodings_scanned": ["base64", "base64url", "hex", "raw"],
                "channels": privacy_channels,
                "channel_class_inventory": {
                    "schema": (module.PACKAGED_ACCEPTANCE.PRIVACY_CHANNEL_CLASS_SCHEMA),
                    "status": "complete",
                    "required": list(
                        module.PACKAGED_ACCEPTANCE.REQUIRED_PRIVACY_CHANNEL_CLASSES
                    ),
                    "classes": privacy_classes,
                },
                "sentinels": [
                    {
                        "name": name,
                        "sha256": "sha256:" + digest * 64,
                        "size_bytes": 32,
                    }
                    for name, digest in (
                        ("authority-secret-01", "3"),
                        ("operator-capability-01", "4"),
                        ("postgres-admin-dsn-01", "5"),
                        ("postgres-admin-password-01", "6"),
                        ("postgres-runtime-dsn-01", "7"),
                        ("postgres-runtime-password-01", "8"),
                    )
                ],
            },
        },
        "cleanup": "pass",
    }


@pytest.fixture()
def fixture(tmp_path: Path, monkeypatch):
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
                "source_revision": "1" * 40,
                "build_identity_sha256": "sha256:"
                + hashlib.sha256(
                    fixture_build_identity(manifest_json.read_bytes())
                ).hexdigest(),
                "observed_build_environment": {"fixture": "reference-target"},
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
        "observation_scope": failure.QUEUE_OBSERVATION_SCOPE,
        "all_internal_queues_observed": True,
        "observed_queues": [observed_queue(spec) for spec in failure.QUEUE_SPECS],
    }
    queue_boundary_tests = {
        "schema": failure.QUEUE_BOUNDARY_TEST_SCHEMA,
        "status": "pass",
        "execution_scope": "same_production_queue_paths",
        "total_duration_ms": 5000.0,
        "max_total_seconds": failure.QUEUE_BOUNDARY_MAX_TOTAL_SECONDS,
        "max_output_bytes_per_test": failure.MAX_RUNTIME_LOG_GROWTH_BYTES,
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
                failure.QUEUE_BOUNDARY_TESTS, start=1
            )
        ],
    }
    disk_full = {
        "schema": failure.DISK_FULL_SCHEMA,
        "status": "pass",
        "release_evidence": False,
        "scenario": failure.DISK_FULL_SCENARIO,
        "evidence_class": failure.DISK_FULL_EVIDENCE_CLASS,
        "platform": {
            "system": "Linux",
            "machine": "x86_64",
            "filesystem": "ext4",
        },
        "container": {
            "image": failure.DISK_FULL_CONTAINER_IMAGE,
            "platform": "linux/amd64",
            "privileged": True,
            "network": "none",
            "root_filesystem_read_only": True,
            "daemon_mount_read_only": True,
            "observed_elapsed_ms": 2100.5,
            "captured_output_bytes": 2048,
        },
        "bounds": {
            "filesystem_image_bytes": failure.DISK_FULL_FILESYSTEM_IMAGE_BYTES,
            "attempted_write_bytes": failure.DISK_FULL_ATTEMPTED_WRITE_BYTES,
            "max_daemon_seconds": failure.DISK_FULL_MAX_DAEMON_SECONDS,
            "max_container_seconds": failure.DISK_FULL_MAX_CONTAINER_SECONDS,
            "max_log_bytes": failure.DISK_FULL_MAX_LOG_BYTES,
            "max_capture_bytes": failure.DISK_FULL_MAX_CAPTURE_BYTES,
        },
        "filesystem": {
            "type": "ext4",
            "mount_source_class": "loop_device",
            "image_size_bytes": failure.DISK_FULL_FILESYSTEM_IMAGE_BYTES,
            "block_size_bytes": failure.DISK_FULL_BLOCK_SIZE_BYTES,
            "available_kib_after_fill": 0,
            "database_file_type": "regular",
            "database_file_mode": "0600",
            "fill_bytes_written": 63 * 1024 * 1024,
        },
        "fault": {
            "errno_number": 28,
            "errno_name": "ENOSPC",
            "attempted_write_bytes": failure.DISK_FULL_ATTEMPTED_WRITE_BYTES,
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
    }

    def storage_sampling(
        source: str, initial: int, peak: int, hard_limit: int
    ) -> dict[str, object]:
        return {
            "measurement_source": source,
            "measurement_window": "transition_workload_through_recovery",
            "sampling_interval_ms": failure.STORAGE_SAMPLE_INTERVAL_MS,
            "maximum_gap_ms": failure.RESOURCE_SAMPLE_MAX_GAP_MS,
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
        "schema": failure.RESOURCE_SAMPLING_SCHEMA,
        "status": "measured",
        "sampling_complete": True,
        "peak_semantics": "maximum_observed_at_bounded_sampling_interval",
        "workload_elapsed_ms": 3_600_010,
        "resources": {
            "process_tree_rss_bytes": {
                "measurement_source": "linux_proc_process_tree_vmrss",
                "measurement_window": "transition_workload",
                "sampling_interval_ms": failure.RSS_SAMPLE_INTERVAL_MS,
                "maximum_gap_ms": failure.RESOURCE_SAMPLE_MAX_GAP_MS,
                "observed_max_gap_ms": 50,
                "coverage_duration_ms": 3_600_010,
                "sample_count": 72001,
                "observed_peak_bytes": 10_000_000,
                "configured_hard_limit_bytes": failure.MAX_PEAK_RSS_BYTES,
                "bound_status": "pass",
            },
            "database_bytes": storage_sampling(
                "sqlite_main_plus_wal_regular_file_sizes",
                4096,
                12288,
                failure.MAX_DATABASE_GROWTH_BYTES,
            ),
            "wal_bytes": storage_sampling(
                "sqlite_wal_regular_file_size",
                0,
                4096,
                failure.MAX_WAL_GROWTH_BYTES,
            ),
            "temporary_bytes": storage_sampling(
                "owned_private_working_tree_regular_file_sizes",
                1024,
                4096,
                failure.MAX_TEMP_GROWTH_BYTES,
            ),
            "log_bytes": storage_sampling(
                "owned_private_working_tree_daemon_log_sizes",
                0,
                16000,
                failure.MAX_RUNTIME_LOG_GROWTH_BYTES,
            ),
            "artifact_bytes": storage_sampling(
                "owned_private_working_tree_regular_file_sizes",
                1024,
                20000,
                failure.MAX_AUXILIARY_ARTIFACT_GROWTH_BYTES,
            ),
        },
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
                "disk_full_fault_injection": True,
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
                "max_wal_growth_bytes": failure.MAX_WAL_GROWTH_BYTES,
                "max_temp_growth_bytes": failure.MAX_TEMP_GROWTH_BYTES,
                "max_log_growth_bytes": failure.MAX_RUNTIME_LOG_GROWTH_BYTES,
                "max_artifact_growth_bytes": (
                    failure.MAX_AUXILIARY_ARTIFACT_GROWTH_BYTES
                ),
                "internal_queue_hard_limits": {
                    spec["name"]: {
                        "capacity_scope": spec["capacity_scope"],
                        "hard_limit": spec["hard_limit"],
                        "activity_unit": spec["activity_unit"],
                    }
                    for spec in failure.QUEUE_SPECS
                },
                "queue_boundary_test_max_total_seconds": (
                    failure.QUEUE_BOUNDARY_MAX_TOTAL_SECONDS
                ),
                "queue_boundary_test_max_output_bytes": (
                    failure.MAX_RUNTIME_LOG_GROWTH_BYTES
                ),
                "max_peak_rss_bytes": failure.MAX_PEAK_RSS_BYTES,
                "database_workload_binding": failure.RELEASE_DATABASE_WORKLOAD_BINDING,
            },
            "named_gate_evidence": {
                "manifest_evidence_id": failure.EVIDENCE_ID,
                "release_gate": True,
                "release_evidence": False,
            },
            "disk_full": disk_full,
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
                    "peak_rss_bytes": 10_000_000,
                    "peak_rss_bytes_per_run": [10_000_000],
                    "observed_peak_delta_bytes": 0,
                    "sampling_interval_ms": failure.RSS_SAMPLE_INTERVAL_MS,
                    "maximum_gap_ms": failure.RESOURCE_SAMPLE_MAX_GAP_MS,
                    "observed_max_gap_ms": 50,
                    "coverage_duration_ms": 3_600_010,
                    "sample_count": 72001,
                },
            },
            "resource_sampling": resource_sampling,
            "database": {
                "status": "measured",
                "workload_binding": failure.RELEASE_DATABASE_WORKLOAD_BINDING,
                "initial_bytes": 4096,
                "final_bytes": 8192,
                "growth_bytes": 4096,
                "wal_initial_bytes": 0,
                "wal_final_bytes": 2048,
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
            "queue_observation": queue_observation,
            "queue_boundary_tests": queue_boundary_tests,
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
                "resource_sampling": resource_sampling,
                "queue_observation": queue_observation,
                "queue_boundary_tests": queue_boundary_tests,
            },
            "elapsed_seconds": 3600.01,
            "one_hour_window_completed": True,
        },
    )
    kill_value = valid_kill_report(tmp_path, monkeypatch, distribution=distribution)
    kill = write_json(tmp_path / "raw-kill.json", kill_value)
    snapshot_fixture_report = write_json(
        tmp_path / "snapshot-fixture.json",
        {
            "schema": "worldstream/reference-snapshot-tail-fixture/v1",
            "status": "complete",
        },
    )
    target_value = target_report(
        module,
        manifest=json.loads(manifest_json.read_text(encoding="utf-8")),
        distribution=distribution,
        acceptance=json.loads(packaged_acceptance.read_text(encoding="utf-8")),
        acceptance_raw=packaged_acceptance.read_bytes(),
        soak=json.loads(soak.read_text(encoding="utf-8")),
        soak_raw=soak.read_bytes(),
        kill=kill_value,
        kill_raw=kill.read_bytes(),
        package_archive=archive,
        package_report=package_report,
        daemon=daemon,
        snapshot_fixture_report=snapshot_fixture_report,
        manifest_toml=manifest_toml,
        manifest_json=manifest_json,
    )
    target = write_json(tmp_path / "raw-target.json", target_value)
    normalized_dir = tmp_path / "normalized"
    aggregate = tmp_path / "reference.json"
    inputs = projector.project(
        argparse.Namespace(
            packaged_acceptance_report=packaged_acceptance,
            soak_report=soak,
            kill_point_report=kill,
            target_report=target,
            package_archive=archive,
            package_report=package_report,
            daemon_bin=daemon,
            snapshot_fixture_bin=daemon,
            snapshot_fixture_report=snapshot_fixture_report,
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
        "kill": kill,
        "target": target,
        "snapshot_fixture_report": snapshot_fixture_report,
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
        value["daemon"],
        value["snapshot_fixture_report"],
        value["packaged_acceptance"],
        value["soak"],
        value["kill"],
        value["target"],
        value["manifest_toml"],
        value["manifest_json"],
    )
    return json.loads(output.read_text()), json.loads(artifact.read_text())


def refresh_aggregate(value: dict) -> None:
    write_json(value["aggregate"], value["module"].recompute_report(value["inputs"]))


def test_cli_parser_keeps_normalized_and_raw_soak_reports_distinct():
    module = load_module()
    arguments = module.parser().parse_args(
        [
            "--output",
            "producer.json",
            "--artifact-output",
            "artifact.json",
            "--report",
            "aggregate.json",
            "--counter-report",
            "counter.json",
            "--heist-report",
            "heist.json",
            "--sqlite-report",
            "sqlite.json",
            "--postgres-report",
            "postgres.json",
            "--soak-report",
            "normalized-soak.json",
            "--target-report",
            "normalized-target.json",
            "--package-archive",
            "package.tar.gz",
            "--package-report",
            "package.json",
            "--daemon-bin",
            "worldstreamd",
            "--snapshot-fixture-bin",
            "snapshot-fixture",
            "--snapshot-fixture-report",
            "snapshot-fixture.json",
            "--packaged-acceptance-report",
            "acceptance.json",
            "--raw-soak-report",
            "raw-soak.json",
            "--kill-point-report",
            "raw-kill.json",
            "--raw-target-report",
            "raw-target.json",
        ]
    )

    assert arguments.soak_report == Path("normalized-soak.json")
    assert arguments.raw_soak_report == Path("raw-soak.json")
    assert arguments.target_report == Path("normalized-target.json")
    assert arguments.raw_target_report == Path("raw-target.json")
    assert arguments.snapshot_fixture_report == Path("snapshot-fixture.json")


def test_complete_measurements_emit_closed_byte_bound_non_sla_publication(fixture):
    producer, bundle = produce(fixture)

    assert producer["evidence_id"] == fixture["module"].EVIDENCE_ID
    assert producer["status"] == "passed"
    assert producer["release_evidence"] is True
    assert producer["artifacts"]["linux-release-profile"] == {
        "sha256": fixture["module"].sha256(fixture["archive"]),
        "size_bytes": fixture["archive"].stat().st_size,
    }
    assert set(producer["outcomes"]) == {
        "packaged_workload_identity",
        "sqlite_measurements",
        "postgresql_measurements",
        "non_sla_publication",
    }
    assert (
        "target comparisons are measured non-SLA;target misses remain publishable"
        in producer["outcomes"]["non_sla_publication"]["observations"][0]["value"]
    )
    assert bundle["performance_class"] == "reference_non_release"
    assert bundle["distribution"]["archive_sha256"] == fixture["module"].sha256(
        fixture["archive"]
    )
    assert bundle["distribution"]["source_revision"] == "1" * 40
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
    assert bundle["cell_runner_materials"]["source_revision"] == "1" * 40
    programs = bundle["cell_runner_materials"]["programs"]
    assert set(programs) == {"counter", "heist"}
    for story, path in fixture["module"].CELL_RUNNER_PATHS.items():
        row = programs[story]
        assert row["path"] == path.relative_to(ROOT).as_posix()
        assert row["sha256"] == fixture["module"].sha256(path)
        assert row["size_bytes"] == path.stat().st_size
    fixture_bundle = bundle["snapshot_fixture_report"]
    fixture_raw = fixture["snapshot_fixture_report"].read_bytes()
    assert fixture_bundle["sha256"] == sha_bytes(fixture_raw)
    assert fixture_bundle["size_bytes"] == len(fixture_raw)
    assert (
        base64.b64decode(fixture_bundle["content_base64"], validate=True) == fixture_raw
    )
    assert fixture_bundle["subjects"] == {
        "package_archive_sha256": bundle["distribution"]["archive_sha256"],
        "daemon_binary_sha256": bundle["distribution"]["binary_sha256"],
        "source_revision": bundle["distribution"]["source_revision"],
        "generator_source_sha256": json.loads(fixture["target"].read_text())[
            "bindings"
        ]["snapshot_fixture_source"]["sha256"],
        "generator_binary_sha256": json.loads(fixture["target"].read_text())[
            "bindings"
        ]["snapshot_fixture_binary"]["sha256"],
    }


def test_fixture_report_bytes_cannot_change_after_target_projection(fixture):
    fixture["snapshot_fixture_report"].write_bytes(
        b'{"schema":"worldstream/reference-snapshot-tail-fixture/v1","status":"changed"}\n'
    )

    with pytest.raises(
        fixture["module"].ReferenceError,
        match="raw byte bindings differ|fixture report bytes differ",
    ):
        produce(fixture)


def test_fixture_report_is_rejected_before_an_oversized_allocation(fixture):
    fixture["snapshot_fixture_report"].write_bytes(
        b"x" * (fixture["module"].MAX_FIXTURE_REPORT_BYTES + 1)
    )

    with pytest.raises(
        fixture["module"].ReferenceError,
        match="empty or too large",
    ):
        produce(fixture)


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


def test_package_report_revision_must_match_archived_build_identity(fixture):
    package = json.loads(fixture["package_report"].read_text(encoding="utf-8"))
    package["identity"]["source_revision"] = "2" * 40
    write_json(fixture["package_report"], package)

    with pytest.raises(
        fixture["module"].ReferenceError,
        match="source revision or build identity is not exactly bound",
    ):
        produce(fixture)


@pytest.mark.parametrize(
    ("path", "value"),
    [
        (("browser_story", "checks", "stale_head_rejected"), False),
        (("browser_story", "privacy", "credentials_absent"), False),
        (
            ("browser_story", "browser", "sha256"),
            "sha256:" + "0" * 64,
        ),
    ],
)
def test_packaged_browser_story_drift_is_not_promoted(fixture, path, value):
    acceptance = json.loads(fixture["packaged_acceptance"].read_text())
    target = acceptance
    for key in path[:-1]:
        target = target[key]
    target[path[-1]] = value
    write_json(fixture["packaged_acceptance"], acceptance)
    module = fixture["module"]
    manifest = module.ADAPTER.COLLECTOR.load_manifest(
        fixture["manifest_toml"], fixture["manifest_json"]
    )
    distribution, _package, _raw, binding = module.verify_packaged_distribution(
        fixture["archive"],
        fixture["package_report"],
        fixture["daemon"],
        fixture["manifest_toml"],
        fixture["manifest_json"],
        manifest,
    )
    with pytest.raises(fixture["module"].ReferenceError, match="browser story"):
        module.verify_packaged_acceptance(
            fixture["packaged_acceptance"], distribution, binding
        )

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


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        (lambda report: report.pop("privacy"), "complete releasable six-cell"),
        (
            lambda report: report["privacy"]["secret_scan"]["channels"][0].update(
                {"sha256": "sha256:not-a-digest"}
            ),
            "exact SHA-256 reference",
        ),
        (
            lambda report: report["privacy"]["secret_scan"]["sentinels"].pop(),
            "sentinel classes are incomplete",
        ),
        (
            lambda report: report["privacy"]["secret_scan"].pop(
                "channel_class_inventory"
            ),
            "privacy scan matrix is invalid",
        ),
        (
            lambda report: report["privacy"]["secret_scan"][
                "channel_class_inventory"
            ].update({"status": "partial"}),
            "channel-class contract is invalid",
        ),
        (
            lambda report: report["privacy"]["secret_scan"]["channel_class_inventory"][
                "required"
            ].pop(),
            "channel-class contract is invalid",
        ),
        (
            lambda report: report["privacy"]["secret_scan"]["channel_class_inventory"][
                "classes"
            ][1].update(
                {
                    "channel_count": 1,
                    "channels": report["privacy"]["secret_scan"][
                        "channel_class_inventory"
                    ]["classes"][0]["channels"],
                }
            ),
            "channel-class mapping is invalid",
        ),
        (
            lambda report: report["privacy"]["secret_scan"]["channels"].append(
                {
                    "channel": "child-9999-unclassified",
                    "sha256": "sha256:" + "f" * 64,
                    "size_bytes": 1,
                }
            ),
            "do not exactly cover scanned channels",
        ),
    ],
)
def test_packaged_acceptance_requires_closed_privacy_scan_evidence(
    fixture, mutation, message
):
    acceptance = json.loads(fixture["packaged_acceptance"].read_text())
    mutation(acceptance)
    write_json(fixture["packaged_acceptance"], acceptance)

    with pytest.raises(fixture["module"].ReferenceError, match=message):
        produce(fixture)


def test_reference_projector_rejects_incomplete_channel_class_inventory(fixture):
    acceptance = json.loads(fixture["packaged_acceptance"].read_text())
    acceptance["privacy"]["secret_scan"]["channel_class_inventory"]["classes"].pop()
    write_json(fixture["packaged_acceptance"], acceptance)
    projector = load_projector()

    with pytest.raises(
        projector.REFERENCE_PRODUCER.ReferenceError,
        match="channel-class contract is invalid",
    ):
        projector.project(
            argparse.Namespace(
                packaged_acceptance_report=fixture["packaged_acceptance"],
                soak_report=fixture["soak"],
                package_archive=fixture["archive"],
                package_report=fixture["package_report"],
                daemon_bin=fixture["daemon"],
                snapshot_fixture_bin=fixture["daemon"],
                output_dir=fixture["root"] / "rejected-normalized",
                aggregate_report=fixture["root"] / "rejected-reference.json",
                manifest_toml=fixture["manifest_toml"],
                manifest_json=fixture["manifest_json"],
            )
        )


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
    with pytest.raises(module.ReferenceError, match="independently project"):
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
    with pytest.raises(module.ReferenceError, match="independently project"):
        produce(fixture)


def validate_raw_target(fixture: dict, report: dict) -> dict:
    module = fixture["module"]
    manifest = module.ADAPTER.COLLECTOR.load_manifest(
        fixture["manifest_toml"], fixture["manifest_json"]
    )
    distribution, _package, _raw, _binding = module.verify_packaged_distribution(
        fixture["archive"],
        fixture["package_report"],
        fixture["daemon"],
        fixture["manifest_toml"],
        fixture["manifest_json"],
        manifest,
    )
    original = json.loads(fixture["target"].read_text(encoding="utf-8"))
    return module.validate_reference_target(
        report,
        manifest=manifest,
        distribution=distribution,
        packaged_acceptance_sha256=module.sha256(fixture["packaged_acceptance"]),
        expected_bindings=original["bindings"],
        expected_external_sources=original["bound_external_sources"],
        expected_packaged_sdk=module.packaged_sdk_identity(fixture["archive"]),
        kill_cell_count=36,
        soak_elapsed_seconds=3600.01,
    )


@pytest.mark.parametrize(
    ("case", "message"),
    [
        ("incomplete", "completion/classification"),
        ("classification", "completion/classification"),
        ("rate_arithmetic", "transition-rate measurement"),
        ("rate_bucket_coherence", "transition-rate measurement"),
        ("external_source", "external-source facts"),
        ("sdk_client", "SDK bytes"),
        ("history_substitution", "exact recent non-empty tail"),
        ("history_source_revision", "source identity is incomplete"),
        ("target_host_attribution", "disclosure is invalid"),
        ("rss_samples", "retained measurements"),
        ("scaled", "scaled, simulated"),
    ],
)
def test_frozen_target_proof_boundaries_fail_closed(fixture, case, message):
    report = json.loads(fixture["target"].read_text(encoding="utf-8"))
    if case == "incomplete":
        report["dimensions"][0]["completed"] = False
    elif case == "classification":
        report["dimensions"][0]["classification"] = "bound_hard_gate"
    elif case == "rate_arithmetic":
        report["dimensions"][3]["observed"]["aggregate_accepted_per_second"] = 99.0
        report["measurements"]["transition_rate"] = report["dimensions"][3]["observed"]
    elif case == "rate_bucket_coherence":
        rate = report["dimensions"][3]["observed"]
        rate["accepted_transition_count"] = 179_999
        rate["action_attempt_count"] = 179_999
        rate["aggregate_accepted_per_second"] = round(179_999 / 1_800, 3)
        report["dimensions"][4]["observed"]["sample_count"] = 179_999
        report["measurements"]["transition_rate"] = rate
        report["measurements"]["commit_to_ack_latency"] = report["dimensions"][4][
            "observed"
        ]
    elif case == "external_source":
        report["bound_external_sources"]["one_hour_soak"]["source_attribution"] = (
            "target_workload_source"
        )
    elif case == "sdk_client":
        report["identity"]["packaged_sdk"]["client_module_sha256"] = (
            "sha256:" + "0" * 64
        )
    elif case == "history_substitution":
        report["dimensions"][6]["observed"]["snapshot"]["newest_snapshot_room_seq"] = (
            99_999
        )
    elif case == "history_source_revision":
        report["dimensions"][6]["observed"]["setup"]["source_revision"] = "2" * 40
    elif case == "target_host_attribution":
        report["reference_environment"]["engines"]["postgresql"]["status"] = (
            "not_observed_by_sqlite_process_soak"
        )
    elif case == "rss_samples":
        report["measurements"]["resource_observation"][
            "process_tree_rss_sample_count"
        ] = 0
    elif case == "scaled":
        report["execution"]["scaled"] = True
    else:  # pragma: no cover
        raise AssertionError(case)
    with pytest.raises(fixture["module"].ReferenceError, match=message):
        validate_raw_target(fixture, report)


def test_honest_frozen_performance_miss_remains_publishable(fixture):
    report = json.loads(fixture["target"].read_text(encoding="utf-8"))
    rate = report["dimensions"][3]["observed"]
    rate.update(
        {
            "action_attempt_count": 178_200,
            "dispatch_queue_full_count": 46_288,
            "minimum_accepted_per_full_second": 99,
            "accepted_transition_count": 178_200,
            "aggregate_accepted_per_second": 99.0,
        }
    )
    report["dimensions"][3].update({"target_met": False, "outcome": "missed"})
    report["dimensions"][4]["observed"]["sample_count"] = 178_200
    report["measurements"]["transition_rate"] = rate
    report["measurements"]["commit_to_ack_latency"] = report["dimensions"][4][
        "observed"
    ]

    validated = validate_raw_target(fixture, report)

    assert "sustained_accepted_transition_rate" in validated["target_miss_ids"]
    assert "snapshot_tail_recovery" not in validated["target_miss_ids"]


def test_snapshot_recovery_duration_miss_is_computed_and_publishable(fixture):
    report = json.loads(fixture["target"].read_text(encoding="utf-8"))
    history = report["dimensions"][6]
    history["observed"]["recovery"]["recovery_ms"] = 5_001.0
    history.update({"target_met": False, "outcome": "missed"})
    report["limitations"] = [
        {
            "code": "snapshot_tail_recovery_target_missed",
            "dimension": "snapshot_tail_recovery",
            "publishable_non_sla_target_miss": True,
        }
    ]

    validated = validate_raw_target(fixture, report)

    assert "snapshot_tail_recovery" in validated["target_miss_ids"]

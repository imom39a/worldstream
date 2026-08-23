"""Fail-closed projection tests for raw package acceptance and process soak bytes."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import shutil
import sys
from pathlib import Path

import pytest
from reference_target_support import storage_bindings, target_report

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/reference-evidence-project.py"


def load_module():
    spec = importlib.util.spec_from_file_location(
        "worldstream_reference_evidence_project_test", SCRIPT
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_json(path: Path, value: object) -> Path:
    path.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    return path


def disclosure(story: str) -> dict:
    return {
        "payload_sizes_bytes": [128, 512],
        "pack_id": (
            "worldstream.counter" if story == "counter" else "worldstream.agent-heist"
        ),
        "participants_per_room": 2 if story == "counter" else 3,
        "fan_out": 3,
        "snapshot_cadence_transitions": 4,
    }


def performance(*, postgres: bool) -> dict:
    value = {
        "latency_ms": {
            "status": "measured",
            "definition": "nearest-rank",
            "sample_count": 3,
            "samples_ms": [1.0, 2.0, 3.0],
            "p50_ms": 2.0,
            "p95_ms": 3.0,
            "p99_ms": 3.0,
        },
        "load": {"active_rooms": 1, "actions_per_second": 2.0},
        "fan_out": {"observation_fan_out": 3},
        "memory": {"status": "measured", "peak_rss_bytes": 1024},
        "recovery": {"durations_ms": [10.0, 11.0]},
    }
    if postgres:
        value["database_growth"] = {
            "status": "measured",
            "growth_bytes": 4096,
            "cluster_wal_growth_bytes": 2048,
        }
    return value


@pytest.fixture()
def fixture(tmp_path: Path, monkeypatch):
    module = load_module()
    manifest_toml = tmp_path / "compatibility.toml"
    manifest_json = tmp_path / "compatibility.json"
    shutil.copy2(ROOT / "compatibility.toml", manifest_toml)
    shutil.copy2(ROOT / "compatibility.json", manifest_json)
    manifest = json.loads(manifest_json.read_text(encoding="utf-8"))
    version = manifest["release_candidate"]
    package_archive = tmp_path / "worldstream-package.tar.gz"
    package_archive.write_bytes(b"package archive")
    package_report = write_json(tmp_path / "package-report.json", {"fixture": True})
    daemon = tmp_path / "worldstreamd"
    daemon.write_bytes(b"packaged daemon")
    daemon.chmod(0o700)
    distribution = {
        "binary_sha256": "sha256:" + "1" * 64,
        "binary_size_bytes": daemon.stat().st_size,
        "packaged_artifact_bound": True,
        "reference_class": "fresh_packaged_linux_x86_64",
        "archive_sha256": "sha256:" + "2" * 64,
        "archive_size_bytes": package_archive.stat().st_size,
        "package_report_sha256": "sha256:" + "3" * 64,
        "manifest_sha256": "sha256:"
        + hashlib.sha256(manifest_json.read_bytes()).hexdigest(),
        "manifest_json_sha256": "sha256:"
        + hashlib.sha256(manifest_json.read_bytes()).hexdigest(),
        "manifest_toml_sha256": "sha256:"
        + hashlib.sha256(manifest_toml.read_bytes()).hexdigest(),
        "target": "linux-x86_64",
        "version": version,
        "source_revision": "1" * 40,
        "control_binary_sha256": "sha256:" + "4" * 64,
        "control_binary_size_bytes": 100,
    }
    acceptance = {
        "schema": "worldstream/packaged-backend-parity/v1",
        "reference_environment": {
            "platform": {
                "system": "Linux",
                "distribution": "Ubuntu",
                "distribution_version": "24.04",
                "machine": "x86_64",
            },
            "hardware": {
                "cpu_model": "acceptance fixture CPU",
                "logical_cpu_count": 4,
                "memory_bytes": 8 * 1024 * 1024 * 1024,
            },
            "filesystem": {
                "type": "ext4",
                "mount_options": ["rw", "noatime"],
                "storage_class": "local_ssd_or_nvme",
            },
            "storage_bindings": storage_bindings(
                {
                    "type": "ext4",
                    "mount_options": ["rw", "noatime"],
                    "storage_class": "local_ssd_or_nvme",
                }
            ),
            "engines": {
                **json.loads(json.dumps(module.EXPECTED_NORMALIZED_ENGINES)),
            },
        },
        "reference_workloads": {
            story: {
                backend: {"disclosure": disclosure(story)}
                for backend in ("sqlite", "postgres_direct", "transaction_pooler")
            }
            for story in ("counter", "heist")
        },
        "cells": {
            story: {
                backend: {
                    "exit_code": 0,
                    "performance": performance(postgres=backend != "sqlite"),
                }
                for backend in ("sqlite", "postgres_direct", "transaction_pooler")
            }
            for story in ("counter", "heist")
        },
    }
    acceptance_path = write_json(tmp_path / "acceptance.json", acceptance)
    acceptance_raw = acceptance_path.read_bytes()
    acceptance_sha256 = "sha256:" + hashlib.sha256(acceptance_raw).hexdigest()
    runtime_distribution = {
        key: value
        for key, value in distribution.items()
        if key not in {"control_binary_sha256", "control_binary_size_bytes"}
    }
    soak = {
        "schema": "worldstream/soak-evidence/v1",
        "identity": {
            "packaged_acceptance_sha256": acceptance_sha256,
        },
        "environment_observation": {
            "platform": {
                "system": "Linux",
                "distribution": "Ubuntu",
                "distribution_version": "24.04",
                "machine": "x86_64",
            },
            "hardware": {
                "cpu_model": "certified fixture CPU",
                "logical_cpu_count": 4,
                "memory_bytes": 8 * 1024 * 1024 * 1024,
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
        "reference_workload": {
            **disclosure("counter"),
            "participants_per_room": 3,
        },
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
            "memory": {"status": "measured", "peak_rss_bytes": 2048},
            "database_growth": {
                "status": "measured",
                "growth_bytes": 8192,
                "wal_growth_bytes": 4096,
            },
            "recovery": {"durations_ms": [12.0]},
        },
        "distribution": runtime_distribution,
    }
    soak_path = write_json(tmp_path / "soak.json", soak)
    packaged_sdk = {
        "source": "verified_native_archive",
        "pyproject_sha256": "sha256:" + "a" * 64,
        "pyproject_size_bytes": 101,
        "lock_sha256": "sha256:" + "b" * 64,
        "lock_size_bytes": 102,
        "module_init_sha256": "sha256:" + "c" * 64,
        "module_init_size_bytes": 103,
        "client_module_sha256": "sha256:" + "e" * 64,
        "client_module_size_bytes": 104,
        "compatibility_identity_sha256": "sha256:" + "d" * 64,
        "compatibility_identity_size_bytes": 105,
        "runtime_module_under_packaged_sdk_root": True,
    }
    monkeypatch.setattr(
        module.REFERENCE_PRODUCER,
        "packaged_sdk_identity",
        lambda _archive: packaged_sdk,
    )
    kill = {
        "schema": "worldstream/kill-point-evidence/v1",
        "platform": {"system": "Linux", "machine": "x86_64"},
    }
    kill_path = write_json(tmp_path / "kill.json", kill)
    snapshot_fixture_report = write_json(
        tmp_path / "snapshot-fixture.json",
        {
            "schema": "worldstream/reference-snapshot-tail-fixture/v1",
            "status": "complete",
        },
    )
    target = target_report(
        module.REFERENCE_PRODUCER,
        manifest=manifest,
        distribution=distribution,
        acceptance=acceptance,
        acceptance_raw=acceptance_raw,
        soak=soak,
        soak_raw=soak_path.read_bytes(),
        kill=kill,
        kill_raw=kill_path.read_bytes(),
        package_archive=package_archive,
        package_report=package_report,
        daemon=daemon,
        snapshot_fixture_report=snapshot_fixture_report,
        manifest_toml=manifest_toml,
        manifest_json=manifest_json,
    )
    target_path = write_json(tmp_path / "target.json", target)
    monkeypatch.setattr(
        module.REFERENCE_PRODUCER,
        "verify_packaged_distribution",
        lambda *_args: (distribution, {}, b"{}\n", {"status": "pass"}),
    )
    monkeypatch.setattr(
        module.REFERENCE_PRODUCER,
        "verify_packaged_acceptance",
        lambda *_args: (acceptance, acceptance_raw, acceptance_sha256),
    )
    monkeypatch.setattr(
        module.FAILURE_PRODUCER,
        "validate_soak",
        lambda *_args, **_kwargs: {
            "distribution": runtime_distribution,
            "elapsed_seconds": 3600.01,
        },
    )
    monkeypatch.setattr(
        module.FAILURE_PRODUCER,
        "validate_kill",
        lambda *_args, **_kwargs: {
            "distribution": runtime_distribution,
            "cell_count": 36,
        },
    )
    args = argparse.Namespace(
        packaged_acceptance_report=acceptance_path,
        soak_report=soak_path,
        kill_point_report=kill_path,
        target_report=target_path,
        package_archive=package_archive,
        package_report=package_report,
        daemon_bin=daemon,
        snapshot_fixture_bin=daemon,
        snapshot_fixture_report=snapshot_fixture_report,
        output_dir=tmp_path / "normalized",
        aggregate_report=tmp_path / "aggregate.json",
        manifest_toml=manifest_toml,
        manifest_json=manifest_json,
    )
    return module, args, acceptance, soak, acceptance_sha256, runtime_distribution


def test_projects_only_verified_raw_measurements_into_six_closed_inputs(fixture):
    module, args, _acceptance, _soak, acceptance_sha256, _distribution = fixture

    paths = module.project(args)

    assert set(paths) == set(module.KINDS)
    values = {
        kind: json.loads(path.read_text(encoding="utf-8"))
        for kind, path in paths.items()
    }
    assert (
        values["counter"]["reference_environment"]
        == values["postgres"]["reference_environment"]
    )
    assert (
        values["sqlite"]["reference_environment"]
        == values["soak"]["reference_environment"]
    )
    assert (
        values["counter"]["reference_environment"]
        != values["sqlite"]["reference_environment"]
    )
    assert values["sqlite"]["reference_environment"]["engines"]["postgresql"] == {
        "status": "not_observed_by_sqlite_process_soak"
    }
    assert all(
        value["identity"]["packaged_acceptance_sha256"] == acceptance_sha256
        for value in values.values()
    )
    assert (
        values["counter"]["projection_source"] == "packaged-acceptance:counter.sqlite"
    )
    assert values["heist"]["projection_source"] == "packaged-acceptance:heist.sqlite"
    assert (
        values["postgres"]["projection_source"]
        == "packaged-acceptance:counter.postgres_direct"
    )
    assert values["postgres"]["measurements"]["database_growth"] == {
        "status": "measured",
        "growth_bytes": 4096,
        "wal_growth_bytes": 2048,
    }
    aggregate = json.loads(args.aggregate_report.read_text(encoding="utf-8"))
    assert aggregate["status"] == "pass"
    assert aggregate["gaps"] == []


@pytest.mark.parametrize(
    ("section", "field", "value", "message"),
    [
        ("platform", "distribution_version", "22.04", "not certified"),
        ("hardware", "logical_cpu_count", 8, "not certified"),
        ("hardware", "memory_bytes", 16 * 1024**3, "not certified"),
        ("filesystem", "type", "xfs", "not certified"),
        ("filesystem", "storage_class", "network", "not certified"),
    ],
)
def test_projection_rejects_noncertified_host(fixture, section, field, value, message):
    module, args, _acceptance, soak, _acceptance_sha, _distribution = fixture
    soak["environment_observation"][section][field] = value
    write_json(args.soak_report, soak)

    with pytest.raises(module.ProjectionError, match=message):
        module.project(args)


def test_projection_rejects_missing_raw_workload_disclosure(fixture):
    module, args, acceptance, _soak, _acceptance_sha, _distribution = fixture
    del acceptance["reference_workloads"]["heist"]["sqlite"]["disclosure"]

    with pytest.raises(module.ProjectionError, match="raw workload disclosure"):
        module.project(args)


def test_projection_rejects_soak_package_identity_drift(fixture, monkeypatch):
    module, args, _acceptance, _soak, _acceptance_sha, distribution = fixture
    drifted = {**distribution, "archive_sha256": "sha256:" + "9" * 64}
    monkeypatch.setattr(
        module.FAILURE_PRODUCER,
        "validate_soak",
        lambda *_args, **_kwargs: {"distribution": drifted},
    )

    with pytest.raises(module.ProjectionError, match="independently verified package"):
        module.project(args)


def test_projection_rejects_unobserved_or_drifted_engine_settings(fixture):
    module, args, acceptance, _soak, _acceptance_sha, _distribution = fixture
    acceptance["reference_environment"]["engines"]["postgresql"]["settings"][
        "synchronous_commit"
    ] = "off"

    with pytest.raises(module.ProjectionError, match="engine observations"):
        module.project(args)

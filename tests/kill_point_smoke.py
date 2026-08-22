"""Contract and tamper tests for Linux process evidence harnesses."""

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import os
import pathlib
import sqlite3
import subprocess
import sys
import tarfile

import pytest

ROOT = pathlib.Path(__file__).resolve().parents[1]
KILL_SCRIPT = ROOT / "scripts" / "kill-point-matrix.py"
SOAK_SCRIPT = ROOT / "scripts" / "daemon-transition-soak.py"


def load(path: pathlib.Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="module")
def kill():
    return load(KILL_SCRIPT, "kill_point_matrix_tests")


@pytest.fixture(scope="module")
def soak():
    return load(SOAK_SCRIPT, "daemon_transition_soak_tests")


def write_archive_fixture(
    archive: pathlib.Path,
    binary_bytes: bytes,
    manifest_toml: bytes,
    manifest_json: bytes,
    extra_kind: str | None = None,
) -> None:
    with tarfile.open(archive, "w:gz") as output:
        for name, content, mode in (
            ("worldstream-0.1.0/bin/worldstreamd", binary_bytes, 0o755),
            (
                "worldstream-0.1.0/manifest/compatibility.toml",
                manifest_toml,
                0o644,
            ),
            (
                "worldstream-0.1.0/manifest/compatibility.json",
                manifest_json,
                0o644,
            ),
        ):
            info = tarfile.TarInfo(name)
            info.size = len(content)
            info.mode = mode
            output.addfile(info, io.BytesIO(content))
        if extra_kind == "link":
            info = tarfile.TarInfo("worldstream-0.1.0/unsafe-link")
            info.type = tarfile.SYMTYPE
            info.linkname = "../../outside"
            output.addfile(info)
        elif extra_kind == "duplicate":
            info = tarfile.TarInfo("worldstream-0.1.0/manifest/compatibility.json")
            info.size = len(manifest_json)
            info.mode = 0o644
            output.addfile(info, io.BytesIO(manifest_json))


def write_package_fixture(root: pathlib.Path, kill, binary_bytes: bytes) -> tuple:
    binary = root / "worldstreamd"
    binary.write_bytes(binary_bytes)
    binary.chmod(0o700)
    manifest = {
        "manifest_revision": 1,
        "schema": "worldstream/storage-compatibility-manifest/v1",
    }
    manifest_toml = (
        b"manifest_revision = 1\n"
        b'schema = "worldstream/storage-compatibility-manifest/v1"\n'
    )
    manifest_json = (
        json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode()
    (root / "compatibility.toml").write_bytes(manifest_toml)
    (root / "compatibility.json").write_bytes(manifest_json)
    archive = root / "worldstream-0.1.0-linux-x86_64.tar.gz"
    write_archive_fixture(archive, binary_bytes, manifest_toml, manifest_json)
    report = root / "package-report.json"
    report.write_text(
        json.dumps(
            {
                "schema": "worldstream/package-report/v1",
                "artifact": archive.name,
                "path": archive.name,
                "kind": "archive",
                "sha256": kill.sha256_file(archive),
                "size_bytes": archive.stat().st_size,
                "inventory": {
                    "archive_verified": True,
                    "manifest_source": "compatibility.toml",
                    "manifest_mirror": "compatibility.json",
                },
                "identity": {
                    "target": "linux-x86_64",
                    "version": "0.1.0",
                    "manifest_sha256": hashlib.sha256(manifest_json).hexdigest(),
                    "manifest_json_sha256": hashlib.sha256(manifest_json).hexdigest(),
                    "manifest_toml_sha256": hashlib.sha256(manifest_toml).hexdigest(),
                },
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return binary, archive, report


def test_distribution_identity_binds_extracted_binary_archive_report_and_manifest(
    tmp_path: pathlib.Path, kill, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(kill, "ROOT", tmp_path)
    binary, archive, report = write_package_fixture(
        tmp_path, kill, b"packaged daemon bytes"
    )
    identity = kill.distribution_identity(binary, archive, report)
    assert identity["packaged_artifact_bound"] is True
    assert identity["target"] == "linux-x86_64"
    assert identity["version"] == "0.1.0"
    assert (
        identity["binary_sha256"]
        == "sha256:" + hashlib.sha256(b"packaged daemon bytes").hexdigest()
    )
    assert all(
        identity[name].startswith("sha256:")
        for name in (
            "archive_sha256",
            "package_report_sha256",
            "manifest_sha256",
            "manifest_json_sha256",
            "manifest_toml_sha256",
        )
    )


@pytest.mark.parametrize(
    "tamper",
    [
        "binary",
        "archive_digest",
        "report_path",
        "legacy_manifest_digest",
        "json_manifest_digest",
        "toml_manifest_digest",
        "target",
        "mirror_semantics",
        "mirror_canonical_encoding",
        "archive_link",
        "duplicate_archive_member",
    ],
)
def test_distribution_identity_rejects_every_tampered_binding(
    tmp_path: pathlib.Path, kill, monkeypatch: pytest.MonkeyPatch, tamper: str
) -> None:
    monkeypatch.setattr(kill, "ROOT", tmp_path)
    binary, archive, report = write_package_fixture(
        tmp_path, kill, b"packaged daemon bytes"
    )
    if tamper == "binary":
        binary.write_bytes(b"different daemon bytes")
    elif tamper in {
        "mirror_semantics",
        "mirror_canonical_encoding",
        "archive_link",
        "duplicate_archive_member",
    }:
        manifest_toml = (
            b"manifest_revision = 1\n"
            b'schema = "worldstream/storage-compatibility-manifest/v1"\n'
        )
        if tamper == "mirror_semantics":
            mirror = b'{"manifest_revision":2,"schema":"worldstream/storage-compatibility-manifest/v1"}\n'
        elif tamper == "mirror_canonical_encoding":
            mirror = b'{"manifest_revision":1,"schema":"worldstream/storage-compatibility-manifest/v1"}\n'
        else:
            mirror = (tmp_path / "compatibility.json").read_bytes()
        extra = {
            "archive_link": "link",
            "duplicate_archive_member": "duplicate",
        }.get(tamper)
        write_archive_fixture(
            archive,
            b"packaged daemon bytes",
            manifest_toml,
            mirror,
            extra_kind=extra,
        )
        (tmp_path / "compatibility.toml").write_bytes(manifest_toml)
        (tmp_path / "compatibility.json").write_bytes(mirror)
        value = json.loads(report.read_text(encoding="utf-8"))
        value["sha256"] = kill.sha256_file(archive)
        value["size_bytes"] = archive.stat().st_size
        mirror_digest = hashlib.sha256(mirror).hexdigest()
        value["identity"]["manifest_sha256"] = mirror_digest
        value["identity"]["manifest_json_sha256"] = mirror_digest
        report.write_text(json.dumps(value) + "\n", encoding="utf-8")
    else:
        value = json.loads(report.read_text(encoding="utf-8"))
        if tamper == "archive_digest":
            value["sha256"] = "sha256:" + "0" * 64
        elif tamper == "report_path":
            value["path"] = "/unbound/archive.tar.gz"
        elif tamper == "legacy_manifest_digest":
            value["identity"]["manifest_sha256"] = "0" * 64
        elif tamper == "json_manifest_digest":
            value["identity"]["manifest_json_sha256"] = "0" * 64
        elif tamper == "toml_manifest_digest":
            value["identity"]["manifest_toml_sha256"] = "0" * 64
        else:
            value["identity"]["target"] = "windows-x64"
        report.write_text(json.dumps(value) + "\n", encoding="utf-8")
    with pytest.raises(kill.EvidenceFailure):
        kill.distribution_identity(binary, archive, report)


@pytest.mark.skipif(os.name != "posix", reason="SIGKILL process test requires POSIX")
def test_harness_sends_external_sigkill_only_after_exact_private_marker(
    tmp_path: pathlib.Path, kill
) -> None:
    tmp_path.chmod(0o700)
    crash = kill.CrashSpec(
        "action", "after_commit_before_publication", "01ARZ3NDEKTSV4RRFFQ69G5FC6"
    )
    daemon = kill.Daemon(
        tmp_path,
        pathlib.Path(sys.executable),
        startup_timeout=1,
        shutdown_timeout=2,
    )
    daemon.process = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(30)"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    marker = {
        "schema": kill.MARKER_SCHEMA,
        "operation": crash.operation,
        "boundary": crash.boundary,
        "match_id": crash.match_id,
        "publication_contract": "telemetry_and_live_room_frames",
    }
    marker_path = tmp_path / "crash-marker.json"
    marker_path.write_text(json.dumps(marker), encoding="utf-8")
    marker_path.chmod(0o600)
    result = daemon.kill_at_marker(crash, 2)
    assert result["signal"] == "SIGKILL"
    assert result["signal_sent"] is True
    assert result["process_exit_observed"] is True
    assert daemon.process is None


def create_witness_database(path: pathlib.Path, count: int) -> None:
    connection = sqlite3.connect(path)
    try:
        connection.executescript(
            "CREATE TABLE rooms(room_id TEXT);"
            "CREATE TABLE room_genesis(room_id TEXT);"
            "CREATE TABLE semantic_receipts(resolution_kind TEXT, transition_seq INTEGER);"
        )
        for index in range(count):
            room_id = f"room-{index}"
            connection.execute("INSERT INTO rooms VALUES (?)", (room_id,))
            connection.execute("INSERT INTO room_genesis VALUES (?)", (room_id,))
            connection.execute(
                "INSERT INTO semantic_receipts VALUES ('genesis_created', NULL)"
            )
        connection.commit()
    finally:
        connection.close()


@pytest.mark.parametrize("count", [0, 1])
def test_offline_create_observer_is_read_only_and_distinguishes_commit(
    tmp_path: pathlib.Path, kill, count: int
) -> None:
    database = tmp_path / "worldstream.sqlite3"
    create_witness_database(database, count)
    before = kill.sha256_file(database)
    witness = kill.offline_create_witness(database)
    after = kill.sha256_file(database)
    assert before == after
    assert witness == {
        "room_count": count,
        "genesis_count": count,
        "creation_receipt_count": count,
    }
    assert sqlite3.sqlite_version


def test_frozen_matrix_and_one_hour_boundary_are_exact(kill, soak) -> None:
    expected = {
        (operation, boundary)
        for operation in ("room_create", "action", "timer", "activation_lease")
        for boundary in (
            "before_commit",
            "after_commit_before_publication",
            "after_publication_before_reply",
        )
    }
    assert len(expected) == 12
    assert len(kill.BOUNDARIES) == 3
    assert soak.WORKLOAD_BINDING == "worldstream-daemon-transition-soak/v1"
    assert soak.COUNTER_ACTIONS_PER_ROOM == 8
    assert soak.workload_target_seconds(True, 1) == 3600
    assert soak.workload_target_seconds(False, 7) == 7
    assert not soak.release_window_is_complete(
        one_hour=False, target_seconds=3600, elapsed_seconds=3600
    )
    assert not soak.release_window_is_complete(
        one_hour=True, target_seconds=3599, elapsed_seconds=3600
    )
    assert not soak.release_window_is_complete(
        one_hour=True, target_seconds=3600, elapsed_seconds=3599.999
    )
    assert soak.release_window_is_complete(
        one_hour=True, target_seconds=3600, elapsed_seconds=3600
    )


def packaged_acceptance_fixture(distribution: dict) -> dict:
    cell = {"exit_code": 0, "report": {"status": "completed"}}
    return {
        "schema": "worldstream/packaged-backend-parity/v1",
        "status": "pass",
        "release_evidence": True,
        "secrets_emitted": False,
        "cleanup": "pass",
        "package_binding": {
            "status": "pass",
            "archive_sha256": distribution["archive_sha256"],
            "identity": {
                "target": distribution["target"],
                "version": distribution["version"],
                "manifest_sha256": distribution["manifest_sha256"].removeprefix(
                    "sha256:"
                ),
                "manifest_json_sha256": distribution[
                    "manifest_json_sha256"
                ].removeprefix("sha256:"),
                "manifest_toml_sha256": distribution[
                    "manifest_toml_sha256"
                ].removeprefix("sha256:"),
            },
        },
        "cells": {
            story: {
                backend: cell
                for backend in ("sqlite", "postgres_direct", "transaction_pooler")
            }
            for story in ("counter", "heist")
        },
        "comparison": {
            "counter": {"status": "pass"},
            "heist": {"status": "pass"},
        },
    }


@pytest.mark.parametrize("tamper", [None, "archive", "status", "cell", "link"])
def test_optional_acceptance_hash_binds_only_verified_raw_six_cell_report(
    tmp_path: pathlib.Path,
    soak,
    tamper: str | None,
) -> None:
    distribution = {
        "packaged_artifact_bound": True,
        "archive_sha256": "sha256:" + "a" * 64,
        "binary_sha256": "sha256:" + "b" * 64,
        "manifest_sha256": "sha256:" + "c" * 64,
        "manifest_json_sha256": "sha256:" + "c" * 64,
        "manifest_toml_sha256": "sha256:" + "d" * 64,
        "target": "linux-x86_64",
        "version": "0.1.0",
    }
    report = packaged_acceptance_fixture(distribution)
    if tamper == "archive":
        report["package_binding"]["archive_sha256"] = "sha256:" + "0" * 64
    elif tamper == "status":
        report["status"] = "incomplete"
    elif tamper == "cell":
        report["cells"]["heist"]["transaction_pooler"]["exit_code"] = 1
    path = tmp_path / "acceptance.json"
    raw = (json.dumps(report, sort_keys=True) + "\n").encode()
    path.write_bytes(raw)
    if tamper == "link":
        target = tmp_path / "acceptance-target.json"
        path.rename(target)
        path.symlink_to(target)
    if tamper is None:
        assert soak.packaged_acceptance_sha256(path, distribution) == (
            "sha256:" + hashlib.sha256(raw).hexdigest()
        )
        identity = soak.reference_identity(
            distribution, soak.packaged_acceptance_sha256(path, distribution)
        )
        assert identity["artifact_sha256"] == distribution["archive_sha256"]
        assert identity["packaged_acceptance_sha256"].startswith("sha256:")
    else:
        with pytest.raises(soak.SoakFailure):
            soak.packaged_acceptance_sha256(path, distribution)


def test_raw_soak_reference_workload_is_counter_specific_and_not_cross_source(soak):
    workload = {
        "action_payload_sizes_bytes": [2],
        "fan_out_memberships_per_room": 3,
        "observer_fan_out_per_transition": 2,
        "snapshot_cadence_transitions": 1,
    }
    assert soak.reference_workload_disclosure(workload) == {
        "payload_sizes_bytes": [2],
        "pack_id": "worldstream.counter",
        "participants_per_room": 3,
        "fan_out": 2,
        "snapshot_cadence_transitions": 1,
    }
    source = SOAK_SCRIPT.read_text(encoding="utf-8")
    assert '"environment_observation": observed_environment' in source
    assert '"reference_environment":' not in source


def test_cli_contract_exposes_packaged_identity_reports_and_retained_logs(
    kill, soak
) -> None:
    kill_options = {
        option for action in kill.parser()._actions for option in action.option_strings
    }
    soak_options = {
        option for action in soak.parser()._actions for option in action.option_strings
    }
    required = {
        "--daemon-bin",
        "--package-archive",
        "--package-report",
        "--output",
        "--log-output",
    }
    assert required <= kill_options
    assert (
        required
        | {
            "--one-hour",
            "--duration-seconds",
            "--packaged-acceptance-report",
            "--preflight-report",
        }
        <= soak_options
    )


def test_offline_observer_wording_does_not_claim_bundled_engine_identity() -> None:
    text = KILL_SCRIPT.read_text(encoding="utf-8")
    assert "offline read-only SQLite observer" in text
    assert "offline_observer_sqlite_version" in text
    assert "read-only bundled SQLite witness" not in text
    assert "engine conformance" in text


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__]))

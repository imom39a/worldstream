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
                },
                "mode": "one_hour",
                "platform": {"system": "Linux", "machine": "x86_64"},
                "distribution": distribution,
                "configuration": {
                    "max_total_seconds": 3600,
                    "one_hour_target_seconds": 3600,
                    "max_output_bytes": 262144,
                    "max_database_growth_bytes": 268435456,
                    "max_temp_growth_bytes": module.MAX_TEMP_GROWTH_BYTES,
                    "max_log_growth_bytes": module.MAX_RUNTIME_LOG_GROWTH_BYTES,
                    "max_artifact_growth_bytes": (
                        module.MAX_AUXILIARY_ARTIFACT_GROWTH_BYTES
                    ),
                    "max_internal_queue_depth": module.TELEMETRY_QUEUE_HARD_LIMIT,
                    "database_workload_binding": (
                        module.RELEASE_DATABASE_WORKLOAD_BINDING
                    ),
                },
                "named_gate_evidence": {
                    "manifest_evidence_id": module.EVIDENCE_ID,
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
                        "peak_rss_bytes_per_run": [134217728],
                        "observed_peak_delta_bytes": 0,
                    },
                },
                "database": {
                    "status": "measured",
                    "workload_binding": module.RELEASE_DATABASE_WORKLOAD_BINDING,
                    "initial_bytes": 4096,
                    "final_bytes": 5120,
                    "growth_bytes": 1024,
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
                "internal_queues": {
                    "status": "measured",
                    "configured_hard_limits_enforced": True,
                    "queues": [
                        {
                            "status": "measured",
                            "name": "telemetry_exporter",
                            "measurement_source": "public_prometheus_metrics",
                            "depth_metric": "worldstream_telemetry_queued",
                            "capacity_metric": ("worldstream_telemetry_queue_capacity"),
                            "configured_hard_limit": (
                                module.TELEMETRY_QUEUE_HARD_LIMIT
                            ),
                            "maximum_observed_depth": 3,
                            "sample_count": 72000,
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
        for operation in module.REQUIRED_KILL_OPERATIONS
        for boundary in module.REQUIRED_KILL_BOUNDARIES
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
    assert artifact["packaged_acceptance"]["sha256"].startswith("sha256:")
    assert artifact["packaged_acceptance"]["content_base64"]
    declared = producer["artifacts"]["failure-soak"]
    assert declared["size_bytes"] == artifact_path.stat().st_size
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


def test_internal_queue_must_expose_and_obey_its_frozen_capacity(fixture) -> None:
    module, toml, mirror, soak, kill, logs, root = fixture
    report = json.loads(soak.read_text())
    report["internal_queues"]["queues"][0]["maximum_observed_depth"] = 257
    soak.write_text(json.dumps(report) + "\n")

    with pytest.raises(module.EvidenceError, match="internal queue exceeded"):
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

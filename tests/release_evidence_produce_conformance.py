"""Strict contract tests for conformance release producer inputs."""

from __future__ import annotations

import argparse
import importlib.util
import json
import shutil
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/release-evidence-produce-conformance.py"


def load_module():
    spec = importlib.util.spec_from_file_location(
        "worldstream_release_conformance_producer", SCRIPT
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_json(path: Path, value: object) -> Path:
    path.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    return path


def manifest_report() -> dict:
    return {
        "schema": "worldstream/manifest-evidence-wave6/v1",
        "claim_policy": {
            "writes_manifest": False,
            "writes_release_directory": False,
            "manufactures_external_evidence": False,
            "implementation_identities_are_release_evidence": False,
        },
        "manifest": {
            "parity": {"semantic": True, "canonical": True},
            "state": {
                "manifest_kind": "release",
                "release_ready": True,
                "unresolved_required_fields": [],
                "release_artifact_ids_unique": True,
                "evidence_shape_ok": True,
            },
        },
        "release_inventory": {
            "inventory_ids_match": True,
            "profile_mismatches": [],
            "all_manifest_digests_empty": True,
            "release_directory_created": False,
        },
        "migrations": {
            provider: {
                "source_count": count,
                "source_ids_not_in_manifest": [],
                "manifest_ids_not_in_source": [],
                "checksum_mismatches": [],
            }
            for provider, count in (("sqlite", 8), ("postgresql", 9))
        },
    }


def sqlite_report() -> dict:
    group_names = {
        "migration",
        "snapshot",
        "recovery",
        "resource_or_storage_fault",
        "corruption_or_quarantine",
        "backup_restore",
        "canonical_hash_parity",
        "native_restore",
    }
    count = 19
    return {
        "schema": "worldstream/soak-evidence/v1",
        "status": "pass",
        "release_evidence": False,
        "mode": "default",
        "preflight": {
            "test_list_parse": {
                "status": "pass",
                "missing_groups": [],
                "listed_test_count": count,
                "coverage_groups": {
                    name: {"status": "covered", "match_count": 1}
                    for name in group_names
                },
            }
        },
        "matrix_runs": [
            {
                "status": "passed",
                "failure_class": "none",
                "output_truncated": False,
                "reported_failed_test_runs": 0,
                "test_count_validation": {
                    "status": "pass",
                    "reported_passed_tests": count,
                },
            }
        ],
        "fixture_hooks": [
            {"category": name, "status": "passed"}
            for name in ("resource", "fault", "corruption")
        ],
    }


def postgres_report() -> dict:
    return {
        "schema": "worldstream/postgresql-live-evidence/v1",
        "status": "pass",
        "reason": "disposable_postgresql_shared_conformance_and_transfer_contracts_passed",
        "exit_code": 0,
        "release_evidence": False,
        "secrets_emitted": False,
        "errors": [],
        "cleanup": {"status": "pass"},
        "images": {
            "postgres": {
                "digest": "postgres@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
            },
            "pgbouncer": {
                "digest": "edoburu/pgbouncer@sha256:4c1ca296ef525f108f5d3552cc337c0c09587cf8dae7f0067fd93349e47dc1cd"
            },
        },
        "profiles": {"direct_admin_runtime": "pass", "transaction_pooler": "pass"},
        "migration_and_conformance": {
            "live_adapter": "pass",
            "production_gateway": "pass",
            "redacted_harness": "pass",
        },
        "marker_witnesses": {
            "direct_admin_migrate_verify_major_17": "pass",
            "direct_admin_restart_idempotent": "pass",
            "runtime_ddl_create_denied": "pass",
            "normalized_commit_conflict_fence_rollback": "pass",
            "pooler_duplicate_resolve": "pass",
        },
        "imo_50_shared_conformance": {
            "catalog": "worldstream-conformance::SCENARIOS",
            "scenario_count": 7,
            "direct": "pass",
            "transaction_pooler": "pass",
            "all_scenarios_pass": True,
        },
        "transfer_parity_epoch": {"status": "pass"},
    }


def transfer_report() -> dict:
    return {
        "schema": "worldstream/sqlite-postgresql-transfer-evidence/v1",
        "status": "pass",
        "release_evidence": False,
        "secrets_emitted": False,
        "source": {
            "canonical_evidence": {"status": "complete"},
            "transfer_contract_gate": {"status": "observed"},
        },
        "transfer": {
            "status": "pass",
            "scope": "whole_deployment",
            "record_count": 65,
            "chunk_count": 2,
            "first_chunk": "applied",
            "interruption_first_chunk": "applied",
            "interruption_abort": "pass",
            "conflicting_chunk_rejected": True,
            "checkpoint_resume_replay": "alreadyapplied",
            "pre_authority_verification": "pass",
            "target_fence": "verified",
            "target_epoch_fence": "verified",
            "whole_deployment_acceptance": "pass",
            "finalization": "pass",
            "target_authority": "pass",
            "target_room_readback": "exact_bytes_and_hashes_verified",
            "authoritative_deployment_identity_gate": "pass",
            "source_identity_witness": "pass",
            "source_membership_witness": "pass",
            "target_identity_witness": "pass",
            "target_membership_witness": "pass",
            "full_room_semantics_hydrated": True,
        },
        "acceptance": {"whole_deployment": "pass"},
    }


def restore_report() -> dict:
    return {
        "schema": "worldstream/native-postgres-restore-evidence/v2",
        "status": "ready",
        "release_evidence": False,
        "native_dump_restore": "pass",
        "source_unchanged": True,
        "exact_restored_row_set": True,
        "snapshots_disposable": True,
        "target_isolated": True,
        "target_published": False,
        "native_witness_minted": True,
        "secrets_emitted": False,
        "semantic_receipts_verified": True,
        "restored_snapshot_count_before": 2,
        "restored_snapshot_count_after": 0,
        "verifier": {"status": "ready"},
    }


def producer_args(tmp_path: Path, values: dict[str, dict]) -> argparse.Namespace:
    manifest_toml = tmp_path / "compatibility.toml"
    manifest_json = tmp_path / "compatibility.json"
    shutil.copy2(ROOT / "compatibility.toml", manifest_toml)
    shutil.copy2(ROOT / "compatibility.json", manifest_json)
    return argparse.Namespace(
        output_dir=tmp_path / "producers",
        artifact_dir=tmp_path / "artifacts",
        manifest_report=write_json(
            tmp_path / "manifest-report.json", values["manifest"]
        ),
        sqlite_report=write_json(tmp_path / "sqlite-report.json", values["sqlite"]),
        postgres_report=write_json(
            tmp_path / "postgres-report.json", values["postgres"]
        ),
        transfer_report=write_json(
            tmp_path / "transfer-report.json", values["transfer"]
        ),
        postgres_restore_report=write_json(
            tmp_path / "restore-report.json", values["restore"]
        ),
        manifest_toml=manifest_toml,
        manifest_json=manifest_json,
    )


def complete_values() -> dict[str, dict]:
    return {
        "manifest": manifest_report(),
        "sqlite": sqlite_report(),
        "postgres": postgres_report(),
        "transfer": transfer_report(),
        "restore": restore_report(),
    }


def test_exact_diagnostics_emit_six_artifact_bound_typed_producers(tmp_path):
    module = load_module()
    args = producer_args(tmp_path, complete_values())

    module.produce(args)

    assert {path.stem for path in args.output_dir.glob("*.json")} == set(
        module.SOURCE_INPUTS
    )
    assert {path.stem for path in args.artifact_dir.glob("*.json")} == set(
        module.SOURCE_INPUTS
    )
    for source_id in module.SOURCE_INPUTS:
        producer = json.loads(
            (args.output_dir / f"{source_id}.json").read_text(encoding="utf-8")
        )
        artifact = json.loads(
            (args.artifact_dir / f"{source_id}.json").read_text(encoding="utf-8")
        )
        assert producer["status"] == "passed"
        assert producer["release_evidence"] is True
        assert artifact["diagnostic_inputs"]
        assert all(
            item["sha256"].startswith("sha256:")
            for item in artifact["diagnostic_inputs"]
        )


@pytest.mark.parametrize(
    ("report_name", "tamper", "message"),
    [
        (
            "manifest",
            lambda value: value["migrations"]["sqlite"].update(
                {"checksum_mismatches": ["sqlite-0008"]}
            ),
            "migration history",
        ),
        (
            "sqlite",
            lambda value: value["preflight"]["test_list_parse"]["coverage_groups"][
                "native_restore"
            ].update({"status": "missing"}),
            "coverage groups",
        ),
        (
            "postgres",
            lambda value: value["imo_50_shared_conformance"].update(
                {"scenario_count": 0}
            ),
            "scenario catalog",
        ),
        (
            "transfer",
            lambda value: value["transfer"].update(
                {"target_room_readback": "exit_code_zero"}
            ),
            "exact-byte",
        ),
        (
            "restore",
            lambda value: value.update({"target_published": True}),
            "native PostgreSQL restore",
        ),
    ],
)
def test_diagnostic_content_drift_fails_closed(tmp_path, report_name, tamper, message):
    module = load_module()
    values = complete_values()
    tamper(values[report_name])
    args = producer_args(tmp_path, values)

    with pytest.raises(module.EvidenceError, match=message):
        module.produce(args)


@pytest.mark.parametrize(
    ("path", "invalid", "message"),
    [
        (("transfer", "status"), "not-run", "checkpoint/resume"),
        (("transfer", "scope"), "one-room", "checkpoint/resume"),
        (("transfer", "record_count"), 0, "checkpoint/resume"),
        (("transfer", "chunk_count"), 0, "checkpoint/resume"),
        (("transfer", "first_chunk"), "skipped", "checkpoint/resume"),
        (("transfer", "interruption_first_chunk"), "skipped", "checkpoint/resume"),
        (("transfer", "interruption_abort"), "not-run", "checkpoint/resume"),
        (("transfer", "conflicting_chunk_rejected"), False, "checkpoint/resume"),
        (("transfer", "checkpoint_resume_replay"), "applied", "checkpoint/resume"),
        (("transfer", "pre_authority_verification"), "not-run", "checkpoint/resume"),
        (("transfer", "target_fence"), "not-run", "epoch-fencing"),
        (("transfer", "target_epoch_fence"), "not-run", "epoch-fencing"),
        (
            ("transfer", "whole_deployment_acceptance"),
            "not-run",
            "whole-deployment",
        ),
        (("acceptance", "whole_deployment"), "not-run", "whole-deployment"),
    ],
)
def test_transfer_claims_require_each_resume_and_fencing_witness(
    path, invalid, message
):
    module = load_module()
    report = transfer_report()
    report[path[0]][path[1]] = invalid

    with pytest.raises(module.EvidenceError, match=message):
        module.validate_transfer(report)


@pytest.mark.parametrize(
    ("section", "field", "invalid", "message"),
    [
        (None, "exit_code", 13, "live diagnostic"),
        (None, "errors", ["live_marker_missing"], "live diagnostic"),
        (
            "migration_and_conformance",
            "production_gateway",
            "failed",
            "migration/adapter",
        ),
        (
            "marker_witnesses",
            "direct_admin_migrate_verify_major_17",
            "missing",
            "marker witnesses",
        ),
        (
            "marker_witnesses",
            "direct_admin_restart_idempotent",
            "missing",
            "marker witnesses",
        ),
        (
            "marker_witnesses",
            "runtime_ddl_create_denied",
            "missing",
            "marker witnesses",
        ),
        (
            "marker_witnesses",
            "normalized_commit_conflict_fence_rollback",
            "missing",
            "marker witnesses",
        ),
        (
            "marker_witnesses",
            "pooler_duplicate_resolve",
            "missing",
            "marker witnesses",
        ),
    ],
)
def test_postgres_claims_require_exit_errors_gateway_and_named_markers(
    section, field, invalid, message
):
    module = load_module()
    report = postgres_report()
    target = report if section is None else report[section]
    target[field] = invalid

    with pytest.raises(module.EvidenceError, match=message):
        module.validate_postgres(report)

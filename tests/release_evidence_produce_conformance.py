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
RESTORE_DURABLE_DOMAINS = (
    "schema_migrations",
    "operation_guards",
    "room_roots",
    "genesis",
    "materializations",
    "member_delivery_state",
    "timers",
    "transitions",
    "frames",
    "observation_consequences",
    "activation_decisions",
    "activation_intents",
    "activation_operation_receipts",
    "semantic_receipts",
    "integrity_incidents",
    "authority_fences",
    "retired_authority_fences",
    "authority_state",
    "authority_principals",
    "authority_runners",
    "authority_capabilities",
    "authority_capability_scopes",
    "authority_runner_capability_memberships",
    "authority_change_receipts",
    "authority_audit",
    "transfer_imports",
    "transfer_chunks",
    "transfer_target_fence",
    "deployment_metadata",
    "deployment_identity_metadata",
    "deployment_pack_identities",
    "deployment_resource_identities",
    "deployment_resource_blobs",
)


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
        "provider_mode": "docker",
        "provider_image": {
            "reference": "postgres:17.11-alpine@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73",
            "repository_digest": "postgres@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73",
        },
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


def native_restore_report() -> dict:
    inventory = [
        {
            "domain": domain,
            "source_row_count": 1,
            "restored_row_count": 1,
            "source_digest": f"{index:064x}",
            "restored_digest": f"{index:064x}",
        }
        for index, domain in enumerate(RESTORE_DURABLE_DOMAINS, start=1)
    ]
    return {
        "schema": "worldstream/native-postgres-restore-evidence/v2",
        "status": "ready",
        "reason": "postgres_native_restore_verified_by_unified_verifier",
        "release_evidence": False,
        "native_dump_restore": "pass",
        "backup_id": "postgres-native-" + "b" * 64,
        "native_point_digest": (
            "d12ef884fee4573a111d13e5adbb2b2e05683f65f7f219bc0019c9b9c8a899dc"
        ),
        "native_dump_digest": "b" * 64,
        "native_dump_size_bytes": 4096,
        "source_unchanged": True,
        "exact_restored_row_set": True,
        "snapshots_disposable": True,
        "target_isolated": True,
        "target_published": False,
        "cleanup_required": True,
        "native_witness_minted": True,
        "secrets_emitted": False,
        "source_provider_identity": {
            "system_identifier": "7400000000000000001",
            "database_oid": "16384",
            "database_name": "worldstream_source",
        },
        "target_provider_identity": {
            "system_identifier": "7400000000000000001",
            "database_oid": "16385",
            "database_name": "worldstream_target",
        },
        "source_version_num": 170_011,
        "restored_version_num": 170_011,
        "semantic_receipts_verified": True,
        "activation_intents_verified": True,
        "activation_operation_receipts_verified": True,
        "activation_request_evidence": "stored_canonical_hash_only_verified",
        "authority_state_verified": True,
        "durable_domains_verified": True,
        "verifier_scope": {
            "profile": "full_deployment_all_durable_domains",
            "source_pack_identity_count": 2,
            "restored_pack_identity_count": 2,
            "source_resource_identity_count": 1,
            "restored_resource_identity_count": 1,
            "source_fired_timer_count": 1,
            "restored_fired_timer_count": 1,
            "general_deployment_support_verified": True,
        },
        "source_durable_domains_digest": "a" * 64,
        "restored_durable_domains_digest": "a" * 64,
        "durable_domain_inventory": inventory,
        "restored_snapshot_count_before": 2,
        "restored_snapshot_count_after": 0,
        "verifier": {
            "readiness": "Ready",
            "rooms": {
                "01ARZ3NDEKTSV4RRFFQ69G5FAV": "Verified",
                "01ARZ3NDEKTSV4RRFFQ69G5FC5": "Verified",
                "01ARZ3NDEKTSV4RRFFQ69G5FQ0": "IsolatedPreExisting",
            },
            "diagnostics": [
                {
                    "class": "Integrity",
                    "code": "pre_existing_isolation_preserved",
                    "subject": "subject:020e3e550f71",
                    "action": "keep the Room isolated; investigate or repair it through the separate verifier-repair contract",
                    "disposition": "PermittedPreExistingIsolation",
                }
            ],
        },
    }


def restore_report() -> dict:
    return {
        "schema": "worldstream/native-postgres-restore-smoke-evidence/v1",
        "status": "ready",
        "reason": "native_postgres_restore_smoke_completed",
        "exit_code": 0,
        "release_evidence": False,
        "native_restore": native_restore_report(),
        "live_restore_concurrency": {
            "observed": True,
            "target_connection_limit_during_restore": 2,
            "target_client_backends_excluding_observer": 2,
            "direct_superuser_keeper_backends": 1,
            "one_use_restore_role_backends": 1,
            "keeper_and_restore_pids_distinct": True,
            "final_captured_role_sessions_across_cluster": 0,
            "final_captured_role_exists": 0,
            "captured_restore_role": (
                "worldstream_restore_0123456789abcdef0123456789abcdef"
            ),
            "captured_keeper_pid": 101,
            "captured_restore_pid": 102,
        },
        "target_isolated": True,
        "target_published": False,
        "secrets_emitted": False,
    }


def test_restore_accepts_actual_full_deployment_verifier_shape():
    module = load_module()

    outcomes = module.validate_restore(sqlite_report(), restore_report())

    assert "two Pack identities" in outcomes["bounded_fixture_semantic_verifier"]
    assert "one immutable resource" in outcomes["bounded_fixture_semantic_verifier"]


@pytest.mark.parametrize(
    ("field", "invalid"),
    [
        ("observed", False),
        ("target_connection_limit_during_restore", 1),
        ("target_client_backends_excluding_observer", 1),
        ("direct_superuser_keeper_backends", 0),
        ("one_use_restore_role_backends", 0),
        ("keeper_and_restore_pids_distinct", False),
        ("final_captured_role_sessions_across_cluster", 1),
        ("final_captured_role_exists", 1),
        ("captured_restore_role", "postgres"),
        ("captured_keeper_pid", 0),
        ("captured_restore_pid", 0),
    ],
)
def test_restore_requires_each_live_one_use_credential_witness(field, invalid):
    module = load_module()
    report = restore_report()
    report["live_restore_concurrency"][field] = invalid

    with pytest.raises(module.EvidenceError, match="one-use credential proof"):
        module.validate_restore(sqlite_report(), report)


def test_restore_rejects_unversioned_or_extended_smoke_wrapper():
    module = load_module()
    unversioned = native_restore_report()
    with pytest.raises(module.EvidenceError, match="smoke wrapper"):
        module.validate_restore(sqlite_report(), unversioned)

    extended = restore_report()
    extended["ignored_proof"] = True
    with pytest.raises(module.EvidenceError, match="smoke wrapper"):
        module.validate_restore(sqlite_report(), extended)


def test_restore_rejects_same_live_keeper_and_restore_pid():
    module = load_module()
    report = restore_report()
    report["live_restore_concurrency"]["captured_restore_pid"] = report[
        "live_restore_concurrency"
    ]["captured_keeper_pid"]

    with pytest.raises(module.EvidenceError, match="one-use credential proof"):
        module.validate_restore(sqlite_report(), report)


@pytest.mark.parametrize(
    ("tamper", "message"),
    [
        (lambda verifier: verifier.update({"status": "ready"}), "return ready"),
        (lambda verifier: verifier.update({"readiness": "NotReady"}), "return ready"),
        (
            lambda verifier: verifier["rooms"].update(
                {"01ARZ3NDEKTSV4RRFFQ69G5FAV": "Blocked"}
            ),
            "rooms are malformed",
        ),
        (lambda verifier: verifier.update({"rooms": {}}), "rooms are malformed"),
        (
            lambda verifier: verifier["diagnostics"][0].update(
                {"disposition": "Blocking"}
            ),
            "blocking diagnostics",
        ),
        (
            lambda verifier: verifier["diagnostics"][0].update(
                {"raw_payload": "not-redacted"}
            ),
            "blocking diagnostics",
        ),
        (
            lambda verifier: verifier["diagnostics"][0].update(
                {"subject": "subject:TOP_SECRET"}
            ),
            "blocking diagnostics",
        ),
        (
            lambda verifier: verifier["diagnostics"][0].update(
                {"action": "accept an unreviewed isolation"}
            ),
            "blocking diagnostics",
        ),
    ],
)
def test_restore_rejects_malformed_or_blocking_real_verifier_shape(tamper, message):
    module = load_module()
    report = restore_report()
    tamper(report["native_restore"]["verifier"])

    with pytest.raises(module.EvidenceError, match=message):
        module.validate_restore(sqlite_report(), report)


def test_restore_enforces_verifier_room_and_diagnostic_bounds():
    module = load_module()
    module.MAX_RESTORE_VERIFIER_ROOMS = 2
    with pytest.raises(module.EvidenceError, match="unbounded"):
        module.validate_restore(sqlite_report(), restore_report())

    module = load_module()
    module.MAX_RESTORE_VERIFIER_DIAGNOSTICS = 0
    with pytest.raises(module.EvidenceError, match="diagnostics.*unbounded"):
        module.validate_restore(sqlite_report(), restore_report())


@pytest.mark.parametrize(
    ("field", "invalid"),
    [
        ("restored_snapshot_count_before", True),
        ("restored_snapshot_count_after", False),
    ],
)
def test_restore_rejects_boolean_snapshot_counts(field, invalid):
    module = load_module()
    report = restore_report()
    report["native_restore"][field] = invalid

    with pytest.raises(module.EvidenceError, match="isolated semantic contract"):
        module.validate_restore(sqlite_report(), report)


def test_restore_rejects_boolean_restored_domain_count():
    module = load_module()
    report = restore_report()
    authority_index = RESTORE_DURABLE_DOMAINS.index("authority_state")
    report["native_restore"]["durable_domain_inventory"][authority_index][
        "restored_row_count"
    ] = True

    with pytest.raises(module.EvidenceError, match="count or digest mismatch"):
        module.validate_restore(sqlite_report(), report)


@pytest.mark.parametrize(
    ("field", "invalid"),
    [
        ("activation_intents_verified", False),
        ("activation_operation_receipts_verified", False),
        ("activation_request_evidence", "exact_request_bytes_verified"),
        ("authority_state_verified", False),
        ("durable_domains_verified", False),
    ],
)
def test_restore_requires_each_typed_activation_and_authority_witness(field, invalid):
    module = load_module()
    report = restore_report()
    report["native_restore"][field] = invalid
    with pytest.raises(module.EvidenceError, match="isolated semantic contract"):
        module.validate_restore(sqlite_report(), report)


@pytest.mark.parametrize(
    ("field", "invalid"),
    [
        ("backup_id", "postgres-native-" + "0" * 64),
        ("native_point_digest", "c" * 64),
        ("native_dump_digest", "not-a-digest"),
        ("native_dump_size_bytes", 0),
        ("reason", "uncommitted"),
        ("cleanup_required", False),
        ("source_provider_identity", {"database_name": "worldstream_source"}),
        (
            "target_provider_identity",
            {
                "system_identifier": "7400000000000000001",
                "database_oid": "0",
                "database_name": "worldstream_target",
            },
        ),
        ("source_version_num", 170_010),
        ("restored_version_num", 170_010),
    ],
)
def test_restore_requires_exact_provider_native_backup_identity(field, invalid):
    module = load_module()
    report = restore_report()
    report["native_restore"][field] = invalid
    with pytest.raises(module.EvidenceError, match="isolated semantic contract"):
        module.validate_restore(sqlite_report(), report)


@pytest.mark.parametrize(
    ("field", "invalid"),
    [
        ("profile", "single_pack_no_resources_no_fired_timers"),
        ("source_pack_identity_count", 1),
        ("restored_pack_identity_count", 1),
        ("source_resource_identity_count", 0),
        ("restored_resource_identity_count", 0),
        ("source_fired_timer_count", 0),
        ("restored_fired_timer_count", 0),
        ("general_deployment_support_verified", False),
    ],
)
def test_restore_requires_exact_bounded_verifier_scope(field, invalid):
    module = load_module()
    report = restore_report()
    report["native_restore"]["verifier_scope"][field] = invalid
    with pytest.raises(module.EvidenceError, match="scope.*overclaims"):
        module.validate_restore(sqlite_report(), report)


def test_restore_requires_complete_bounded_verifier_scope():
    module = load_module()
    report = restore_report()
    report["native_restore"]["verifier_scope"].pop("source_fired_timer_count")
    with pytest.raises(module.EvidenceError, match="scope.*overclaims"):
        module.validate_restore(sqlite_report(), report)


@pytest.mark.parametrize("domain", RESTORE_DURABLE_DOMAINS)
def test_restore_rejects_each_durable_domain_omission(domain):
    module = load_module()
    report = restore_report()
    report["native_restore"]["durable_domain_inventory"] = [
        row
        for row in report["native_restore"]["durable_domain_inventory"]
        if row["domain"] != domain
    ]
    with pytest.raises(module.EvidenceError, match="inventory is incomplete"):
        module.validate_restore(sqlite_report(), report)


@pytest.mark.parametrize(
    ("field", "invalid", "message"),
    [
        ("source_row_count", 2, "count or digest"),
        ("restored_row_count", 2, "count or digest"),
        ("source_digest", "b" * 64, "count or digest"),
        ("restored_digest", "b" * 64, "count or digest"),
        ("source_digest", "not-a-digest", "count or digest"),
    ],
)
def test_restore_binds_each_domain_count_and_digest(field, invalid, message):
    module = load_module()
    report = restore_report()
    report["native_restore"]["durable_domain_inventory"][0][field] = invalid
    with pytest.raises(module.EvidenceError, match=message):
        module.validate_restore(sqlite_report(), report)


def test_restore_binds_aggregate_digest_order_and_authority_singleton():
    module = load_module()

    aggregate = restore_report()
    aggregate["native_restore"]["restored_durable_domains_digest"] = "b" * 64
    with pytest.raises(module.EvidenceError, match="aggregate digest"):
        module.validate_restore(sqlite_report(), aggregate)

    order = restore_report()
    inventory = order["native_restore"]["durable_domain_inventory"]
    inventory[0], inventory[1] = (
        inventory[1],
        inventory[0],
    )
    with pytest.raises(module.EvidenceError, match="inventory drifted"):
        module.validate_restore(sqlite_report(), order)

    singleton = restore_report()
    index = RESTORE_DURABLE_DOMAINS.index("authority_state")
    singleton["native_restore"]["durable_domain_inventory"][index][
        "source_row_count"
    ] = 0
    singleton["native_restore"]["durable_domain_inventory"][index][
        "restored_row_count"
    ] = 0
    with pytest.raises(module.EvidenceError, match="authority singleton"):
        module.validate_restore(sqlite_report(), singleton)


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


def test_producer_rejects_parsed_input_path_substitution_without_hashing_victim(
    tmp_path, monkeypatch
):
    module = load_module()
    args = producer_args(tmp_path, complete_values())
    admitted_bytes = args.manifest_report.read_bytes()
    victim_bytes = b'{"private_canary":"must-survive"}\n'
    original_validate = module.validate_manifest
    substituted = False

    def substitute_after_parse(report):
        nonlocal substituted
        result = original_validate(report)
        if substituted:
            return result
        substituted = True
        held = args.manifest_report.with_name("held-manifest-report.json")
        args.manifest_report.rename(held)
        args.manifest_report.write_bytes(victim_bytes)
        assert held.read_bytes() == admitted_bytes
        return result

    monkeypatch.setattr(module, "validate_manifest", substitute_after_parse)

    with pytest.raises(module.EvidenceError, match="changed while it was retained"):
        module.produce(args)

    assert args.manifest_report.read_bytes() == victim_bytes
    assert not args.output_dir.exists()
    assert not args.artifact_dir.exists()


def test_producer_rejects_same_inode_same_size_input_mutation(tmp_path, monkeypatch):
    module = load_module()
    args = producer_args(tmp_path, complete_values())
    original = args.transfer_report.read_bytes()
    original_validate = module.validate_manifest

    def mutate_after_parse(report):
        result = original_validate(report)
        args.transfer_report.write_bytes(bytes([original[0] ^ 1]) + original[1:])
        assert args.transfer_report.stat().st_size == len(original)
        return result

    monkeypatch.setattr(module, "validate_manifest", mutate_after_parse)

    with pytest.raises(module.EvidenceError, match="changed while it was retained"):
        module.produce(args)

    assert not args.output_dir.exists()
    assert not args.artifact_dir.exists()


def test_postgres_exit_code_rejects_boolean_zero():
    module = load_module()
    report = postgres_report()
    report["exit_code"] = False

    with pytest.raises(module.EvidenceError, match="live diagnostic"):
        module.validate_postgres(report)


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("system_identifier", "١"),
        ("system_identifier", str(1 << 64)),
        ("database_oid", "١"),
        ("database_oid", str(1 << 32)),
        ("database_oid", "01"),
    ],
)
def test_restore_provider_numeric_identity_is_ascii_canonical_and_bounded(field, value):
    module = load_module()
    report = restore_report()
    report["native_restore"]["target_provider_identity"][field] = value

    with pytest.raises(module.EvidenceError, match="isolated semantic contract"):
        module.validate_restore(sqlite_report(), report)


def test_restore_source_and_target_require_distinct_authoritative_numeric_tuple():
    module = load_module()
    report = restore_report()
    source = report["native_restore"]["source_provider_identity"]
    target = report["native_restore"]["target_provider_identity"]
    target.update(
        system_identifier=source["system_identifier"],
        database_oid=source["database_oid"],
        database_name="different_display_name",
    )

    with pytest.raises(module.EvidenceError, match="isolated semantic contract"):
        module.validate_restore(sqlite_report(), report)


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
            lambda value: value["native_restore"].update({"target_published": True}),
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

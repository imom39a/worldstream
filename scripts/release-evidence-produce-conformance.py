#!/usr/bin/env python3
"""Produce six typed release attestations from exact conformance diagnostics.

The inputs are machine-readable reports emitted by their owning verifier
boundaries.  This adapter validates their schemas and semantic results; it
never treats a command exit code, an arbitrary log, or file existence as a
passed check.  Each producer is bound to a canonical artifact that inventories
the exact diagnostic bytes used for that source.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import stat
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
ADAPTER_PATH = ROOT / "scripts/release-evidence-produce.py"
SOURCE_INPUTS = {
    "manifest-contract": ("manifest",),
    "sqlite-conformance": ("sqlite",),
    "postgres-conformance": ("postgres",),
    "migration-history": ("manifest", "postgres"),
    "transfer": ("transfer",),
    "restore": ("sqlite", "postgres-restore"),
}


def load_adapter():
    spec = importlib.util.spec_from_file_location(
        "worldstream_release_evidence_adapter", ADAPTER_PATH
    )
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {ADAPTER_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


ADAPTER = load_adapter()


class EvidenceError(RuntimeError):
    """A diagnostic cannot support a release attestation."""


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


def sha256(path: Path) -> str:
    try:
        return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        raise EvidenceError(f"cannot hash diagnostic {path}: {error}") from error


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def atomic_write(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    require(not path.is_symlink() and not path.is_dir(), f"unsafe output path: {path}")
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(canonical_json(value))
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def validate_manifest(report: dict[str, Any]) -> dict[str, str]:
    require(
        report.get("schema") == "worldstream/manifest-evidence-wave6/v1",
        "manifest diagnostic schema mismatch",
    )
    policy = report.get("claim_policy")
    require(
        isinstance(policy, dict)
        and policy.get("writes_manifest") is False
        and policy.get("writes_release_directory") is False
        and policy.get("manufactures_external_evidence") is False
        and policy.get("implementation_identities_are_release_evidence") is False,
        "manifest diagnostic claim policy is invalid",
    )
    manifest = report.get("manifest")
    parity = manifest.get("parity") if isinstance(manifest, dict) else None
    state = manifest.get("state") if isinstance(manifest, dict) else None
    require(
        isinstance(parity, dict)
        and parity
        and all(value is True for value in parity.values()),
        "manifest TOML/JSON parity did not pass",
    )
    require(
        isinstance(state, dict)
        and state.get("manifest_kind") == "release"
        and state.get("release_ready") is True
        and state.get("unresolved_required_fields") == []
        and state.get("release_artifact_ids_unique") is True
        and state.get("evidence_shape_ok") is True,
        "embedded release contract is incomplete",
    )
    inventory = report.get("release_inventory")
    require(
        isinstance(inventory, dict)
        and inventory.get("inventory_ids_match") is True
        and inventory.get("profile_mismatches") == []
        and inventory.get("all_manifest_digests_empty") is True
        and inventory.get("release_directory_created") is False,
        "manifest release inventory diagnostic is invalid",
    )
    return {
        "toml_json_semantic_equal": "semantic and canonical mirror parity verified",
        "canonical_json_mirror": "reviewed source and canonical mirror identities verified",
        "release_contract_complete": "release contract complete with detached identities",
    }


def sqlite_matrix(report: dict[str, Any]) -> tuple[dict[str, Any], dict[str, str]]:
    require(
        report.get("schema") == "worldstream/soak-evidence/v1",
        "SQLite diagnostic schema mismatch",
    )
    require(
        report.get("status") == "pass"
        and report.get("release_evidence") is False
        and report.get("mode") == "default",
        "bounded SQLite diagnostic did not pass in conformance mode",
    )
    parsed = report.get("preflight", {}).get("test_list_parse")
    require(
        isinstance(parsed, dict)
        and parsed.get("status") == "pass"
        and parsed.get("missing_groups") == []
        and isinstance(parsed.get("listed_test_count"), int)
        and parsed["listed_test_count"] > 0,
        "SQLite named coverage preflight did not pass",
    )
    groups = parsed.get("coverage_groups")
    required_groups = {
        "migration",
        "snapshot",
        "recovery",
        "resource_or_storage_fault",
        "corruption_or_quarantine",
        "backup_restore",
        "canonical_hash_parity",
        "native_restore",
    }
    require(
        isinstance(groups, dict)
        and required_groups.issubset(groups)
        and all(groups[name].get("status") == "covered" for name in required_groups),
        "SQLite release conformance coverage groups are incomplete",
    )
    runs = report.get("matrix_runs")
    require(
        isinstance(runs, list) and runs, "SQLite diagnostic has no complete matrix run"
    )
    for run in runs:
        require(
            isinstance(run, dict)
            and run.get("status") == "passed"
            and run.get("failure_class") == "none"
            and run.get("output_truncated") is False
            and run.get("reported_failed_test_runs") == 0
            and run.get("test_count_validation", {}).get("status") == "pass"
            and run.get("test_count_validation", {}).get("reported_passed_tests")
            == parsed["listed_test_count"],
            "SQLite matrix run is failed, truncated, or incomplete",
        )
    hooks = {
        item.get("category"): item
        for item in report.get("fixture_hooks", [])
        if isinstance(item, dict)
    }
    require(
        all(
            hooks.get(name, {}).get("status") == "passed"
            for name in ("resource", "fault", "corruption")
        ),
        "SQLite fault/corruption fixture hooks did not pass",
    )
    observations = {
        "migration_history": f"matrix_tests={parsed['listed_test_count']}; migration coverage passed",
        "backup_restore": "snapshot and exact native backup/restore coverage passed",
        "crash_recovery": "recovery, fault, and corruption coverage passed",
        "canonical_hash_parity": "canonical hash/parity coverage passed in complete matrix",
    }
    return parsed, observations


def validate_postgres(report: dict[str, Any]) -> dict[str, str]:
    require(
        set(report)
        == {
            "schema",
            "status",
            "reason",
            "exit_code",
            "release_evidence",
            "secrets_emitted",
            "images",
            "profiles",
            "migration_and_conformance",
            "marker_witnesses",
            "imo_50_shared_conformance",
            "transfer_parity_epoch",
            "cleanup",
            "errors",
        },
        "PostgreSQL diagnostic has wrong fields",
    )
    require(
        report.get("schema") == "worldstream/postgresql-live-evidence/v1",
        "PostgreSQL diagnostic schema mismatch",
    )
    require(
        report.get("status") == "pass"
        and report.get("reason")
        == "disposable_postgresql_shared_conformance_and_transfer_contracts_passed"
        and report.get("exit_code") == 0
        and report.get("errors") == []
        and report.get("release_evidence") is False
        and report.get("secrets_emitted") is False
        and report.get("cleanup", {}).get("status") == "pass",
        "PostgreSQL live diagnostic did not pass safely",
    )
    images = report.get("images")
    require(
        isinstance(images, dict)
        and images.get("postgres", {}).get("digest")
        == "postgres@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
        and images.get("pgbouncer", {}).get("digest")
        == "edoburu/pgbouncer@sha256:4c1ca296ef525f108f5d3552cc337c0c09587cf8dae7f0067fd93349e47dc1cd",
        "PostgreSQL/PgBouncer image identity mismatch",
    )
    profiles = report.get("profiles")
    migration = report.get("migration_and_conformance")
    shared = report.get("imo_50_shared_conformance")
    require(
        isinstance(profiles, dict)
        and profiles.get("direct_admin_runtime") == "pass"
        and profiles.get("transaction_pooler") == "pass",
        "direct and transaction-pooler profiles did not both pass",
    )
    require(
        isinstance(migration, dict)
        and migration.get("live_adapter") == "pass"
        and migration.get("production_gateway") == "pass"
        and migration.get("redacted_harness") == "pass",
        "PostgreSQL migration/adapter conformance did not pass",
    )
    require(
        report.get("marker_witnesses")
        == {
            "direct_admin_migrate_verify_major_17": "pass",
            "direct_admin_restart_idempotent": "pass",
            "runtime_ddl_create_denied": "pass",
            "normalized_commit_conflict_fence_rollback": "pass",
            "pooler_duplicate_resolve": "pass",
        },
        "PostgreSQL direct/pooler marker witnesses did not all pass",
    )
    require(
        isinstance(shared, dict)
        and shared.get("catalog") == "worldstream-conformance::SCENARIOS"
        and shared.get("scenario_count") == 7
        and shared.get("direct") == "pass"
        and shared.get("transaction_pooler") == "pass"
        and shared.get("all_scenarios_pass") is True,
        "shared direct/pooler scenario catalog did not pass",
    )
    return {
        "postgres_version": "pinned PostgreSQL 17.11 image verified",
        "direct_runtime": "direct admin/runtime profile passed",
        "transaction_pooler": "PgBouncer transaction-pool profile passed",
        "adapter_conformance": "seven shared scenarios passed on both paths",
    }


def validate_migrations(
    manifest_report: dict[str, Any], postgres_report: dict[str, Any]
) -> dict[str, str]:
    validate_manifest(manifest_report)
    validate_postgres(postgres_report)
    migrations = manifest_report.get("migrations")
    require(isinstance(migrations, dict), "migration diagnostic is missing")
    for provider, expected_count in (("sqlite", 8), ("postgresql", 9)):
        row = migrations.get(provider)
        require(
            isinstance(row, dict)
            and row.get("source_count") == expected_count
            and row.get("source_ids_not_in_manifest") == []
            and row.get("manifest_ids_not_in_source") == []
            and row.get("checksum_mismatches") == [],
            f"{provider} forward migration history differs from implementation",
        )
    return {
        "sqlite_forward_history": "all 8 typed SQLite migrations matched",
        "postgresql_forward_history": "all 9 typed PostgreSQL migrations matched and ran live",
        "migration_checksums": "both provider checksum maps matched implementation bytes",
    }


def validate_transfer(report: dict[str, Any]) -> dict[str, str]:
    require(
        report.get("schema") == "worldstream/sqlite-postgresql-transfer-evidence/v1",
        "transfer diagnostic schema mismatch",
    )
    require(
        report.get("status") == "pass"
        and report.get("release_evidence") is False
        and report.get("secrets_emitted") is False,
        "live transfer diagnostic did not pass safely",
    )
    source = report.get("source")
    transfer = report.get("transfer")
    require(
        isinstance(source, dict)
        and source.get("canonical_evidence", {}).get("status") == "complete"
        and source.get("transfer_contract_gate", {}).get("status") == "observed",
        "transfer source canonical identity is incomplete",
    )
    require(isinstance(transfer, dict), "transfer result is missing")
    exact = {
        "finalization": "pass",
        "target_authority": "pass",
        "target_room_readback": "exact_bytes_and_hashes_verified",
        "authoritative_deployment_identity_gate": "pass",
        "source_identity_witness": "pass",
        "source_membership_witness": "pass",
        "target_identity_witness": "pass",
        "target_membership_witness": "pass",
    }
    require(
        all(transfer.get(key) == value for key, value in exact.items())
        and transfer.get("full_room_semantics_hydrated") is True,
        "transfer exact-byte, identity, membership, or authority verification failed",
    )
    require(
        transfer.get("status") == "pass"
        and transfer.get("scope") == "whole_deployment"
        and type(transfer.get("record_count")) is int
        and transfer["record_count"] > 0
        and type(transfer.get("chunk_count")) is int
        and transfer["chunk_count"] > 0
        and transfer.get("first_chunk") == "applied"
        and transfer.get("interruption_first_chunk") == "applied"
        and transfer.get("interruption_abort") == "pass"
        and transfer.get("conflicting_chunk_rejected") is True
        and transfer.get("checkpoint_resume_replay") == "alreadyapplied"
        and transfer.get("pre_authority_verification") == "pass",
        "transfer checkpoint/resume and interruption evidence is incomplete",
    )
    require(
        transfer.get("target_fence") == "verified"
        and transfer.get("target_epoch_fence") == "verified",
        "transfer epoch-fencing evidence is incomplete",
    )
    require(
        transfer.get("whole_deployment_acceptance") == "pass"
        and report.get("acceptance", {}).get("whole_deployment") == "pass",
        "whole-deployment transfer acceptance did not pass",
    )
    return {
        "byte_parity": "source/target exact bytes and hashes verified",
        "checkpoint_resume": "bounded staged transfer resume verified",
        "epoch_fencing": "target-wide epoch/identity fence verified",
        "finalization": "target authority finalized after semantic verification",
    }


def validate_restore(
    sqlite_report: dict[str, Any], postgres_report: dict[str, Any]
) -> dict[str, str]:
    parsed, _ = sqlite_matrix(sqlite_report)
    require(
        parsed["coverage_groups"]["native_restore"].get("status") == "covered",
        "SQLite exact isolated native restore test is absent",
    )
    require(
        postgres_report.get("schema")
        == "worldstream/native-postgres-restore-evidence/v2"
        and postgres_report.get("status") == "ready"
        and postgres_report.get("release_evidence") is False
        and postgres_report.get("native_dump_restore") == "pass"
        and postgres_report.get("source_unchanged") is True
        and postgres_report.get("exact_restored_row_set") is True
        and postgres_report.get("snapshots_disposable") is True
        and postgres_report.get("target_isolated") is True
        and postgres_report.get("target_published") is False
        and postgres_report.get("native_witness_minted") is True
        and postgres_report.get("secrets_emitted") is False
        and postgres_report.get("semantic_receipts_verified") is True
        and postgres_report.get("restored_snapshot_count_before", 0) > 0
        and postgres_report.get("restored_snapshot_count_after") == 0,
        "native PostgreSQL restore did not satisfy the isolated semantic contract",
    )
    verifier = postgres_report.get("verifier")
    require(
        isinstance(verifier, dict) and verifier.get("status") == "ready",
        "provider-neutral restore verifier did not return ready",
    )
    return {
        "sqlite_isolated_restore": "complete SQLite matrix included exact native envelope restore",
        "postgresql_isolated_restore": "native PostgreSQL dump/restore was isolated and exact",
        "full_semantic_verifier": "provider-neutral verifier ready; snapshots disposed after proof",
    }


def input_record(name: str, path: Path) -> dict[str, Any]:
    return {
        "id": name,
        "sha256": sha256(path),
        "size_bytes": path.stat().st_size,
    }


def produce(args: argparse.Namespace) -> None:
    manifest = ADAPTER.COLLECTOR.load_manifest(args.manifest_toml, args.manifest_json)
    paths = {
        "manifest": regular_file(args.manifest_report, "manifest diagnostic"),
        "sqlite": regular_file(args.sqlite_report, "SQLite diagnostic"),
        "postgres": regular_file(args.postgres_report, "PostgreSQL diagnostic"),
        "transfer": regular_file(args.transfer_report, "transfer diagnostic"),
        "postgres-restore": regular_file(
            args.postgres_restore_report, "PostgreSQL restore diagnostic"
        ),
    }
    reports = {
        name: read_json(path, f"{name} diagnostic") for name, path in paths.items()
    }
    outcomes = {
        "manifest-contract": validate_manifest(reports["manifest"]),
        "sqlite-conformance": sqlite_matrix(reports["sqlite"])[1],
        "postgres-conformance": validate_postgres(reports["postgres"]),
        "migration-history": validate_migrations(
            reports["manifest"], reports["postgres"]
        ),
        "transfer": validate_transfer(reports["transfer"]),
        "restore": validate_restore(reports["sqlite"], reports["postgres-restore"]),
    }
    require(
        not args.output_dir.is_symlink(),
        "producer output directory must not be a symlink",
    )
    require(
        not args.artifact_dir.is_symlink(),
        "artifact output directory must not be a symlink",
    )
    args.output_dir.mkdir(parents=True, exist_ok=True)
    args.artifact_dir.mkdir(parents=True, exist_ok=True)
    for source_id, check_observations in outcomes.items():
        spec = ADAPTER.SOURCE_BY_ID[source_id]
        require(
            set(check_observations) == set(spec.checks),
            f"producer/collector check mapping drift for {source_id}",
        )
        inputs = [input_record(name, paths[name]) for name in SOURCE_INPUTS[source_id]]
        artifact = {
            "schema": "worldstream/release-conformance-artifact/v1",
            "source_id": source_id,
            "evidence_id": spec.evidence_id,
            "version": manifest["release_candidate"],
            "platform": spec.platform,
            "contract": manifest["contracts"],
            "status": "passed",
            "release_evidence": True,
            "validated_checks": list(spec.checks),
            "diagnostic_inputs": inputs,
        }
        artifact_path = args.artifact_dir / f"{source_id}.json"
        producer_path = args.output_dir / f"{source_id}.json"
        atomic_write(artifact_path, artifact)
        binding_id = ADAPTER.REQUIRED_ARTIFACT_BINDINGS[source_id][0]
        typed = {
            "schema": ADAPTER.PRODUCER_SCHEMA,
            "producer_id": f"conformance/{source_id}/v1",
            "evidence_id": spec.evidence_id,
            "status": "passed",
            "release_evidence": True,
            "fail_closed": False,
            "phase": ADAPTER.COLLECTOR.PRE_SIGN_PHASE,
            "version": manifest["release_candidate"],
            "platform": spec.platform,
            "contract": manifest["contracts"],
            "outcomes": {
                check: {
                    "status": "passed",
                    "observations": [
                        {"kind": "verified-diagnostic", "value": observation}
                    ],
                }
                for check, observation in check_observations.items()
            },
            "artifacts": {
                binding_id: {
                    "sha256": sha256(artifact_path),
                    "size_bytes": artifact_path.stat().st_size,
                }
            },
        }
        atomic_write(producer_path, typed)


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--output-dir", type=Path, required=True)
    command.add_argument("--artifact-dir", type=Path, required=True)
    command.add_argument("--manifest-report", type=Path, required=True)
    command.add_argument("--sqlite-report", type=Path, required=True)
    command.add_argument("--postgres-report", type=Path, required=True)
    command.add_argument("--transfer-report", type=Path, required=True)
    command.add_argument("--postgres-restore-report", type=Path, required=True)
    command.add_argument(
        "--manifest-toml", type=Path, default=ADAPTER.COLLECTOR.DEFAULT_MANIFEST_TOML
    )
    command.add_argument(
        "--manifest-json", type=Path, default=ADAPTER.COLLECTOR.DEFAULT_MANIFEST_JSON
    )
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        produce(args)
    except (EvidenceError, KeyError, TypeError, ValueError) as error:
        print(f"conformance evidence production failed: {error}", file=sys.stderr)
        return 1
    print("produced 6 verifier-backed conformance release producers")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

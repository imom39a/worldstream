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
import re
import stat
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import tomllib

ROOT = Path(__file__).resolve().parents[1]
ADAPTER_PATH = ROOT / "scripts/release-evidence-produce.py"
BLAKE3_IMPLEMENTATION_PATH = ROOT / "scripts/manifest-evidence-wave6.py"
SOURCE_INPUTS = {
    "manifest-contract": ("manifest",),
    "sqlite-conformance": ("sqlite",),
    "postgres-conformance": ("postgres",),
    "migration-history": ("manifest", "postgres"),
    "transfer": ("transfer",),
    "restore": ("sqlite", "postgres-restore"),
}
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
BLAKE3_DIGEST = re.compile(r"^[0-9a-f]{64}$")
ASCII_DECIMAL = re.compile(r"^[0-9]+$")
MAX_INPUT_BYTES = 64 * 1024 * 1024
MAX_POSTGRES_SYSTEM_IDENTIFIER = (1 << 64) - 1
MAX_POSTGRES_DATABASE_OID = (1 << 32) - 1
MAX_RESTORE_VERIFIER_ROOMS = 100_000
MAX_RESTORE_VERIFIER_DIAGNOSTICS = 256
RESTORE_SMOKE_FIELDS = {
    "schema",
    "status",
    "reason",
    "exit_code",
    "release_evidence",
    "native_restore",
    "live_restore_concurrency",
    "target_isolated",
    "target_published",
    "secrets_emitted",
}
LIVE_RESTORE_CONCURRENCY_FIELDS = {
    "observed",
    "target_connection_limit_during_restore",
    "target_client_backends_excluding_observer",
    "direct_superuser_keeper_backends",
    "one_use_restore_role_backends",
    "keeper_and_restore_pids_distinct",
    "final_captured_role_sessions_across_cluster",
    "final_captured_role_exists",
    "captured_restore_role",
    "captured_keeper_pid",
    "captured_restore_pid",
}
NATIVE_RESTORE_REPORT_FIELDS = {
    "schema",
    "status",
    "reason",
    "release_evidence",
    "native_dump_restore",
    "backup_id",
    "native_point_digest",
    "native_dump_digest",
    "native_dump_size_bytes",
    "source_provider_identity",
    "target_provider_identity",
    "verifier",
    "source_unchanged",
    "exact_restored_row_set",
    "snapshots_disposable",
    "target_isolated",
    "target_published",
    "cleanup_required",
    "native_witness_minted",
    "secrets_emitted",
    "source_version_num",
    "restored_version_num",
    "semantic_receipts_verified",
    "activation_intents_verified",
    "activation_operation_receipts_verified",
    "activation_request_evidence",
    "authority_state_verified",
    "durable_domains_verified",
    "verifier_scope",
    "source_durable_domains_digest",
    "restored_durable_domains_digest",
    "durable_domain_inventory",
    "restored_snapshot_count_before",
    "restored_snapshot_count_after",
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


def load_blake3():
    name = "worldstream_release_conformance_blake3"
    existing = sys.modules.get(name)
    if existing is not None:
        return existing.blake3
    spec = importlib.util.spec_from_file_location(name, BLAKE3_IMPLEMENTATION_PATH)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load BLAKE3 verifier: {BLAKE3_IMPLEMENTATION_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module.blake3


BLAKE3 = load_blake3()


def postgres_native_point_digest(dump_digest: str) -> str:
    point = {
        "PostgresNative": {
            "major": 17,
            "engine_identity": "postgresql-17.11",
            "point_id": dump_digest,
            "mechanism": "Dump",
        }
    }
    canonical = json.dumps(point, ensure_ascii=False, separators=(",", ":")).encode(
        "utf-8"
    )
    return BLAKE3(canonical).hex()


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


def _same_identity(left: os.stat_result, right: os.stat_result) -> bool:
    return left.st_dev == right.st_dev and left.st_ino == right.st_ino


def _read_descriptor(descriptor: int, maximum: int, label: str) -> bytes:
    os.lseek(descriptor, 0, os.SEEK_SET)
    chunks: list[bytes] = []
    observed = 0
    while True:
        chunk = os.read(descriptor, min(1024 * 1024, maximum + 1 - observed))
        if not chunk:
            break
        observed += len(chunk)
        require(observed <= maximum, f"{label} exceeds its byte bound")
        chunks.append(chunk)
    return b"".join(chunks)


@dataclass
class RetainedInput:
    """Bytes parsed and identified through one retained regular-file handle."""

    path: Path
    label: str
    descriptor: int
    admitted: os.stat_result
    raw: bytes
    sha256: str
    size_bytes: int

    @classmethod
    def open(cls, path: Path, label: str, *, maximum: int = MAX_INPUT_BYTES):
        path = Path(os.path.abspath(path))
        regular_file(path, label)
        admitted = path.lstat()
        descriptor = -1
        try:
            flags = os.O_RDONLY | getattr(os, "O_BINARY", 0)
            flags |= getattr(os, "O_NOFOLLOW", 0)
            descriptor = os.open(path, flags)
            opened = os.fstat(descriptor)
            require(
                stat.S_ISREG(opened.st_mode)
                and _same_identity(admitted, opened)
                and 0 < opened.st_size <= maximum,
                f"{label} changed during admission or exceeds its byte bound",
            )
            raw = _read_descriptor(descriptor, maximum, label)
            require(len(raw) == opened.st_size, f"{label} changed while it was read")
            retained = cls(
                path=path,
                label=label,
                descriptor=descriptor,
                admitted=opened,
                raw=raw,
                sha256="sha256:" + hashlib.sha256(raw).hexdigest(),
                size_bytes=len(raw),
            )
            retained.verify()
            return retained
        except BaseException:
            if descriptor >= 0:
                os.close(descriptor)
            raise

    def verify(self) -> None:
        try:
            opened = os.fstat(self.descriptor)
            named = self.path.lstat()
            observed = _read_descriptor(self.descriptor, MAX_INPUT_BYTES, self.label)
        except OSError as error:
            raise EvidenceError(f"cannot recheck {self.label}: {error}") from error
        require(
            stat.S_ISREG(opened.st_mode)
            and _same_identity(self.admitted, opened)
            and _same_identity(self.admitted, named)
            and opened.st_size == self.size_bytes
            and observed == self.raw,
            f"{self.label} changed while it was retained",
        )

    def json_object(self) -> dict[str, Any]:
        try:
            return ADAPTER.COLLECTOR.strict_json_object(self.raw, self.label)
        except ADAPTER.COLLECTOR.CollectionError as error:
            raise EvidenceError(str(error)) from error

    def close(self) -> None:
        descriptor, self.descriptor = self.descriptor, -1
        if descriptor >= 0:
            os.close(descriptor)


def read_json(path: Path, label: str) -> dict[str, Any]:
    retained = RetainedInput.open(path, label)
    try:
        return retained.json_object()
    finally:
        retained.close()


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def atomic_write(path: Path, value: object) -> tuple[str, int]:
    encoded = canonical_json(value)
    path.parent.mkdir(parents=True, exist_ok=True)
    require(not path.is_symlink() and not path.is_dir(), f"unsafe output path: {path}")
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(encoded)
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise
    return "sha256:" + hashlib.sha256(encoded).hexdigest(), len(encoded)


def _canonical_decimal(value: Any, maximum: int) -> bool:
    return (
        isinstance(value, str)
        and ASCII_DECIMAL.fullmatch(value) is not None
        and value == str(int(value))
        and 0 < int(value) <= maximum
    )


def provider_identity_is_canonical(identity: Any) -> bool:
    return (
        isinstance(identity, dict)
        and set(identity) == {"system_identifier", "database_oid", "database_name"}
        and _canonical_decimal(
            identity.get("system_identifier"), MAX_POSTGRES_SYSTEM_IDENTIFIER
        )
        and _canonical_decimal(identity.get("database_oid"), MAX_POSTGRES_DATABASE_OID)
        and isinstance(identity.get("database_name"), str)
        and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,62}", identity["database_name"])
        is not None
    )


def provider_numeric_identity(identity: dict[str, Any]) -> tuple[int, int]:
    return (int(identity["system_identifier"]), int(identity["database_oid"]))


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
        and type(report.get("exit_code")) is int
        and report["exit_code"] == 0
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
    require(
        report.get("provider_mode") == "docker"
        and report.get("provider_image")
        == {
            "reference": "postgres:17.11-alpine@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73",
            "repository_digest": "postgres@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73",
        },
        "transfer PostgreSQL provider is not the reviewed digest-pinned image",
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
        set(postgres_report) == RESTORE_SMOKE_FIELDS
        and postgres_report.get("schema")
        == "worldstream/native-postgres-restore-smoke-evidence/v1"
        and postgres_report.get("status") == "ready"
        and postgres_report.get("reason") == "native_postgres_restore_smoke_completed"
        and type(postgres_report.get("exit_code")) is int
        and postgres_report["exit_code"] == 0
        and postgres_report.get("release_evidence") is False
        and postgres_report.get("target_isolated") is True
        and postgres_report.get("target_published") is False
        and postgres_report.get("secrets_emitted") is False
        and isinstance(postgres_report.get("native_restore"), dict),
        "native PostgreSQL restore smoke wrapper is incomplete",
    )
    live_concurrency = postgres_report.get("live_restore_concurrency")
    require(
        isinstance(live_concurrency, dict)
        and set(live_concurrency) == LIVE_RESTORE_CONCURRENCY_FIELDS
        and live_concurrency.get("observed") is True
        and type(live_concurrency.get("target_connection_limit_during_restore")) is int
        and live_concurrency["target_connection_limit_during_restore"] == 2
        and type(live_concurrency.get("target_client_backends_excluding_observer"))
        is int
        and live_concurrency["target_client_backends_excluding_observer"] == 2
        and type(live_concurrency.get("direct_superuser_keeper_backends")) is int
        and live_concurrency["direct_superuser_keeper_backends"] == 1
        and type(live_concurrency.get("one_use_restore_role_backends")) is int
        and live_concurrency["one_use_restore_role_backends"] == 1
        and live_concurrency.get("keeper_and_restore_pids_distinct") is True
        and type(live_concurrency.get("final_captured_role_sessions_across_cluster"))
        is int
        and live_concurrency["final_captured_role_sessions_across_cluster"] == 0
        and type(live_concurrency.get("final_captured_role_exists")) is int
        and live_concurrency["final_captured_role_exists"] == 0
        and isinstance(live_concurrency.get("captured_restore_role"), str)
        and re.fullmatch(
            r"worldstream_restore_[0-9a-f]{32}",
            live_concurrency["captured_restore_role"],
        )
        is not None
        and type(live_concurrency.get("captured_keeper_pid")) is int
        and live_concurrency["captured_keeper_pid"] > 0
        and type(live_concurrency.get("captured_restore_pid")) is int
        and live_concurrency["captured_restore_pid"] > 0
        and live_concurrency["captured_keeper_pid"]
        != live_concurrency["captured_restore_pid"],
        "native PostgreSQL restore live one-use credential proof is incomplete",
    )
    postgres_report = postgres_report["native_restore"]
    require(
        set(postgres_report) == NATIVE_RESTORE_REPORT_FIELDS,
        "native PostgreSQL restore report has wrong fields",
    )
    provider_identities = (
        postgres_report.get("source_provider_identity"),
        postgres_report.get("target_provider_identity"),
    )

    require(
        postgres_report.get("schema")
        == "worldstream/native-postgres-restore-evidence/v2"
        and postgres_report.get("status") == "ready"
        and postgres_report.get("reason")
        == "postgres_native_restore_verified_by_unified_verifier"
        and postgres_report.get("release_evidence") is False
        and postgres_report.get("native_dump_restore") == "pass"
        and isinstance(postgres_report.get("native_dump_digest"), str)
        and BLAKE3_DIGEST.fullmatch(postgres_report["native_dump_digest"]) is not None
        and postgres_report.get("backup_id")
        == "postgres-native-" + postgres_report["native_dump_digest"]
        and isinstance(postgres_report.get("native_point_digest"), str)
        and BLAKE3_DIGEST.fullmatch(postgres_report["native_point_digest"]) is not None
        and postgres_report["native_point_digest"]
        == postgres_native_point_digest(postgres_report["native_dump_digest"])
        and type(postgres_report.get("native_dump_size_bytes")) is int
        and postgres_report["native_dump_size_bytes"] > 0
        and postgres_report.get("source_unchanged") is True
        and postgres_report.get("exact_restored_row_set") is True
        and postgres_report.get("snapshots_disposable") is True
        and postgres_report.get("target_isolated") is True
        and postgres_report.get("target_published") is False
        and postgres_report.get("cleanup_required") is True
        and postgres_report.get("native_witness_minted") is True
        and postgres_report.get("secrets_emitted") is False
        and all(provider_identity_is_canonical(value) for value in provider_identities)
        and provider_numeric_identity(provider_identities[0])
        != provider_numeric_identity(provider_identities[1])
        and postgres_report.get("source_version_num") == 170_011
        and postgres_report.get("restored_version_num") == 170_011
        and postgres_report.get("semantic_receipts_verified") is True
        and postgres_report.get("activation_intents_verified") is True
        and postgres_report.get("activation_operation_receipts_verified") is True
        and postgres_report.get("activation_request_evidence")
        == "stored_canonical_hash_only_verified"
        and postgres_report.get("authority_state_verified") is True
        and postgres_report.get("durable_domains_verified") is True
        and type(postgres_report.get("restored_snapshot_count_before")) is int
        and postgres_report["restored_snapshot_count_before"] > 0
        and type(postgres_report.get("restored_snapshot_count_after")) is int
        and postgres_report["restored_snapshot_count_after"] == 0,
        "native PostgreSQL restore did not satisfy the isolated semantic contract",
    )
    verifier_scope = postgres_report.get("verifier_scope")
    require(
        isinstance(verifier_scope, dict)
        and set(verifier_scope)
        == {
            "profile",
            "source_pack_identity_count",
            "restored_pack_identity_count",
            "source_resource_identity_count",
            "restored_resource_identity_count",
            "source_fired_timer_count",
            "restored_fired_timer_count",
            "general_deployment_support_verified",
        }
        and verifier_scope.get("profile") == "full_deployment_all_durable_domains"
        and all(
            type(verifier_scope.get(field)) is int
            for field in (
                "source_pack_identity_count",
                "restored_pack_identity_count",
                "source_resource_identity_count",
                "restored_resource_identity_count",
                "source_fired_timer_count",
                "restored_fired_timer_count",
            )
        )
        and verifier_scope.get("source_pack_identity_count") == 2
        and verifier_scope.get("restored_pack_identity_count") == 2
        and verifier_scope.get("source_resource_identity_count") == 1
        and verifier_scope.get("restored_resource_identity_count") == 1
        and verifier_scope.get("source_fired_timer_count") == 1
        and verifier_scope.get("restored_fired_timer_count") == 1
        and verifier_scope.get("general_deployment_support_verified") is True,
        "native PostgreSQL restore verifier scope is missing or overclaims general deployment support",
    )
    source_digest = postgres_report.get("source_durable_domains_digest")
    restored_digest = postgres_report.get("restored_durable_domains_digest")
    require(
        isinstance(source_digest, str)
        and BLAKE3_DIGEST.fullmatch(source_digest) is not None
        and source_digest == restored_digest,
        "native PostgreSQL restore durable-domain aggregate digest mismatch",
    )
    inventory = postgres_report.get("durable_domain_inventory")
    require(
        isinstance(inventory, list) and len(inventory) == len(RESTORE_DURABLE_DOMAINS),
        "native PostgreSQL restore durable-domain inventory is incomplete",
    )
    observed_domains: list[str] = []
    for row in inventory:
        require(
            isinstance(row, dict)
            and set(row)
            == {
                "domain",
                "source_row_count",
                "restored_row_count",
                "source_digest",
                "restored_digest",
            },
            "native PostgreSQL restore durable-domain row is malformed",
        )
        domain = row.get("domain")
        source_count = row.get("source_row_count")
        restored_count = row.get("restored_row_count")
        row_source_digest = row.get("source_digest")
        row_restored_digest = row.get("restored_digest")
        require(
            isinstance(domain, str)
            and type(source_count) is int
            and source_count >= 0
            and type(restored_count) is int
            and source_count == restored_count
            and isinstance(row_source_digest, str)
            and BLAKE3_DIGEST.fullmatch(row_source_digest) is not None
            and row_source_digest == row_restored_digest,
            "native PostgreSQL restore durable-domain count or digest mismatch",
        )
        observed_domains.append(domain)
    require(
        observed_domains == list(RESTORE_DURABLE_DOMAINS),
        "native PostgreSQL restore durable-domain inventory drifted",
    )
    authority_state = inventory[RESTORE_DURABLE_DOMAINS.index("authority_state")]
    require(
        authority_state.get("source_row_count") == 1,
        "native PostgreSQL restore authority singleton evidence is incomplete",
    )
    verifier = postgres_report.get("verifier")
    require(
        isinstance(verifier, dict)
        and set(verifier) == {"readiness", "rooms", "diagnostics"}
        and verifier.get("readiness") == "Ready",
        "provider-neutral restore verifier did not return ready",
    )
    rooms = verifier.get("rooms")
    require(
        isinstance(rooms, dict)
        and 0 < len(rooms) <= MAX_RESTORE_VERIFIER_ROOMS
        and all(
            isinstance(room_id, str)
            and 0 < len(room_id) <= 512
            and disposition in {"Verified", "IsolatedPreExisting"}
            for room_id, disposition in rooms.items()
        ),
        "provider-neutral restore verifier rooms are malformed, blocked, or unbounded",
    )
    diagnostics = verifier.get("diagnostics")
    require(
        isinstance(diagnostics, list)
        and len(diagnostics) <= MAX_RESTORE_VERIFIER_DIAGNOSTICS,
        "provider-neutral restore verifier diagnostics are malformed or unbounded",
    )
    for diagnostic in diagnostics:
        require(
            isinstance(diagnostic, dict)
            and set(diagnostic) == {"class", "code", "subject", "action", "disposition"}
            and diagnostic.get("class") == "Integrity"
            and diagnostic.get("code") == "pre_existing_isolation_preserved"
            and isinstance(diagnostic.get("subject"), str)
            and re.fullmatch(r"subject:[0-9a-f]{12}", diagnostic["subject"]) is not None
            and diagnostic.get("action")
            == "keep the Room isolated; investigate or repair it through the separate verifier-repair contract"
            and diagnostic.get("disposition") == "PermittedPreExistingIsolation",
            "provider-neutral restore verifier contains malformed or blocking diagnostics",
        )
    return {
        "sqlite_isolated_restore": "complete SQLite matrix included exact native envelope restore",
        "postgresql_isolated_restore": "native PostgreSQL dump/restore was isolated and exact",
        "bounded_fixture_semantic_verifier": "provider-neutral verifier ready for the bounded full-deployment fixture with two Pack identities, one immutable resource, one fired Timer, exact Activation/authority/transfer-fence domains, and complete durable-domain support",
    }


def input_record(name: str, retained: RetainedInput) -> dict[str, Any]:
    return {
        "id": name,
        "sha256": retained.sha256,
        "size_bytes": retained.size_bytes,
    }


def retained_manifest(
    authored_input: RetainedInput, mirror_input: RetainedInput
) -> dict[str, Any]:
    require(
        authored_input.path.name == "compatibility.toml"
        and mirror_input.path.name == "compatibility.json",
        "collector requires compatibility.toml and compatibility.json filenames",
    )
    try:
        authored = tomllib.loads(authored_input.raw.decode("utf-8"))
    except (UnicodeError, tomllib.TOMLDecodeError) as error:
        raise EvidenceError(
            f"cannot read compatibility manifest pair: {error}"
        ) from error
    mirror = mirror_input.json_object()
    require(
        isinstance(authored, dict)
        and authored == mirror
        and mirror_input.raw == ADAPTER.COLLECTOR.canonical_json(authored),
        "compatibility.toml and compatibility.json differ semantically or canonically",
    )
    return authored


def produce(args: argparse.Namespace) -> None:
    retained: dict[str, RetainedInput] = {}
    manifest_inputs: list[RetainedInput] = []
    try:
        retained = {
            "manifest": RetainedInput.open(args.manifest_report, "manifest diagnostic"),
            "sqlite": RetainedInput.open(args.sqlite_report, "SQLite diagnostic"),
            "postgres": RetainedInput.open(
                args.postgres_report, "PostgreSQL diagnostic"
            ),
            "transfer": RetainedInput.open(args.transfer_report, "transfer diagnostic"),
            "postgres-restore": RetainedInput.open(
                args.postgres_restore_report, "PostgreSQL restore diagnostic"
            ),
        }
        manifest_inputs = [
            RetainedInput.open(args.manifest_toml, "compatibility.toml"),
            RetainedInput.open(args.manifest_json, "compatibility.json"),
        ]
        all_inputs = [*retained.values(), *manifest_inputs]
        require(
            len({(item.admitted.st_dev, item.admitted.st_ino) for item in all_inputs})
            == len(all_inputs),
            "conformance inputs must have distinct retained file identities",
        )
        manifest = retained_manifest(*manifest_inputs)
        reports = {name: value.json_object() for name, value in retained.items()}
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
        for item in all_inputs:
            item.verify()
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
            inputs = [
                input_record(name, retained[name]) for name in SOURCE_INPUTS[source_id]
            ]
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
            artifact_sha256, artifact_size = atomic_write(artifact_path, artifact)
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
                        "sha256": artifact_sha256,
                        "size_bytes": artifact_size,
                    }
                },
            }
            atomic_write(producer_path, typed)
    finally:
        for item in [*retained.values(), *manifest_inputs]:
            item.close()


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

"""Focused validation for typed hosted platform diagnostic emitters."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/release-platform-diagnostic.py"
PLATFORM_PRODUCER = ROOT / "scripts/release-evidence-produce-platform.py"
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
        "worldstream_release_platform_diagnostic", SCRIPT
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def load_platform_producer():
    spec = importlib.util.spec_from_file_location(
        "worldstream_release_evidence_produce_platform", PLATFORM_PRODUCER
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_postgres_native_point_digest_matches_the_frozen_rust_shape() -> None:
    module = load_module()
    assert module.postgres_native_point_digest("b" * 64) == (
        "d12ef884fee4573a111d13e5adbb2b2e05683f65f7f219bc0019c9b9c8a899dc"
    )


def test_generic_json_input_is_retained_and_bounded(tmp_path, monkeypatch) -> None:
    module = load_module()
    oversized = tmp_path / "oversized.json"
    oversized.write_bytes(b'{"value":true}\n')
    monkeypatch.setattr(module, "MAX_RELEASE_JSON_BYTES", 8)

    with pytest.raises(
        module.DiagnosticError, match="invalid or unexpected byte length"
    ):
        module.read_json(oversized, "oversized report")


def test_generic_digest_streams_from_one_retained_file(tmp_path, monkeypatch) -> None:
    module = load_module()
    artifact = tmp_path / "artifact.tar"
    content = b"streamed release artifact" * 1024
    artifact.write_bytes(content)
    monkeypatch.setattr(
        Path,
        "read_bytes",
        lambda _self: (_ for _ in ()).throw(
            AssertionError("release artifact digest must not use Path.read_bytes")
        ),
    )

    assert module.digest(artifact) == ("sha256:" + hashlib.sha256(content).hexdigest())


def test_cleanup_only_failure_is_translated_to_diagnostic_error(
    tmp_path, monkeypatch
) -> None:
    module = load_module()
    report = write_json(tmp_path / "report.json", {"status": "pass"})
    actual_close = module.RetainedJsonInput.close

    def close_then_fail(retained) -> None:
        actual_close(retained)
        raise OSError("injected close failure")

    monkeypatch.setattr(module.RetainedJsonInput, "close", close_then_fail)

    with pytest.raises(module.DiagnosticError, match="cleanup failed closed"):
        module.read_json(report, "test report")


def write_json(path: Path, value: object) -> Path:
    path.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    return path


def gate_report(system: str, outcomes: set[str]) -> dict:
    return {
        "schema": "worldstream/compatibility-gate-report/v1",
        "platform": f"{system}-hosted-runner",
        "tier": "minimal-ci",
        "strict": True,
        "outcomes": [
            {
                "name": name,
                "status": "PASS",
                "detail": f"native-{system.lower()} exact typed gate passed",
                "classification": "pass",
            }
            for name in sorted(outcomes)
        ],
    }


def windows_native_restore_report(module, dump_bytes: bytes = b"d" * 4096) -> dict:
    dump_digest = module.BLAKE3(dump_bytes).hex()
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
        "backup_id": "postgres-native-" + dump_digest,
        "native_point_digest": module.postgres_native_point_digest(dump_digest),
        "native_dump_digest": dump_digest,
        "native_dump_size_bytes": len(dump_bytes),
        "source_provider_identity": {
            "system_identifier": "123456789",
            "database_oid": "5",
            "database_name": "postgres",
        },
        "target_provider_identity": {
            "system_identifier": "123456789",
            "database_oid": "16384",
            "database_name": "worldstream_native_restore",
        },
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
        "source_unchanged": True,
        "exact_restored_row_set": True,
        "snapshots_disposable": True,
        "target_isolated": True,
        "target_published": False,
        "cleanup_required": True,
        "native_witness_minted": True,
        "secrets_emitted": False,
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
        "restored_snapshot_count_before": 4,
        "restored_snapshot_count_after": 0,
    }


def native_args(tmp_path: Path, module, source: str) -> argparse.Namespace:
    tmp_path.mkdir(parents=True, exist_ok=True)
    system = "Linux" if source == "native-linux" else "Windows"
    target = "linux-x86_64" if source == "native-linux" else "windows-x64"
    version = module.manifest()["release_candidate"]
    artifact = tmp_path / f"worldstream-{version}-{target}.archive"
    artifact.write_bytes(b"exact packaged native archive")
    manifest_json_sha = "1" * 64
    manifest_toml_sha = "2" * 64
    source_revision = "5" * 40
    build_identity_sha256 = "sha256:" + "6" * 64
    package_report = {
        "schema": "worldstream/package-report/v1",
        "kind": "archive",
        "artifact": artifact.name,
        "path": artifact.name,
        "sha256": module.digest(artifact),
        "size_bytes": artifact.stat().st_size,
        "identity": {
            "target": target,
            "version": version,
            "manifest_sha256": manifest_json_sha,
            "manifest_json_sha256": manifest_json_sha,
            "manifest_toml_sha256": manifest_toml_sha,
            "source_revision": source_revision,
            "build_identity_sha256": build_identity_sha256,
        },
        "inventory": {
            "archive_verified": True,
            "manifest_source": "compatibility.toml",
            "manifest_mirror": "compatibility.json",
            "release_evidence": False,
        },
    }
    package_report_path = write_json(tmp_path / "package-report.json", package_report)
    runtime_required = {
        "source-version-drift",
        "compatibility-manifest-verify",
        "rust-tests",
        "sqlite-critical-matrix",
        "postgres-local",
        "evidence-backup-restore-local",
        "evidence-privacy-local",
        "evidence-capability-local",
        "evidence-lease-local",
        "evidence-telemetry-local",
        "filesystem-owner-only",
        "evidence-filesystem-local",
        "ci-cell",
    }
    if source == "native-linux":
        runtime_required |= {
            "postgres-live-contract",
            "process-kill-point",
            "telemetry-failure-pressure",
            "bounded-sqlite-soak",
            "filesystem-link-policy",
        }
    else:
        runtime_required |= {"filesystem-acl-policy"}
    runtime_report = {
        "schema": "worldstream/native-package-runtime-smoke/v1",
        "status": "pass",
        "release_evidence": False,
        "secrets_emitted": False,
        "platform": {"system": system, "machine": "x86_64"},
        "package_binding": {
            "artifact": artifact.name,
            "target": target,
            "version": version,
            "archive_sha256": module.digest(artifact),
            "archive_size_bytes": artifact.stat().st_size,
            "package_report_sha256": module.digest(package_report_path),
            "package_report_size_bytes": package_report_path.stat().st_size,
            "manifest_sha256": "sha256:" + manifest_json_sha,
            "manifest_json_sha256": "sha256:" + manifest_json_sha,
            "manifest_toml_sha256": "sha256:" + manifest_toml_sha,
            "source_revision": source_revision,
            "build_identity_sha256": build_identity_sha256,
            "worldstreamd_sha256": "sha256:" + "3" * 64,
            "worldstreamd_size_bytes": 100,
            "worldstreamctl_sha256": "sha256:" + "4" * 64,
            "worldstreamctl_size_bytes": 101,
            "canonical_archive_verified": True,
            "exact_archive_bytes_executed": True,
        },
        "profiles": {
            "sqlite-bundled": {
                "status": "pass",
                "healthz": "pass",
                "readyz": "pass",
                "version": "manifest_and_engine_exact",
                "engine_identity": "sqlite/3.53.4; journal_mode=WAL; synchronous=FULL",
                "packaged_ctl": {
                    "config_validate": "pass",
                    "config_effective": "redacted_and_precedence_exact",
                    "doctor": "bounded_diagnostics_exposed",
                    "health": "pass",
                    "version": "pass",
                    "binary_version": version,
                    "source_revision": source_revision,
                },
                "sqlite_operator": {
                    "backup": "native_and_semantic_pass",
                    "restore": "native_and_semantic_pass",
                    "verify": "native_only_pass_semantic_not_invoked",
                    "envelope": "exact_bytes_preserved",
                },
            },
            "postgres-primary": {
                "status": "pass",
                "healthz": "pass",
                "readyz": "pass",
                "version": "manifest_and_engine_exact",
                "engine_identity": "postgresql/17.11; server_version_num=170011",
                "packaged_ctl": {
                    "config_validate": "pass",
                    "config_effective": "redacted_and_precedence_exact",
                    "doctor": "bounded_diagnostics_exposed",
                    "health": "pass",
                    "version": "pass",
                    "binary_version": version,
                    "source_revision": source_revision,
                },
            },
        },
        "postgres_admin": {
            "server_version_num": "170011",
            "dsn_delivery": "owner_only_file",
            "migrate": "pass",
            "verify": "pass",
            "runtime_role": "least_privilege",
        },
        "transfer_operator": {
            "status": "pass",
            "control_binding": {
                "archive_sha256": module.digest(artifact),
                "archive_size_bytes": artifact.stat().st_size,
                "package_report_sha256": module.digest(package_report_path),
                "package_report_size_bytes": package_report_path.stat().st_size,
                "worldstreamctl_sha256": "sha256:" + "4" * 64,
                "worldstreamctl_size_bytes": 101,
            },
            "abort": {
                "restart_checkpoint": "pass",
                "pre_abort_checkpoint_rows": 1,
                "pre_abort_checkpoint_bytes": 128,
                "provider_cleanup": "all_durable_domains_empty_with_exact_tombstone",
                "source_authority_restored": "pass",
                "abort_replay": "pass",
                "bundle_sha256": "sha256:" + "7" * 64,
                "bundle_size_bytes": 512,
            },
            "finalized": {
                "restart_resume": "pass",
                "resume_invocations": 2,
                "completed_resume_replay": "no_new_generation",
                "canonical_bundle_sha256": "sha256:" + "8" * 64,
                "canonical_bundle_size_bytes": 1024,
                "operator_bundle_hash": "9" * 64,
                "record_count": 3,
                "target_epoch": 2,
                "hydration_verified_before_handoff": "pass",
                "source_retired": "pass",
                "target_authoritative": "pass",
                "finalize_replay": "pass",
            },
        },
        "control_output": {
            "status": "persisted",
            "archive_sha256": module.digest(artifact),
            "archive_size_bytes": artifact.stat().st_size,
            "package_report_sha256": module.digest(package_report_path),
            "package_report_size_bytes": package_report_path.stat().st_size,
            "worldstreamctl_sha256": "sha256:" + "4" * 64,
            "worldstreamctl_size_bytes": 101,
        },
        "cleanup": "pass",
    }
    runtime_report_path = write_json(tmp_path / "runtime.json", runtime_report)
    fixture_report = {
        "schema": "worldstream/sqlite-postgresql-transfer-evidence/v1",
        "status": "pass",
        "release_evidence": False,
        "secrets_emitted": False,
        "source_construction": {
            "classification": module.UNTRUSTED_FIXTURE_CLASSIFICATION,
            "source_revision": source_revision,
            "trusted_product_execution": False,
        },
    }
    fixture_report_path = write_json(
        tmp_path / "native-source-fixture.json", fixture_report
    )
    dump_bytes = b"d" * 4096
    native_restore = windows_native_restore_report(module, dump_bytes)
    raw_native_bytes = (json.dumps(native_restore, sort_keys=True) + "\n").encode(
        "utf-8"
    )
    canonical_native = json.dumps(
        native_restore, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    empty_sha256 = "sha256:" + hashlib.sha256(b"").hexdigest()
    directory_receipt = {
        "schema": "worldstream/postgres-native-artifact-directory-identity/v1",
        "status": "observed",
        "artifact_directory_identity": {
            "storage_id": "0000000000000001",
            "file_id": "00000000000000000000000000000002",
        },
        "secrets_emitted": False,
    }
    snapshot_receipt = {
        "schema": "worldstream/postgres-native-snapshot-rebuild-receipt/v1",
        "status": "complete",
        "rebuilt_snapshot_count": 2,
        "source_provider_identity": native_restore["source_provider_identity"],
        "secrets_emitted": False,
    }
    restore_receipt = {
        "schema": "worldstream/postgres-native-restore-receipt/v1",
        "status": "committed",
        "report_digest": "blake3:" + module.BLAKE3(raw_native_bytes).hex(),
        "report_size_bytes": len(raw_native_bytes),
        "native_dump_digest": native_restore["native_dump_digest"],
        "native_dump_size_bytes": native_restore["native_dump_size_bytes"],
        "native_dump_identity": {
            "storage_id": "0000000000000001",
            "file_id": "00000000000000000000000000000003",
        },
        "report_identity": {
            "storage_id": "0000000000000001",
            "file_id": "00000000000000000000000000000004",
        },
        "recovery_record_identity": {
            "storage_id": "0000000000000001",
            "file_id": "00000000000000000000000000000005",
        },
        "recovery_record_name": (".worldstream_native_recovery_" + "a" * 32 + ".json"),
        "source_provider_identity": native_restore["source_provider_identity"],
        "target_provider_identity": native_restore["target_provider_identity"],
        "secrets_emitted": False,
    }
    action_receipts = (directory_receipt, snapshot_receipt, restore_receipt)
    action_stdout = [
        (json.dumps(receipt, sort_keys=True) + "\n").encode("utf-8")
        for receipt in action_receipts
    ]
    hosted_restore = {
        "schema": module.HOSTED_NATIVE_RESTORE_SCHEMA,
        "status": "pass",
        "release_evidence": False,
        "secrets_emitted": False,
        "platform": {"system": system, "machine": "x86_64"},
        "package_binding": {
            key: runtime_report["package_binding"][key]
            for key in (
                "archive_sha256",
                "archive_size_bytes",
                "package_report_sha256",
                "package_report_size_bytes",
                "source_revision",
                "worldstreamctl_sha256",
                "worldstreamctl_size_bytes",
            )
        },
        "source_fixture": {
            "classification": module.UNTRUSTED_FIXTURE_CLASSIFICATION,
            "fixture_report_sha256": module.digest(fixture_report_path),
            "fixture_report_size_bytes": fixture_report_path.stat().st_size,
            "source_revision": source_revision,
        },
        "product_execution": {
            "controller": {
                "execution": "retained_exact_packaged_binary",
                "sha256": runtime_report["package_binding"]["worldstreamctl_sha256"],
                "size_bytes": runtime_report["package_binding"][
                    "worldstreamctl_size_bytes"
                ],
            },
            "provider_tools": {
                tool: {"sha256": "sha256:" + character * 64, "size_bytes": size}
                for tool, character, size in (
                    ("pg_dump", "5", 201),
                    ("pg_restore", "6", 202),
                    ("psql", "7", 203),
                )
            },
            "source": {
                "host": "127.0.0.1",
                "port": 5432,
                "database": "postgres",
                "username": "postgres",
                "tls_mode": "disable",
            },
            "target": {
                "host": "127.0.0.1",
                "port": 5432,
                "database": "worldstream_native_restore",
                "username": "postgres",
                "tls_mode": "disable",
            },
            "actions": [
                {
                    "operation": "artifact_directory_identity",
                    "status": "pass",
                    "exit_code": 0,
                    "stdout_sha256": "sha256:"
                    + hashlib.sha256(action_stdout[0]).hexdigest(),
                    "stdout_size_bytes": len(action_stdout[0]),
                    "stderr_sha256": empty_sha256,
                    "stderr_size_bytes": 0,
                    "environment": "sanitized_no_ambient_pg",
                    "receipt": directory_receipt,
                },
                {
                    "operation": "snapshot_rebuild",
                    "status": "pass",
                    "exit_code": 0,
                    "stdout_sha256": "sha256:"
                    + hashlib.sha256(action_stdout[1]).hexdigest(),
                    "stdout_size_bytes": len(action_stdout[1]),
                    "stderr_sha256": empty_sha256,
                    "stderr_size_bytes": 0,
                    "environment": "sanitized_no_ambient_pg",
                    "receipt": snapshot_receipt,
                },
                {
                    "operation": "native_restore",
                    "status": "pass",
                    "exit_code": 0,
                    "stdout_sha256": "sha256:"
                    + hashlib.sha256(action_stdout[2]).hexdigest(),
                    "stdout_size_bytes": len(action_stdout[2]),
                    "stderr_sha256": empty_sha256,
                    "stderr_size_bytes": 0,
                    "environment": "sanitized_no_ambient_pg",
                    "receipt": restore_receipt,
                },
            ],
            "native_report_sha256": "sha256:"
            + hashlib.sha256(raw_native_bytes).hexdigest(),
            "native_report_size_bytes": len(raw_native_bytes),
            "native_report_canonical_sha256": "sha256:"
            + hashlib.sha256(canonical_native).hexdigest(),
        },
        "native_restore": native_restore,
        "cleanup": {
            "status": "pass",
            "admitted_target_identity": {
                "system_identifier": "123456789",
                "database_oid": "16384",
                "database_name": "worldstream_native_restore",
            },
            "target_marker_before_drop": "worldstream/native-postgres-disposable-target/v1",
            "target_connection_limit_before_drop": 0,
            "target_backends_before_drop": 0,
            "generated_restore_roles_before_drop": 0,
            "target_database_after_drop": "absent",
            "provider_cleanup_transcript_sha256": "sha256:" + "b" * 64,
            "private_artifacts_disposition": (
                "exact_retained_root_scrubbed_to_zero_length_placeholders"
            ),
            "private_artifact_placeholder_count": 3,
            "operator_passfile_disposition": (
                "exact_retained_file_scrubbed_to_zero_length"
            ),
        },
    }
    binding_parent = tmp_path / "native-binding-parent"
    binding_parent.mkdir(mode=0o700)
    binding_root = binding_parent / module.PRIVATE_NATIVE_BINDING_ROOT
    binding_root.mkdir(mode=0o700)
    binding_values = {
        "artifact_directory_stdout": action_stdout[0],
        "snapshot_rebuild_stdout": action_stdout[1],
        "native_restore_stdout": action_stdout[2],
        "native_report": raw_native_bytes,
        "native_dump": dump_bytes,
    }
    binding_paths = {}
    for key, value in binding_values.items():
        path = binding_root / module.PRIVATE_NATIVE_BINDING_FILES[key]
        path.write_bytes(value)
        path.chmod(0o600)
        binding_paths[key] = path

    def exact_identity(path: Path, *, directory: bool = False) -> dict[str, str]:
        opener = (
            module.NATIVE_AUTHORITY._open_exact_directory
            if directory
            else module.NATIVE_AUTHORITY._open_exact
        )
        descriptor, _metadata = opener(path, "test private binding")
        try:
            return module.NATIVE_AUTHORITY._canonical_file_identity(descriptor)
        finally:
            os.close(descriptor)

    hosted_restore["private_binding"] = {
        "schema": module.PRIVATE_NATIVE_BINDING_SCHEMA,
        "parent_identity": exact_identity(binding_parent, directory=True),
        "root_name": module.PRIVATE_NATIVE_BINDING_ROOT,
        "root_identity": exact_identity(binding_root, directory=True),
        "files": {
            key: {
                "name": path.name,
                "identity": exact_identity(path),
                "sha256": "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest(),
                "size_bytes": path.stat().st_size,
            }
            for key, path in binding_paths.items()
        },
    }
    native_restore_report_path = write_json(
        tmp_path / f"{source}-postgres-restore.json", hosted_restore
    )
    return argparse.Namespace(
        source=source,
        package_report=package_report_path,
        gate_report=write_json(
            tmp_path / "gate.json", gate_report(system, runtime_required)
        ),
        runtime_report=runtime_report_path,
        native_restore_report=native_restore_report_path,
        native_fixture_report=fixture_report_path,
        native_binding_parent=binding_parent,
        artifact=artifact,
        output_dir=tmp_path / "diagnostics",
    )


def private_binding_path(args, module, key: str) -> Path:
    return (
        args.native_binding_parent
        / module.PRIVATE_NATIVE_BINDING_ROOT
        / module.PRIVATE_NATIVE_BINDING_FILES[key]
    )


def assert_private_binding_scrubbed(args, module) -> None:
    root = args.native_binding_parent / module.PRIVATE_NATIVE_BINDING_ROOT
    children = list(root.iterdir())
    assert len(children) == len(module.PRIVATE_NATIVE_BINDING_FILES)
    assert all(path.is_file() and not path.is_symlink() for path in children)
    assert all(path.stat().st_size == 0 for path in children)


def rewrite_private_json_binding(args, module, report, key: str, value: dict) -> None:
    raw = (json.dumps(value, sort_keys=True) + "\n").encode("utf-8")
    path = private_binding_path(args, module, key)
    path.write_bytes(raw)
    record = report["private_binding"]["files"][key]
    record["sha256"] = "sha256:" + hashlib.sha256(raw).hexdigest()
    record["size_bytes"] = len(raw)


def stub_native_archive_verification(monkeypatch, module, args):
    package_report = json.loads(args.package_report.read_text(encoding="utf-8"))
    runtime_report = json.loads(args.runtime_report.read_text(encoding="utf-8"))
    binding = runtime_report["package_binding"]
    archive_binaries = {
        key: binding[key]
        for key in (
            "worldstreamd_sha256",
            "worldstreamd_size_bytes",
            "worldstreamctl_sha256",
            "worldstreamctl_size_bytes",
        )
    }
    monkeypatch.setattr(
        module,
        "independently_verify_native_archive",
        lambda artifact, source_id, version: (package_report, archive_binaries),
    )


@pytest.mark.parametrize("source", ["native-linux", "native-windows"])
def test_native_diagnostic_requires_exact_packaged_binary_runtime_profiles(
    tmp_path, monkeypatch, source
):
    module = load_module()
    args = native_args(tmp_path, module, source)
    stub_native_archive_verification(monkeypatch, module, args)

    module.emit_native(args)
    assert_private_binding_scrubbed(args, module)

    runtime = json.loads(
        (args.output_dir / f"{source}-runtime.json").read_text(encoding="utf-8")
    )
    assert runtime["facts"]["runtime_report_sha256"] == module.digest(
        args.runtime_report
    )
    assert runtime["facts"]["packaged_binary_sha256"] == "sha256:" + "3" * 64
    assert runtime["facts"]["packaged_control_sha256"] == "sha256:" + "4" * 64
    assert "migrate/verify" in runtime["facts"]["postgres_admin"]
    restore = json.loads(
        (args.output_dir / f"{source}-native-postgres-restore.json").read_text(
            encoding="utf-8"
        )
    )
    assert restore["facts"]["native_restore_report_sha256"] == module.digest(
        args.native_restore_report
    )
    assert restore["facts"]["release_archive_sha256"] == module.digest(args.artifact)
    assert restore["facts"]["restore_profile"] == (
        "full_deployment_all_durable_domains"
    )
    assert restore["facts"]["fixture_report_sha256"] == module.digest(
        args.native_fixture_report
    )
    expected_native_restore_facts = (
        "platform_identity",
        "release_candidate",
        "native_restore_report_sha256",
        "native_restore_report_size_bytes",
        "release_archive_sha256",
        "release_archive_size_bytes",
        "package_report_sha256",
        "package_report_size_bytes",
        "packaged_runtime_report_sha256",
        "packaged_runtime_report_size_bytes",
        "provider_identity",
        "restore_profile",
        "verified_scope",
        "target_safety",
        "source_durable_domains_digest",
        "backup_id",
        "native_point_digest",
        "native_dump_digest",
        "native_dump_size_bytes",
        "source_provider_identity",
        "target_provider_identity",
        "fixture_classification",
        "fixture_report_sha256",
        "fixture_report_size_bytes",
        "source_revision",
        "packaged_control_sha256",
        "packaged_control_size_bytes",
        "native_raw_report_sha256",
        "native_raw_report_size_bytes",
        "native_product_execution_sha256",
        "native_provider_tools_sha256",
        "native_cleanup_sha256",
    )
    producer = load_platform_producer()
    assert "native-postgres-restore" in producer.PLATFORM_SPECS[source]["reports"]
    assert (
        producer.REQUIRED_FACTS[(source, "native-postgres-restore")]
        == expected_native_restore_facts
    )
    assert set(restore["facts"]) == set(expected_native_restore_facts)
    producer_manifest = producer.load_manifest(
        ROOT / "compatibility.toml", ROOT / "compatibility.json"
    )
    assert (
        producer.read_report(
            args.output_dir / f"{source}-native-postgres-restore.json",
            source,
            "native-postgres-restore",
            producer_manifest,
        )
        == restore
    )


def test_windows_native_diagnostic_requires_restore_report_and_rejects_substitution(
    tmp_path, monkeypatch
):
    module = load_module()
    args = native_args(tmp_path, module, "native-windows")
    stub_native_archive_verification(monkeypatch, module, args)
    args.native_restore_report = None
    with pytest.raises(module.DiagnosticError, match="missing hosted Windows"):
        module.emit_native(args)

    args = native_args(tmp_path / "missing-fixture", module, "native-windows")
    stub_native_archive_verification(monkeypatch, module, args)
    args.native_fixture_report = None
    with pytest.raises(module.DiagnosticError, match="missing hosted Windows"):
        module.emit_native(args)

    args = native_args(tmp_path / "substitution", module, "native-windows")
    stub_native_archive_verification(monkeypatch, module, args)
    args.native_restore_report = args.runtime_report
    with pytest.raises(module.DiagnosticError, match="substituted by another input"):
        module.emit_native(args)

    args = native_args(tmp_path / "fixture-substitution", module, "native-windows")
    stub_native_archive_verification(monkeypatch, module, args)
    args.native_fixture_report = args.runtime_report
    with pytest.raises(module.DiagnosticError, match="substituted by another input"):
        module.emit_native(args)


@pytest.mark.parametrize(
    ("key", "message"),
    [
        ("artifact_directory_stdout", "content mismatch"),
        ("native_report", "content mismatch"),
        ("native_dump", "content mismatch"),
    ],
)
def test_native_private_binding_rejects_same_inode_same_size_tamper_and_scrubs(
    tmp_path, monkeypatch, key, message
):
    module = load_module()
    args = native_args(tmp_path, module, "native-linux")
    stub_native_archive_verification(monkeypatch, module, args)
    path = private_binding_path(args, module, key)
    original = path.read_bytes()
    path.write_bytes(bytes([original[0] ^ 1]) + original[1:])
    assert path.stat().st_size == len(original)

    with pytest.raises(module.DiagnosticError, match=message):
        module.emit_native(args)

    assert_private_binding_scrubbed(args, module)
    assert not args.output_dir.exists()


@pytest.mark.skipif(os.name == "nt", reason="Unix pathname substitution regression")
def test_native_private_binding_substitution_never_scrubs_victim(tmp_path, monkeypatch):
    module = load_module()
    args = native_args(tmp_path, module, "native-linux")
    stub_native_archive_verification(monkeypatch, module, args)
    path = private_binding_path(args, module, "snapshot_rebuild_stdout")
    held = path.with_name("held-snapshot-rebuild.stdout")
    original = path.read_bytes()
    path.rename(held)
    victim = path.parent.parent / "victim-private-canary"
    victim_bytes = b"victim must never be admitted or scrubbed"
    victim.write_bytes(victim_bytes)
    victim.rename(path)

    with pytest.raises(module.DiagnosticError, match="child identity mismatch"):
        module.emit_native(args)

    assert path.read_bytes() == victim_bytes
    assert held.read_bytes() == original
    assert not args.output_dir.exists()
    for key, name in module.PRIVATE_NATIVE_BINDING_FILES.items():
        candidate = path.parent / name
        if key != "snapshot_rebuild_stdout":
            assert candidate.stat().st_size == 0


def test_native_validation_failure_still_scrubs_exact_private_binding(
    tmp_path, monkeypatch
):
    module = load_module()
    args = native_args(tmp_path, module, "native-linux")
    stub_native_archive_verification(monkeypatch, module, args)
    report = json.loads(args.native_restore_report.read_text(encoding="utf-8"))
    report["native_restore"]["status"] = "incomplete"
    write_json(args.native_restore_report, report)

    with pytest.raises(module.DiagnosticError, match="semantic contract"):
        module.emit_native(args)

    assert_private_binding_scrubbed(args, module)
    assert not args.output_dir.exists()


def test_native_parsed_input_path_substitution_fails_without_touching_victim(
    tmp_path, monkeypatch
):
    module = load_module()
    args = native_args(tmp_path, module, "native-linux")
    stub_native_archive_verification(monkeypatch, module, args)
    original_admit = module.PrivateNativeBinding.admit
    admitted_bytes = args.package_report.read_bytes()
    victim_bytes = b'{"private_canary":"must-survive"}\n'

    def admit_then_substitute(self, parent, binding):
        original_admit(self, parent, binding)
        held = args.package_report.with_name("held-package-report.json")
        args.package_report.rename(held)
        args.package_report.write_bytes(victim_bytes)
        assert held.read_bytes() == admitted_bytes

    monkeypatch.setattr(module.PrivateNativeBinding, "admit", admit_then_substitute)

    with pytest.raises(module.DiagnosticError, match="changed while retained"):
        module.emit_native(args)

    assert args.package_report.read_bytes() == victim_bytes
    assert_private_binding_scrubbed(args, module)
    assert not args.output_dir.exists()


@pytest.mark.skipif(os.name == "nt", reason="Unix pathname substitution regression")
def test_native_artifact_replacement_during_independent_verification_is_rejected(
    tmp_path, monkeypatch
) -> None:
    module = load_module()
    args = native_args(tmp_path, module, "native-linux")
    package_report = json.loads(args.package_report.read_text(encoding="utf-8"))
    runtime_report = json.loads(args.runtime_report.read_text(encoding="utf-8"))
    binding = runtime_report["package_binding"]
    archive_binaries = {
        key: binding[key]
        for key in (
            "worldstreamd_sha256",
            "worldstreamd_size_bytes",
            "worldstreamctl_sha256",
            "worldstreamctl_size_bytes",
        )
    }
    original = args.artifact.read_bytes()
    victim = b"replacement archive must not become release evidence"

    def verify_then_replace(_artifact, _source_id, _version):
        held = args.artifact.with_name("held-release-archive")
        args.artifact.rename(held)
        args.artifact.write_bytes(victim)
        assert held.read_bytes() == original
        return package_report, archive_binaries

    monkeypatch.setattr(
        module, "independently_verify_native_archive", verify_then_replace
    )

    with pytest.raises(module.DiagnosticError, match="changed while retained"):
        module.emit_native(args)

    assert args.artifact.read_bytes() == victim
    assert_private_binding_scrubbed(args, module)
    assert not args.output_dir.exists()


@pytest.mark.parametrize(
    ("section", "field", "message"),
    [
        (("product_execution", "actions", 0), "exit_code", "product action"),
        (("cleanup",), "target_backends_before_drop", "cleanup"),
        (("cleanup",), "generated_restore_roles_before_drop", "cleanup"),
    ],
)
def test_native_exact_numeric_fields_reject_boolean_zero(
    tmp_path, monkeypatch, section, field, message
):
    module = load_module()
    args = native_args(tmp_path, module, "native-linux")
    stub_native_archive_verification(monkeypatch, module, args)
    report = json.loads(args.native_restore_report.read_text(encoding="utf-8"))
    target = report
    for part in section:
        target = target[part]
    target[field] = False
    write_json(args.native_restore_report, report)

    with pytest.raises(module.DiagnosticError, match=message):
        module.emit_native(args)

    assert_private_binding_scrubbed(args, module)


def test_native_receipt_artifact_identities_must_be_distinct(tmp_path, monkeypatch):
    module = load_module()
    args = native_args(tmp_path, module, "native-linux")
    stub_native_archive_verification(monkeypatch, module, args)
    report = json.loads(args.native_restore_report.read_text(encoding="utf-8"))
    action = report["product_execution"]["actions"][2]
    receipt = action["receipt"]
    receipt["report_identity"] = receipt["native_dump_identity"]
    rewrite_private_json_binding(args, module, report, "native_restore_stdout", receipt)
    raw = private_binding_path(args, module, "native_restore_stdout").read_bytes()
    action["stdout_sha256"] = "sha256:" + hashlib.sha256(raw).hexdigest()
    action["stdout_size_bytes"] = len(raw)
    write_json(args.native_restore_report, report)

    with pytest.raises(module.DiagnosticError, match="identities must be distinct"):
        module.emit_native(args)

    assert_private_binding_scrubbed(args, module)


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
def test_native_provider_numeric_identity_is_ascii_canonical_and_bounded(field, value):
    module = load_module()
    report = windows_native_restore_report(module)
    report["target_provider_identity"][field] = value

    with pytest.raises(module.DiagnosticError, match="semantic contract"):
        module.verify_native_restore_report(report)


def test_native_source_and_target_require_distinct_authoritative_numeric_tuple():
    module = load_module()
    report = windows_native_restore_report(module)
    source = report["source_provider_identity"]
    report["target_provider_identity"] = {
        **source,
        "database_name": "different_display_name",
    }

    with pytest.raises(module.DiagnosticError, match="semantic contract"):
        module.verify_native_restore_report(report)


@pytest.mark.parametrize(
    ("tamper", "message"),
    [
        (lambda report: report.update({"status": "incomplete"}), "semantic contract"),
        (
            lambda report: report.update({"native_witness_minted": False}),
            "semantic contract",
        ),
        (
            lambda report: report.update({"native_point_digest": "d" * 64}),
            "semantic contract",
        ),
        (
            lambda report: report["verifier_scope"].update(
                {"profile": "single_pack_no_resources_no_fired_timers"}
            ),
            "scope is incomplete",
        ),
        (
            lambda report: report["durable_domain_inventory"].pop(),
            "inventory is incomplete",
        ),
        (
            lambda report: report["durable_domain_inventory"][0].update(
                {"restored_row_count": 2}
            ),
            "row is malformed",
        ),
        (
            lambda report: report.update({"restored_snapshot_count_before": True}),
            "semantic contract",
        ),
        (
            lambda report: report.update({"restored_snapshot_count_after": False}),
            "semantic contract",
        ),
        (
            lambda report: report["target_provider_identity"].update(
                {"database_oid": "0"}
            ),
            "semantic contract",
        ),
        (
            lambda report: report["verifier_scope"].update(
                {"source_resource_identity_count": True}
            ),
            "scope is incomplete",
        ),
        (
            lambda report: report["durable_domain_inventory"][0].update(
                {"restored_row_count": True}
            ),
            "row is malformed",
        ),
        (
            lambda report: report["verifier"]["diagnostics"][0].update(
                {"subject": "subject:TOP_SECRET"}
            ),
            "malformed or blocking diagnostic",
        ),
        (
            lambda report: report["verifier"]["diagnostics"][0].update(
                {"action": "accept an unreviewed isolation"}
            ),
            "malformed or blocking diagnostic",
        ),
        (
            lambda report: report["verifier"]["rooms"].update(
                {"01ARZ3NDEKTSV4RRFFQ69G5FAV": "Blocked"}
            ),
            "rooms are blocked",
        ),
        (lambda report: report.update({"unexpected": True}), "wrong fields"),
    ],
)
def test_windows_native_diagnostic_rejects_tampered_restore_report(
    tmp_path, monkeypatch, tamper, message
):
    module = load_module()
    args = native_args(tmp_path, module, "native-windows")
    stub_native_archive_verification(monkeypatch, module, args)
    report = json.loads(args.native_restore_report.read_text(encoding="utf-8"))
    tamper(report["native_restore"])
    write_json(args.native_restore_report, report)

    with pytest.raises(module.DiagnosticError, match=message):
        module.emit_native(args)


@pytest.mark.parametrize(
    ("tamper", "message"),
    [
        (
            lambda report: report["package_binding"].update(
                {"worldstreamctl_sha256": "sha256:" + "0" * 64}
            ),
            "exact package/control bytes",
        ),
        (
            lambda report: report["source_fixture"].update(
                {"fixture_report_size_bytes": 0}
            ),
            "fixture binding mismatch",
        ),
        (
            lambda report: report["platform"].update({"machine": "aarch64"}),
            "platform identity mismatch",
        ),
        (lambda report: report.update({"unexpected": True}), "wrapper is incomplete"),
    ],
)
def test_native_diagnostic_rejects_tampered_hosted_restore_wrapper(
    tmp_path, monkeypatch, tamper, message
):
    module = load_module()
    args = native_args(tmp_path, module, "native-linux")
    stub_native_archive_verification(monkeypatch, module, args)
    report = json.loads(args.native_restore_report.read_text(encoding="utf-8"))
    tamper(report)
    write_json(args.native_restore_report, report)

    with pytest.raises(module.DiagnosticError, match=message):
        module.emit_native(args)


def test_native_diagnostic_rejects_fixture_revision_claim_from_wrapper_alone(
    tmp_path, monkeypatch
):
    module = load_module()
    args = native_args(tmp_path, module, "native-linux")
    stub_native_archive_verification(monkeypatch, module, args)
    fixture = json.loads(args.native_fixture_report.read_text(encoding="utf-8"))
    fixture["source_construction"]["source_revision"] = "0" * 40
    write_json(args.native_fixture_report, fixture)
    wrapper = json.loads(args.native_restore_report.read_text(encoding="utf-8"))
    wrapper["source_fixture"]["fixture_report_sha256"] = module.digest(
        args.native_fixture_report
    )
    wrapper["source_fixture"]["fixture_report_size_bytes"] = (
        args.native_fixture_report.stat().st_size
    )
    write_json(args.native_restore_report, wrapper)

    with pytest.raises(
        module.DiagnosticError, match="explicitly untrusted and source-bound"
    ):
        module.emit_native(args)


def test_native_diagnostic_rejects_unversioned_fixture_provenance(
    tmp_path, monkeypatch
):
    module = load_module()
    args = native_args(tmp_path, module, "native-linux")
    stub_native_archive_verification(monkeypatch, module, args)
    fixture = json.loads(args.native_fixture_report.read_text(encoding="utf-8"))
    fixture["schema"] = "worldstream/sqlite-postgresql-transfer-evidence/unversioned"
    write_json(args.native_fixture_report, fixture)
    wrapper = json.loads(args.native_restore_report.read_text(encoding="utf-8"))
    wrapper["source_fixture"]["fixture_report_sha256"] = module.digest(
        args.native_fixture_report
    )
    wrapper["source_fixture"]["fixture_report_size_bytes"] = (
        args.native_fixture_report.stat().st_size
    )
    write_json(args.native_restore_report, wrapper)

    with pytest.raises(
        module.DiagnosticError, match="explicitly untrusted and source-bound"
    ):
        module.emit_native(args)


@pytest.mark.parametrize(
    ("tamper", "message"),
    [
        ("archive", "exact archive"),
        ("binary", "exact archive"),
        ("machine", "runtime smoke is incomplete"),
        ("profile", "both storage profiles"),
        ("health", "contract is incomplete"),
        ("admin", "administration contract"),
        ("source-revision", "exact archive/report/manifests/binaries"),
        ("package-report-size", "exact archive/report/manifests/binaries"),
        ("sqlite-operator", "SQLite operator backup/restore"),
        ("transfer-abort", "transfer abort/restart"),
        ("transfer-finalize", "transfer hydration/finalization"),
        ("control-output", "retained control output"),
    ],
)
def test_native_diagnostic_rejects_unbound_or_partial_runtime(
    tmp_path, monkeypatch, tamper, message
):
    module = load_module()
    args = native_args(tmp_path, module, "native-linux")
    stub_native_archive_verification(monkeypatch, module, args)
    runtime = json.loads(args.runtime_report.read_text(encoding="utf-8"))
    if tamper == "archive":
        runtime["package_binding"]["archive_sha256"] = "sha256:" + "0" * 64
    elif tamper == "binary":
        runtime["package_binding"]["worldstreamd_sha256"] = "sha256:" + "0" * 64
    elif tamper == "machine":
        runtime["platform"]["machine"] = "aarch64"
    elif tamper == "profile":
        del runtime["profiles"]["postgres-primary"]
    elif tamper == "health":
        runtime["profiles"]["postgres-primary"]["healthz"] = "not_run"
    elif tamper == "admin":
        runtime["postgres_admin"]["verify"] = "not_run"
    elif tamper == "source-revision":
        runtime["package_binding"]["source_revision"] = "0" * 40
    elif tamper == "package-report-size":
        runtime["package_binding"]["package_report_size_bytes"] += 1
    elif tamper == "sqlite-operator":
        runtime["profiles"]["sqlite-bundled"]["sqlite_operator"]["restore"] = (
            "native_only"
        )
    elif tamper == "transfer-abort":
        runtime["transfer_operator"]["abort"]["pre_abort_checkpoint_rows"] = 0
    elif tamper == "transfer-finalize":
        runtime["transfer_operator"]["finalized"]["source_retired"] = "not_run"
    else:
        runtime["control_output"]["worldstreamctl_sha256"] = "sha256:" + "0" * 64
    write_json(args.runtime_report, runtime)

    with pytest.raises(module.DiagnosticError, match=message):
        module.emit_native(args)


@pytest.mark.parametrize("source", ["native-linux", "native-windows"])
def test_native_diagnostic_rejects_arbitrary_non_archive_bytes(tmp_path, source):
    module = load_module()
    args = native_args(tmp_path, module, source)

    with pytest.raises(
        module.DiagnosticError, match="native archive verification failed"
    ):
        module.emit_native(args)


def https_report(module) -> dict:
    return {
        "schema": "worldstream/telemetry-https-evidence/v1",
        "status": "passed",
        "release_evidence": False,
        "evidence_class": "local_transport_integration",
        "platform": {"system": "Linux", "machine": "x86_64"},
        "timeout_seconds_per_test": 300,
        "checks": dict(module.TELEMETRY_HTTPS_CHECKS),
        "tests": [
            {
                "id": test_id,
                "package": package,
                "filter": test_filter,
                "passed_count": 1,
            }
            for test_id, (package, test_filter) in module.TELEMETRY_HTTPS_TESTS.items()
        ],
    }


def macos_report(module, architecture: str) -> dict:
    sha = "sha256:" + "a" * 64
    browser_checks = {
        "browser_identity_verified": True,
        "catch_up_or_reset_installed": True,
        "embedded_ui_loaded": True,
        "final_reveal_dom_visible": True,
        "new_session_resynchronized": True,
        "package_bound_reference_clients": False,
        "package_bound_runtime": False,
        "precomplete_reveal_locked": True,
        "privacy_negative_dom_and_browser_channels": True,
        "replay_hashes_verified": True,
        "six_phase_story_complete": True,
        "stale_head_rejected": True,
        "typed_actions_accepted_in_dom": True,
    }
    return {
        "schema": "worldstream/macos-source-quickstart/v2",
        "status": "passed",
        "release_evidence": False,
        "signed_or_notarized_binary": False,
        "version": module.manifest()["release_candidate"],
        "platform": {
            "system": "Darwin",
            "version": "15.6.1",
            "filesystem": "apfs",
            "machine": architecture,
        },
        "toolchains": {
            **module.pinned_macos_toolchains(),
        },
        "source_revision": "1" * 40,
        "elapsed_seconds": 500,
        "browser_story": {
            "schema": "worldstream/package-browser-heist/v1",
            "canonical_encoding": "utf8-sorted-key-compact-json-lf",
            "status": "pass",
            "release_evidence": False,
            "source_mode": "source-build",
            "elapsed_ms": 12_000,
            "browser": module.MACOS_BROWSER_IDENTITIES.get(
                architecture, module.MACOS_BROWSER_IDENTITIES["arm64"]
            ),
            "tools": {
                "adapter": {
                    "name": "worldstream-cdp-browser",
                    "protocol": "Chrome DevTools Protocol",
                    "sha256": module.digest(ROOT / "scripts/cdp-browser.py"),
                    "size_bytes": (ROOT / "scripts/cdp-browser.py").stat().st_size,
                },
                "python": {"implementation": "cpython", "version": "3.14.7"},
            },
            "checks": browser_checks,
            "runtime": {
                "worldstreamd": {
                    "origin": "source-build:target/debug/worldstreamd",
                    "sha256": sha,
                    "size_bytes": 10,
                },
                "ui": {
                    "origin": "source-build:web/console/dist",
                    "tree_sha256": sha,
                    "index_sha256": sha,
                    "file_count": 2,
                    "total_bytes": 10,
                },
                "sdk": {
                    "origin": "source:sdk/python/src",
                    "tree_sha256": sha,
                    "file_count": 2,
                    "total_bytes": 10,
                },
                "heist_reference_clients": {
                    "origin": "source:examples/heist",
                    "tree_sha256": sha,
                    "file_count": 4,
                    "total_bytes": 10,
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
                    "public_claims": 2,
                    "plans": 1,
                    "endorsements": 2,
                    "challenges": 0,
                    "commitment_count": 2,
                    "aggregate_outcome_present": True,
                },
                "final_replay": {
                    "verified": True,
                    "hash_parity": {
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
                        "expected": {
                            "pack": "blake3:pack",
                            "core": "blake3:core",
                            "activity": "blake3:activity",
                            "aggregate_authoritative": "blake3:aggregate",
                            "transition": "blake3:transition",
                        },
                        "replayed": {
                            "pack": "blake3:pack",
                            "core": "blake3:core",
                            "activity": "blake3:activity",
                            "aggregate_authoritative": "blake3:aggregate",
                            "transition": "blake3:transition",
                        },
                    },
                },
            },
            "dom_evidence": {
                key: sha
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
                key: sha
                for key in (
                    "inspect_clue",
                    "publish_clue",
                    "propose_plan",
                    "commit_move",
                    "acknowledge_result",
                )
            },
            "privacy": {
                "status": "pass",
                "private_canary_absent": True,
                "credentials_absent": True,
                "private_claim_absent_from_retained_evidence": True,
            },
        },
        "checks": {
            "complete_heist": True,
            "embedded_ui": True,
            "pinned_toolchain": True,
            "privacy": True,
            "real_browser": True,
            "replay": True,
            "source_revision": True,
            "source_build": True,
            "stale_resync": True,
            "quickstart": True,
        },
    }


def macos_args(tmp_path: Path, module, architectures: list[str]) -> argparse.Namespace:
    return argparse.Namespace(
        quickstart_report=[
            write_json(
                tmp_path / f"macos-{index}-{architecture}.json",
                macos_report(module, architecture),
            )
            for index, architecture in enumerate(architectures)
        ],
        source_revision="1" * 40,
        artifact_output=tmp_path / "macos-source-matrix.json",
        output_dir=tmp_path / "diagnostics",
    )


def test_macos_diagnostic_requires_and_binds_both_supported_architectures(tmp_path):
    module = load_module()
    args = macos_args(tmp_path, module, ["x86_64", "arm64"])

    module.emit_macos(args)

    artifact = json.loads(args.artifact_output.read_text(encoding="utf-8"))
    assert artifact["schema"] == "worldstream/macos-source-matrix/v1"
    assert artifact["architectures"] == ["arm64", "x86_64"]
    assert artifact["source_revision"] == "1" * 40
    assert artifact["toolchains"] == module.pinned_macos_toolchains()
    assert [report["architecture"] for report in artifact["quickstart_reports"]] == [
        "arm64",
        "x86_64",
    ]
    assert all(
        report["sha256"].startswith("sha256:")
        for report in artifact["quickstart_reports"]
    )
    diagnostic = json.loads(
        (args.output_dir / "macos-source-source-quickstart.json").read_text(
            encoding="utf-8"
        )
    )
    assert diagnostic["facts"]["architectures"] == ["arm64", "x86_64"]
    assert diagnostic["facts"]["source_revision"] == "1" * 40
    assert "arm64 and x86_64" in diagnostic["facts"]["quickstart"]


@pytest.mark.parametrize(
    ("architectures", "message"),
    [
        (["arm64"], "exactly arm64 and x86_64"),
        (["arm64", "arm64"], "duplicate"),
        (["arm64", "i386"], "unexpected architecture"),
    ],
)
def test_macos_diagnostic_rejects_missing_duplicate_or_unexpected_architecture(
    tmp_path, architectures, message
):
    module = load_module()
    args = macos_args(tmp_path, module, architectures)

    with pytest.raises(module.DiagnosticError, match=message):
        module.emit_macos(args)


@pytest.mark.parametrize("toolchain", ["rust", "python", "node", "pnpm", "uv"])
def test_macos_diagnostic_rejects_every_noncanonical_toolchain_pin(tmp_path, toolchain):
    module = load_module()
    args = macos_args(tmp_path, module, ["arm64", "x86_64"])
    report_path = args.quickstart_report[0]
    report = json.loads(report_path.read_text(encoding="utf-8"))
    report["toolchains"][toolchain] = "0.0.0"
    write_json(report_path, report)

    with pytest.raises(module.DiagnosticError, match="incomplete"):
        module.emit_macos(args)


@pytest.mark.parametrize(
    ("path", "value"),
    [
        (("browser_story", "checks", "stale_head_rejected"), False),
        (("browser_story", "checks", "package_bound_runtime"), True),
        (("browser_story", "browser", "sha256"), "sha256:" + "0" * 64),
        (("elapsed_seconds",), 600),
    ],
)
def test_macos_diagnostic_rejects_incomplete_browser_story(tmp_path, path, value):
    module = load_module()
    args = macos_args(tmp_path, module, ["arm64", "x86_64"])
    report_path = args.quickstart_report[0]
    report = json.loads(report_path.read_text(encoding="utf-8"))
    target = report
    for key in path[:-1]:
        target = target[key]
    target[path[-1]] = value
    write_json(report_path, report)

    with pytest.raises(module.DiagnosticError):
        module.emit_macos(args)


@pytest.mark.skipif(os.name == "nt", reason="Unix pathname substitution regression")
def test_macos_adapter_replacement_before_emission_is_rejected(
    tmp_path, monkeypatch
) -> None:
    module = load_module()
    reports_root = tmp_path / "reports"
    reports_root.mkdir()
    args = macos_args(reports_root, module, ["arm64", "x86_64"])
    isolated_root = tmp_path / "isolated-root"
    (isolated_root / "scripts").mkdir(parents=True)
    for relative in (
        "rust-toolchain.toml",
        ".python-version",
        ".node-version",
        ".uv-version",
        "package.json",
        "scripts/cdp-browser.py",
    ):
        target = isolated_root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes((ROOT / relative).read_bytes())
    module.ROOT = isolated_root
    adapter = isolated_root / "scripts/cdp-browser.py"
    original = adapter.read_bytes()
    victim = b"replacement browser adapter must not become evidence\n"
    actual_verify = module.verify_macos_browser_story
    replaced = False

    def verify_then_replace(*verify_args, **verify_kwargs) -> None:
        nonlocal replaced
        actual_verify(*verify_args, **verify_kwargs)
        if not replaced:
            replaced = True
            adapter.rename(adapter.with_name("held-cdp-browser.py"))
            adapter.write_bytes(victim)

    monkeypatch.setattr(module, "verify_macos_browser_story", verify_then_replace)

    with pytest.raises(module.DiagnosticError, match="changed while retained"):
        module.emit_macos(args)

    assert adapter.read_bytes() == victim
    assert adapter.with_name("held-cdp-browser.py").read_bytes() == original
    assert not args.artifact_output.exists()
    assert not args.output_dir.exists()


@pytest.mark.parametrize(
    "package_manager",
    [
        "pnpm@11.19.0",
        "pnpm@11.19.0+sha512." + "0" * 127,
        "pnpm@11.19.0+sha256." + "0" * 64,
    ],
)
def test_macos_toolchain_pin_requires_exact_pnpm_tarball_integrity(
    tmp_path, package_manager
):
    module = load_module()
    for relative in (
        "rust-toolchain.toml",
        ".python-version",
        ".node-version",
        ".uv-version",
    ):
        (tmp_path / relative).write_bytes((ROOT / relative).read_bytes())
    package = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))
    package["packageManager"] = package_manager
    write_json(tmp_path / "package.json", package)
    module.ROOT = tmp_path

    with pytest.raises(module.DiagnosticError, match="integrity-bound pnpm pin"):
        module.pinned_macos_toolchains()


def test_macos_diagnostic_rejects_wrong_or_mismatched_source_revision(tmp_path):
    module = load_module()
    args = macos_args(tmp_path, module, ["arm64", "x86_64"])
    args.source_revision = "not-a-commit"
    with pytest.raises(module.DiagnosticError, match="40-hex"):
        module.emit_macos(args)

    mismatch_root = tmp_path / "mismatch"
    mismatch_root.mkdir()
    args = macos_args(mismatch_root, module, ["arm64", "x86_64"])
    report_path = args.quickstart_report[0]
    report = json.loads(report_path.read_text(encoding="utf-8"))
    report["source_revision"] = "2" * 40
    write_json(report_path, report)
    with pytest.raises(module.DiagnosticError, match="incomplete"):
        module.emit_macos(args)


def security_args(tmp_path: Path, module, report: dict) -> argparse.Namespace:
    common = {
        "secret-scan",
        "evidence-privacy-local",
        "evidence-capability-local",
        "evidence-lease-local",
        "evidence-telemetry-local",
        "filesystem-owner-only",
        "evidence-filesystem-local",
        "config-contract",
    }
    runtime = {
        "schema": "worldstream/native-package-runtime-smoke/v1",
        "status": "pass",
        "release_evidence": False,
        "profiles": {
            profile: {
                "packaged_ctl": {
                    "config_validate": "pass",
                    "config_effective": "redacted_and_precedence_exact",
                    "doctor": "bounded_diagnostics_exposed",
                }
            }
            for profile in ("sqlite-bundled", "postgres-primary")
        },
    }
    linux_runtime = {**runtime, "platform": {"system": "Linux"}}
    windows_runtime = {**runtime, "platform": {"system": "Windows"}}
    return argparse.Namespace(
        linux_gate_report=write_json(
            tmp_path / "linux.json",
            gate_report("Linux", common | {"telemetry-failure-pressure"}),
        ),
        windows_gate_report=write_json(
            tmp_path / "windows.json",
            gate_report("Windows", common | {"filesystem-acl-policy"}),
        ),
        linux_runtime_report=write_json(tmp_path / "linux-runtime.json", linux_runtime),
        windows_runtime_report=write_json(
            tmp_path / "windows-runtime.json", windows_runtime
        ),
        https_report=write_json(tmp_path / "https.json", report),
        artifact_output=tmp_path / "security-profile.json",
        output_dir=tmp_path / "diagnostics",
    )


def test_security_diagnostic_requires_and_binds_exact_https_transport_report(tmp_path):
    module = load_module()
    args = security_args(tmp_path, module, https_report(module))

    module.emit_security(args)

    artifact = json.loads(args.artifact_output.read_text(encoding="utf-8"))
    assert artifact["status"] == "passed"
    assert artifact["release_evidence"] is True
    assert [item["platform"] for item in artifact["platform_reports"]] == [
        "native-linux-x86_64",
        "native-windows-x64",
        "native-linux-x86_64/telemetry-https",
        "native-linux-x86_64/config-contract",
        "native-windows-x64/config-contract",
    ]
    diagnostic = json.loads(
        (args.output_dir / "security-observability-security.json").read_text(
            encoding="utf-8"
        )
    )
    remote_tls = diagnostic["facts"]["remote_tls"]
    assert "trusted-CA" in remote_tls
    assert "hostname mismatch" in remote_tls
    assert "no provider credentials" not in remote_tls


@pytest.mark.parametrize("tamper", ["check", "filter", "count"])
def test_security_diagnostic_rejects_incomplete_https_claim(tmp_path, tamper):
    module = load_module()
    report = https_report(module)
    if tamper == "check":
        report["checks"]["hostname_verification"] = False
    elif tamper == "filter":
        report["tests"][0]["filter"] = "generic_exit_code_only"
    else:
        report["tests"][0]["passed_count"] = 0
    args = security_args(tmp_path, module, report)

    with pytest.raises(module.DiagnosticError, match="telemetry HTTPS"):
        module.emit_security(args)


@pytest.mark.parametrize("system", ["linux", "windows"])
def test_security_diagnostic_rejects_missing_packaged_config_command(tmp_path, system):
    module = load_module()
    args = security_args(tmp_path, module, https_report(module))
    path = getattr(args, f"{system}_runtime_report")
    runtime = json.loads(path.read_text(encoding="utf-8"))
    runtime["profiles"]["postgres-primary"]["packaged_ctl"]["doctor"] = "not-run"
    write_json(path, runtime)

    with pytest.raises(module.DiagnosticError, match="config/effective/doctor"):
        module.emit_security(args)


@pytest.mark.skipif(os.name == "nt", reason="Unix pathname substitution regression")
def test_security_evidence_replacement_before_emission_is_rejected(
    tmp_path, monkeypatch
) -> None:
    module = load_module()
    args = security_args(tmp_path, module, https_report(module))
    actual_gate_outcomes = module.gate_outcomes
    original = args.linux_gate_report.read_bytes()
    victim = b'{"private_canary":"must-survive"}\n'
    replaced = False

    def validate_then_replace(report, expected_system, required):
        nonlocal replaced
        outcomes = actual_gate_outcomes(report, expected_system, required)
        if not replaced:
            replaced = True
            held = args.linux_gate_report.with_name("held-linux-gate.json")
            args.linux_gate_report.rename(held)
            args.linux_gate_report.write_bytes(victim)
            assert held.read_bytes() == original
        return outcomes

    monkeypatch.setattr(module, "gate_outcomes", validate_then_replace)

    with pytest.raises(module.DiagnosticError, match="changed while retained"):
        module.emit_security(args)

    assert args.linux_gate_report.read_bytes() == victim
    assert not args.artifact_output.exists()
    assert not args.output_dir.exists()


@pytest.mark.parametrize("primary_failure", [False, True])
def test_security_cleanup_attempts_every_close_and_preserves_primary_failure(
    tmp_path, monkeypatch, primary_failure
) -> None:
    module = load_module()
    report = https_report(module)
    if primary_failure:
        report["checks"]["hostname_verification"] = False
    args = security_args(tmp_path, module, report)
    actual_close = module.RetainedJsonInput.close
    closed_labels: list[str] = []

    def close_then_fail(retained) -> None:
        closed_labels.append(retained.retained.label)
        actual_close(retained)
        raise OSError(f"injected close failure: {retained.retained.label}")

    monkeypatch.setattr(module.RetainedJsonInput, "close", close_then_fail)
    expected = "telemetry HTTPS" if primary_failure else "cleanup failed closed"

    with pytest.raises(module.DiagnosticError, match=expected):
        module.emit_security(args)

    assert set(closed_labels) == {
        "Linux platform gate report",
        "Windows platform gate report",
        "Linux native packaged runtime report",
        "Windows native packaged runtime report",
        "telemetry HTTPS report",
    }
    assert len(closed_labels) == 5
    assert not args.artifact_output.exists()
    assert not args.output_dir.exists()


def test_oci_diagnostic_requires_binary_healthcheck_to_fail_without_daemon(
    tmp_path, monkeypatch
):
    module = load_module()
    actual_context_verifier = module.independently_verify_oci_context
    actual_archive_verifier = module.independently_verify_oci_archive
    sqlite = module.manifest()["storage"]["sqlite"]
    sqlite_identity = f"sqlite/{sqlite['version']}; source_id={sqlite['source_id']}"
    context = {
        "schema": "worldstream/oci-context-report/v1",
        "kind": "oci-context",
        "verified": True,
        "release_evidence": False,
        "sha256": "sha256:" + "d" * 64,
    }
    runtime = {
        "schema": "worldstream/oci-runtime-smoke/v1",
        "status": "PASS",
        "release_evidence": False,
        "health": "healthy",
        "healthcheck_without_daemon": "rejected",
        "read_only_root": True,
        "non_root": "65532:65532",
        "persistent_volume": "/var/lib/worldstream",
        "sqlite_volume": {
            "path": "/var/lib/worldstream",
            "type": "docker-volume",
            "driver": "local",
            "scope": "local",
            "driver_options": {},
            "mount_device": "254:1",
            "filesystem": "ext4",
            "mount_source": "/dev/vda1",
            "locality": "local-block-device",
        },
        "authority_secret_source": "owner-readable-read-only-volume-file",
        "standalone_config": "valid",
        "rejected_layouts": [
            "tmpfs",
            "wrong-data-directory",
            "network-configured-volume",
        ],
        "profiles": {
            "sqlite-bundled": {
                "status": "pass",
                "health": "healthy",
                "filesystem": "ext4-or-xfs-explicit-volume",
                "engine_identity": sqlite_identity,
                "healthz": "pass",
                "readyz": "pass",
                "version": "pass",
            },
            "postgres-primary": {
                "status": "pass",
                "provider_image": module.OCI_POSTGRES_IMAGE,
                "engine_identity": module.OCI_POSTGRES_IDENTITY,
                "packaged_admin_migrate": "pass",
                "packaged_admin_verify": "pass",
                "runtime_role_least_privilege": True,
                "healthz": "pass",
                "readyz": "pass",
                "version": "pass",
                "network": "disabled-unix-socket",
            },
        },
        "secrets_emitted": False,
    }
    metadata = {
        "base_image": "alpine@sha256:" + "a" * 64,
        "version": module.manifest()["release_candidate"],
        "profile": "oci-linux-amd64",
        "target": "linux/amd64",
    }
    context_dir = tmp_path / "oci-context"
    context_dir.mkdir()
    context_metadata = write_json(context_dir / "oci-metadata.json", metadata)
    artifact = tmp_path / "image.oci.tar"
    artifact.write_bytes(b"oci image fixture")
    artifact_sha256 = "sha256:" + hashlib.sha256(artifact.read_bytes()).hexdigest()
    image_config_digest = "sha256:" + "c" * 64
    runtime["artifact_binding"] = {
        "status": "pass",
        "artifact": artifact.name,
        "artifact_sha256": artifact_sha256,
        "artifact_size_bytes": artifact.stat().st_size,
        "artifact_image_manifest_digest": "sha256:" + "b" * 64,
        "artifact_image_config_digest": image_config_digest,
        "tested_image_config_digest": image_config_digest,
        "layer_count": 3,
        "layer_descriptors_bound": True,
        "rootfs_diff_ids_bound": True,
        "closed_blob_inventory": True,
    }
    runtime["secret_scan"] = {
        "schema": "worldstream/secret-absence-scan/v1",
        "status": "pass",
        "secrets_emitted": False,
        "sentinel_sha256": "sha256:" + "e" * 64,
        "encodings_scanned": ["base64", "base64url", "hex", "raw"],
        "channels": [
            {
                "channel": channel,
                "sha256": "sha256:" + "f" * 64,
                "size_bytes": 1,
            }
            for channel in sorted(module.OCI_SECRET_SCAN_CHANNELS)
        ],
    }
    monkeypatch.setattr(
        module, "independently_verify_oci_context", lambda candidate: context
    )
    monkeypatch.setattr(
        module,
        "independently_verify_oci_archive",
        lambda candidate, tested_digest: runtime["artifact_binding"],
    )
    args = argparse.Namespace(
        context=context_dir,
        context_report=write_json(tmp_path / "context.json", context),
        runtime_report=write_json(tmp_path / "runtime.json", runtime),
        context_metadata=context_metadata,
        artifact=artifact,
        output_dir=tmp_path / "diagnostics",
    )

    module.emit_oci(args)
    diagnostic = json.loads(
        (args.output_dir / "oci-linux-oci.json").read_text(encoding="utf-8")
    )
    facts = diagnostic["facts"]
    assert facts["storage_profiles"] == "sqlite-bundled and postgres-primary passed"
    assert facts["postgres_provider_image"] == module.OCI_POSTGRES_IMAGE
    assert facts["postgres_engine_identity"] == module.OCI_POSTGRES_IDENTITY
    assert facts["postgres_packaged_administration"] == "migrate and verify passed"
    assert facts["postgres_runtime_role"] == "least privilege verified"
    assert facts["postgres_network"] == "disabled network with local Unix socket"
    assert facts["secrets"] == "not emitted"
    assert "local block source" in facts["filesystem_policy"]
    assert facts["context_report_sha256"] == module.digest(args.context_report)
    assert facts["context_metadata_sha256"] == module.digest(args.context_metadata)
    assert facts["runtime_report_sha256"] == module.digest(args.runtime_report)
    producer = load_platform_producer()
    producer_manifest = producer.load_manifest(
        ROOT / "compatibility.toml", ROOT / "compatibility.json"
    )
    assert (
        producer.read_report(
            args.output_dir / "oci-linux-oci.json",
            "oci-linux",
            "oci",
            producer_manifest,
        )
        == diagnostic
    )

    monkeypatch.setattr(
        module, "independently_verify_oci_context", actual_context_verifier
    )
    with pytest.raises(module.DiagnosticError, match="context verification failed"):
        module.emit_oci(args)
    monkeypatch.setattr(
        module, "independently_verify_oci_context", lambda candidate: context
    )

    monkeypatch.setattr(
        module, "independently_verify_oci_archive", actual_archive_verifier
    )
    with pytest.raises(module.DiagnosticError, match="archive verification failed"):
        module.emit_oci(args)
    monkeypatch.setattr(
        module,
        "independently_verify_oci_archive",
        lambda candidate, tested_digest: runtime["artifact_binding"],
    )

    write_json(args.context_report, {**context, "verified": False})
    with pytest.raises(module.DiagnosticError, match="exact independently verified"):
        module.emit_oci(args)
    write_json(args.context_report, context)

    external_metadata = write_json(tmp_path / "metadata-copy.json", metadata)
    args.context_metadata = external_metadata
    with pytest.raises(module.DiagnosticError, match="exact metadata"):
        module.emit_oci(args)
    args.context_metadata = context_metadata

    write_json(context_metadata, {"base_image": metadata["base_image"]})
    with pytest.raises(module.DiagnosticError, match="metadata identity"):
        module.emit_oci(args)
    write_json(context_metadata, metadata)

    for required in (
        "healthcheck_without_daemon",
        "authority_secret_source",
        "standalone_config",
        "sqlite_volume",
        "artifact_binding",
        "profiles",
        "secrets_emitted",
        "secret_scan",
    ):
        tampered = dict(runtime)
        tampered.pop(required)
        write_json(args.runtime_report, tampered)
        if required == "secret_scan":
            message = "secret-absence"
        elif required == "sqlite_volume":
            message = "volume locality"
        else:
            message = "runtime diagnostic"
        with pytest.raises(module.DiagnosticError, match=message):
            module.emit_oci(args)

    for field, invalid in (
        ("driver", "nfs-plugin"),
        ("scope", "global"),
        ("driver_options", {"type": "nfs"}),
        ("mount_device", "0:30"),
        ("filesystem", "nfs"),
        ("mount_source", "server:/worldstream"),
        ("locality", "unknown"),
    ):
        tampered = json.loads(json.dumps(runtime))
        tampered["sqlite_volume"][field] = invalid
        write_json(args.runtime_report, tampered)
        with pytest.raises(module.DiagnosticError, match="volume locality"):
            module.emit_oci(args)

    tampered = json.loads(json.dumps(runtime))
    tampered["secret_scan"]["channels"][0]["channel"] = "unscanned-channel"
    write_json(args.runtime_report, tampered)
    with pytest.raises(module.DiagnosticError, match="exact channel set"):
        module.emit_oci(args)

    for profile, field, invalid in (
        ("sqlite-bundled", "status", "not-run"),
        ("sqlite-bundled", "engine_identity", "sqlite/unverified"),
        ("sqlite-bundled", "healthz", "not-run"),
        ("sqlite-bundled", "readyz", "not-run"),
        ("sqlite-bundled", "version", "not-run"),
        ("postgres-primary", "provider_image", "postgres:17.11"),
        ("postgres-primary", "engine_identity", "postgresql/17.10"),
        ("postgres-primary", "packaged_admin_migrate", "not-run"),
        ("postgres-primary", "packaged_admin_verify", "not-run"),
        ("postgres-primary", "runtime_role_least_privilege", False),
        ("postgres-primary", "healthz", "not-run"),
        ("postgres-primary", "readyz", "not-run"),
        ("postgres-primary", "version", "not-run"),
        ("postgres-primary", "network", "bridge"),
    ):
        tampered = json.loads(json.dumps(runtime))
        tampered["profiles"][profile][field] = invalid
        write_json(args.runtime_report, tampered)
        with pytest.raises(module.DiagnosticError, match="runtime diagnostic"):
            module.emit_oci(args)

    for field, invalid in (
        ("layer_count", 0),
        ("layer_descriptors_bound", False),
        ("rootfs_diff_ids_bound", False),
        ("closed_blob_inventory", False),
    ):
        tampered = json.loads(json.dumps(runtime))
        tampered["artifact_binding"][field] = invalid
        write_json(args.runtime_report, tampered)
        with pytest.raises(module.DiagnosticError, match="artifact binding"):
            module.emit_oci(args)

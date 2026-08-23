"""Boundary tests for hosted native PostgreSQL restore supervision."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/postgres-native-restore-hosted-report.py"
REVISION = "1" * 40


def load_module():
    spec = importlib.util.spec_from_file_location(
        "worldstream_postgres_native_restore_hosted_report", SCRIPT
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_json(path: Path, value: object) -> Path:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    return path


def sha256(path: Path) -> str:
    return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()


def wrapper_identity(module, path: Path, *, directory: bool = False) -> dict[str, str]:
    if directory:
        descriptor, _metadata = module._open_exact_directory(path, "test directory")
    else:
        descriptor, _metadata = module._open_exact(path, "test file")
    try:
        return module._canonical_file_identity(descriptor)
    finally:
        os.close(descriptor)


def canonical_identity(module, path: Path) -> dict[str, str]:
    descriptor, _metadata = module._open_exact(path, "test committed artifact")
    try:
        return module._canonical_file_identity(descriptor)
    finally:
        os.close(descriptor)


def native_report(module) -> dict:
    return {
        "schema": module.NATIVE_SCHEMA,
        "status": "ready",
        "reason": "postgres_native_restore_verified_by_unified_verifier",
        "release_evidence": False,
        "secrets_emitted": False,
        "native_witness_minted": True,
        "native_dump_digest": "c" * 64,
        "native_dump_size_bytes": 42,
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
    }


def cleanup_report(module) -> dict:
    return {
        "status": "pass",
        "admitted_target_identity": {
            "system_identifier": "123456789",
            "database_oid": "16384",
            "database_name": "worldstream_native_restore",
        },
        "target_marker_before_drop": module.DISPOSABLE_TARGET_MARKER,
        "target_connection_limit_before_drop": 0,
        "target_backends_before_drop": 0,
        "generated_restore_roles_before_drop": 0,
        "target_database_after_drop": "absent",
        "provider_cleanup_transcript_sha256": "sha256:" + "9" * 64,
        "private_artifacts_disposition": module.PRIVATE_ARTIFACT_DISPOSITION,
        "private_artifact_placeholder_count": 3,
        "operator_passfile_disposition": (
            "exact_retained_file_scrubbed_to_zero_length"
        ),
    }


def arguments(tmp_path: Path, module, monkeypatch) -> argparse.Namespace:
    tmp_path.mkdir(mode=0o700, parents=True, exist_ok=True)
    monkeypatch.setattr(module.platform, "system", lambda: "Linux")
    monkeypatch.setattr(module.platform, "machine", lambda: "x86_64")
    package_report = write_json(tmp_path / "package.json", {"status": "pass"})
    artifact = tmp_path / "worldstream.tar.gz"
    artifact.write_bytes(b"exact release archive")
    control = tmp_path / "worldstreamctl"
    control.write_bytes(b"exact packaged control")
    tools = {}
    for name in ("pg_dump", "pg_restore", "psql"):
        path = tmp_path / name
        path.write_bytes(f"exact {name}".encode())
        tools[name] = path
    passfile = tmp_path / "operator.pgpass"
    passfile.write_text(
        "127.0.0.1:5432:*:postgres:fixture-password\n", encoding="utf-8"
    )
    passfile.chmod(0o600)
    work_parent = tmp_path / "private-work"
    work_parent.mkdir(mode=0o700)
    runtime = {
        "schema": module.RUNTIME_SCHEMA,
        "status": "pass",
        "release_evidence": False,
        "secrets_emitted": False,
        "package_binding": {
            "archive_sha256": sha256(artifact),
            "archive_size_bytes": artifact.stat().st_size,
            "package_report_sha256": sha256(package_report),
            "package_report_size_bytes": package_report.stat().st_size,
            "source_revision": REVISION,
            "worldstreamctl_sha256": sha256(control),
            "worldstreamctl_size_bytes": control.stat().st_size,
        },
    }
    fixture = {
        "schema": module.FIXTURE_SCHEMA,
        "status": "pass",
        "release_evidence": False,
        "secrets_emitted": False,
        "source_construction": {
            "classification": module.FIXTURE_CLASSIFICATION,
            "source_revision": REVISION,
            "trusted_product_execution": False,
        },
    }
    passfile_identity = wrapper_identity(module, passfile)
    work_parent_identity = wrapper_identity(module, work_parent, directory=True)
    return argparse.Namespace(
        source="native-linux",
        package_report=package_report,
        runtime_report=write_json(tmp_path / "runtime.json", runtime),
        artifact=artifact,
        packaged_control=control,
        fixture_report=write_json(tmp_path / "fixture.json", fixture),
        source_host="127.0.0.1",
        source_port=5432,
        source_database="postgres",
        source_username="postgres",
        source_tls_mode="disable",
        target_host="127.0.0.1",
        target_port=5432,
        target_database="worldstream_native_restore",
        target_username="postgres",
        target_tls_mode="disable",
        passfile=passfile,
        passfile_storage_id=passfile_identity["storage_id"],
        passfile_file_id=passfile_identity["file_id"],
        passfile_size=passfile.stat().st_size,
        passfile_sha256=sha256(passfile),
        scrub_passfile_on_success=True,
        pg_dump=tools["pg_dump"],
        pg_restore=tools["pg_restore"],
        psql=tools["psql"],
        work_parent=work_parent,
        work_parent_storage_id=work_parent_identity["storage_id"],
        work_parent_file_id=work_parent_identity["file_id"],
        timeout_seconds=30,
        output=tmp_path / "hosted.json",
    )


def command_evidence(module, operation: str, exit_code: int = 0):
    return module.CommandEvidence(
        operation=operation,
        exit_code=exit_code,
        stdout_sha256="sha256:" + "a" * 64,
        stdout_size_bytes=16,
        stderr_sha256="sha256:" + "b" * 64,
        stderr_size_bytes=0,
    )


def exact_command_evidence(module, operation: str, stdout: bytes, stderr: bytes = b""):
    return module.CommandEvidence(
        operation=operation,
        exit_code=0,
        stdout_sha256="sha256:" + hashlib.sha256(stdout).hexdigest(),
        stdout_size_bytes=len(stdout),
        stderr_sha256="sha256:" + hashlib.sha256(stderr).hexdigest(),
        stderr_size_bytes=len(stderr),
    )


def install_successful_execution(monkeypatch, module, *, report: dict | None = None):
    calls: list[tuple[str, list[str], str]] = []

    def fake_run_exact(executable, operation, argv, **_kwargs):
        calls.append((operation, argv, executable.sha256))
        if operation == "artifact_directory_identity":
            root = _kwargs["directory_authority"]
            identity = module._expected_directory_identity(root)
            receipt = {
                "schema": module.DIRECTORY_RECEIPT_SCHEMA,
                "status": "observed",
                "artifact_directory_identity": identity,
                "secrets_emitted": False,
            }
            stdout = (json.dumps(receipt) + "\n").encode()
            return exact_command_evidence(module, operation, stdout), stdout, b""
        if operation == "snapshot_rebuild":
            receipt = {
                "schema": module.SNAPSHOT_RECEIPT_SCHEMA,
                "status": "complete",
                "rebuilt_snapshot_count": 2,
                "source_provider_identity": native_report(module)[
                    "source_provider_identity"
                ],
                "secrets_emitted": False,
            }
            stdout = (json.dumps(receipt) + "\n").encode()
            return exact_command_evidence(module, operation, stdout), stdout, b""
        if operation == "native_restore":
            report_index = argv.index("--report") + 1
            raw_path = write_json(
                Path(argv[report_index]), report or native_report(module)
            )
            raw = raw_path.read_bytes()
            observed = json.loads(raw)
            dump_index = argv.index("--dump") + 1
            dump_path = Path(argv[dump_index])
            dump_path.write_bytes(b"d" * observed["native_dump_size_bytes"])
            recovery_name = ".worldstream_native_recovery_" + "a" * 32 + ".json"
            recovery_path = raw_path.with_name(recovery_name)
            recovery_path.write_bytes(b'{"disposition":"publication_acknowledged"}\n')
            receipt = {
                "schema": module.RESTORE_RECEIPT_SCHEMA,
                "status": "committed",
                "report_digest": "blake3:" + module.BLAKE3(raw).hex(),
                "report_size_bytes": len(raw),
                "native_dump_digest": observed["native_dump_digest"],
                "native_dump_size_bytes": observed["native_dump_size_bytes"],
                "native_dump_identity": canonical_identity(module, dump_path),
                "report_identity": canonical_identity(module, raw_path),
                "recovery_record_identity": canonical_identity(module, recovery_path),
                "recovery_record_name": recovery_name,
                "source_provider_identity": observed["source_provider_identity"],
                "target_provider_identity": observed["target_provider_identity"],
                "secrets_emitted": False,
            }
            stdout = (json.dumps(receipt) + "\n").encode()
            return exact_command_evidence(module, operation, stdout), stdout, b""
        return command_evidence(module, operation), b'{"status":"ok"}\n', b""

    monkeypatch.setattr(module, "run_exact", fake_run_exact)
    monkeypatch.setattr(
        module,
        "observe_source_identity",
        lambda *args, **kwargs: native_report(module)["source_provider_identity"],
    )
    monkeypatch.setattr(
        module,
        "observe_target_identity",
        lambda *args, **kwargs: cleanup_report(module)["admitted_target_identity"],
    )
    monkeypatch.setattr(
        module,
        "cleanup_target",
        lambda *args, **kwargs: cleanup_report(module),
    )
    return calls


def test_supervisor_executes_exact_control_then_publishes_after_cleanup(
    tmp_path, monkeypatch
):
    module = load_module()
    args = arguments(tmp_path, module, monkeypatch)
    calls = install_successful_execution(monkeypatch, module)

    module.emit(args)

    report = json.loads(args.output.read_text(encoding="utf-8"))
    assert [call[0] for call in calls] == [
        "artifact_directory_identity",
        "snapshot_rebuild",
        "native_restore",
    ]
    assert calls[0][1][:3] == ["postgres", "native", "directory-identity"]
    assert calls[1][1][:3] == ["postgres", "snapshots", "rebuild"]
    assert calls[2][1][:3] == ["postgres", "native", "restore"]
    assert all(call[2] == sha256(args.packaged_control) for call in calls)
    assert report["schema"] == module.SCHEMA
    assert report["product_execution"]["controller"] == {
        "execution": "retained_exact_packaged_binary",
        "sha256": sha256(args.packaged_control),
        "size_bytes": args.packaged_control.stat().st_size,
    }
    assert [
        action["operation"] for action in report["product_execution"]["actions"]
    ] == ["artifact_directory_identity", "snapshot_rebuild", "native_restore"]
    assert report["native_restore"] == native_report(module)
    assert report["cleanup"] == cleanup_report(module)
    assert report["private_binding"]["schema"] == module.PRIVATE_BINDING_SCHEMA
    assert report["private_binding"]["root_name"] == module.PRIVATE_BINDING_ROOT_NAME
    assert set(report["private_binding"]["files"]) == set(module.PRIVATE_BINDING_FILES)
    assert args.passfile.read_bytes() == b""
    retained_roots = list(args.work_parent.iterdir())
    assert len(retained_roots) == 2
    binding_root = args.work_parent / module.PRIVATE_BINDING_ROOT_NAME
    scrubbed_root = next(path for path in retained_roots if path != binding_root)
    assert all(path.stat().st_size == 0 for path in scrubbed_root.iterdir())
    assert {path.name for path in binding_root.iterdir()} == set(
        module.PRIVATE_BINDING_FILES.values()
    )
    assert all(path.stat().st_size > 0 for path in binding_root.iterdir())


def test_supervisor_requires_explicit_success_passfile_scrub(tmp_path, monkeypatch):
    module = load_module()
    args = arguments(tmp_path, module, monkeypatch)
    args.scrub_passfile_on_success = False
    original = args.passfile.read_bytes()

    with pytest.raises(module.HostedReportError, match="explicit exact passfile scrub"):
        module.emit(args)

    assert args.passfile.read_bytes() == original
    assert not args.output.exists()


def test_structurally_plausible_ready_bytes_cannot_bypass_product_execution(
    tmp_path, monkeypatch
):
    module = load_module()
    args = arguments(tmp_path, module, monkeypatch)

    def reject_execution(*_args, **_kwargs):
        raise module.HostedReportError("exact packaged execution did not run")

    monkeypatch.setattr(module, "run_exact", reject_execution)
    monkeypatch.setattr(
        module,
        "observe_target_identity",
        lambda *args, **kwargs: cleanup_report(module)["admitted_target_identity"],
    )
    monkeypatch.setattr(module, "cleanup_target", lambda *args, **kwargs: {})
    write_json(tmp_path / "plausible-native.json", native_report(module))

    with pytest.raises(module.HostedReportError, match="did not run"):
        module.emit(args)
    assert not args.output.exists()


@pytest.mark.parametrize(
    ("tamper", "message"),
    [
        (
            lambda args: args.artifact.write_bytes(b"substituted archive"),
            "invalid or unexpected byte length",
        ),
        (
            lambda args: args.packaged_control.write_bytes(b"substituted control"),
            "invalid or unexpected byte length",
        ),
        (
            lambda args: write_json(
                args.fixture_report,
                {
                    **json.loads(args.fixture_report.read_text(encoding="utf-8")),
                    "source_construction": {
                        "classification": "trusted_product_execution",
                        "source_revision": REVISION,
                        "trusted_product_execution": True,
                    },
                },
            ),
            "explicitly untrusted",
        ),
    ],
)
def test_supervisor_rejects_unbound_inputs(tmp_path, monkeypatch, tamper, message):
    module = load_module()
    args = arguments(tmp_path, module, monkeypatch)
    install_successful_execution(monkeypatch, module)
    tamper(args)

    with pytest.raises(module.HostedReportError, match=message):
        module.emit(args)
    assert not args.output.exists()


def test_supervisor_rejects_uncommitted_native_report_and_cleanup_failure(
    tmp_path, monkeypatch
):
    module = load_module()
    args = arguments(tmp_path, module, monkeypatch)
    incomplete = {**native_report(module), "status": "incomplete"}
    install_successful_execution(monkeypatch, module, report=incomplete)
    with pytest.raises(module.HostedReportError, match="committed Ready"):
        module.emit(args)
    assert not args.output.exists()
    assert args.passfile.read_bytes() == b""

    args = arguments(tmp_path / "cleanup", module, monkeypatch)
    install_successful_execution(monkeypatch, module)

    def cleanup_failure(*_args, **_kwargs):
        raise module.HostedReportError("cleanup observation failed")

    monkeypatch.setattr(module, "cleanup_target", cleanup_failure)
    with pytest.raises(module.HostedReportError, match="cleanup observation failed"):
        module.emit(args)
    assert not args.output.exists()


def test_containment_uncertainty_preserves_provider_and_private_artifacts(
    tmp_path, monkeypatch
):
    module = load_module()
    args = arguments(tmp_path, module, monkeypatch)
    operations: list[str] = []
    cleanup_calls: list[object] = []
    scrub_calls: list[object] = []

    def uncertain_execution(_executable, operation, _argv, **kwargs):
        operations.append(operation)
        if operation == "artifact_directory_identity":
            root = kwargs["directory_authority"]
            (root / "preserved-artifact").write_bytes(b"recovery authority")
            receipt = {
                "schema": module.DIRECTORY_RECEIPT_SCHEMA,
                "status": "observed",
                "artifact_directory_identity": module._expected_directory_identity(
                    root
                ),
                "secrets_emitted": False,
            }
            return (
                command_evidence(module, operation),
                (json.dumps(receipt) + "\n").encode(),
                b"",
            )
        raise module.ContainmentUncertainError("injected containment uncertainty")

    monkeypatch.setattr(module, "run_exact", uncertain_execution)
    monkeypatch.setattr(
        module,
        "observe_source_identity",
        lambda *args, **kwargs: native_report(module)["source_provider_identity"],
    )
    monkeypatch.setattr(
        module,
        "observe_target_identity",
        lambda *args, **kwargs: cleanup_report(module)["admitted_target_identity"],
    )
    monkeypatch.setattr(
        module,
        "cleanup_target",
        lambda *args, **kwargs: cleanup_calls.append(object()),
    )
    original_scrub = module.RetainedDirectory.scrub_exact

    def record_scrub(root):
        scrub_calls.append(root)
        return original_scrub(root)

    monkeypatch.setattr(module.RetainedDirectory, "scrub_exact", record_scrub)

    with pytest.raises(
        module.ContainmentUncertainError, match="injected containment uncertainty"
    ):
        module.emit(args)

    assert operations == ["artifact_directory_identity", "snapshot_rebuild"]
    assert cleanup_calls == []
    assert scrub_calls == []
    preserved = list(args.work_parent.glob("*/preserved-artifact"))
    assert len(preserved) == 1
    assert preserved[0].read_bytes() == b"recovery authority"
    assert not args.output.exists()


def test_failed_identity_bound_recovery_cannot_be_overridden_by_cleanup(
    tmp_path, monkeypatch
):
    module = load_module()
    args = arguments(tmp_path, module, monkeypatch)
    calls = install_successful_execution(monkeypatch, module)
    cleanup_calls: list[object] = []
    scrub_calls: list[object] = []

    def fail_restore(executable, operation, argv, **kwargs):
        if operation == "native_restore":
            raise module.HostedReportError("injected native restore failure")
        return original_run(executable, operation, argv, **kwargs)

    original_run = module.run_exact
    monkeypatch.setattr(module, "run_exact", fail_restore)
    monkeypatch.setattr(
        module,
        "_recover_if_needed",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            module.HostedReportError("identity-bound recovery refused")
        ),
    )
    monkeypatch.setattr(
        module,
        "cleanup_target",
        lambda *args, **kwargs: cleanup_calls.append(object()),
    )
    monkeypatch.setattr(
        module.RetainedDirectory,
        "scrub_exact",
        lambda root: scrub_calls.append(root),
    )

    with pytest.raises(module.RecoveryPreservedError, match="native restore failure"):
        module.emit(args)

    assert [call[0] for call in calls] == [
        "artifact_directory_identity",
        "snapshot_rebuild",
    ]
    assert cleanup_calls == []
    assert scrub_calls == []
    assert not args.output.exists()


def test_nested_recovery_containment_uncertainty_overrides_primary_error(
    tmp_path, monkeypatch
):
    module = load_module()
    args = arguments(tmp_path, module, monkeypatch)
    install_successful_execution(monkeypatch, module)
    original_run = module.run_exact

    def fail_restore(executable, operation, argv, **kwargs):
        if operation == "native_restore":
            raise module.HostedReportError("primary restore failure")
        return original_run(executable, operation, argv, **kwargs)

    monkeypatch.setattr(module, "run_exact", fail_restore)
    monkeypatch.setattr(
        module,
        "_recover_if_needed",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            module.ContainmentUncertainError("nested recovery containment uncertainty")
        ),
    )
    cleanup_calls: list[object] = []
    monkeypatch.setattr(
        module,
        "cleanup_target",
        lambda *args, **kwargs: cleanup_calls.append(object()),
    )

    with pytest.raises(
        module.ContainmentUncertainError,
        match="nested recovery containment uncertainty",
    ):
        module.emit(args)

    assert cleanup_calls == []
    assert args.passfile.read_bytes()
    assert not args.output.exists()


@pytest.mark.parametrize(
    "failure_type",
    ("ContainmentUncertainError", "RecoveryPreservedError"),
)
def test_cli_reserves_exit_42_for_preserved_recovery(
    tmp_path, monkeypatch, failure_type
):
    module = load_module()
    monkeypatch.setattr(module, "parse_args", lambda _argv: object())
    error_type = getattr(module, failure_type)
    monkeypatch.setattr(
        module,
        "emit",
        lambda _args: (_ for _ in ()).throw(error_type("preserved")),
    )

    assert module.main([]) == module.PRESERVED_RECOVERY_EXIT_CODE


def test_cli_reserves_exit_43_for_typed_safe_failure(monkeypatch):
    module = load_module()
    monkeypatch.setattr(module, "parse_args", lambda _argv: object())
    monkeypatch.setattr(
        module,
        "emit",
        lambda _args: (_ for _ in ()).throw(module.HostedReportError("safe")),
    )

    assert module.main([]) == module.SAFE_FAILURE_EXIT_CODE


def test_hosted_report_is_no_clobber_and_platform_bound(tmp_path, monkeypatch):
    module = load_module()
    args = arguments(tmp_path, module, monkeypatch)
    install_successful_execution(monkeypatch, module)
    args.output.write_text("sentinel", encoding="utf-8")
    with pytest.raises(module.HostedReportError, match="exclusively publish"):
        module.emit(args)
    assert args.output.read_text(encoding="utf-8") == "sentinel"
    assert args.passfile.read_bytes() == b""

    args = arguments(tmp_path / "platform", module, monkeypatch)
    install_successful_execution(monkeypatch, module)
    monkeypatch.setattr(module.platform, "system", lambda: "Windows")
    with pytest.raises(module.HostedReportError, match="platform identity"):
        module.emit(args)


def test_supervisor_rejects_symlinked_json_input(tmp_path, monkeypatch):
    module = load_module()
    args = arguments(tmp_path, module, monkeypatch)
    install_successful_execution(monkeypatch, module)
    original = args.fixture_report
    args.fixture_report = tmp_path / "fixture-link.json"
    try:
        args.fixture_report.symlink_to(original)
    except OSError:
        pytest.skip("symlinks are unavailable")

    with pytest.raises(module.HostedReportError, match="regular file"):
        module.emit(args)


def test_fingerprint_rejects_same_inode_same_size_mutation_between_passes(
    tmp_path, monkeypatch
):
    module = load_module()
    artifact = tmp_path / "artifact.tar.gz"
    initial = b"A" * (2 * 1024 * 1024)
    replacement = b"B" * len(initial)
    artifact.write_bytes(initial)
    target_identity = (artifact.stat().st_dev, artifact.stat().st_ino)
    original_read = module.os.read
    mutated = False

    def racing_read(descriptor: int, length: int) -> bytes:
        nonlocal mutated
        chunk = original_read(descriptor, length)
        observed = module.os.fstat(descriptor)
        if (
            not mutated
            and chunk
            and (observed.st_dev, observed.st_ino) == target_identity
        ):
            artifact.write_bytes(replacement)
            mutated = True
        return chunk

    monkeypatch.setattr(module.os, "read", racing_read)
    with pytest.raises(module.HostedReportError, match="changed while it was read"):
        module.fingerprint(
            artifact,
            "release archive",
            expected_size=len(initial),
            maximum_size=len(initial),
        )


def test_read_json_rejects_same_inode_same_size_mutation_between_passes(
    tmp_path, monkeypatch
):
    module = load_module()
    report = tmp_path / "report.json"
    initial = b'{"status":"pass"}\n'
    replacement = b'{"status":"fail"}\n'
    assert len(initial) == len(replacement)
    report.write_bytes(initial)
    target_identity = (report.stat().st_dev, report.stat().st_ino)
    original_read = module.os.read
    mutated = False

    def racing_read(descriptor: int, length: int) -> bytes:
        nonlocal mutated
        chunk = original_read(descriptor, length)
        observed = module.os.fstat(descriptor)
        if (
            not mutated
            and chunk
            and (observed.st_dev, observed.st_ino) == target_identity
        ):
            report.write_bytes(replacement)
            mutated = True
        return chunk

    monkeypatch.setattr(module.os, "read", racing_read)
    with pytest.raises(module.HostedReportError, match="changed while it was read"):
        module.read_json(report, "native restore report")


def test_failure_journal_runs_packaged_recovery_but_never_emits_pass(
    tmp_path, monkeypatch
):
    module = load_module()
    args = arguments(tmp_path, module, monkeypatch)
    calls: list[str] = []

    def fake_run_exact(executable, operation, argv, **kwargs):
        calls.append(operation)
        if operation == "artifact_directory_identity":
            root = kwargs["directory_authority"]
            receipt = {
                "schema": module.DIRECTORY_RECEIPT_SCHEMA,
                "status": "observed",
                "artifact_directory_identity": module._expected_directory_identity(
                    root
                ),
                "secrets_emitted": False,
            }
            return (
                command_evidence(module, operation),
                (json.dumps(receipt) + "\n").encode(),
                b"",
            )
        if operation == "snapshot_rebuild":
            receipt = {
                "schema": module.SNAPSHOT_RECEIPT_SCHEMA,
                "status": "complete",
                "rebuilt_snapshot_count": 2,
                "source_provider_identity": native_report(module)[
                    "source_provider_identity"
                ],
                "secrets_emitted": False,
            }
            return (
                command_evidence(module, operation),
                (json.dumps(receipt) + "\n").encode(),
                b"",
            )
        if operation == "native_restore":
            journal = kwargs["directory"] / (
                ".worldstream_native_recovery_" + "a" * 32 + ".json"
            )
            journal.write_text("recovery\n", encoding="utf-8")
            return command_evidence(module, operation, 1), b"failed\n", b""
        return command_evidence(module, operation), b'{"status":"ok"}\n', b""

    monkeypatch.setattr(module, "run_exact", fake_run_exact)
    monkeypatch.setattr(
        module,
        "observe_source_identity",
        lambda *args, **kwargs: native_report(module)["source_provider_identity"],
    )
    monkeypatch.setattr(
        module,
        "observe_target_identity",
        lambda *args, **kwargs: cleanup_report(module)["admitted_target_identity"],
    )
    monkeypatch.setattr(
        module,
        "cleanup_target",
        lambda *args, **kwargs: cleanup_report(module),
    )

    with pytest.raises(module.HostedReportError, match="native restore.*failed closed"):
        module.emit(args)
    assert calls == [
        "artifact_directory_identity",
        "snapshot_rebuild",
        "native_restore",
        "native_recover",
    ]
    assert not args.output.exists()
    retained_roots = list(args.work_parent.iterdir())
    assert len(retained_roots) == 1
    assert all(path.stat().st_size == 0 for path in retained_roots[0].iterdir())


def test_parser_has_no_asserted_cleanup_or_caller_native_report():
    module = load_module()
    parser_source = SCRIPT.read_text(encoding="utf-8")
    assert callable(module.parse_args)
    assert "--cleanup-complete" not in parser_source
    assert "--native-report" not in parser_source
    assert '"--packaged-control"' in parser_source
    assert '"--work-parent"' in parser_source
    assert '"--passfile-sha256"' in parser_source
    assert '"--passfile-file-id"' in parser_source
    assert '"--work-parent-file-id"' in parser_source


def test_wrapper_passfile_binding_rejects_replacement_before_any_provider_action(
    tmp_path, monkeypatch
):
    module = load_module()
    args = arguments(tmp_path / "binding", module, monkeypatch)
    original = args.passfile.read_bytes()
    held = args.passfile.with_name("held.pgpass")
    args.passfile.rename(held)
    args.passfile.write_bytes(b"127.0.0.1:5432:*:postgres:victim-password\n")
    args.passfile.chmod(0o600)

    with pytest.raises(
        module.HostedReportError,
        match="differs from the wrapper-retained identity or content",
    ):
        module.emit(args)

    assert held.read_bytes() == original
    assert args.passfile.read_bytes().endswith(b"victim-password\n")
    assert list(args.work_parent.iterdir()) == []


def test_wrapper_passfile_binding_rejects_same_inode_content_rewrite(
    tmp_path, monkeypatch
):
    module = load_module()
    args = arguments(tmp_path / "content-binding", module, monkeypatch)
    replacement = b"127.0.0.1:5432:*:postgres:changed-password\n"
    args.passfile.write_bytes(replacement)
    args.passfile.chmod(0o600)

    with pytest.raises(
        module.HostedReportError,
        match="differs from the wrapper-retained identity or content",
    ):
        module.emit(args)

    assert args.passfile.read_bytes() == replacement
    assert list(args.work_parent.iterdir()) == []


def test_wrapper_work_parent_binding_rejects_substitution_before_artifact_creation(
    tmp_path, monkeypatch
):
    module = load_module()
    args = arguments(tmp_path / "work-binding", module, monkeypatch)
    admitted = args.work_parent.with_name("admitted-work-parent")
    args.work_parent.rename(admitted)
    args.work_parent.mkdir(mode=0o700)

    with pytest.raises(
        module.HostedReportError,
        match="work parent differs from the wrapper-retained identity",
    ):
        module.emit(args)

    assert list(admitted.iterdir()) == []
    assert list(args.work_parent.iterdir()) == []
    assert args.passfile.read_bytes()


@pytest.mark.skipif(
    os.name == "nt", reason="Unix private-child substitution regression"
)
def test_hosted_cleanup_never_scrubs_child_substituted_after_committed_receipt(
    tmp_path, monkeypatch
):
    module = load_module()
    args = arguments(tmp_path / "child-substitution", module, monkeypatch)
    install_successful_execution(monkeypatch, module)
    original_scrub = module.RetainedDirectory.scrub_exact
    observations: dict[str, Path] = {}

    def substitute_before_scrub(root):
        if root.name == module.PRIVATE_BINDING_ROOT_NAME:
            return original_scrub(root)
        report = root.path / "native-restore-report.json"
        held = root.parent_path / "held-native-restore-report.json"
        victim = root.parent_path / "victim.json"
        report.rename(held)
        victim.write_bytes(b"victim must survive")
        victim.rename(report)
        observations.update(report=report, held=held)
        return original_scrub(root)

    monkeypatch.setattr(
        module.RetainedDirectory,
        "scrub_exact",
        substitute_before_scrub,
    )

    with pytest.raises(
        module.RecoveryPreservedError,
        match="retained private artifact changed before exact scrub",
    ):
        module.emit(args)

    assert observations["report"].read_bytes() == b"victim must survive"
    assert observations["held"].read_bytes()
    assert args.passfile.read_bytes()
    assert not args.output.exists()


def test_exact_runner_bounds_output_scans_secrets_and_sanitizes_pg_environment(
    tmp_path, monkeypatch
):
    module = load_module()
    executable_path = Path(sys.executable).resolve()
    executable = module.RetainedFile.open(
        executable_path,
        "test executable",
        maximum_size=module.MAX_CONTROL_BYTES,
    )
    try:
        monkeypatch.setenv("PGPASSWORD", "ambient-must-not-pass")
        evidence, stdout, stderr = module.run_exact(
            executable,
            "environment_probe",
            [
                "-c",
                "import os; print(','.join(sorted(k for k in os.environ if k.startswith('PG'))))",
            ],
            inherited=(),
            environment=module._sanitized_environment(),
            directory=tmp_path,
            timeout_seconds=10,
            secrets=(b"fixture-password",),
        )
        assert evidence.exit_code == 0
        assert stdout == b"\n"
        assert stderr == b""

        with pytest.raises(module.HostedReportError, match="credential material"):
            module.run_exact(
                executable,
                "secret_probe",
                ["-c", "print('fixture-password')"],
                inherited=(),
                environment=module._sanitized_environment(),
                directory=tmp_path,
                timeout_seconds=10,
                secrets=(b"fixture-password",),
            )

        with pytest.raises(module.HostedReportError, match="bounded output"):
            module.run_exact(
                executable,
                "output_probe",
                [
                    "-c",
                    f"import sys; sys.stdout.buffer.write(b'x'*{module.MAX_COMMAND_OUTPUT_BYTES + 1})",
                ],
                inherited=(),
                environment=module._sanitized_environment(),
                directory=tmp_path,
                timeout_seconds=10,
                secrets=(),
            )
    finally:
        executable.close()


def _pid_is_running(pid: int) -> bool:
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        handle = kernel32.OpenProcess(0x1000, False, pid)
        if not handle:
            return False
        try:
            exit_code = wintypes.DWORD()
            assert kernel32.GetExitCodeProcess(handle, ctypes.byref(exit_code))
            return exit_code.value == 259
        finally:
            kernel32.CloseHandle(handle)
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    status = Path(f"/proc/{pid}/stat")
    if status.exists():
        try:
            return status.read_text(encoding="utf-8").split()[2] != "Z"
        except (OSError, IndexError):
            return False
    return True


def test_exact_runner_terminates_descendants_after_successful_leader(tmp_path):
    module = load_module()
    executable = module.RetainedFile.open(
        Path(sys.executable).resolve(),
        "test executable",
        maximum_size=module.MAX_CONTROL_BYTES,
    )
    pid_path = tmp_path / "descendant.pid"
    leader = (
        "import pathlib,subprocess,sys;"
        "child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)']);"
        "pathlib.Path(sys.argv[1]).write_text(str(child.pid));"
        "print('retained leader complete')"
    )
    try:
        evidence, stdout, stderr = module.run_exact(
            executable,
            "descendant_probe",
            ["-c", leader, os.fspath(pid_path)],
            inherited=(),
            environment=module._sanitized_environment(),
            directory=tmp_path,
            timeout_seconds=10,
            secrets=(),
        )
        assert evidence.exit_code == 0
        assert stdout == b"retained leader complete\n"
        assert stderr == b""
        descendant = int(pid_path.read_text(encoding="utf-8"))
        deadline = time.monotonic() + 2
        while _pid_is_running(descendant) and time.monotonic() < deadline:
            time.sleep(0.01)
        assert not _pid_is_running(descendant)
    finally:
        executable.close()


def test_exact_runner_fails_on_non_absence_process_group_error(tmp_path, monkeypatch):
    module = load_module()
    executable = module.RetainedFile.open(
        Path(sys.executable).resolve(),
        "test executable",
        maximum_size=module.MAX_CONTROL_BYTES,
    )

    def reject_killpg(_process_group: int, _signal: int) -> None:
        raise OSError(5, "injected process-group failure")

    monkeypatch.setattr(module.os, "killpg", reject_killpg)
    try:
        with pytest.raises(module.HostedReportError, match="could not be terminated"):
            module.run_exact(
                executable,
                "kill_error_probe",
                ["-c", "print('complete')"],
                inherited=(),
                environment=module._sanitized_environment(),
                directory=tmp_path,
                timeout_seconds=10,
                secrets=(),
            )
    finally:
        executable.close()


@pytest.mark.skipif(os.name == "nt", reason="Unix wait classification regression")
def test_process_wait_error_is_containment_uncertainty(monkeypatch):
    module = load_module()
    monkeypatch.setattr(module.os, "killpg", lambda *_args: None)

    class WaitFailure:
        pid = 31337

        @staticmethod
        def wait(*, timeout):
            assert timeout == 5
            raise ChildProcessError("injected wait failure")

    with pytest.raises(
        module.ContainmentUncertainError, match="did not terminate within its bound"
    ):
        module._terminate_process_tree(WaitFailure(), None)


def test_provider_environment_maps_tls_exactly_and_drops_ambient_pg(monkeypatch):
    module = load_module()
    monkeypatch.setenv("PGPASSWORD", "ambient-secret")
    monkeypatch.setenv("PGSSLMODE", "allow")

    disabled = module._sanitized_environment(
        passfile="/retained/passfile", tls_mode="disable"
    )
    required = module._sanitized_environment(
        passfile="/retained/passfile", tls_mode="require"
    )

    assert disabled["PGSSLMODE"] == "disable"
    assert required["PGSSLMODE"] == "verify-full"
    assert disabled["PGGSSENCMODE"] == required["PGGSSENCMODE"] == "disable"
    assert "PGSSLROOTCERT" not in disabled
    assert required["PGSSLROOTCERT"] == "system"
    assert "PGPASSWORD" not in disabled
    assert "PGPASSWORD" not in required


def test_passfile_secret_scan_decodes_escaped_passwords_and_rejects_truncation(
    tmp_path,
):
    module = load_module()
    passfile_path = tmp_path / "operator.pgpass"
    encoded_password = b"prefix\\:suffix\\\\tail"
    passfile_path.write_bytes(b"127.0.0.1:5432:*:postgres:" + encoded_password + b"\n")
    passfile_path.chmod(0o600)
    retained = module.RetainedFile.open(
        passfile_path,
        "operator passfile",
        maximum_size=module.MAX_PASSFILE_BYTES,
    )
    try:
        assert module._passfile_secrets(retained) == (
            b"prefix:suffix\\tail",
            encoded_password,
        )
    finally:
        retained.close()

    passfile_path.write_bytes(b"127.0.0.1:5432:*:postgres:trailing\\\n")
    retained = module.RetainedFile.open(
        passfile_path,
        "operator passfile",
        maximum_size=module.MAX_PASSFILE_BYTES,
    )
    try:
        with pytest.raises(module.HostedReportError, match="trailing escape"):
            module._passfile_secrets(retained)
    finally:
        retained.close()


def test_retained_passfile_scrub_zeros_only_the_admitted_file(tmp_path):
    module = load_module()
    passfile_path = tmp_path / "operator.pgpass"
    passfile_path.write_bytes(b"127.0.0.1:5432:*:postgres:sensitive\n")
    passfile_path.chmod(0o600)
    retained = module.RetainedFile.open(
        passfile_path,
        "operator passfile",
        maximum_size=module.MAX_PASSFILE_BYTES,
        writable=True,
    )
    try:
        retained.scrub_exact()
        assert passfile_path.read_bytes() == b""
        retained.verify()
    finally:
        retained.close()


@pytest.mark.skipif(os.name == "nt", reason="Unix hard-link fixture")
def test_passfile_scrub_admission_rejects_hard_link_victim(tmp_path):
    module = load_module()
    victim = tmp_path / "victim"
    victim.write_bytes(b"127.0.0.1:5432:*:postgres:must-survive\n")
    victim.chmod(0o600)
    passfile = tmp_path / "operator.pgpass"
    os.link(victim, passfile)

    with pytest.raises(module.HostedReportError, match="must not have hard links"):
        module.RetainedFile.open(
            passfile,
            "operator passfile",
            maximum_size=module.MAX_PASSFILE_BYTES,
            writable=True,
        )
    assert victim.read_bytes().endswith(b"must-survive\n")


@pytest.mark.skipif(os.name == "nt", reason="Unix publication substitution regression")
def test_output_parent_substitution_never_overwrites_victim(tmp_path, monkeypatch):
    module = load_module()
    parent = tmp_path / "publish"
    parent.mkdir(mode=0o700)
    output = parent / "hosted.json"
    renamed = tmp_path / "retained-publish"
    original_read = module._read_pass
    substituted = False

    def substitute_before_verification(descriptor, maximum_size, label):
        nonlocal substituted
        if label == "hosted restore output" and not substituted:
            substituted = True
            parent.rename(renamed)
            parent.mkdir(mode=0o700)
            output.write_bytes(b"victim must survive")
        return original_read(descriptor, maximum_size, label)

    monkeypatch.setattr(module, "_read_pass", substitute_before_verification)
    with pytest.raises(module.HostedReportError, match="changed during publication"):
        module.write_exclusive(output, {"status": "pass"})

    assert output.read_bytes() == b"victim must survive"
    assert (renamed / "hosted.json").read_bytes() == b""


@pytest.mark.skipif(os.name == "nt", reason="Unix rename/substitution regression")
def test_private_root_substitution_preserves_exact_root_and_never_touches_victim(
    tmp_path,
):
    module = load_module()
    parent = tmp_path / "parent"
    parent.mkdir(mode=0o700)
    root = module._private_work_root(parent)
    exact = module.RetainedFile.create_in(
        root,
        "native-restore.dump",
        "private dump",
        b"sensitive exact bytes",
        maximum_size=1024,
    )
    exact.close()
    renamed = parent / "renamed-exact-root"
    os.rename(root.path, renamed)
    root.path.mkdir(mode=0o700)
    victim = root.path / "victim.txt"
    victim.write_bytes(b"must survive")
    try:
        with pytest.raises(module.HostedReportError, match="private work root changed"):
            root.scrub_exact()
        assert (renamed / "native-restore.dump").read_bytes() == (
            b"sensitive exact bytes"
        )
        assert victim.read_bytes() == b"must survive"
    finally:
        root.close()


@pytest.mark.skipif(os.name == "nt", reason="Unix hard-link fixture")
def test_private_root_scrub_rejects_hard_link_without_touching_victim(tmp_path):
    module = load_module()
    parent = tmp_path / "parent"
    parent.mkdir(mode=0o700)
    root = module._private_work_root(parent)
    exact = module.RetainedFile.create_in(
        root,
        "native-restore.dump",
        "private dump",
        b"exact secret",
        maximum_size=1024,
    )
    exact.close()
    victim = parent / "victim"
    victim.write_bytes(b"victim must survive")
    os.link(victim, root.path / "injected-hard-link")
    try:
        with pytest.raises(module.HostedReportError, match="unsafe artifact"):
            root.scrub_exact()
        assert victim.read_bytes() == b"victim must survive"
        assert (root.path / "native-restore.dump").read_bytes() == b"exact secret"
    finally:
        root.close()


@pytest.mark.skipif(os.name == "nt", reason="Unix child substitution regression")
def test_private_root_scrub_rejects_replaced_retained_child_without_victim_damage(
    tmp_path,
):
    module = load_module()
    parent = tmp_path / "parent"
    parent.mkdir(mode=0o700)
    root = module._private_work_root(parent)
    exact = module.RetainedFile.create_in(
        root,
        "native-restore.dump",
        "private dump",
        b"exact secret",
        maximum_size=1024,
    )
    exact.close()
    held = parent / "held-exact"
    (root.path / "native-restore.dump").rename(held)
    victim = parent / "victim"
    victim.write_bytes(b"victim must survive")
    victim.rename(root.path / "native-restore.dump")
    try:
        with pytest.raises(
            module.HostedReportError,
            match="retained private artifact changed before exact scrub",
        ):
            root.scrub_exact()
        assert (root.path / "native-restore.dump").read_bytes() == (
            b"victim must survive"
        )
        assert held.read_bytes() == b"exact secret"
    finally:
        root.close()


@pytest.mark.skipif(os.name != "nt", reason="Windows ancestor handle contract")
def test_windows_private_root_retains_every_ancestor_against_rename(tmp_path):
    module = load_module()
    outer = tmp_path / "outer"
    parent = outer / "parent"
    parent.mkdir(mode=0o700, parents=True)
    root = module._private_work_root(parent)
    try:
        attempt = subprocess.run(
            [
                sys.executable,
                "-c",
                "import os,sys; os.rename(sys.argv[1],sys.argv[2])",
                os.fspath(outer),
                os.fspath(tmp_path / "moved-outer"),
            ],
            check=False,
            capture_output=True,
        )
        assert attempt.returncode != 0
        root.verify()
    finally:
        root.close()


@pytest.mark.skipif(os.name != "nt", reason="Windows reparse-point contract")
def test_windows_private_root_rejects_reparse_parent(tmp_path):
    module = load_module()
    target = tmp_path / "target"
    target.mkdir(mode=0o700)
    link = tmp_path / "junction-or-link"
    try:
        link.symlink_to(target, target_is_directory=True)
    except OSError:
        pytest.skip("Windows directory symlinks are unavailable")
    with pytest.raises(module.HostedReportError, match="non-reparse directory"):
        module._private_work_root(link)


@pytest.mark.skipif(os.name == "nt", reason="Unix cleanup-name substitution regression")
def test_conditional_cleanup_script_is_exclusive_and_retained(tmp_path):
    module = load_module()
    parent = tmp_path / "parent"
    parent.mkdir(mode=0o700)
    root = module._private_work_root(parent)
    victim = parent / "victim.psql"
    victim.write_bytes(b"SELECT 'victim';\n")
    named_script = root.path / "cleanup-target.psql"
    try:
        try:
            named_script.symlink_to(victim)
        except OSError:
            pytest.skip("symlinks are unavailable")
        with pytest.raises(module.HostedReportError, match="exclusively create"):
            module.RetainedFile.create_in(
                root,
                "cleanup-target.psql",
                "conditional cleanup script",
                b"SELECT 'safe';\n",
                maximum_size=1024,
            )
        assert victim.read_bytes() == b"SELECT 'victim';\n"
        named_script.unlink()

        retained = module.RetainedFile.create_in(
            root,
            "cleanup-target.psql",
            "conditional cleanup script",
            b"SELECT 'safe';\n",
            maximum_size=1024,
        )
        moved = root.path / "moved-cleanup.psql"
        os.rename(named_script, moved)
        named_script.write_bytes(b"DROP DATABASE victim;\n")
        try:
            assert (
                module._read_pass(
                    retained.descriptor,
                    retained.maximum_size,
                    retained.label,
                )
                == b"SELECT 'safe';\n"
            )
            with pytest.raises(
                module.HostedReportError, match="changed while it was retained"
            ):
                retained.verify()
        finally:
            retained.close()
    finally:
        root.close()


@pytest.mark.skipif(os.name != "nt", reason="Windows share-mode contract")
def test_windows_retained_file_denies_write_and_path_replacement(tmp_path):
    module = load_module()
    protected = tmp_path / "worldstreamctl.exe"
    protected.write_bytes(b"exact-controller")
    replacement = tmp_path / "replacement.exe"
    replacement.write_bytes(b"substitution")
    retained = module.RetainedFile.open(
        protected,
        "retained Windows control",
        maximum_size=module.MAX_CONTROL_BYTES,
    )
    try:
        write_attempt = subprocess.run(
            [
                sys.executable,
                "-c",
                "import pathlib,sys; pathlib.Path(sys.argv[1]).write_bytes(b'forged')",
                os.fspath(protected),
            ],
            check=False,
            capture_output=True,
        )
        replace_attempt = subprocess.run(
            [
                sys.executable,
                "-c",
                "import os,sys; os.replace(sys.argv[1],sys.argv[2])",
                os.fspath(replacement),
                os.fspath(protected),
            ],
            check=False,
            capture_output=True,
        )
        assert write_attempt.returncode != 0
        assert replace_attempt.returncode != 0
        retained.verify()
        assert protected.read_bytes() == b"exact-controller"
    finally:
        retained.close()


@pytest.mark.skipif(os.name != "nt", reason="Windows ancestor handle contract")
def test_windows_retained_file_denies_ancestor_replacement(tmp_path):
    module = load_module()
    outer = tmp_path / "outer"
    parent = outer / "tools"
    parent.mkdir(parents=True)
    protected = parent / "psql.exe"
    protected.write_bytes(b"exact-provider-tool")
    retained = module.RetainedFile.open(
        protected,
        "retained Windows psql",
        maximum_size=module.MAX_PROVIDER_TOOL_BYTES,
    )
    try:
        attempt = subprocess.run(
            [
                sys.executable,
                "-c",
                "import os,sys; os.rename(sys.argv[1],sys.argv[2])",
                os.fspath(outer),
                os.fspath(tmp_path / "moved-outer"),
            ],
            check=False,
            capture_output=True,
        )
        assert attempt.returncode != 0
        retained.verify()
    finally:
        retained.close()


@pytest.mark.skipif(os.name != "nt", reason="Windows passfile reopen contract")
def test_windows_read_retained_passfile_reopens_exactly_for_scrub(tmp_path):
    module = load_module()
    passfile = tmp_path / "operator.pgpass"
    passfile.write_bytes(b"127.0.0.1:5432:*:postgres:sensitive\n")
    retained = module.RetainedFile.open(
        passfile,
        "operator passfile",
        maximum_size=module.MAX_PASSFILE_BYTES,
        writable=False,
    )
    try:
        retained.scrub_exact()
        assert passfile.read_bytes() == b""
        retained.verify()
    finally:
        retained.close()


def test_cleanup_refuses_identity_change_and_conditionally_drops_in_one_session(
    tmp_path, monkeypatch
):
    module = load_module()
    args = argparse.Namespace(
        target_host="127.0.0.1",
        target_port=5432,
        target_database="worldstream_native_restore",
        target_username="postgres",
        target_tls_mode="disable",
    )
    expected = {
        "system_identifier": "123456789",
        "database_oid": "16384",
        "database_name": args.target_database,
    }
    psql = object()
    passfile = argparse.Namespace(execution_path=lambda: "/retained/passfile")
    work_parent = tmp_path / "work"
    work_parent.mkdir(mode=0o700)
    root = module._private_work_root(work_parent)
    monkeypatch.setattr(
        module,
        "observe_target_identity",
        lambda *args, **kwargs: {**expected, "database_oid": "16385"},
    )
    with pytest.raises(module.HostedReportError, match="identity changed"):
        module.cleanup_target(
            args,
            psql,
            passfile,
            root,
            (),
            require_target=True,
            expected_identity=expected,
        )

    monkeypatch.setattr(
        module, "observe_target_identity", lambda *args, **kwargs: expected
    )
    observations = iter(["0", "0", "123456789\t0\t0"])
    monkeypatch.setattr(
        module,
        "_run_psql",
        lambda *args, **kwargs: (
            command_evidence(module, "cleanup_probe"),
            next(observations),
        ),
    )
    drop_argv: list[str] = []
    observed_script: list[str] = []

    def fake_drop(executable, operation, argv, **kwargs):
        drop_argv.extend(argv)
        cleanup_file = kwargs["inherited"][1]
        observed_script.append(
            module._read_pass(
                cleanup_file.descriptor,
                cleanup_file.maximum_size,
                cleanup_file.label,
            ).decode()
        )
        return command_evidence(module, operation), b"DROP DATABASE\n", b""

    monkeypatch.setattr(module, "run_exact", fake_drop)
    cleanup = module.cleanup_target(
        args,
        psql,
        passfile,
        root,
        (),
        require_target=True,
        expected_identity=expected,
    )
    script = observed_script[0]
    assert "\\if :admitted" in script
    assert "system_identifier::text='123456789'" in script
    assert "database.oid=16384" in script
    assert 'DROP DATABASE "worldstream_native_restore";' in script
    assert cleanup["admitted_target_identity"] == expected
    root.scrub_exact()
    root.close()


def test_absent_target_cleanup_proof_is_bound_to_admitted_cluster(
    tmp_path, monkeypatch
):
    module = load_module()
    args = argparse.Namespace(
        target_host="127.0.0.1",
        target_port=5432,
        target_database="worldstream_native_restore",
        target_username="postgres",
        target_tls_mode="disable",
    )
    expected = {
        "system_identifier": "123456789",
        "database_oid": "16384",
        "database_name": args.target_database,
    }
    work_parent = tmp_path / "work"
    work_parent.mkdir(mode=0o700)
    root = module._private_work_root(work_parent)
    monkeypatch.setattr(module, "observe_target_identity", lambda *args, **kwargs: None)
    monkeypatch.setattr(
        module,
        "_run_psql",
        lambda *args, **kwargs: (
            command_evidence(module, "cleanup_bound_absent_target"),
            "987654321\t0\t0",
        ),
    )

    with pytest.raises(module.HostedReportError, match="admitted provider"):
        module.cleanup_target(
            args,
            object(),
            argparse.Namespace(execution_path=lambda: "/retained/passfile"),
            root,
            (),
            require_target=False,
            expected_identity=expected,
        )
    root.close()


def test_target_observation_retains_cluster_identity_when_database_is_absent(
    monkeypatch,
):
    module = load_module()
    args = argparse.Namespace(target_database="worldstream_native_restore")
    statements: list[str] = []

    def absent_observation(*call_args, **_kwargs):
        statements.append(call_args[5])
        return command_evidence(module, "observe_target"), "123456789\t\t\t"

    monkeypatch.setattr(module, "_run_psql", absent_observation)
    assert (
        module.observe_target_identity(
            args,
            object(),
            object(),
            object(),
            (),
            "observe_target",
            expected_system_identifier="123456789",
        )
        is None
    )
    assert "COALESCE(database.oid::text,'')" in statements[0]

    with pytest.raises(module.HostedReportError, match="cluster changed"):
        module.observe_target_identity(
            args,
            object(),
            object(),
            object(),
            (),
            "observe_target",
            expected_system_identifier="987654321",
        )

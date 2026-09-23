"""Focused contract tests for the standalone Agent Swarm archives."""

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import sys
import tarfile
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "worldstream_agent_swarm_package", ROOT / "scripts/agent-swarm-package.py"
)
if SPEC is None or SPEC.loader is None:  # pragma: no cover
    raise RuntimeError("could not load Agent Swarm packager")
PACKAGE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PACKAGE
SPEC.loader.exec_module(PACKAGE)

VERIFY_SPEC = importlib.util.spec_from_file_location(
    "worldstream_agent_swarm_package_verifier",
    ROOT / "scripts/verify-agent-swarm-package.py",
)
if VERIFY_SPEC is None or VERIFY_SPEC.loader is None:  # pragma: no cover
    raise RuntimeError("could not load Agent Swarm package verifier")
VERIFY = importlib.util.module_from_spec(VERIFY_SPEC)
sys.modules[VERIFY_SPEC.name] = VERIFY
VERIFY_SPEC.loader.exec_module(VERIFY)

DIGEST = "blake3:" + "a" * 64


def write_pack(path: Path, version: str = "0.2.0", digest: str = DIGEST) -> None:
    descriptor = json.dumps(
        {
            "pack_id": PACKAGE.PACK_ID,
            "explanatory_version": version,
            "revision_digest": digest,
        },
        separators=(",", ":"),
    ).encode()
    with tarfile.open(path, "w:") as archive:
        info = tarfile.TarInfo("descriptor.json")
        info.size = len(descriptor)
        info.mode = 0o644
        archive.addfile(info, io.BytesIO(descriptor))


def fixture(tmp_path: Path, target_name: str) -> tuple[Path, Path]:
    target = PACKAGE.TARGETS[target_name]
    binaries = tmp_path / "bin"
    binaries.mkdir()
    for stem in PACKAGE.BINARIES:
        binary = binaries / f"{stem}{target.executable_suffix}"
        binary.write_bytes(f"fixture:{stem}".encode())
        binary.chmod(0o755)
    pack = tmp_path / "agent-swarm.wspack"
    write_pack(pack)
    return binaries, pack


def test_inspects_exact_pack_identity_for_native_build_wrappers(tmp_path: Path):
    pack = tmp_path / "agent-swarm.wspack"
    write_pack(pack)

    assert PACKAGE.inspect_pack(pack) == {
        "id": PACKAGE.PACK_ID,
        "version": "0.2.0",
        "digest": DIGEST,
    }


@pytest.mark.parametrize("target_name", ["macos-arm64", "windows-x64"])
def test_builds_and_verifies_complete_target_archive(tmp_path: Path, target_name: str):
    binaries, pack = fixture(tmp_path, target_name)
    output = tmp_path / "dist"
    archive = PACKAGE.build(
        target_name=target_name,
        binary_dir=binaries,
        pack=pack,
        output_dir=output,
        version="0.1.0",
        pack_version="0.2.0",
        pack_digest=DIGEST,
        enforce_native=False,
    )

    install = PACKAGE.verify(archive, target_name=target_name)
    assert install["execution_default"] == "suspended_until_explicit_resume"
    assert (
        install["provider_management"] == "discover_only_never_install_or_reconfigure"
    )
    assert install["pack"]["digest"] == DIGEST
    assert len(install["files"]) == len(PACKAGE.BINARIES) + 2
    assert install["configuration"] == {
        "path": PACKAGE.LOCAL_CONFIG_PATH,
        "state_roots": "operator_selected_protected_external_paths",
    }


def test_extracts_only_after_verification_and_never_replaces_destination(
    tmp_path: Path,
):
    binaries, pack = fixture(tmp_path, "macos-arm64")
    archive = PACKAGE.build(
        target_name="macos-arm64",
        binary_dir=binaries,
        pack=pack,
        output_dir=tmp_path / "dist",
        version="0.1.0",
        pack_version="0.2.0",
        pack_digest=DIGEST,
        enforce_native=False,
    )

    destination = tmp_path / "verified-extraction"
    root = PACKAGE.extract_verified(
        archive,
        target_name="macos-arm64",
        output_dir=destination,
    )
    binary = root / "bin" / "worldstream-agent-swarm"
    assert binary.read_bytes() == b"fixture:worldstream-agent-swarm"
    assert binary.stat().st_mode & 0o100
    with pytest.raises(PACKAGE.PackageError, match="refusing to replace"):
        PACKAGE.extract_verified(
            archive,
            target_name="macos-arm64",
            output_dir=destination,
        )

    members = PACKAGE._read_archive(archive)
    binary_name = next(
        name for name in members if name.endswith("/bin/worldstream-agent-swarm")
    )
    members[binary_name] = (b"tampered", members[binary_name][1])
    tampered = tmp_path / "tampered.tar.gz"
    with tarfile.open(tampered, "w:gz") as output:
        for name, (content, mode) in sorted(members.items()):
            info = tarfile.TarInfo(name)
            info.size = len(content)
            info.mode = mode
            output.addfile(info, io.BytesIO(content))
    rejected_destination = tmp_path / "rejected-extraction"
    with pytest.raises(PACKAGE.PackageError, match="digest mismatch"):
        PACKAGE.extract_verified(
            tampered,
            target_name="macos-arm64",
            output_dir=rejected_destination,
        )
    assert not rejected_destination.exists()


def test_only_previous_archive_may_omit_bundled_configuration(tmp_path: Path):
    binaries, pack = fixture(tmp_path, "macos-arm64")
    archive = PACKAGE.build(
        target_name="macos-arm64",
        binary_dir=binaries,
        pack=pack,
        output_dir=tmp_path / "dist",
        version="0.1.0",
        pack_version="0.2.0",
        pack_digest=DIGEST,
        enforce_native=False,
    )
    members = PACKAGE._read_archive(archive)
    root = next(iter({Path(name).parts[0] for name in members}))
    install_name = f"{root}/metadata/install.json"
    checksums_name = f"{root}/checksums.sha256"
    install = json.loads(members[install_name][0])
    configuration_path = install.pop("configuration")["path"]
    install["files"].pop(configuration_path)
    members.pop(f"{root}/{configuration_path}")
    install_bytes = PACKAGE._canonical_json(install)
    members[install_name] = (install_bytes, 0o644)
    checksum_entries = [
        *sorted(install["files"].items()),
        ("metadata/install.json", hashlib.sha256(install_bytes).hexdigest()),
    ]
    members[checksums_name] = (
        "".join(
            f"{digest}  {name}\n" for name, digest in sorted(checksum_entries)
        ).encode(),
        0o644,
    )
    legacy = tmp_path / "legacy.tar.gz"
    with tarfile.open(legacy, "w:gz") as output:
        for name, (content, mode) in sorted(members.items()):
            info = tarfile.TarInfo(name)
            info.size = len(content)
            info.mode = mode
            output.addfile(info, io.BytesIO(content))

    with pytest.raises(PACKAGE.PackageError, match="configuration metadata"):
        PACKAGE.verify(legacy, target_name="macos-arm64")
    verified = PACKAGE.verify(
        legacy,
        target_name="macos-arm64",
        require_configuration=False,
    )
    assert "configuration" not in verified


def test_rejects_pack_identity_mismatch_before_writing_archive(tmp_path: Path):
    binaries, pack = fixture(tmp_path, "macos-arm64")
    write_pack(pack, version="9.9.9")
    with pytest.raises(PACKAGE.PackageError, match="version mismatch"):
        PACKAGE.build(
            target_name="macos-arm64",
            binary_dir=binaries,
            pack=pack,
            output_dir=tmp_path / "dist",
            version="0.1.0",
            pack_version="0.2.0",
            pack_digest=DIGEST,
            enforce_native=False,
        )


def test_rejects_truncated_tar_without_a_traceback(tmp_path: Path):
    archive = tmp_path / "truncated.tar.gz"
    archive.write_bytes(b"\x1f\x8b\x08\x00")

    with pytest.raises(PACKAGE.PackageError, match="invalid tar archive"):
        PACKAGE.verify(archive, target_name="macos-arm64")


def test_rejects_wrong_target_and_missing_binary(tmp_path: Path):
    binaries, pack = fixture(tmp_path, "windows-x64")
    (binaries / "worldstreamctl.exe").unlink()
    with pytest.raises(PACKAGE.PackageError, match="missing binary"):
        PACKAGE.build(
            target_name="windows-x64",
            binary_dir=binaries,
            pack=pack,
            output_dir=tmp_path / "dist",
            version="0.1.0",
            pack_version="0.2.0",
            pack_digest=DIGEST,
            enforce_native=False,
        )


def test_update_extractions_are_side_by_side_and_independent(tmp_path: Path):
    members = {
        "worldstream-agent-swarm-0.2.0/bin/app": (b"binary", 0o755),
        "worldstream-agent-swarm-0.2.0/install.json": (b"{}", 0o644),
    }
    old = VERIFY.extract_application(members, tmp_path / "old")
    update = VERIFY.extract_application(members, tmp_path / "update")

    assert old != update
    assert VERIFY.tree_digest(old) == VERIFY.tree_digest(update)
    (update / "install.json").write_text('{"updated":true}', encoding="utf-8")
    assert (old / "install.json").read_text(encoding="utf-8") == "{}"


def test_update_requires_recovery_status_before_explicit_resume():
    VERIFY.assert_recovery_required(
        {
            "kind": "status",
            "value": {
                "swarms": [
                    {
                        "swarm_id": "swarm-a",
                        "desired": "running",
                        "phase": "recovery_required",
                    }
                ]
            },
        },
        "swarm-a",
    )

    with pytest.raises(RuntimeError, match="explicit Resume"):
        VERIFY.assert_recovery_required(
            {
                "kind": "status",
                "value": {
                    "swarms": [
                        {
                            "swarm_id": "swarm-a",
                            "desired": "running",
                            "phase": "running",
                        }
                    ]
                },
            },
            "swarm-a",
        )


def test_update_receipt_requires_distinct_verified_application_versions():
    VERIFY.assert_distinct_application_versions(
        {"application_version": "0.1.0"},
        {"application_version": "0.2.0"},
    )
    with pytest.raises(RuntimeError, match="distinct application version"):
        VERIFY.assert_distinct_application_versions(
            {"application_version": "0.2.0"},
            {"application_version": "0.2.0"},
        )


def test_packaged_managed_smoke_uses_bundled_binaries_and_pack(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    app_root = tmp_path / "application"
    binary_dir = app_root / "bin"
    pack = app_root / "packs" / "exact.wspack"
    binary_dir.mkdir(parents=True)
    pack.parent.mkdir(parents=True)
    pack.write_bytes(b"exact pack")
    configuration = app_root / "config" / "local.toml"
    configuration.parent.mkdir(parents=True)
    configuration.write_text("config_version = 1\n", encoding="utf-8")
    smoke_root = tmp_path / "managed smoke"
    working_area = tmp_path / "unrelated"
    working_area.mkdir()

    def fake_run(command, **kwargs):
        assert kwargs["cwd"] == working_area
        assert command[command.index("--binary-dir") + 1] == str(binary_dir)
        assert command[command.index("--pack") + 1] == str(pack)
        assert command[command.index("--config") + 1] == str(configuration)
        (smoke_root / "managed-processes-stopped").touch()
        receipt = {
            "status": "ok",
            "backend": "managed_local",
            "rooms": 2,
            "reopened": True,
            "reviewed_result": True,
            "artifact_resolved": True,
            "automatic_progress_review": True,
            "automatic_progress_review_waited_for_capacity": True,
            "coordinator_worker_contribution": True,
            "guarded_report_check": True,
            "reviewed_code_change": True,
            "code_change_conflict_preserved": True,
            "room_code_change_result": True,
            "native_shared_capacity": True,
            "native_progress_review_priority": True,
            "native_budget_pause": True,
            "native_effect_recovery": True,
        }
        return VERIFY.subprocess.CompletedProcess(command, 0, json.dumps(receipt), "")

    monkeypatch.setattr(VERIFY.subprocess, "run", fake_run)
    receipt = VERIFY.exercise_packaged_managed_application(
        app_root=app_root,
        install={
            "pack": {"path": "packs/exact.wspack"},
            "configuration": {"path": "config/local.toml"},
        },
        smoke_root=smoke_root,
        working_area=working_area,
    )
    assert receipt["rooms"] == 2


def test_packaged_update_reopen_uses_new_binary_and_retained_selected_config(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    app_root = tmp_path / "application-update"
    binary_dir = app_root / "bin"
    binary_dir.mkdir(parents=True)
    selected_root = tmp_path / "application-old"
    configuration = selected_root / "config" / "local.toml"
    configuration.parent.mkdir(parents=True)
    configuration.write_text("config_version = 1\n", encoding="utf-8")
    smoke_root = tmp_path / "retained-state"
    smoke_root.mkdir()
    working_area = tmp_path / "unrelated"
    working_area.mkdir()

    def fake_run(command, **kwargs):
        assert kwargs["cwd"] == working_area
        assert "--reopen-existing" in command
        assert command[command.index("--binary-dir") + 1] == str(binary_dir)
        assert command[command.index("--config") + 1] == str(configuration)
        (smoke_root / "managed-update-reopened").touch()
        receipt = {
            "status": "ok",
            "swarm_id": "swarm-retained",
            "accepted_results_preserved": True,
            "contributions_preserved": True,
            "artifact_preserved": True,
            "provider_selections_preserved": True,
            "tui_restored": True,
        }
        return VERIFY.subprocess.CompletedProcess(command, 0, json.dumps(receipt), "")

    monkeypatch.setattr(VERIFY.subprocess, "run", fake_run)
    receipt = VERIFY.reopen_packaged_managed_application(
        app_root=app_root,
        install={"configuration": {"path": "config/local.toml"}},
        configuration_root=selected_root,
        configuration_install={"configuration": {"path": "config/local.toml"}},
        smoke_root=smoke_root,
        working_area=working_area,
    )
    assert receipt["swarm_id"] == "swarm-retained"

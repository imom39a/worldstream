"""Boundary tests for package-only native runtime validation."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import stat
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/native-package-smoke.py"


def load_module():
    spec = importlib.util.spec_from_file_location(
        "worldstream_native_package_smoke_test", SCRIPT
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def sha256(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def package_fixture(tmp_path: Path, module):
    version = "1.2.3"
    target = "linux-x86_64"
    archive = tmp_path / f"worldstream-{version}-{target}.tar.gz"
    archive.write_bytes(b"canonical package fixture")
    manifest_toml = tmp_path / "compatibility.toml"
    manifest_toml.write_bytes(b"release_ready = true\n")
    manifest_json = tmp_path / "compatibility.json"
    manifest_json.write_text(
        json.dumps(
            {
                "manifest_kind": "release",
                "release_ready": True,
                "release_candidate": version,
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    daemon_bytes = b"packaged daemon bytes"
    control_bytes = b"packaged control bytes"
    source_revision = "1" * 40
    build_identity = (
        json.dumps({"source": {"revision": source_revision}}, sort_keys=True) + "\n"
    ).encode()
    archive_root = f"worldstream-{version}-{target}"
    entries = {
        f"{archive_root}/manifest/compatibility.toml": manifest_toml.read_bytes(),
        f"{archive_root}/manifest/compatibility.json": manifest_json.read_bytes(),
        f"{archive_root}/bin/worldstreamd": daemon_bytes,
        f"{archive_root}/bin/worldstreamctl": control_bytes,
        f"{archive_root}/{module.PACKAGE.BUILD_IDENTITY.BUILD_METADATA_PATH}": (
            build_identity
        ),
    }
    report = {
        "schema": "worldstream/package-report/v1",
        "kind": "archive",
        "artifact": archive.name,
        "path": archive.name,
        "sha256": "sha256:" + sha256(archive.read_bytes()),
        "size_bytes": archive.stat().st_size,
        "inventory": {
            "archive_verified": True,
            "release_evidence": False,
            "manifest_source": "compatibility.toml",
            "manifest_mirror": "compatibility.json",
        },
        "identity": {
            "target": target,
            "version": version,
            "manifest_sha256": sha256(manifest_json.read_bytes()),
            "manifest_json_sha256": sha256(manifest_json.read_bytes()),
            "manifest_toml_sha256": sha256(manifest_toml.read_bytes()),
            "source_revision": source_revision,
            "build_identity_sha256": "sha256:" + sha256(build_identity),
        },
    }
    report_path = tmp_path / "package-report.json"
    report_path.write_text(json.dumps(report) + "\n", encoding="utf-8")
    return {
        "archive": archive,
        "report": report,
        "report_path": report_path,
        "manifest_toml": manifest_toml,
        "manifest_json": manifest_json,
        "entries": entries,
        "daemon_bytes": daemon_bytes,
        "control_bytes": control_bytes,
    }


def test_verify_and_extract_executes_only_exact_canonical_package_bytes(
    tmp_path, monkeypatch
):
    module = load_module()
    fixture = package_fixture(tmp_path, module)
    monkeypatch.setattr(module.PACKAGE, "verify_archive", lambda _archive: None)
    monkeypatch.setattr(
        module.PACKAGE, "archive_entries", lambda _archive: fixture["entries"]
    )
    monkeypatch.setattr(module.platform, "system", lambda: "Linux")
    monkeypatch.setattr(module.platform, "machine", lambda: "x86_64")

    daemon, control, binding, manifest = module.verify_and_extract(
        fixture["archive"],
        fixture["report_path"],
        fixture["manifest_toml"],
        fixture["manifest_json"],
        tmp_path / "extracted",
    )

    assert daemon.read_bytes() == fixture["daemon_bytes"]
    assert control.read_bytes() == fixture["control_bytes"]
    assert stat.S_IMODE(daemon.stat().st_mode) == 0o700
    assert binding["canonical_archive_verified"] is True
    assert binding["exact_archive_bytes_executed"] is True
    assert binding["worldstreamd_sha256"] == module.sha256_bytes(
        fixture["daemon_bytes"]
    )
    assert binding["worldstreamctl_sha256"] == module.sha256_bytes(
        fixture["control_bytes"]
    )
    assert manifest["release_ready"] is True


def test_packaged_control_version_requires_the_exact_source_revision():
    module = load_module()
    manifest_summary = {"contracts": {"product": "1.2.3"}, "release_ready": True}
    revision = "1" * 40
    value = {
        **manifest_summary,
        "product_build": {
            "product": "1.2.3",
            "binary": "worldstreamctl",
            "build_version": "1.2.3",
            "source_revision": revision,
        },
    }

    module.validate_control_version(
        value,
        manifest_summary=manifest_summary,
        product="1.2.3",
        source_revision=revision,
        code="ctl_revision_mismatch",
    )
    value["product_build"]["source_revision"] = "2" * 40
    with pytest.raises(module.SmokeError, match="ctl_revision_mismatch"):
        module.validate_control_version(
            value,
            manifest_summary=manifest_summary,
            product="1.2.3",
            source_revision=revision,
            code="ctl_revision_mismatch",
        )


@pytest.mark.parametrize(
    "raw",
    [
        b'{"status":"bad","status":"ok"}',
        b'{"status":NaN}',
        b'{"status":Infinity}',
        b'{"status":-Infinity}',
    ],
)
def test_runtime_json_boundaries_reject_duplicate_and_nonfinite_values(raw: bytes):
    module = load_module()

    with pytest.raises(module.SmokeError, match="runtime_not_strict_json"):
        module.strict_runtime_json(raw, "runtime_not_strict_json")


def test_runtime_process_capture_fails_before_stdout_can_grow_unbounded():
    module = load_module()

    with pytest.raises(module.SmokeError, match="runtime_output_too_large"):
        module.run_bounded_process(
            [
                sys.executable,
                "-c",
                "import os; os.write(1, b'x' * (1024 * 1024))",
            ],
            environment=None,
            timeout=10,
            stdout_limit=1024,
            stderr_limit=1024,
            code="runtime_output_too_large",
        )


@pytest.mark.parametrize(
    ("tamper", "reason"),
    [
        ("report_digest", "package_report_identity_mismatch"),
        ("report_path", "package_report_identity_mismatch"),
        ("manifest_digest", "package_report_identity_mismatch"),
        ("mixed_root", "package_archive_root_mismatch"),
        ("manifest_bytes", "package_archive_manifest_bytes_mismatch"),
        ("missing_control", "package_binary_inventory_incomplete"),
    ],
)
def test_verify_and_extract_rejects_unbound_or_incomplete_package(
    tmp_path, monkeypatch, tamper, reason
):
    module = load_module()
    fixture = package_fixture(tmp_path, module)
    report = fixture["report"]
    entries = dict(fixture["entries"])
    if tamper == "report_digest":
        report["sha256"] = "sha256:" + "0" * 64
    elif tamper == "report_path":
        report["path"] = "/runner/private/package.tar.gz"
    elif tamper == "manifest_digest":
        report["identity"]["manifest_toml_sha256"] = "0" * 64
    elif tamper == "mixed_root":
        entries["different-root/bin/extra"] = b"unexpected"
    elif tamper == "manifest_bytes":
        key = next(name for name in entries if name.endswith("compatibility.toml"))
        entries[key] = b"release_ready = false\n"
    else:
        key = next(name for name in entries if name.endswith("bin/worldstreamctl"))
        del entries[key]
    fixture["report_path"].write_text(json.dumps(report) + "\n", encoding="utf-8")
    monkeypatch.setattr(module.PACKAGE, "verify_archive", lambda _archive: None)
    monkeypatch.setattr(module.PACKAGE, "archive_entries", lambda _archive: entries)
    monkeypatch.setattr(module.platform, "system", lambda: "Linux")
    monkeypatch.setattr(module.platform, "machine", lambda: "x86_64")

    with pytest.raises(module.SmokeError, match=reason):
        module.verify_and_extract(
            fixture["archive"],
            fixture["report_path"],
            fixture["manifest_toml"],
            fixture["manifest_json"],
            tmp_path / "extracted",
        )


def test_admin_dsn_is_read_from_owner_only_file_and_not_a_product_environment(
    tmp_path, monkeypatch
):
    module = load_module()
    dsn = tmp_path / "admin.dsn"
    scheme = "postgresql"
    dsn_text = f"{scheme}://admin:private-value@127.0.0.1:5432/worldstream"
    dsn.write_text(
        f"{dsn_text}\n",
        encoding="utf-8",
    )
    dsn.chmod(0o600)

    parsed = module.parse_dsn(dsn)
    environment = module.daemon_environment(
        tmp_path / "authority.secret", tmp_path / "runtime.dsn"
    )

    assert parsed == {
        "host": "127.0.0.1",
        "port": "5432",
        "dbname": "worldstream",
        "user": "admin",
        "password": "private-value",
    }
    assert "WORLDSTREAM_POSTGRES_URL" not in environment
    assert "WORLDSTREAM__STORAGE__POSTGRESQL__DSN" not in environment
    assert environment["WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE"].endswith(
        "runtime.dsn"
    )


@pytest.mark.parametrize(
    ("directory", "permission"), [(False, "(F)"), (True, "(OI)(CI)(F)")]
)
def test_windows_acl_builder_matches_runtime_three_principal_policy(
    tmp_path, monkeypatch, directory, permission
):
    module = load_module()
    calls = []
    monkeypatch.setattr(
        module, "current_windows_identity", lambda: "RUNNER\\worldstream"
    )

    def record(command, **_kwargs):
        calls.append(command)
        return type("Completed", (), {"returncode": 0})()

    monkeypatch.setattr(module.subprocess, "run", record)
    module.protect_windows_path(tmp_path / "credential", directory=directory)

    assert calls == [
        [
            "icacls",
            str(tmp_path / "credential"),
            "/inheritance:r",
            "/grant:r",
            f"RUNNER\\worldstream:{permission}",
            f"*S-1-5-18:{permission}",
            f"*S-1-5-32-544:{permission}",
        ]
    ]


def test_script_has_no_arbitrary_binary_success_cli_and_runs_packaged_admin_commands():
    source = SCRIPT.read_text(encoding="utf-8")
    assert 'command.add_argument("--worldstreamd"' not in source
    assert 'command.add_argument("--worldstreamctl"' not in source
    assert '"postgres",\n                "migrate"' in source
    assert '"postgres",\n                "verify"' in source
    assert '"--dsn-file"' in source
    assert "PGPASSWORD" in source
    assert "input_text=sql" in source

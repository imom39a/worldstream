"""Boundary tests for package-only native runtime validation."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import stat
import sys
import threading
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/native-package-smoke.py"
RUNTIME_ROLE_SQL_FRAGMENTS = (
    "role.rolsuper::text",
    "role.rolcreaterole::text",
    "role.rolcreatedb::text",
    "role.rolreplication::text",
    "role.rolbypassrls::text",
    "has_database_privilege(current_user, current_database(), 'CREATE')",
    "pg_catalog.pg_auth_members",
    "has_schema_privilege(current_user, 'public', 'CREATE')",
    "pg_catalog.pg_namespace",
    "pg_catalog.pg_class",
    "pg_catalog.pg_proc",
    "pg_catalog.pg_type",
    "'public.worldstream_schema_migrations', 'INSERT'",
    "'public.worldstream_schema_migrations', 'UPDATE'",
    "'public.worldstream_schema_migrations', 'DELETE'",
    "'public.worldstream_schema_migrations', 'TRUNCATE'",
    "protected_table.table_name, 'INSERT'",
    "protected_table.table_name, 'UPDATE'",
    "protected_table.table_name, 'DELETE'",
    "protected_table.table_name, 'TRUNCATE'",
)


def assert_runtime_role_sql(query: str) -> None:
    assert query.count("|| '|' ||") == 16
    for fragment in RUNTIME_ROLE_SQL_FRAGMENTS:
        assert fragment in query
    assert "unnest(ARRAY['INSERT'" not in query


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


def test_control_output_is_exact_bound_executable_and_no_clobber(tmp_path):
    module = load_module()
    control = tmp_path / "extracted-worldstreamctl"
    control_bytes = b"exact packaged control binary"
    control.write_bytes(control_bytes)
    output_root = tmp_path / "retained"
    output_root.mkdir()
    output = output_root / "worldstreamctl"
    binding = {
        "archive_sha256": "sha256:" + "1" * 64,
        "archive_size_bytes": 123,
        "package_report_sha256": "sha256:" + "2" * 64,
        "package_report_size_bytes": 456,
        "worldstreamctl_sha256": module.sha256_bytes(control_bytes),
        "worldstreamctl_size_bytes": len(control_bytes),
    }

    retained = module.persist_control_output(control, output, binding)

    assert output.read_bytes() == control_bytes
    if module.os.name != "nt":
        assert stat.S_IMODE(output.stat().st_mode) == 0o700
    assert retained == {
        "status": "persisted",
        "archive_sha256": binding["archive_sha256"],
        "archive_size_bytes": binding["archive_size_bytes"],
        "package_report_sha256": binding["package_report_sha256"],
        "package_report_size_bytes": binding["package_report_size_bytes"],
        "worldstreamctl_sha256": binding["worldstreamctl_sha256"],
        "worldstreamctl_size_bytes": binding["worldstreamctl_size_bytes"],
    }
    with pytest.raises(module.SmokeError, match="control_output_already_exists"):
        module.persist_control_output(control, output, binding)

    symlink_output = output_root / "worldstreamctl-link"
    try:
        symlink_output.symlink_to(control)
    except OSError:
        return
    with pytest.raises(module.SmokeError, match="control_output_already_exists"):
        module.persist_control_output(control, symlink_output, binding)


def test_control_output_uses_platform_owner_only_executable_protection(
    tmp_path, monkeypatch
):
    module = load_module()
    output = tmp_path / "worldstreamctl"
    output.write_bytes(b"binary")

    monkeypatch.setattr(module.os, "name", "posix")
    module.protect_executable(output)
    assert stat.S_IMODE(output.stat().st_mode) == 0o700

    calls = []
    monkeypatch.setattr(module.os, "name", "nt")
    monkeypatch.setattr(
        module,
        "protect_windows_path",
        lambda path, *, directory: calls.append((path, directory)),
    )
    module.protect_executable(output)
    assert calls == [(output, False)]


def test_control_output_windows_hard_link_publication_branch(tmp_path, monkeypatch):
    module = load_module()
    control = tmp_path / "extracted-worldstreamctl.exe"
    control_bytes = b"exact Windows packaged control binary"
    control.write_bytes(control_bytes)
    output = tmp_path / "retained" / "worldstreamctl.exe"
    binding = {
        "archive_sha256": "sha256:" + "1" * 64,
        "archive_size_bytes": 123,
        "package_report_sha256": "sha256:" + "2" * 64,
        "package_report_size_bytes": 456,
        "worldstreamctl_sha256": module.sha256_bytes(control_bytes),
        "worldstreamctl_size_bytes": len(control_bytes),
    }
    protected = []
    monkeypatch.setattr(module, "running_on_windows", lambda: True)
    monkeypatch.setattr(
        module,
        "protect_directory",
        lambda path: protected.append((path, True)),
    )
    monkeypatch.setattr(
        module,
        "protect_executable",
        lambda path: protected.append((path, False)),
    )

    module.persist_control_output(control, output, binding)

    assert output.read_bytes() == control_bytes
    assert protected[0] == (output.parent, True)
    assert protected[1][0].name.startswith(".worldstreamctl.exe.partial-")
    assert protected[1][1] is False
    assert not list(output.parent.glob(".worldstreamctl.exe.partial-*"))


def test_windows_control_staging_requests_delete_access_and_handle_disposition(
    tmp_path, monkeypatch
):
    import ctypes

    module = load_module()
    calls = {}

    class FakeCall:
        def __init__(self, callback):
            self.callback = callback
            self.argtypes = None
            self.restype = None

        def __call__(self, *args):
            return self.callback(*args)

    def create_file(path, access, sharing, _security, creation, attributes, _template):
        calls["create"] = (access, sharing, creation, attributes)
        return module.os.open(
            path, module.os.O_RDWR | module.os.O_CREAT | module.os.O_EXCL, 0o600
        )

    def close_handle(handle):
        module.os.close(handle)
        return 1

    def set_file_information(handle, information_class, value, size):
        calls["disposition"] = (
            handle,
            information_class,
            ctypes.cast(value, ctypes.POINTER(ctypes.c_ubyte)).contents.value,
            size,
        )
        return 1

    kernel32 = type(
        "FakeKernel32",
        (),
        {
            "CreateFileW": FakeCall(create_file),
            "CloseHandle": FakeCall(close_handle),
            "SetFileInformationByHandle": FakeCall(set_file_information),
        },
    )()
    fake_msvcrt = type(
        "FakeMsvcrt",
        (),
        {
            "open_osfhandle": staticmethod(lambda handle, _flags: handle),
            "get_osfhandle": staticmethod(lambda descriptor: descriptor),
        },
    )()
    monkeypatch.setattr(
        ctypes, "WinDLL", lambda *_args, **_kwargs: kernel32, raising=False
    )
    monkeypatch.setitem(sys.modules, "msvcrt", fake_msvcrt)
    monkeypatch.setattr(module.os, "name", "nt")

    descriptor, staging = module.create_windows_delete_capable_staging(
        tmp_path, "worldstreamctl.exe"
    )
    access, sharing, creation, attributes = calls["create"]
    assert access & 0x00010000
    assert sharing & 0x00000004
    assert creation == 1
    assert attributes == 0x00000080

    module.dispose_control_staging_from_handle(descriptor, staging)
    assert calls["disposition"] == (descriptor, 4, 1, 1)
    module.os.close(descriptor)
    staging.unlink()


def test_control_output_windows_success_never_unlinks_a_replacement_staging_name(
    tmp_path, monkeypatch
):
    module = load_module()
    control = tmp_path / "worldstreamctl.exe"
    output = tmp_path / "retained" / "worldstreamctl.exe"
    control_bytes = b"MZ-exact-packaged-control"
    control.write_bytes(control_bytes)
    binding = {
        "archive_sha256": "sha256:" + "1" * 64,
        "archive_size_bytes": 123,
        "package_report_sha256": "sha256:" + "2" * 64,
        "package_report_size_bytes": 456,
        "worldstreamctl_sha256": module.sha256_bytes(control_bytes),
        "worldstreamctl_size_bytes": len(control_bytes),
    }
    monkeypatch.setattr(module, "running_on_windows", lambda: True)
    monkeypatch.setattr(module, "protect_directory", lambda _path: None)
    monkeypatch.setattr(module, "protect_executable", lambda _path: None)
    replacement = b"unrelated-same-service-file"
    swapped_path = None
    held_path = None

    def swap_before_handle_disposition(descriptor, path):
        nonlocal held_path, swapped_path
        held_path = path.with_name(f"{path.name}.held")
        path.rename(held_path)
        path.write_bytes(replacement)
        swapped_path = path
        assert module.file_identity(held_path, "test_identity") == (
            module.os.fstat(descriptor).st_dev,
            module.os.fstat(descriptor).st_ino,
        )
        # Model Windows delete-on-close removing only the retained handle's
        # original hard-link name, never the newly substituted pathname.
        held_path.unlink()

    monkeypatch.setattr(
        module,
        "dispose_control_staging_from_handle",
        swap_before_handle_disposition,
    )
    module.persist_control_output(control, output, binding)

    assert output.read_bytes() == control.read_bytes()
    assert swapped_path is not None
    assert swapped_path.read_bytes() == replacement
    assert held_path is not None and not held_path.exists()


def test_control_output_windows_failure_never_unlinks_a_replacement_name(
    tmp_path, monkeypatch
):
    module = load_module()
    control = tmp_path / "worldstreamctl.exe"
    output = tmp_path / "retained" / "worldstreamctl.exe"
    control_bytes = b"MZ-failing-packaged-control"
    control.write_bytes(control_bytes)
    binding = {
        "archive_sha256": "sha256:" + "1" * 64,
        "archive_size_bytes": 123,
        "package_report_sha256": "sha256:" + "2" * 64,
        "package_report_size_bytes": 456,
        "worldstreamctl_sha256": module.sha256_bytes(control_bytes),
        "worldstreamctl_size_bytes": len(control_bytes),
    }
    monkeypatch.setattr(module, "running_on_windows", lambda: True)
    monkeypatch.setattr(module, "protect_directory", lambda _path: None)
    monkeypatch.setattr(
        module,
        "protect_executable",
        lambda _path: (_ for _ in ()).throw(
            module.SmokeError("injected_windows_protect_failure")
        ),
    )
    real_preserve = module.preserve_scrubbed_windows_control_names
    replacement = b"unrelated-same-service-file"
    swapped_path = None
    held_path = None

    def swap_before_preserving(paths, identity):
        nonlocal held_path, swapped_path
        staging = next(path for path in paths if path is not None and path != output)
        held_path = staging.with_name(f"{staging.name}.held")
        staging.rename(held_path)
        staging.write_bytes(replacement)
        swapped_path = staging
        real_preserve(paths, identity)

    monkeypatch.setattr(
        module, "preserve_scrubbed_windows_control_names", swap_before_preserving
    )
    with pytest.raises(module.SmokeError, match="injected_windows_protect_failure"):
        module.persist_control_output(control, output, binding)

    assert not output.exists()
    assert swapped_path is not None and swapped_path.read_bytes() == replacement
    assert held_path is not None and held_path.read_bytes() == b""


def test_control_output_failure_scrubs_exact_created_descriptor(tmp_path, monkeypatch):
    module = load_module()
    control = tmp_path / "extracted-worldstreamctl"
    control_bytes = b"partial output must not remain"
    control.write_bytes(control_bytes)
    output_root = tmp_path / "retained"
    output_root.mkdir()
    output = output_root / "worldstreamctl"
    binding = {
        "archive_sha256": "sha256:" + "1" * 64,
        "archive_size_bytes": 123,
        "package_report_sha256": "sha256:" + "2" * 64,
        "package_report_size_bytes": 456,
        "worldstreamctl_sha256": module.sha256_bytes(control_bytes),
        "worldstreamctl_size_bytes": len(control_bytes),
    }
    monkeypatch.setattr(
        module,
        "protect_executable",
        lambda _path: (_ for _ in ()).throw(
            module.SmokeError("injected_protect_failure")
        ),
    )
    original_unlink = Path.unlink

    def fail_staging_unlink(path, *args, **kwargs):
        if path.parent == output_root and path.name.startswith(
            ".worldstreamctl.partial-"
        ):
            raise OSError("injected unlink failure")
        return original_unlink(path, *args, **kwargs)

    monkeypatch.setattr(Path, "unlink", fail_staging_unlink)
    with pytest.raises(module.SmokeError, match="injected_protect_failure"):
        module.persist_control_output(control, output, binding)
    assert not output.exists()
    staging = list(output_root.glob(".worldstreamctl.partial-*"))
    assert len(staging) == 1
    assert staging[0].stat().st_size == 0
    original_unlink(staging[0])


def test_control_output_is_absent_until_atomic_publication(tmp_path, monkeypatch):
    module = load_module()
    control = tmp_path / "extracted-worldstreamctl"
    control_bytes = b"complete packaged control bytes" * 4096
    control.write_bytes(control_bytes)
    output = tmp_path / "retained" / "worldstreamctl"
    binding = {
        "archive_sha256": "sha256:" + "1" * 64,
        "archive_size_bytes": 123,
        "package_report_sha256": "sha256:" + "2" * 64,
        "package_report_size_bytes": 456,
        "worldstreamctl_sha256": module.sha256_bytes(control_bytes),
        "worldstreamctl_size_bytes": len(control_bytes),
    }
    publication_entered = threading.Event()
    allow_publication = threading.Event()
    original_link = module.os.link

    def delayed_link(source, destination):
        assert Path(source).read_bytes() == control_bytes
        assert not Path(destination).exists()
        publication_entered.set()
        assert allow_publication.wait(timeout=5)
        return original_link(source, destination)

    monkeypatch.setattr(module.os, "link", delayed_link)
    outcome = []

    def persist():
        outcome.append(module.persist_control_output(control, output, binding))

    thread = threading.Thread(target=persist)
    thread.start()
    assert publication_entered.wait(timeout=5)
    for _ in range(1_000):
        assert not output.exists()
    allow_publication.set()
    thread.join(timeout=5)

    assert not thread.is_alive()
    assert len(outcome) == 1
    assert output.read_bytes() == control_bytes
    assert not list(output.parent.glob(".worldstreamctl.partial-*"))


def test_control_output_destination_race_is_no_clobber(tmp_path, monkeypatch):
    module = load_module()
    control = tmp_path / "extracted-worldstreamctl"
    control_bytes = b"exact packaged control binary"
    control.write_bytes(control_bytes)
    output = tmp_path / "retained" / "worldstreamctl"
    competitor = b"concurrent owner output"
    binding = {
        "archive_sha256": "sha256:" + "1" * 64,
        "archive_size_bytes": 123,
        "package_report_sha256": "sha256:" + "2" * 64,
        "package_report_size_bytes": 456,
        "worldstreamctl_sha256": module.sha256_bytes(control_bytes),
        "worldstreamctl_size_bytes": len(control_bytes),
    }
    original_link = module.os.link

    def racing_link(source, destination):
        Path(destination).write_bytes(competitor)
        return original_link(source, destination)

    monkeypatch.setattr(module.os, "link", racing_link)
    with pytest.raises(module.SmokeError, match="control_output_already_exists"):
        module.persist_control_output(control, output, binding)

    assert output.read_bytes() == competitor
    assert not list(output.parent.glob(".worldstreamctl.partial-*"))


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
    poisoned = (
        "PGHOST",
        "PGUSER",
        "PGPASSWORD",
        "PGPASSFILE",
        "PGSERVICE",
        "PGOPTIONS",
        "PGSSLMODE",
        "PGSSLROOTCERT",
        "PGAPPNAME",
        "WORLDSTREAM_POSTGRES_URL",
        "WORLDSTREAM__STORAGE__POSTGRESQL__DSN",
        "WORLDSTREAM__STORAGE__POSTGRESQL__DSN_HANDLE",
        "WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_HANDLE",
    )
    for name in poisoned:
        monkeypatch.setenv(name, f"poisoned-{name.lower()}")
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
    assert all(name not in environment for name in poisoned)
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
    assert '"PGPASSWORD"' not in source
    assert '"PGPASSFILE"' in source
    assert "input_text=sql" in source


def test_psql_uses_owner_only_passfile_without_secret_argv_or_environment(
    tmp_path, monkeypatch
):
    module = load_module()
    captured = {}
    cleanup_calls = 0
    real_destroy_pgpass = module.destroy_pgpass
    connection = {
        "host": "127.0.0.1",
        "port": "5432",
        "dbname": "worldstream",
        "user": "operator",
        "password": "private:with\\escaping",
    }

    def fake_run(argv, *, environment=None, input_text=None, timeout=120):
        del timeout
        captured["argv"] = argv
        captured["environment"] = environment
        captured["input"] = input_text
        passfile = Path(environment["PGPASSFILE"])
        captured["passfile"] = passfile
        assert passfile.is_file()
        if module.os.name != "nt":
            assert stat.S_IMODE(passfile.stat().st_mode) == 0o600
        assert passfile.read_text(encoding="utf-8") == (
            "127.0.0.1:5432:worldstream:operator:private\\:with\\\\escaping\n"
        )
        return type("Completed", (), {"returncode": 0, "stdout": "17\n"})()

    def counted_destroy_pgpass(path, root, identity):
        nonlocal cleanup_calls
        cleanup_calls += 1
        return real_destroy_pgpass(path, root, identity)

    monkeypatch.setattr(module, "run_process", fake_run)
    monkeypatch.setattr(module, "destroy_pgpass", counted_destroy_pgpass)
    assert (
        module.psql(Path("/usr/bin/psql"), connection, "SELECT 17;", "failed") == "17"
    )

    secret = connection["password"]
    assert secret not in repr(captured["argv"])
    assert secret not in repr(captured["environment"])
    assert "PGPASSWORD" not in captured["environment"]
    assert captured["input"] == "SELECT 17;\n"
    assert cleanup_calls == 1
    assert not captured["passfile"].exists()
    assert not captured["passfile"].parent.exists()


def test_psql_scrubs_passfile_and_fails_closed_when_unlink_fails(monkeypatch):
    module = load_module()
    captured = {}
    original_unlink = module.os.unlink

    def fake_run(_argv, *, environment=None, input_text=None, timeout=120):
        del input_text, timeout
        captured["passfile"] = Path(environment["PGPASSFILE"])
        return type("Completed", (), {"returncode": 0, "stdout": "1\n"})()

    def fail_passfile_unlink(path, *args, **kwargs):
        if Path(path) == captured.get("passfile"):
            raise OSError("injected unlink failure")
        return original_unlink(path, *args, **kwargs)

    monkeypatch.setattr(module, "run_process", fake_run)
    monkeypatch.setattr(module.os, "unlink", fail_passfile_unlink)
    connection = {
        "host": "127.0.0.1",
        "port": "5432",
        "dbname": "worldstream",
        "user": "operator",
        "password": "must-not-remain",
    }

    with pytest.raises(module.SmokeError, match="pgpass_cleanup_failed"):
        module.psql(Path("/usr/bin/psql"), connection, "SELECT 1;", "failed")

    passfile = captured["passfile"]
    assert passfile.read_bytes() == b""
    original_unlink(passfile)
    passfile.parent.rmdir()


def test_extracted_control_surface_checks_sqlite_and_transfer_commands(monkeypatch):
    module = load_module()
    observed = []

    def fake_run(argv, **_kwargs):
        observed.append(argv)
        help_output = b"".join(
            (
                b"backup restore verify begin resume finalize abort ",
                b"--database --output --companion --envelope --backup ",
                b"--sqlite --bundle --state-dir --transfer-id --dsn-file --chunk-records",
            )
        )
        return (
            0,
            help_output,
            b"",
        )

    monkeypatch.setattr(module, "run_bounded_process", fake_run)
    module.validate_packaged_operator_surface(Path("/package/bin/worldstreamctl"))

    assert observed == [
        ["/package/bin/worldstreamctl", "sqlite", "--help"],
        ["/package/bin/worldstreamctl", "sqlite", "backup", "--help"],
        ["/package/bin/worldstreamctl", "sqlite", "restore", "--help"],
        ["/package/bin/worldstreamctl", "sqlite", "verify", "--help"],
        ["/package/bin/worldstreamctl", "postgres", "transfer", "--help"],
        ["/package/bin/worldstreamctl", "postgres", "transfer", "begin", "--help"],
        ["/package/bin/worldstreamctl", "postgres", "transfer", "resume", "--help"],
        ["/package/bin/worldstreamctl", "postgres", "transfer", "finalize", "--help"],
        ["/package/bin/worldstreamctl", "postgres", "transfer", "abort", "--help"],
    ]


def test_packaged_sqlite_roundtrip_invokes_extracted_ready_path(tmp_path, monkeypatch):
    module = load_module()
    control = tmp_path / "package" / "bin" / "worldstreamctl"
    control.parent.mkdir(parents=True)
    control.write_bytes(b"packaged control")
    database = tmp_path / "worldstream.sqlite3"
    module.write_secret(database, b"source sqlite bytes")
    observed = []

    def fake_run_json(argv, code, **_kwargs):
        observed.append((argv, code))
        operation = argv[2]
        if operation == "backup":
            backup = Path(argv[argv.index("--output") + 1])
            envelope = Path(argv[argv.index("--envelope") + 1])
            module.write_secret(backup, b"native backup bytes")
            module.write_secret(envelope, b'{"sealed":"exact"}')
            return {
                "status": "ok",
                "operation": "backup",
                "native_verifier": "pass",
                "semantic_verifier": "pass",
                "envelope_digest": "1" * 64,
                "envelope_file_digest": "2" * 64,
                "envelope_bytes": envelope.stat().st_size,
            }
        if operation == "restore":
            restored = Path(argv[argv.index("--database") + 1])
            module.write_secret(restored, b"restored sqlite bytes")
            return {
                "status": "ok",
                "operation": "restore",
                "native_verifier": "pass",
                "semantic_verifier": "pass",
                "envelope_digest": "1" * 64,
                "envelope_file_digest": "2" * 64,
                "envelope_bytes": len(b'{"sealed":"exact"}'),
            }
        assert operation == "verify"
        return {
            "status": "ok",
            "operation": "verify",
            "native_verifier": "pass",
            "semantic_verifier": "not_invoked",
        }

    monkeypatch.setattr(module, "run_json", fake_run_json)
    manifest = json.loads((ROOT / "compatibility.json").read_text(encoding="utf-8"))

    result = module.run_packaged_sqlite_roundtrip(
        control,
        database,
        tmp_path / "operator-roundtrip",
        manifest,
    )

    assert result == {
        "backup": "native_and_semantic_pass",
        "restore": "native_and_semantic_pass",
        "verify": "native_only_pass_semantic_not_invoked",
        "envelope": "exact_bytes_preserved",
    }
    assert [entry[0][:3] for entry in observed] == [
        [str(control), "sqlite", "backup"],
        [str(control), "sqlite", "restore"],
        [str(control), "sqlite", "verify"],
    ]
    companion = tmp_path / "operator-roundtrip" / "companion.template.json"
    assert companion.read_bytes() == module.build_empty_sqlite_companion(manifest)


def test_packaged_transfer_uses_exact_control_restart_abort_and_finalize(
    tmp_path, monkeypatch
):
    module = load_module()
    control = tmp_path / "retained" / "worldstreamctl"
    control.parent.mkdir()
    control.write_bytes(b"exact packaged control")
    sqlite = tmp_path / "source.sqlite3"
    admin_dsn = tmp_path / "admin.dsn"
    abort_dsn = tmp_path / "abort.dsn"
    for path, value in (
        (sqlite, b"sqlite"),
        (admin_dsn, b"admin secret"),
        (abort_dsn, b"abort secret"),
    ):
        module.write_secret(path, value)
    binding = {
        "archive_sha256": "sha256:" + "1" * 64,
        "archive_size_bytes": 101,
        "package_report_sha256": "sha256:" + "2" * 64,
        "package_report_size_bytes": 202,
        "worldstreamctl_sha256": module.sha256_file(control),
        "worldstreamctl_size_bytes": control.stat().st_size,
    }
    observed = []
    resumes = {"abort-state": 0, "finalize-state": 0}

    def result(operation, phase, next_ordinal, complete):
        return {
            "schema": "worldstream/operator-transfer-result/v1",
            "status": "ok",
            "operation": operation,
            "phase": phase,
            "bundle_hash": "3" * 64,
            "target_fingerprint": "4" * 64,
            "source_epoch": 1,
            "target_epoch": 2,
            "record_count": 3,
            "next_ordinal": next_ordinal,
            "chunks_complete": complete,
        }

    def fake_transfer(
        invoked_control,
        operation,
        _sqlite,
        bundle,
        state_dir,
        **kwargs,
    ):
        assert invoked_control == control
        assert kwargs.get("dsn_file") in (None, admin_dsn, abort_dsn)
        observed.append((state_dir.name, operation, kwargs.get("chunk_records")))
        if operation == "begin":
            module.write_secret(kwargs["backup"], b"verified backup")
            module.write_secret(bundle, b"canonical transfer bundle")
            return result("begin", "begun", 0, False)
        if operation == "resume":
            resumes[state_dir.name] += 1
            count = resumes[state_dir.name]
            if state_dir.name == "abort-state" or count == 1:
                module.write_secret(
                    state_dir / f"transfer-state-v1-{count:020}-fixture.json", b"{}"
                )
                return result("resume", "importing", 1, False)
            if count == 2:
                module.write_secret(
                    state_dir / f"transfer-state-v1-{count:020}-fixture.json", b"{}"
                )
            return result("resume", "chunks_complete", 3, True)
        if operation == "abort":
            return result("abort", "source_authoritative", 1, False)
        assert operation == "finalize"
        return result("finalize", "target_authoritative", 3, True)

    source_states = iter([("source_authoritative", True), ("source_retired", True)])

    def fake_psql(_path, connection, sql, _code):
        if connection["dbname"] != "abort":
            return "authoritative|0|1"
        if "sum(octet_length(records_bytes))" in sql:
            return "1|128"
        if "information_schema.tables" in sql:
            return "\n".join(sorted(module.POSTGRES_TRANSFER_DURABLE_TABLES))
        if "UNION ALL" in sql:
            counts = {
                name: (
                    1
                    if name
                    in {
                        "worldstream_schema_migrations",
                        "worldstream_authority_state",
                        "worldstream_transfer_target_fence",
                    }
                    else 0
                )
                for name in module.POSTGRES_TRANSFER_DURABLE_TABLES
            }
            return "\n".join(f"{name}|{counts[name]}" for name in sorted(counts))
        assert "SELECT state FROM worldstream_transfer_target_fence" in sql
        return "aborted"

    monkeypatch.setattr(module, "run_transfer_command", fake_transfer)
    monkeypatch.setattr(
        module, "source_transfer_state", lambda _path: next(source_states)
    )
    monkeypatch.setattr(module, "psql", fake_psql)

    evidence = module.run_packaged_transfer_roundtrip(
        control,
        sqlite,
        admin_dsn,
        abort_dsn,
        tmp_path / "transfer",
        Path("/usr/bin/psql"),
        {"dbname": "main"},
        {"dbname": "abort"},
        binding,
        (b"admin secret", b"abort secret"),
    )

    assert evidence["status"] == "pass"
    assert evidence["control_binding"] == binding
    assert evidence["abort"]["pre_abort_checkpoint_rows"] == 1
    assert evidence["abort"]["provider_cleanup"] == (
        "all_durable_domains_empty_with_exact_tombstone"
    )
    assert evidence["finalized"]["resume_invocations"] == 2
    assert evidence["finalized"]["completed_resume_replay"] == "no_new_generation"
    assert observed == [
        ("abort-state", "begin", None),
        ("abort-state", "resume", 1),
        ("abort-state", "abort", None),
        ("abort-state", "abort", None),
        ("finalize-state", "begin", None),
        ("finalize-state", "resume", 1),
        ("finalize-state", "resume", 1024),
        ("finalize-state", "resume", 1024),
        ("finalize-state", "finalize", None),
        ("finalize-state", "finalize", None),
    ]


@pytest.mark.parametrize("index", range(17))
def test_runtime_role_witness_rejects_each_individual_escalation(index: int):
    module = load_module()
    assert module.RUNTIME_ROLE_ADMISSION_FIELDS == (
        "superuser",
        "create_role",
        "create_database",
        "replication",
        "bypass_row_security",
        "database_create",
        "other_role_membership",
        "public_schema_create",
        "owns_public_schema_object",
        "migration_insert",
        "migration_update",
        "migration_delete",
        "migration_truncate",
        "transfer_control_insert",
        "transfer_control_update",
        "transfer_control_delete",
        "transfer_control_truncate",
    )
    module.validate_runtime_role_admission(module.RUNTIME_ROLE_ADMISSION_EXPECTED)
    changed = module.RUNTIME_ROLE_ADMISSION_EXPECTED.split("|")
    changed[index] = "true"
    with pytest.raises(
        module.SmokeError, match="postgres_runtime_role_not_least_privileged"
    ):
        module.validate_runtime_role_admission("|".join(changed))
    assert "NOREPLICATION NOBYPASSRLS" in SCRIPT.read_text(encoding="utf-8")


def test_runtime_role_sql_rejects_each_witness_mutation():
    module = load_module()
    assert_runtime_role_sql(module.RUNTIME_ROLE_ADMISSION_SQL)
    for fragment in RUNTIME_ROLE_SQL_FRAGMENTS:
        with pytest.raises(AssertionError):
            assert_runtime_role_sql(
                module.RUNTIME_ROLE_ADMISSION_SQL.replace(fragment, "mutated")
            )

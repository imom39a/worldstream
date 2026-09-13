#!/usr/bin/env python3
"""Run exact packaged native binaries against SQLite and PostgreSQL profiles."""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import importlib.util
import io
import json
import os
import platform
import secrets
import shlex
import shutil
import socket
import sqlite3
import stat
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path, PurePosixPath
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
PACKAGE_PATH = ROOT / "scripts/package.py"
SCHEMA = "worldstream/native-package-runtime-smoke/v1"
SHA256_PREFIX = "sha256:"
MAX_CONTROL_BYTES = 16 * 1024 * 1024
MAX_LOG_BYTES = 8 * 1024 * 1024
EXPECTED_TARGETS = {
    "linux-x86_64": ("Linux", {"x86_64", "amd64"}, "worldstreamd", "worldstreamctl"),
    "windows-x64": (
        "Windows",
        {"amd64", "x86_64"},
        "worldstreamd.exe",
        "worldstreamctl.exe",
    ),
}
RUNTIME_ROLE_ADMISSION_FIELDS = (
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
    "observation_frame_delete",
)
RUNTIME_ROLE_ADMISSION_EXPECTED = "|".join(
    "false" for _ in RUNTIME_ROLE_ADMISSION_FIELDS
)
POSTGRES_TRANSFER_DURABLE_TABLES = (
    "worldstream_schema_migrations",
    "worldstream_operation_guards",
    "worldstream_room_roots",
    "worldstream_genesis",
    "worldstream_materializations",
    "worldstream_members",
    "worldstream_timers",
    "worldstream_transitions",
    "worldstream_frames",
    "worldstream_observation_consequences",
    "worldstream_activation_decisions",
    "worldstream_activation_intents",
    "worldstream_activation_operation_receipts",
    "worldstream_room_snapshots",
    "worldstream_semantic_receipts",
    "worldstream_integrity_incidents",
    "worldstream_authority_fences",
    "worldstream_authority_state",
    "worldstream_authority_principals",
    "worldstream_authority_runners",
    "worldstream_authority_capabilities",
    "worldstream_authority_capability_scopes",
    "worldstream_authority_runner_capability_memberships",
    "worldstream_authority_change_receipts",
    "worldstream_authority_audit",
    "worldstream_transfer_imports",
    "worldstream_transfer_chunks",
    "worldstream_transfer_target_fence",
    "worldstream_deployment_metadata",
    "worldstream_deployment_identity_metadata",
    "worldstream_deployment_pack_identities",
    "worldstream_deployment_resource_identities",
    "worldstream_deployment_resource_blobs",
    "worldstream_retired_authority_fences_v1",
)
RUNTIME_ROLE_ADMISSION_SQL = (
    "SELECT role.rolsuper::text || '|' || "
    "role.rolcreaterole::text || '|' || "
    "role.rolcreatedb::text || '|' || "
    "role.rolreplication::text || '|' || "
    "role.rolbypassrls::text || '|' || "
    "has_database_privilege(current_user, current_database(), 'CREATE')::text || "
    "'|' || "
    "(EXISTS (SELECT 1 FROM pg_catalog.pg_auth_members AS membership "
    "WHERE membership.member = role.oid))::text || '|' || "
    "has_schema_privilege(current_user, 'public', 'CREATE')::text || '|' || "
    "(EXISTS (SELECT 1 FROM pg_catalog.pg_namespace AS namespace_row "
    "WHERE namespace_row.nspname = 'public' "
    "AND namespace_row.nspowner = role.oid "
    "UNION ALL SELECT 1 FROM pg_catalog.pg_class AS relation_row "
    "JOIN pg_catalog.pg_namespace AS namespace_row "
    "ON namespace_row.oid = relation_row.relnamespace "
    "WHERE namespace_row.nspname = 'public' "
    "AND relation_row.relowner = role.oid "
    "UNION ALL SELECT 1 FROM pg_catalog.pg_proc AS routine_row "
    "JOIN pg_catalog.pg_namespace AS namespace_row "
    "ON namespace_row.oid = routine_row.pronamespace "
    "WHERE namespace_row.nspname = 'public' "
    "AND routine_row.proowner = role.oid "
    "UNION ALL SELECT 1 FROM pg_catalog.pg_type AS type_row "
    "JOIN pg_catalog.pg_namespace AS namespace_row "
    "ON namespace_row.oid = type_row.typnamespace "
    "WHERE namespace_row.nspname = 'public' "
    "AND type_row.typowner = role.oid))::text || '|' || "
    "has_table_privilege(current_user, "
    "'public.worldstream_schema_migrations', 'INSERT')::text || '|' || "
    "has_table_privilege(current_user, "
    "'public.worldstream_schema_migrations', 'UPDATE')::text || '|' || "
    "has_table_privilege(current_user, "
    "'public.worldstream_schema_migrations', 'DELETE')::text || '|' || "
    "has_table_privilege(current_user, "
    "'public.worldstream_schema_migrations', 'TRUNCATE')::text || '|' || "
    "(EXISTS (SELECT 1 FROM unnest(ARRAY["
    "'public.worldstream_transfer_imports',"
    "'public.worldstream_transfer_chunks',"
    "'public.worldstream_transfer_target_fence',"
    "'public.worldstream_transfer_stream_imports_v2',"
    "'public.worldstream_transfer_stream_chunks_v2',"
    "'public.worldstream_transfer_stream_records_v2']::text[]) "
    "AS protected_table(table_name) WHERE has_table_privilege("
    "current_user, protected_table.table_name, 'INSERT')))::text || '|' || "
    "(EXISTS (SELECT 1 FROM unnest(ARRAY["
    "'public.worldstream_transfer_imports',"
    "'public.worldstream_transfer_chunks',"
    "'public.worldstream_transfer_target_fence',"
    "'public.worldstream_transfer_stream_imports_v2',"
    "'public.worldstream_transfer_stream_chunks_v2',"
    "'public.worldstream_transfer_stream_records_v2']::text[]) "
    "AS protected_table(table_name) WHERE has_table_privilege("
    "current_user, protected_table.table_name, 'UPDATE')))::text || '|' || "
    "(EXISTS (SELECT 1 FROM unnest(ARRAY["
    "'public.worldstream_transfer_imports',"
    "'public.worldstream_transfer_chunks',"
    "'public.worldstream_transfer_target_fence',"
    "'public.worldstream_transfer_stream_imports_v2',"
    "'public.worldstream_transfer_stream_chunks_v2',"
    "'public.worldstream_transfer_stream_records_v2']::text[]) "
    "AS protected_table(table_name) WHERE has_table_privilege("
    "current_user, protected_table.table_name, 'DELETE')))::text || '|' || "
    "(EXISTS (SELECT 1 FROM unnest(ARRAY["
    "'public.worldstream_transfer_imports',"
    "'public.worldstream_transfer_chunks',"
    "'public.worldstream_transfer_target_fence',"
    "'public.worldstream_transfer_stream_imports_v2',"
    "'public.worldstream_transfer_stream_chunks_v2',"
    "'public.worldstream_transfer_stream_records_v2']::text[]) "
    "AS protected_table(table_name) WHERE has_table_privilege("
    "current_user, protected_table.table_name, 'TRUNCATE')))::text || '|' || "
    "has_table_privilege(current_user, "
    "'public.worldstream_frames', 'DELETE')::text "
    "FROM pg_catalog.pg_roles AS role WHERE role.rolname = current_user"
)


def load_package():
    spec = importlib.util.spec_from_file_location(
        "worldstream_native_package_smoke_verifier", PACKAGE_PATH
    )
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {PACKAGE_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


PACKAGE = load_package()


class SmokeError(RuntimeError):
    """A package or runtime boundary failed closed."""


def validate_runtime_role_admission(witness: str) -> None:
    require(
        witness == RUNTIME_ROLE_ADMISSION_EXPECTED,
        "postgres_runtime_role_not_least_privileged",
    )


def require(condition: bool, code: str) -> None:
    if not condition:
        raise SmokeError(code)


def running_on_windows() -> bool:
    return os.name == "nt"


def sha256_bytes(value: bytes) -> str:
    return SHA256_PREFIX + hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return SHA256_PREFIX + digest.hexdigest()


def sha256_descriptor(descriptor: int) -> str:
    digest = hashlib.sha256()
    os.lseek(descriptor, 0, os.SEEK_SET)
    while chunk := os.read(descriptor, 1024 * 1024):
        digest.update(chunk)
    return SHA256_PREFIX + digest.hexdigest()


def regular_file(path: Path, code: str) -> Path:
    try:
        mode = path.lstat().st_mode
    except OSError as error:
        raise SmokeError(code) from error
    require(stat.S_ISREG(mode) and not stat.S_ISLNK(mode), code)
    return path


def read_json(path: Path, code: str) -> dict[str, Any]:
    regular_file(path, code)
    require(0 < path.stat().st_size <= MAX_CONTROL_BYTES, code)
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise SmokeError(code) from error
    require(isinstance(value, dict), code)
    return value


def strict_runtime_json(raw: bytes, code: str) -> dict[str, Any]:
    """Parse a bounded runtime response with the shared release JSON boundary."""

    require(isinstance(raw, bytes) and 0 < len(raw) <= MAX_CONTROL_BYTES, code)
    try:
        return PACKAGE.BUILD_IDENTITY.strict_json(raw, code)
    except PACKAGE.BUILD_IDENTITY.IdentityError as error:
        raise SmokeError(code) from error


def validate_control_version(
    value: object,
    *,
    manifest_summary: object,
    product: str,
    source_revision: str,
    code: str,
) -> None:
    """Bind the packaged control binary to the daemon/package source revision."""

    require(isinstance(manifest_summary, dict), code)
    expected = {
        **manifest_summary,
        "product_build": {
            "product": product,
            "binary": "worldstreamctl",
            "build_version": product,
            "source_revision": source_revision,
        },
    }
    require(value == expected, code)


def atomic_write(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    require(not path.is_symlink() and not path.is_dir(), "unsafe_report_output")
    descriptor, name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump(value, output, ensure_ascii=False, indent=2, sort_keys=True)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o600)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def current_windows_identity() -> str:
    completed = subprocess.run(
        ["whoami"], capture_output=True, text=True, check=False, timeout=10
    )
    require(
        completed.returncode == 0 and bool(completed.stdout.strip()),
        "windows_identity_unavailable",
    )
    return completed.stdout.strip()


def protect_windows_path(path: Path, *, directory: bool) -> None:
    identity = current_windows_identity()
    permission = "(OI)(CI)(F)" if directory else "(F)"
    completed = subprocess.run(
        [
            "icacls",
            str(path),
            "/inheritance:r",
            "/grant:r",
            f"{identity}:{permission}",
            f"*S-1-5-18:{permission}",
            f"*S-1-5-32-544:{permission}",
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
        timeout=30,
    )
    require(completed.returncode == 0, "windows_protected_acl_failed")


def protect_directory(path: Path) -> None:
    if os.name == "nt":
        protect_windows_path(path, directory=True)
    else:
        os.chmod(path, 0o700)
        require(
            stat.S_IMODE(path.stat().st_mode) == 0o700, "owner_only_directory_failed"
        )


def protect_file(path: Path) -> None:
    if os.name == "nt":
        protect_windows_path(path, directory=False)
    else:
        os.chmod(path, 0o600)
        require(stat.S_IMODE(path.stat().st_mode) == 0o600, "owner_only_file_failed")


def protect_executable(path: Path) -> None:
    if os.name == "nt":
        protect_windows_path(path, directory=False)
    else:
        os.chmod(path, 0o700)
        require(
            stat.S_IMODE(path.stat().st_mode) == 0o700,
            "owner_only_executable_failed",
        )


def scrub_control_identity(path: Path, identity: tuple[int, int]) -> None:
    descriptor: int | None = None
    try:
        descriptor = os.open(path, os.O_RDWR | getattr(os, "O_BINARY", 0))
        metadata = os.fstat(descriptor)
        if (
            not stat.S_ISREG(metadata.st_mode)
            or (metadata.st_dev, metadata.st_ino) != identity
        ):
            return
        os.ftruncate(descriptor, 0)
        os.fsync(descriptor)
    except OSError:
        return
    finally:
        if descriptor is not None:
            with contextlib.suppress(OSError):
                os.close(descriptor)


def create_control_staging(parent: Path, output_name: str) -> tuple[int, Path]:
    if os.name != "nt":
        descriptor, name = tempfile.mkstemp(
            prefix=f".{output_name}.partial-",
            dir=parent,
        )
        return descriptor, Path(name)
    return create_windows_delete_capable_staging(parent, output_name)


def create_windows_delete_capable_staging(
    parent: Path, output_name: str
) -> tuple[int, Path]:
    import ctypes
    import msvcrt
    from ctypes import wintypes

    generic_read = 0x80000000
    generic_write = 0x40000000
    delete_access = 0x00010000
    share_read = 0x00000001
    share_write = 0x00000002
    share_delete = 0x00000004
    create_new = 1
    file_attribute_normal = 0x00000080
    error_file_exists = {80, 183}
    invalid_handle = ctypes.c_void_p(-1).value
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    create_file = kernel32.CreateFileW
    create_file.argtypes = (
        wintypes.LPCWSTR,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.LPVOID,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.HANDLE,
    )
    create_file.restype = wintypes.HANDLE
    close_handle = kernel32.CloseHandle
    close_handle.argtypes = (wintypes.HANDLE,)
    close_handle.restype = wintypes.BOOL

    for _ in range(128):
        path = parent / f".{output_name}.partial-{secrets.token_hex(16)}"
        handle = create_file(
            str(path),
            generic_read | generic_write | delete_access,
            share_read | share_write | share_delete,
            None,
            create_new,
            file_attribute_normal,
            None,
        )
        if handle == invalid_handle:
            error = ctypes.get_last_error()
            if error in error_file_exists:
                continue
            raise SmokeError("control_output_creation_failed")
        try:
            descriptor = msvcrt.open_osfhandle(
                handle, os.O_RDWR | getattr(os, "O_BINARY", 0)
            )
        except (OSError, ValueError) as error:
            close_handle(handle)
            raise SmokeError("control_output_creation_failed") from error
        try:
            os.set_inheritable(descriptor, False)
        except OSError as error:
            os.close(descriptor)
            raise SmokeError("control_output_creation_failed") from error
        return descriptor, path
    raise SmokeError("control_output_creation_failed")


def dispose_control_staging_from_handle(descriptor: int, staging: Path) -> None:
    if os.name != "nt":
        # Test-only Windows-branch simulation on POSIX. The retained descriptor
        # keeps the published inode pinned while its original link is removed.
        os.unlink(staging)
        return

    import ctypes
    import msvcrt
    from ctypes import wintypes

    class FileDispositionInfo(ctypes.Structure):
        # Win32 FILE_DISPOSITION_INFO uses BOOLEAN (one byte), not BOOL.
        _fields_ = [("delete_file", ctypes.c_ubyte)]

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    set_file_information = kernel32.SetFileInformationByHandle
    set_file_information.argtypes = (
        wintypes.HANDLE,
        ctypes.c_int,
        ctypes.c_void_p,
        wintypes.DWORD,
    )
    set_file_information.restype = wintypes.BOOL
    disposition = FileDispositionInfo(1)
    if not set_file_information(
        msvcrt.get_osfhandle(descriptor),
        4,  # FileDispositionInfo
        ctypes.byref(disposition),
        ctypes.sizeof(disposition),
    ):
        raise SmokeError("control_output_staging_disposition_failed")


def preserve_scrubbed_windows_control_names(
    paths: tuple[Path | None, ...], identity: tuple[int, int]
) -> None:
    # Never follow a prior identity check with a pathname unlink. If a name
    # still identifies the created inode, scrub it through a newly retained
    # descriptor and leave the owner-only zero-length placeholder in place.
    # A substituted name is unrelated and remains untouched.
    for path in paths:
        if path is not None:
            scrub_control_identity(path, identity)


def write_secret(path: Path, value: bytes) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(value)
            output.flush()
            os.fsync(output.fileno())
        protect_file(path)
    except BaseException:
        path.unlink(missing_ok=True)
        raise


def persist_control_output(
    control: Path,
    output: Path,
    binding: dict[str, Any],
) -> dict[str, Any]:
    control = regular_file(control, "control_output_source_invalid")
    expected_digest = binding.get("worldstreamctl_sha256")
    expected_size = binding.get("worldstreamctl_size_bytes")
    require(
        isinstance(expected_digest, str)
        and expected_digest.startswith(SHA256_PREFIX)
        and len(expected_digest) == len(SHA256_PREFIX) + 64
        and isinstance(expected_size, int)
        and not isinstance(expected_size, bool)
        and expected_size > 0
        and control.stat().st_size == expected_size
        and sha256_file(control) == expected_digest
        and isinstance(binding.get("archive_sha256"), str)
        and isinstance(binding.get("archive_size_bytes"), int)
        and isinstance(binding.get("package_report_sha256"), str)
        and isinstance(binding.get("package_report_size_bytes"), int),
        "control_output_binding_invalid",
    )
    output = output.absolute()
    parent = output.parent
    require(parent != parent.parent, "control_output_parent_invalid")
    parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    try:
        parent_metadata = parent.lstat()
    except OSError as error:
        raise SmokeError("control_output_parent_invalid") from error
    require(
        stat.S_ISDIR(parent_metadata.st_mode)
        and not stat.S_ISLNK(parent_metadata.st_mode),
        "control_output_parent_invalid",
    )
    protect_directory(parent)
    descriptor: int | None = None
    staging: Path | None = None
    created_identity: tuple[int, int] | None = None
    try:
        descriptor, staging = create_control_staging(parent, output.name)
        metadata = os.fstat(descriptor)
        require(stat.S_ISREG(metadata.st_mode), "control_output_creation_failed")
        created_identity = (metadata.st_dev, metadata.st_ino)
        with control.open("rb") as source:
            while chunk := source.read(1024 * 1024):
                view = memoryview(chunk)
                while view:
                    written = os.write(descriptor, view)
                    require(written > 0, "control_output_write_failed")
                    view = view[written:]
        os.fsync(descriptor)
        protect_executable(staging)
        require(
            file_identity(staging, "control_output_identity_failed") == created_identity
            and os.fstat(descriptor).st_size == expected_size
            and sha256_descriptor(descriptor) == expected_digest
            and file_identity(staging, "control_output_identity_failed")
            == created_identity,
            "control_output_exactness_failed",
        )
        os.fsync(descriptor)
        try:
            os.link(staging, output)
        except FileExistsError as error:
            raise SmokeError("control_output_already_exists") from error
        except OSError as error:
            raise SmokeError("control_output_publication_failed") from error
        require(
            file_identity(output, "control_output_identity_failed") == created_identity,
            "control_output_identity_failed",
        )
        if not running_on_windows():
            parent_descriptor = os.open(parent, os.O_RDONLY)
            try:
                os.fsync(parent_descriptor)
            finally:
                os.close(parent_descriptor)
        require(
            os.fstat(descriptor).st_size == expected_size
            and sha256_descriptor(descriptor) == expected_digest
            and file_identity(output, "control_output_identity_failed")
            == created_identity
            and file_identity(staging, "control_output_identity_failed")
            == created_identity,
            "control_output_exactness_failed",
        )
        if running_on_windows():
            # The staging handle was opened with DELETE access/share. Mark its
            # exact hard-link name delete-on-close through that retained handle;
            # no pathname identity-check/unlink race is involved.
            dispose_control_staging_from_handle(descriptor, staging)
            os.close(descriptor)
            descriptor = None
            staging = None
        else:
            staging.unlink()
            staging = None
        if not running_on_windows():
            parent_descriptor = os.open(parent, os.O_RDONLY)
            try:
                os.fsync(parent_descriptor)
            finally:
                os.close(parent_descriptor)
        if descriptor is None:
            require(
                file_identity(output, "control_output_identity_failed")
                == created_identity
                and output.stat().st_size == expected_size
                and sha256_file(output) == expected_digest
                and file_identity(output, "control_output_identity_failed")
                == created_identity,
                "control_output_exactness_failed",
            )
        else:
            require(
                file_identity(output, "control_output_identity_failed")
                == created_identity
                and os.fstat(descriptor).st_size == expected_size
                and sha256_descriptor(descriptor) == expected_digest,
                "control_output_exactness_failed",
            )
            os.close(descriptor)
            descriptor = None
    except BaseException:
        if descriptor is not None:
            with contextlib.suppress(OSError):
                os.ftruncate(descriptor, 0)
                os.fsync(descriptor)
            with contextlib.suppress(OSError):
                os.close(descriptor)
            descriptor = None
        if created_identity is not None:
            if running_on_windows():
                preserve_scrubbed_windows_control_names(
                    (output, staging), created_identity
                )
            else:
                scrub_control_identity(output, created_identity)
                if staging is not None:
                    scrub_control_identity(staging, created_identity)
                for candidate in (output, staging):
                    if candidate is None:
                        continue
                    with contextlib.suppress(OSError, SmokeError):
                        if (
                            file_identity(candidate, "control_output_cleanup_failed")
                            == created_identity
                        ):
                            candidate.unlink()
            if not running_on_windows():
                with contextlib.suppress(OSError):
                    parent_descriptor = os.open(parent, os.O_RDONLY)
                    try:
                        os.fsync(parent_descriptor)
                    finally:
                        os.close(parent_descriptor)
        raise

    return {
        "status": "persisted",
        "archive_sha256": binding["archive_sha256"],
        "archive_size_bytes": binding["archive_size_bytes"],
        "package_report_sha256": binding["package_report_sha256"],
        "package_report_size_bytes": binding["package_report_size_bytes"],
        "worldstreamctl_sha256": expected_digest,
        "worldstreamctl_size_bytes": expected_size,
    }


def verify_and_extract(
    archive: Path,
    package_report_path: Path,
    manifest_toml: Path,
    manifest_json: Path,
    output_dir: Path,
    release_inventory: str | None = None,
) -> tuple[Path, Path, dict[str, Any], dict[str, Any]]:
    regular_file(archive, "package_archive_invalid")
    report = read_json(package_report_path, "package_report_invalid")
    root_toml = regular_file(manifest_toml, "manifest_toml_invalid").read_bytes()
    root_json = regular_file(manifest_json, "manifest_json_invalid").read_bytes()
    identity = report.get("identity")
    inventory = report.get("inventory")
    require(
        report.get("schema") == "worldstream/package-report/v1"
        and report.get("kind") == "archive"
        and report.get("artifact") == archive.name
        and report.get("path") == archive.name
        and report.get("sha256") == sha256_file(archive)
        and report.get("size_bytes") == archive.stat().st_size
        and isinstance(inventory, dict)
        and inventory.get("archive_verified") is True
        and inventory.get("release_evidence") is False
        and inventory.get("manifest_source") == "compatibility.toml"
        and inventory.get("manifest_mirror") == "compatibility.json"
        and isinstance(identity, dict)
        and identity.get("target") in EXPECTED_TARGETS
        and isinstance(identity.get("version"), str)
        and bool(identity["version"])
        and identity.get("manifest_sha256") == hashlib.sha256(root_json).hexdigest()
        and identity.get("manifest_json_sha256")
        == hashlib.sha256(root_json).hexdigest()
        and identity.get("manifest_toml_sha256")
        == hashlib.sha256(root_toml).hexdigest()
        and isinstance(identity.get("source_revision"), str)
        and PACKAGE.BUILD_IDENTITY.GIT_REVISION.fullmatch(identity["source_revision"])
        is not None
        and isinstance(identity.get("build_identity_sha256"), str)
        and PACKAGE.BUILD_IDENTITY.SHA256_REF.fullmatch(
            identity["build_identity_sha256"]
        )
        is not None,
        "package_report_identity_mismatch",
    )
    expected_system, expected_machines, daemon_name, control_name = EXPECTED_TARGETS[
        identity["target"]
    ]
    require(platform.system() == expected_system, "package_target_host_system_mismatch")
    require(
        platform.machine().lower() in expected_machines,
        "package_target_host_machine_mismatch",
    )
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            if release_inventory is None:
                PACKAGE.verify_archive(archive)
            else:
                PACKAGE.verify_archive(archive, release_inventory)
        entries = PACKAGE.archive_entries(archive)
    except (PACKAGE.PackageError, OSError, ValueError) as error:
        raise SmokeError("canonical_package_verification_failed") from error
    expected_root = f"worldstream-{identity['version']}-{identity['target']}"
    roots = {PurePosixPath(name).parts[0] for name in entries}
    require(roots == {expected_root}, "package_archive_root_mismatch")
    relative = {
        str(PurePosixPath(name).relative_to(expected_root)): content
        for name, content in entries.items()
    }
    require(
        relative.get("manifest/compatibility.toml") == root_toml
        and relative.get("manifest/compatibility.json") == root_json,
        "package_archive_manifest_bytes_mismatch",
    )
    daemon_bytes = relative.get(f"bin/{daemon_name}")
    control_bytes = relative.get(f"bin/{control_name}")
    build_identity_bytes = relative.get(PACKAGE.BUILD_IDENTITY.BUILD_METADATA_PATH)
    require(
        isinstance(daemon_bytes, bytes)
        and bool(daemon_bytes)
        and isinstance(control_bytes, bytes)
        and bool(control_bytes)
        and isinstance(build_identity_bytes, bytes)
        and bool(build_identity_bytes),
        "package_binary_inventory_incomplete",
    )
    build_identity = json.loads(build_identity_bytes)
    require(
        isinstance(build_identity, dict)
        and build_identity.get("source", {}).get("revision")
        == identity["source_revision"]
        and sha256_bytes(build_identity_bytes) == identity["build_identity_sha256"],
        "package_build_identity_mismatch",
    )
    output_dir.mkdir(mode=0o700)
    protect_directory(output_dir)
    daemon = output_dir / daemon_name
    control = output_dir / control_name
    for path, content in ((daemon, daemon_bytes), (control, control_bytes)):
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o700)
        with os.fdopen(descriptor, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        os.chmod(path, 0o700)
        require(path.read_bytes() == content, "extracted_package_binary_mismatch")
    binding = {
        "artifact": archive.name,
        "target": identity["target"],
        "version": identity["version"],
        "archive_sha256": sha256_file(archive),
        "archive_size_bytes": archive.stat().st_size,
        "package_report_sha256": sha256_file(package_report_path),
        "package_report_size_bytes": package_report_path.stat().st_size,
        "manifest_sha256": SHA256_PREFIX + identity["manifest_sha256"],
        "manifest_json_sha256": SHA256_PREFIX + identity["manifest_json_sha256"],
        "manifest_toml_sha256": SHA256_PREFIX + identity["manifest_toml_sha256"],
        "source_revision": identity["source_revision"],
        "build_identity_sha256": identity["build_identity_sha256"],
        "worldstreamd_sha256": sha256_bytes(daemon_bytes),
        "worldstreamd_size_bytes": len(daemon_bytes),
        "worldstreamctl_sha256": sha256_bytes(control_bytes),
        "worldstreamctl_size_bytes": len(control_bytes),
        "canonical_archive_verified": True,
        "exact_archive_bytes_executed": True,
    }
    manifest = json.loads(root_json.decode("utf-8"))
    require(
        isinstance(manifest, dict)
        and manifest.get("release_candidate") == identity["version"]
        and manifest.get("manifest_kind") == "release"
        and manifest.get("release_ready") is True,
        "embedded_release_contract_incomplete",
    )
    return daemon, control, binding, manifest


def parse_dsn(path: Path) -> dict[str, str]:
    regular_file(path, "admin_dsn_file_invalid")
    require(0 < path.stat().st_size <= MAX_CONTROL_BYTES, "admin_dsn_file_invalid")
    protect_file(path)
    value = path.read_text(encoding="utf-8").strip()
    require(value and "\n" not in value and "\r" not in value, "admin_dsn_invalid")
    if "://" in value:
        parsed = urllib.parse.urlsplit(value)
        require(
            parsed.scheme in {"postgres", "postgresql"}
            and parsed.hostname is not None
            and parsed.username is not None
            and parsed.password is not None,
            "admin_dsn_invalid",
        )
        return {
            "host": parsed.hostname,
            "port": str(parsed.port or 5432),
            "dbname": urllib.parse.unquote(parsed.path.lstrip("/")) or "postgres",
            "user": urllib.parse.unquote(parsed.username),
            "password": urllib.parse.unquote(parsed.password),
        }
    try:
        fields = dict(part.split("=", 1) for part in shlex.split(value))
    except (ValueError, TypeError) as error:
        raise SmokeError("admin_dsn_invalid") from error
    require(
        all(fields.get(name) for name in ("host", "dbname", "user", "password")),
        "admin_dsn_invalid",
    )
    fields.setdefault("port", "5432")
    return {
        name: fields[name] for name in ("host", "port", "dbname", "user", "password")
    }


def run_process(
    argv: list[str],
    *,
    environment: dict[str, str] | None = None,
    input_text: str | None = None,
    timeout: float = 120,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        argv,
        cwd=ROOT,
        env=environment,
        input=input_text,
        text=True,
        capture_output=True,
        check=False,
        timeout=timeout,
    )


def run_bounded_process(
    argv: list[str],
    *,
    environment: dict[str, str] | None,
    timeout: float,
    stdout_limit: int,
    stderr_limit: int,
    code: str,
) -> tuple[int, bytes, bytes]:
    """Capture a child without permitting either output pipe to grow unbounded."""

    try:
        process = subprocess.Popen(
            argv,
            cwd=ROOT,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError as error:
        raise SmokeError(code) from error
    require(process.stdout is not None and process.stderr is not None, code)
    overflow = threading.Event()
    buffers = {"stdout": bytearray(), "stderr": bytearray()}

    def drain(name: str, stream, limit: int) -> None:
        try:
            while chunk := stream.read(64 * 1024):
                remaining = max(0, limit + 1 - len(buffers[name]))
                if remaining:
                    buffers[name].extend(chunk[:remaining])
                if len(chunk) > remaining or len(buffers[name]) > limit:
                    overflow.set()
                    with contextlib.suppress(OSError):
                        process.kill()
                    return
        except OSError:
            overflow.set()
            with contextlib.suppress(OSError):
                process.kill()

    readers = [
        threading.Thread(
            target=drain,
            args=("stdout", process.stdout, stdout_limit),
            daemon=True,
        ),
        threading.Thread(
            target=drain,
            args=("stderr", process.stderr, stderr_limit),
            daemon=True,
        ),
    ]
    for reader in readers:
        reader.start()
    try:
        returncode = process.wait(timeout=timeout)
    except subprocess.TimeoutExpired as error:
        process.kill()
        process.wait()
        for reader in readers:
            reader.join(timeout=5)
        raise SmokeError(code) from error
    for reader in readers:
        reader.join(timeout=5)
    if any(reader.is_alive() for reader in readers):
        process.kill()
        process.wait()
        raise SmokeError(code)
    require(not overflow.is_set(), code)
    return returncode, bytes(buffers["stdout"]), bytes(buffers["stderr"])


def run_json(
    argv: list[str],
    code: str,
    *,
    environment: dict[str, str] | None = None,
    timeout: float = 120,
) -> dict[str, Any]:
    returncode, raw, _stderr = run_bounded_process(
        argv,
        environment=environment,
        timeout=timeout,
        stdout_limit=MAX_CONTROL_BYTES,
        stderr_limit=MAX_LOG_BYTES,
        code=code,
    )
    require(returncode == 0, code)
    return strict_runtime_json(raw, code)


def minimal_child_environment() -> dict[str, str]:
    return {
        name: os.environ[name]
        for name in (
            "PATH",
            "LANG",
            "LC_ALL",
            "LC_CTYPE",
            "SYSTEMROOT",
            "WINDIR",
            "COMSPEC",
            "PATHEXT",
            "TEMP",
            "TMP",
            "TMPDIR",
        )
        if name in os.environ
    }


def validate_packaged_operator_surface(control: Path) -> None:
    commands = (
        ([str(control), "sqlite", "--help"], (b"backup", b"restore", b"verify")),
        (
            [str(control), "sqlite", "backup", "--help"],
            (b"--database", b"--output", b"--companion", b"--envelope"),
        ),
        (
            [str(control), "sqlite", "restore", "--help"],
            (b"--backup", b"--database", b"--envelope"),
        ),
        ([str(control), "sqlite", "verify", "--help"], (b"--database",)),
        (
            [str(control), "postgres", "transfer", "--help"],
            (b"begin", b"resume", b"finalize", b"abort"),
        ),
        (
            [str(control), "postgres", "transfer", "begin", "--help"],
            (b"--sqlite", b"--backup", b"--bundle", b"--state-dir", b"--transfer-id"),
        ),
        (
            [str(control), "postgres", "transfer", "resume", "--help"],
            (
                b"--sqlite",
                b"--bundle",
                b"--state-dir",
                b"--dsn-file",
                b"--chunk-records",
            ),
        ),
        (
            [str(control), "postgres", "transfer", "finalize", "--help"],
            (b"--sqlite", b"--bundle", b"--state-dir", b"--dsn-file"),
        ),
        (
            [str(control), "postgres", "transfer", "abort", "--help"],
            (b"--sqlite", b"--bundle", b"--state-dir", b"--dsn-file"),
        ),
    )
    for argv, required in commands:
        returncode, stdout, _stderr = run_bounded_process(
            argv,
            environment=minimal_child_environment(),
            timeout=10,
            stdout_limit=MAX_CONTROL_BYTES,
            stderr_limit=MAX_LOG_BYTES,
            code="packaged_operator_surface_missing",
        )
        require(
            returncode == 0 and all(value in stdout for value in required),
            "packaged_operator_surface_missing",
        )


SQLITE_ENVELOPE_COVERAGE = (
    "retired_authority_fences_v1",
    "principals",
    "runners",
    "capabilities",
    "capability_scopes",
    "runner_capability_memberships",
    "authority_change_receipts",
    "authority_audit",
    "room_integrity",
    "room_members",
    "timers",
    "observation_frames",
    "observation_consequences",
    "activation_decisions",
    "activation_intents",
    "activation_operation_receipts",
    "semantic_receipts",
    "integrity_incidents",
)


def blake3_digest_text(value: object, code: str) -> str:
    require(
        isinstance(value, str)
        and value.startswith("blake3:")
        and len(value) == len("blake3:") + 64
        and all(character in "0123456789abcdef" for character in value[7:]),
        code,
    )
    return value[7:]


def build_empty_sqlite_companion(manifest: dict[str, Any]) -> bytes:
    migrations = manifest.get("migrations")
    executors = manifest.get("pack_executors")
    sqlite = manifest.get("storage", {}).get("sqlite")
    require(
        isinstance(migrations, dict)
        and isinstance(migrations.get("entries"), list)
        and isinstance(executors, list)
        and isinstance(sqlite, dict)
        and isinstance(sqlite.get("version"), str),
        "sqlite_companion_manifest_invalid",
    )
    migration_records = []
    for entry in migrations["entries"]:
        require(isinstance(entry, dict), "sqlite_companion_manifest_invalid")
        checksum = entry.get("sqlite_checksum")
        if not checksum:
            continue
        migration_records.append(
            {
                "version": len(migration_records) + 1,
                "migration_id": entry.get("id"),
                "checksum": blake3_digest_text(
                    checksum, "sqlite_companion_manifest_invalid"
                ),
            }
        )
    counter = next(
        (
            value
            for value in executors
            if isinstance(value, dict)
            and value.get("pack_id") == "worldstream.counter"
            and value.get("explanatory_version") == "2.0.0"
            and value.get("status") == "resolved"
            and value.get("runnable_for_retained_rooms") is True
        ),
        None,
    )
    require(
        isinstance(counter, dict) and len(migration_records) > 0,
        "sqlite_companion_manifest_invalid",
    )
    counter_source = regular_file(
        ROOT / "crates/worldstream-core/src/counter.rs",
        "sqlite_companion_source_invalid",
    ).read_bytes()
    canonical_source = counter_source.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
    executor = (
        b"worldstream/counter-executor-source/v1\0" + b"2.0.0\0" + canonical_source
    )
    zero = "0" * 64
    native_point = {
        "SqliteOnlineBackup": {
            "engine_identity": sqlite["version"],
            "point_id": "package-smoke-template",
        }
    }
    witness = {
        "backend": "SqliteBundled",
        "native_point": native_point,
        "deployment_lineage": "deployment/package-smoke-sqlite-bundled",
        "storage_epoch": 1,
        "evidence_digest": zero,
        "membership_digest": zero,
    }
    resource_id = "package-smoke-counter-v2-executor"
    value = {
        "schema": "worldstream/native-sqlite-backup-envelope/v1",
        "origin": {
            "producer": "package-smoke-template",
            "capture_id": "package-smoke-template",
            "coverage": "package smoke empty deployment companion",
        },
        "manifest": {
            "schema": "worldstream/backup-manifest/v1",
            "backup_id": "package-smoke-sqlite-backup",
            "deployment_lineage": "deployment/package-smoke-sqlite-bundled",
            "storage_epoch": 1,
            "backend": "SqliteBundled",
            "native_point": native_point,
            "migration_contract": {
                "logical_history_id": migrations.get("logical_history_id"),
                "schema_contract_fingerprint": blake3_digest_text(
                    migrations.get("schema_contract_fingerprint"),
                    "sqlite_companion_manifest_invalid",
                ),
                "records": migration_records,
            },
            "expected_packs": [
                {
                    "pack_id": counter["pack_id"],
                    "revision_digest": blake3_digest_text(
                        counter.get("revision_digest"),
                        "sqlite_companion_manifest_invalid",
                    ),
                    "executor_digest": blake3_digest_text(
                        counter.get("executor_artifact_digest"),
                        "sqlite_companion_manifest_invalid",
                    ),
                    "schema_bundle_digest": blake3_digest_text(
                        counter.get("schema_bundle_digest"),
                        "sqlite_companion_manifest_invalid",
                    ),
                    "codec_bundle_digest": blake3_digest_text(
                        counter.get("codec_bundle_digest"),
                        "sqlite_companion_manifest_invalid",
                    ),
                    "resource_ids": [resource_id],
                }
            ],
            "expected_resources": [
                {
                    "resource_id": resource_id,
                    "kind": "executor",
                    "byte_len": len(executor),
                    "digest": blake3_digest_text(
                        counter.get("executor_artifact_digest"),
                        "sqlite_companion_manifest_invalid",
                    ),
                }
            ],
            "expected_global_digest": zero,
        },
        "resources": [{"resource_id": resource_id, "bytes": list(executor)}],
        "request_witnesses": [],
        "authoritative_materializations": [],
        "timer_relations": [],
        "source": witness,
        "restored": witness,
        "coverage": {
            "tables": list(SQLITE_ENVELOPE_COVERAGE),
            "source_evidence_digest": zero,
            "restored_evidence_digest": zero,
            "source_membership_digest": zero,
            "restored_membership_digest": zero,
        },
        "envelope_digest": zero,
    }
    encoded = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode(
        "utf-8"
    )
    require(0 < len(encoded) <= MAX_CONTROL_BYTES, "sqlite_companion_too_large")
    return encoded


def owner_only_runtime_file(path: Path, code: str) -> Path:
    path = regular_file(path, code)
    if os.name != "nt":
        require(stat.S_IMODE(path.stat().st_mode) & 0o077 == 0, code)
    return path


def run_packaged_sqlite_roundtrip(
    control: Path,
    database: Path,
    root: Path,
    manifest: dict[str, Any],
) -> dict[str, str]:
    owner_only_runtime_file(database, "sqlite_operator_source_invalid")
    root.mkdir(mode=0o700)
    protect_directory(root)
    companion = root / "companion.template.json"
    backup = root / "backup.sqlite3"
    envelope = root / "backup.envelope.json"
    restored = root / "restored.sqlite3"
    write_secret(companion, build_empty_sqlite_companion(manifest))
    environment = minimal_child_environment()
    backup_result = run_json(
        [
            str(control),
            "sqlite",
            "backup",
            "--database",
            str(database),
            "--output",
            str(backup),
            "--companion",
            str(companion),
            "--envelope",
            str(envelope),
        ],
        "packaged_sqlite_backup_failed",
        environment=environment,
    )
    owner_only_runtime_file(backup, "packaged_sqlite_backup_artifact_invalid")
    owner_only_runtime_file(envelope, "packaged_sqlite_envelope_invalid")
    exact_envelope = envelope.read_bytes()
    require(
        backup_result.get("status") == "ok"
        and backup_result.get("operation") == "backup"
        and backup_result.get("native_verifier") == "pass"
        and backup_result.get("semantic_verifier") == "pass"
        and backup_result.get("envelope_bytes") == len(exact_envelope)
        and isinstance(backup_result.get("envelope_digest"), str)
        and len(backup_result["envelope_digest"]) == 64
        and isinstance(backup_result.get("envelope_file_digest"), str)
        and len(backup_result["envelope_file_digest"]) == 64,
        "packaged_sqlite_backup_contract_failed",
    )
    restore_result = run_json(
        [
            str(control),
            "sqlite",
            "restore",
            "--backup",
            str(backup),
            "--database",
            str(restored),
            "--envelope",
            str(envelope),
        ],
        "packaged_sqlite_restore_failed",
        environment=environment,
    )
    owner_only_runtime_file(restored, "packaged_sqlite_restored_artifact_invalid")
    require(
        restore_result.get("status") == "ok"
        and restore_result.get("operation") == "restore"
        and restore_result.get("native_verifier") == "pass"
        and restore_result.get("semantic_verifier") == "pass"
        and restore_result.get("envelope_digest")
        == backup_result.get("envelope_digest")
        and restore_result.get("envelope_file_digest")
        == backup_result.get("envelope_file_digest")
        and restore_result.get("envelope_bytes") == len(exact_envelope)
        and envelope.read_bytes() == exact_envelope,
        "packaged_sqlite_restore_contract_failed",
    )
    verify_result = run_json(
        [str(control), "sqlite", "verify", "--database", str(restored)],
        "packaged_sqlite_verify_failed",
        environment=environment,
    )
    require(
        verify_result.get("status") == "ok"
        and verify_result.get("operation") == "verify"
        and verify_result.get("native_verifier") == "pass"
        and verify_result.get("semantic_verifier") == "not_invoked",
        "packaged_sqlite_verify_contract_failed",
    )
    rendered = json.dumps(
        [backup_result, restore_result, verify_result], sort_keys=True
    ).encode()
    for path in (database, companion, backup, envelope, restored):
        require(str(path).encode("utf-8") not in rendered, "sqlite_operator_path_leak")
    return {
        "backup": "native_and_semantic_pass",
        "restore": "native_and_semantic_pass",
        "verify": "native_only_pass_semantic_not_invoked",
        "envelope": "exact_bytes_preserved",
    }


def connection_dsn_bytes(connection: dict[str, str]) -> bytes:
    user = urllib.parse.quote(connection["user"], safe="")
    password = urllib.parse.quote(connection["password"], safe="")
    database = urllib.parse.quote(connection["dbname"], safe="")
    scheme = "postgresql"
    value = (
        f"{scheme}://{user}:{password}@{connection['host']}:"
        f"{connection['port']}/{database}\n"
    ).encode()
    require(0 < len(value) <= MAX_CONTROL_BYTES, "postgres_dsn_encoding_failed")
    return value


def source_transfer_state(database: Path) -> tuple[str, bool]:
    owner_only_runtime_file(database, "transfer_source_state_invalid")
    connection = sqlite3.connect(f"{database.resolve().as_uri()}?mode=ro", uri=True)
    try:
        row = connection.execute(
            "SELECT state, "
            "last_aborted_bundle_hash IS NOT NULL "
            "AND last_aborted_target_fingerprint IS NOT NULL "
            "FROM source_transfer_lifecycle WHERE lifecycle_id = 1"
        ).fetchone()
    finally:
        connection.close()
    require(
        isinstance(row, tuple)
        and len(row) == 2
        and isinstance(row[0], str)
        and row[1] in (0, 1),
        "transfer_source_state_invalid",
    )
    return row[0], bool(row[1])


def validate_transfer_result(
    result: dict[str, Any],
    operation: str,
    code: str,
) -> None:
    require(
        set(result)
        == {
            "schema",
            "status",
            "operation",
            "phase",
            "bundle_hash",
            "target_fingerprint",
            "source_epoch",
            "target_epoch",
            "record_count",
            "next_ordinal",
            "chunks_complete",
        }
        and result.get("schema") == "worldstream/operator-transfer-result/v1"
        and result.get("status") == "ok"
        and result.get("operation") == operation
        and isinstance(result.get("phase"), str)
        and isinstance(result.get("bundle_hash"), str)
        and len(result["bundle_hash"]) == 64
        and isinstance(result.get("target_fingerprint"), str)
        and len(result["target_fingerprint"]) == 64
        and all(
            character in "0123456789abcdef"
            for character in result["bundle_hash"] + result["target_fingerprint"]
        )
        and isinstance(result.get("source_epoch"), int)
        and not isinstance(result.get("source_epoch"), bool)
        and isinstance(result.get("target_epoch"), int)
        and not isinstance(result.get("target_epoch"), bool)
        and result["target_epoch"] == result["source_epoch"] + 1
        and isinstance(result.get("record_count"), int)
        and not isinstance(result.get("record_count"), bool)
        and result["record_count"] > 1
        and isinstance(result.get("next_ordinal"), int)
        and not isinstance(result.get("next_ordinal"), bool)
        and 0 <= result["next_ordinal"] <= result["record_count"]
        and isinstance(result.get("chunks_complete"), bool),
        code,
    )


def run_transfer_command(
    control: Path,
    operation: str,
    sqlite: Path,
    bundle: Path,
    state_dir: Path,
    *,
    backup: Path | None = None,
    transfer_id: str | None = None,
    dsn_file: Path | None = None,
    chunk_records: int | None = None,
) -> dict[str, Any]:
    argv = [str(control), "postgres", "transfer", operation]
    argv.extend(["--sqlite", str(sqlite)])
    if backup is not None:
        argv.extend(["--backup", str(backup)])
    argv.extend(["--bundle", str(bundle), "--state-dir", str(state_dir)])
    if transfer_id is not None:
        argv.extend(["--transfer-id", transfer_id])
    if dsn_file is not None:
        argv.extend(["--dsn-file", str(dsn_file)])
    if chunk_records is not None:
        argv.extend(["--chunk-records", str(chunk_records)])
    result = run_json(
        argv,
        f"packaged_transfer_{operation}_failed",
        environment=minimal_child_environment(),
        timeout=300,
    )
    validate_transfer_result(
        result, operation, f"packaged_transfer_{operation}_contract"
    )
    return result


def verify_aborted_postgres_is_empty(
    psql_path: Path,
    connection: dict[str, str],
) -> None:
    inventory = psql(
        psql_path,
        connection,
        "SELECT table_name FROM information_schema.tables "
        "WHERE table_schema = 'public' AND table_type = 'BASE TABLE' "
        "ORDER BY table_name;",
        "packaged_transfer_abort_table_inventory_failed",
    ).splitlines()
    require(
        inventory == sorted(POSTGRES_TRANSFER_DURABLE_TABLES),
        "packaged_transfer_abort_table_inventory_failed",
    )
    counts_sql = " UNION ALL ".join(
        f"SELECT '{name}' AS table_name, count(*)::bigint AS row_count FROM {name}"
        for name in POSTGRES_TRANSFER_DURABLE_TABLES
    )
    raw_counts = psql(
        psql_path,
        connection,
        f"SELECT table_name || '|' || row_count::text FROM ({counts_sql}) counts "
        "ORDER BY table_name;",
        "packaged_transfer_abort_table_counts_failed",
    )
    counts: dict[str, int] = {}
    try:
        for line in raw_counts.splitlines():
            name, raw_count = line.split("|", 1)
            counts[name] = int(raw_count)
    except (ValueError, TypeError) as error:
        raise SmokeError("packaged_transfer_abort_table_counts_failed") from error
    require(
        set(counts) == set(POSTGRES_TRANSFER_DURABLE_TABLES)
        and counts["worldstream_schema_migrations"] > 0
        and counts["worldstream_authority_state"] == 1
        and counts["worldstream_transfer_target_fence"] == 1
        and all(
            count == 0
            for name, count in counts.items()
            if name
            not in {
                "worldstream_schema_migrations",
                "worldstream_authority_state",
                "worldstream_transfer_target_fence",
            }
        ),
        "packaged_transfer_abort_target_not_empty",
    )
    fence_state = psql(
        psql_path,
        connection,
        "SELECT state FROM worldstream_transfer_target_fence WHERE fence_id = true;",
        "packaged_transfer_abort_fence_probe_failed",
    )
    require(fence_state == "aborted", "packaged_transfer_abort_fence_probe_failed")


def run_packaged_transfer_roundtrip(
    control: Path,
    sqlite: Path,
    admin_dsn_file: Path,
    abort_dsn_file: Path,
    root: Path,
    psql_path: Path,
    admin: dict[str, str],
    abort_admin: dict[str, str],
    binding: dict[str, Any],
    forbidden_secret_bytes: tuple[bytes, ...],
) -> dict[str, Any]:
    owner_only_runtime_file(sqlite, "packaged_transfer_source_invalid")
    owner_only_runtime_file(admin_dsn_file, "packaged_transfer_dsn_invalid")
    owner_only_runtime_file(abort_dsn_file, "packaged_transfer_dsn_invalid")
    root.mkdir(mode=0o700)
    protect_directory(root)

    abort_state = root / "abort-state"
    abort_state.mkdir(mode=0o700)
    protect_directory(abort_state)
    abort_backup = abort_state / "source.backup.sqlite3"
    abort_bundle = abort_state / "deployment.bundle"
    abort_id = "package-smoke-abort-v1"
    abort_begin = run_transfer_command(
        control,
        "begin",
        sqlite,
        abort_bundle,
        abort_state,
        backup=abort_backup,
        transfer_id=abort_id,
    )
    abort_resume = run_transfer_command(
        control,
        "resume",
        sqlite,
        abort_bundle,
        abort_state,
        dsn_file=abort_dsn_file,
        chunk_records=1,
    )
    require(
        abort_resume["next_ordinal"] == 1 and abort_resume["chunks_complete"] is False,
        "packaged_transfer_abort_restart_boundary_failed",
    )
    checkpoint_probe = psql(
        psql_path,
        abort_admin,
        "SELECT count(*)::text || '|' || "
        "COALESCE(sum(octet_length(records_bytes)), 0)::text "
        "FROM worldstream_transfer_chunks;",
        "packaged_transfer_abort_checkpoint_probe_failed",
    )
    try:
        checkpoint_count, checkpoint_bytes = (
            int(value) for value in checkpoint_probe.split("|", 1)
        )
    except (ValueError, TypeError) as error:
        raise SmokeError("packaged_transfer_abort_checkpoint_probe_failed") from error
    require(
        checkpoint_count == 1 and checkpoint_bytes > 0,
        "packaged_transfer_abort_checkpoint_probe_failed",
    )
    abort_result = run_transfer_command(
        control,
        "abort",
        sqlite,
        abort_bundle,
        abort_state,
        dsn_file=abort_dsn_file,
    )
    abort_replay = run_transfer_command(
        control,
        "abort",
        sqlite,
        abort_bundle,
        abort_state,
        dsn_file=abort_dsn_file,
    )
    source_state, aborted_witness = source_transfer_state(sqlite)
    verify_aborted_postgres_is_empty(psql_path, abort_admin)
    require(
        abort_begin["bundle_hash"]
        == abort_resume["bundle_hash"]
        == abort_result["bundle_hash"]
        == abort_replay["bundle_hash"]
        and abort_result["phase"] == "source_authoritative"
        and abort_replay["phase"] == "source_authoritative"
        and source_state == "source_authoritative"
        and aborted_witness,
        "packaged_transfer_abort_cleanup_failed",
    )

    transfer_state = root / "finalize-state"
    transfer_state.mkdir(mode=0o700)
    protect_directory(transfer_state)
    transfer_backup = transfer_state / "source.backup.sqlite3"
    transfer_bundle = transfer_state / "deployment.bundle"
    transfer_id = "package-smoke-finalize-v1"
    begin = run_transfer_command(
        control,
        "begin",
        sqlite,
        transfer_bundle,
        transfer_state,
        backup=transfer_backup,
        transfer_id=transfer_id,
    )
    owner_only_runtime_file(transfer_backup, "packaged_transfer_backup_invalid")
    owner_only_runtime_file(transfer_bundle, "packaged_transfer_bundle_invalid")
    bundle_bytes = transfer_bundle.read_bytes()
    require(
        0 < len(bundle_bytes) <= 64 * 1024 * 1024,
        "packaged_transfer_bundle_invalid",
    )
    resume = run_transfer_command(
        control,
        "resume",
        sqlite,
        transfer_bundle,
        transfer_state,
        dsn_file=admin_dsn_file,
        chunk_records=1,
    )
    require(
        resume["next_ordinal"] == 1 and resume["chunks_complete"] is False,
        "packaged_transfer_restart_boundary_failed",
    )
    resume_invocations = 1
    maximum_resumes = min(128, (begin["record_count"] + 1_023) // 1_024 + 2)
    while not resume["chunks_complete"]:
        require(
            resume_invocations < maximum_resumes,
            "packaged_transfer_resume_bound_exceeded",
        )
        previous = resume["next_ordinal"]
        resume = run_transfer_command(
            control,
            "resume",
            sqlite,
            transfer_bundle,
            transfer_state,
            dsn_file=admin_dsn_file,
            chunk_records=1_024,
        )
        resume_invocations += 1
        require(
            resume["next_ordinal"] > previous
            and resume["bundle_hash"] == begin["bundle_hash"]
            and resume["target_fingerprint"] == begin["target_fingerprint"],
            "packaged_transfer_resume_checkpoint_failed",
        )
    generations_before_replay = sorted(transfer_state.glob("transfer-state-v1-*.json"))
    completed_replay = run_transfer_command(
        control,
        "resume",
        sqlite,
        transfer_bundle,
        transfer_state,
        dsn_file=admin_dsn_file,
        chunk_records=1_024,
    )
    generations_after_replay = sorted(transfer_state.glob("transfer-state-v1-*.json"))
    require(
        completed_replay == resume
        and generations_after_replay == generations_before_replay,
        "packaged_transfer_completed_resume_not_idempotent",
    )
    finalized = run_transfer_command(
        control,
        "finalize",
        sqlite,
        transfer_bundle,
        transfer_state,
        dsn_file=admin_dsn_file,
    )
    finalized_replay = run_transfer_command(
        control,
        "finalize",
        sqlite,
        transfer_bundle,
        transfer_state,
        dsn_file=admin_dsn_file,
    )
    source_state, _aborted_witness = source_transfer_state(sqlite)
    provider_state = psql(
        psql_path,
        admin,
        "SELECT "
        "COALESCE((SELECT state FROM worldstream_transfer_imports), 'missing') || '|' || "
        "(SELECT count(*) FROM worldstream_transfer_target_fence)::text || '|' || "
        "(SELECT count(*) FROM worldstream_deployment_metadata)::text;",
        "packaged_transfer_target_authority_probe_failed",
    )
    require(
        finalized["phase"] == "target_authoritative"
        and finalized_replay["phase"] == "target_authoritative"
        and finalized["bundle_hash"]
        == begin["bundle_hash"]
        == finalized_replay["bundle_hash"]
        and finalized["target_fingerprint"]
        == begin["target_fingerprint"]
        == finalized_replay["target_fingerprint"]
        and source_state == "source_retired"
        and provider_state == "authoritative|0|1",
        "packaged_transfer_finalization_failed",
    )

    rendered = json.dumps(
        [
            abort_begin,
            abort_resume,
            abort_result,
            abort_replay,
            begin,
            resume,
            completed_replay,
            finalized,
            finalized_replay,
        ],
        sort_keys=True,
    ).encode("utf-8")
    for forbidden in (
        *forbidden_secret_bytes,
        str(sqlite).encode(),
        str(admin_dsn_file).encode(),
        str(abort_dsn_file).encode(),
        str(root).encode(),
        abort_id.encode(),
        transfer_id.encode(),
    ):
        require(
            not forbidden or forbidden not in rendered, "packaged_transfer_output_leak"
        )

    control_binding = {
        name: binding[name]
        for name in (
            "archive_sha256",
            "archive_size_bytes",
            "package_report_sha256",
            "package_report_size_bytes",
            "worldstreamctl_sha256",
            "worldstreamctl_size_bytes",
        )
    }
    return {
        "status": "pass",
        "control_binding": control_binding,
        "abort": {
            "restart_checkpoint": "pass",
            "pre_abort_checkpoint_rows": checkpoint_count,
            "pre_abort_checkpoint_bytes": checkpoint_bytes,
            "provider_cleanup": "all_durable_domains_empty_with_exact_tombstone",
            "source_authority_restored": "pass",
            "abort_replay": "pass",
            "bundle_sha256": sha256_file(abort_bundle),
            "bundle_size_bytes": abort_bundle.stat().st_size,
        },
        "finalized": {
            "restart_resume": "pass",
            "resume_invocations": resume_invocations,
            "completed_resume_replay": "no_new_generation",
            "canonical_bundle_sha256": sha256_bytes(bundle_bytes),
            "canonical_bundle_size_bytes": len(bundle_bytes),
            "operator_bundle_hash": begin["bundle_hash"],
            "record_count": begin["record_count"],
            "target_epoch": begin["target_epoch"],
            "hydration_verified_before_handoff": "pass",
            "source_retired": "pass",
            "target_authoritative": "pass",
            "finalize_replay": "pass",
        },
    }


def psql(
    executable: Path,
    connection: dict[str, str],
    sql: str,
    code: str,
) -> str:
    passfile, passfile_root, identity = create_pgpass(connection)
    environment = minimal_child_environment()
    environment.pop("PG" + "PASSWORD", None)
    environment["PGPASSFILE"] = str(passfile)
    try:
        completed = run_process(
            [
                str(executable),
                "--host",
                connection["host"],
                "--port",
                connection["port"],
                "--dbname",
                connection["dbname"],
                "--username",
                connection["user"],
                "--no-password",
                "--no-psqlrc",
                "--quiet",
                "--no-align",
                "--tuples-only",
                "--set=ON_ERROR_STOP=1",
            ],
            environment=environment,
            input_text=sql + "\n",
        )
    finally:
        destroy_pgpass(passfile, passfile_root, identity)
    require(completed.returncode == 0, code)
    return completed.stdout.strip()


def pgpass_escape(value: str) -> str:
    require(
        bool(value)
        and len(value.encode("utf-8")) <= MAX_CONTROL_BYTES
        and not any(character in value for character in ("\0", "\n", "\r")),
        "pgpass_field_invalid",
    )
    return value.replace("\\", "\\\\").replace(":", "\\:")


def file_identity(path: Path, code: str) -> tuple[int, int]:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise SmokeError(code) from error
    require(stat.S_ISREG(metadata.st_mode) and not stat.S_ISLNK(metadata.st_mode), code)
    return metadata.st_dev, metadata.st_ino


def create_pgpass(
    connection: dict[str, str],
) -> tuple[Path, Path, tuple[int, int]]:
    root = Path(tempfile.mkdtemp(prefix="worldstream-psql-pass-"))
    passfile = root / "pgpass"
    try:
        protect_directory(root)
        fields = [
            pgpass_escape(connection[name])
            for name in ("host", "port", "dbname", "user", "password")
        ]
        payload = (":".join(fields) + "\n").encode("utf-8")
        require(len(payload) <= MAX_CONTROL_BYTES, "pgpass_field_invalid")
        write_secret(passfile, payload)
        identity = file_identity(passfile, "pgpass_identity_failed")
        return passfile, root, identity
    except BaseException:
        if passfile.exists():
            try:
                destroy_pgpass(
                    passfile,
                    root,
                    file_identity(passfile, "pgpass_identity_failed"),
                )
            except (SmokeError, OSError):
                pass
        else:
            with contextlib.suppress(OSError):
                root.rmdir()
        raise


def destroy_pgpass(path: Path, root: Path, identity: tuple[int, int]) -> None:
    descriptor: int | None = None
    try:
        require(
            file_identity(path, "pgpass_cleanup_failed") == identity,
            "pgpass_cleanup_failed",
        )
        flags = os.O_WRONLY
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        descriptor = os.open(path, flags)
        open_metadata = os.fstat(descriptor)
        require(
            (open_metadata.st_dev, open_metadata.st_ino) == identity
            and stat.S_ISREG(open_metadata.st_mode)
            and 0 <= open_metadata.st_size <= MAX_CONTROL_BYTES,
            "pgpass_cleanup_failed",
        )
        remaining = open_metadata.st_size
        os.lseek(descriptor, 0, os.SEEK_SET)
        zeros = b"\0" * min(64 * 1024, max(1, remaining))
        while remaining:
            written = os.write(descriptor, zeros[:remaining])
            require(written > 0, "pgpass_cleanup_failed")
            remaining -= written
        os.fsync(descriptor)
        os.ftruncate(descriptor, 0)
        os.fsync(descriptor)
        os.close(descriptor)
        descriptor = None
        os.unlink(path)
        if os.name != "nt":
            directory = os.open(root, os.O_RDONLY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
        root.rmdir()
        require(not path.exists() and not root.exists(), "pgpass_cleanup_failed")
    except (OSError, SmokeError) as error:
        if descriptor is not None:
            with contextlib.suppress(OSError):
                os.close(descriptor)
        if isinstance(error, SmokeError) and str(error) == "pgpass_cleanup_failed":
            raise
        raise SmokeError("pgpass_cleanup_failed") from error


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def http_json(base_url: str, path: str) -> tuple[int, dict[str, Any]]:
    request = urllib.request.Request(
        base_url + path, headers={"Accept": "application/json"}
    )
    try:
        with urllib.request.urlopen(request, timeout=3) as response:
            status = response.status
            body = response.read(MAX_CONTROL_BYTES + 1)
    except urllib.error.HTTPError as error:
        status = error.code
        body = error.read(MAX_CONTROL_BYTES + 1)
    require(len(body) <= MAX_CONTROL_BYTES, "runtime_probe_response_too_large")
    return status, strict_runtime_json(body, "runtime_probe_invalid_json")


def daemon_environment(secret_file: Path, dsn_file: Path | None) -> dict[str, str]:
    # Start from the explicit process-runtime allowlist so hosted-runner libpq
    # PG* credentials, TLS policy, service/session options, and product secret
    # settings cannot influence an exact owner-only-file probe.
    environment = minimal_child_environment()
    environment["WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE"] = str(secret_file)
    if dsn_file is None:
        environment.pop("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE", None)
    else:
        environment["WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE"] = str(dsn_file)
    return environment


def run_profile(
    profile: str,
    daemon: Path,
    control: Path,
    manifest: dict[str, Any],
    root: Path,
    dsn_file: Path | None,
    forbidden_secret_bytes: tuple[bytes, ...],
    source_revision: str,
) -> dict[str, Any]:
    profile_root = root / profile
    profile_root.mkdir(mode=0o700)
    protect_directory(profile_root)
    data_dir = profile_root / "data"
    secret_file = profile_root / "authority.secret"
    write_secret(secret_file, secrets.token_bytes(32))
    port = free_port()
    bind = f"127.0.0.1:{port}"
    base_url = f"http://{bind}"
    environment = daemon_environment(secret_file, dsn_file)
    if profile == "sqlite-bundled":
        environment["WORLDSTREAM__STORAGE__DEPLOYMENT_LINEAGE"] = (
            "deployment/package-smoke-sqlite-bundled"
        )
        environment["WORLDSTREAM__STORAGE__STORAGE_EPOCH"] = "1"
    config_args = [
        "--storage-profile",
        profile,
        "--data-dir",
        str(data_dir),
        "--bind",
        bind,
    ]
    stdout_path = profile_root / "worldstreamd.stdout.log"
    stderr_path = profile_root / "worldstreamd.stderr.log"
    with (
        stdout_path.open("wb") as stdout,
        stderr_path.open("wb") as stderr,
    ):
        process = subprocess.Popen(
            [str(daemon), *config_args],
            cwd=ROOT,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=stdout,
            stderr=stderr,
        )
    try:
        for _ in range(120):
            require(process.poll() is None, f"{profile}_daemon_exited_before_health")
            try:
                status, health = http_json(base_url, "/healthz")
            except (SmokeError, urllib.error.URLError, TimeoutError, OSError):
                time.sleep(0.25)
                continue
            if status == 200 and health == {"status": "ok"}:
                break
            time.sleep(0.25)
        else:
            raise SmokeError(f"{profile}_health_timeout")
        health_status, health = http_json(base_url, "/healthz")
        ready_status, ready = http_json(base_url, "/readyz")
        version_status, version = http_json(base_url, "/version")
        require(
            health_status == 200 and health == {"status": "ok"},
            f"{profile}_health_contract_failed",
        )
        require(
            ready_status == 200 and ready == {"status": "ready"},
            f"{profile}_ready_contract_failed",
        )
        require(version_status == 200, f"{profile}_version_contract_failed")
        require(
            version.get("product_build", {}).get("source_revision") == source_revision,
            f"{profile}_source_revision_mismatch",
        )
        engine = version.get("engine")
        require(
            isinstance(engine, dict)
            and engine.get("profile") == profile
            and engine.get("status") == "verified"
            and isinstance(engine.get("exact_identity"), str)
            and bool(engine["exact_identity"]),
            f"{profile}_engine_identity_failed",
        )
        probe = run_process(
            [
                sys.executable,
                str(PACKAGE_PATH),
                "probe",
                "--base-url",
                base_url,
                "--version",
                manifest["release_candidate"],
                "--source-revision",
                source_revision,
                "--storage-profile",
                profile,
                "--timeout",
                "3",
            ],
            timeout=30,
        )
        require(probe.returncode == 0, f"{profile}_manifest_runtime_probe_failed")
        config = run_json(
            [str(control), *config_args, "config", "validate"],
            f"{profile}_packaged_ctl_config_failed",
            environment=environment,
        )
        effective = run_json(
            [str(control), *config_args, "config", "effective"],
            f"{profile}_packaged_ctl_effective_failed",
            environment=environment,
        )
        precedence_config = profile_root / "precedence.toml"
        precedence_config.write_text(
            'config_version = 1\n[server]\nbind = "127.0.0.1:9511"\n',
            encoding="utf-8",
        )
        protect_file(precedence_config)
        precedence_environment = {
            **environment,
            "WORLDSTREAM_CONFIG": str(profile_root / "must-not-be-selected.toml"),
            "WORLDSTREAM__SERVER__BIND": "127.0.0.1:9611",
        }
        precedence = run_json(
            [
                str(control),
                "--config",
                str(precedence_config),
                *config_args,
                "config",
                "effective",
            ],
            f"{profile}_packaged_ctl_precedence_failed",
            environment=precedence_environment,
        )
        doctor = run_json(
            [str(control), *config_args, "doctor"],
            f"{profile}_packaged_ctl_doctor_failed",
            environment=environment,
        )
        ctl_health = run_json(
            [str(control), *config_args, "health"],
            f"{profile}_packaged_ctl_health_failed",
            environment=environment,
        )
        ctl_version = run_json(
            [str(control), *config_args, "version"],
            f"{profile}_packaged_ctl_version_failed",
            environment=environment,
        )
        validate_control_version(
            ctl_version,
            manifest_summary=version.get("manifest"),
            product=manifest["release_candidate"],
            source_revision=source_revision,
            code=f"{profile}_packaged_ctl_contract_drift",
        )
        require(
            config == {"status": "valid", "storage_profile": profile}
            and effective.get("config_version") == 1
            and effective.get("server") == {"bind": bind}
            and effective.get("storage", {}).get("profile") == profile
            and effective.get("storage", {}).get("data_dir") == str(data_dir)
            and effective.get("authority", {}).get("bootstrap_secret")
            == {
                "source": "owner_readable_secret_file",
                "value": "[REDACTED]",
            }
            and precedence == effective
            and doctor
            == {
                "status": "incomplete",
                "config": "valid",
                "manifest": "valid_specification",
                "data_directory": "owner_only",
                "storage": "not_initialized",
            }
            and ctl_health == {"status": "ok", "probe": "daemon-healthz"},
            f"{profile}_packaged_ctl_contract_drift",
        )
        if profile == "postgres-primary":
            require(
                effective.get("storage", {}).get("postgresql_dsn")
                == {
                    "source": "owner_readable_secret_file",
                    "value": "[REDACTED]",
                },
                f"{profile}_packaged_ctl_effective_redaction_failed",
            )
        rendered_effective = json.dumps(effective, sort_keys=True).encode()
        for forbidden in (
            *forbidden_secret_bytes,
            secret_file.read_bytes(),
            str(secret_file).encode(),
            str(dsn_file).encode() if dsn_file is not None else b"",
        ):
            require(
                not forbidden or forbidden not in rendered_effective,
                f"{profile}_secret_in_effective_config",
            )
        cli_version = run_process([str(control), "--version"], timeout=10)
        require(
            cli_version.returncode == 0
            and cli_version.stdout.strip()
            == f"worldstreamctl {manifest['release_candidate']}",
            f"{profile}_packaged_ctl_binary_version_failed",
        )
        sqlite_operator = None
        if profile == "sqlite-bundled":
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)
            sqlite_operator = run_packaged_sqlite_roundtrip(
                control,
                data_dir / "worldstream.sqlite3",
                profile_root / "sqlite-operator-roundtrip",
                manifest,
            )
        result = {
            "status": "pass",
            "healthz": "pass",
            "readyz": "pass",
            "version": "manifest_and_engine_exact",
            "engine_identity": engine["exact_identity"],
            "packaged_ctl": {
                "config_validate": "pass",
                "config_effective": "redacted_and_precedence_exact",
                "doctor": "bounded_diagnostics_exposed",
                "health": "pass",
                "version": "pass",
                "binary_version": manifest["release_candidate"],
                "source_revision": source_revision,
            },
        }
        if sqlite_operator is not None:
            result["sqlite_operator"] = sqlite_operator
        return result
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)
        log_bytes = b""
        for path in (stdout_path, stderr_path):
            if path.exists():
                require(
                    path.stat().st_size <= MAX_LOG_BYTES,
                    f"{profile}_daemon_log_too_large",
                )
                log_bytes += path.read_bytes()
        for secret in (*forbidden_secret_bytes, secret_file.read_bytes()):
            require(
                not secret or secret not in log_bytes, f"{profile}_secret_in_daemon_log"
            )


def run(args: argparse.Namespace) -> dict[str, Any]:
    root = Path(tempfile.mkdtemp(prefix="worldstream-native-package-smoke-"))
    protect_directory(root)
    runtime_role = f"worldstream_smoke_{secrets.token_hex(8)}"
    runtime_password = secrets.token_hex(24)
    admin: dict[str, str] | None = None
    runtime_role_created = False
    abort_database: str | None = None
    abort_database_created = False
    try:
        admin = parse_dsn(args.admin_dsn_file)
        daemon, control, binding, manifest = verify_and_extract(
            args.package_archive,
            args.package_report,
            args.manifest_toml,
            args.manifest_json,
            root / "package",
            getattr(args, "release_inventory", None),
        )
        retained_control = root / "verified-operator-control" / control.name
        persist_control_output(control, retained_control, binding)
        control = retained_control
        validate_packaged_operator_surface(control)
        psql_path = regular_file(args.psql, "psql_unavailable")
        version_num = psql(
            psql_path,
            admin,
            "SHOW server_version_num;",
            "postgres_version_probe_failed",
        )
        require(version_num == "170011", "postgres_version_is_not_17_11")
        abort_database = f"worldstream_abort_{secrets.token_hex(8)}"
        psql(
            psql_path,
            admin,
            f'CREATE DATABASE "{abort_database}";',
            "postgres_abort_database_setup_failed",
        )
        abort_database_created = True
        abort_admin = {**admin, "dbname": abort_database}
        abort_dsn = root / "postgres-abort-admin.dsn"
        write_secret(abort_dsn, connection_dsn_bytes(abort_admin))
        setup_sql = (
            f"CREATE ROLE \"{runtime_role}\" LOGIN PASSWORD '{runtime_password}' "
            "NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT "
            "NOREPLICATION NOBYPASSRLS;"
            "REVOKE CREATE ON SCHEMA public FROM PUBLIC;"
            f'GRANT CONNECT ON DATABASE "{admin["dbname"]}" TO "{runtime_role}";'
            f'GRANT USAGE ON SCHEMA public TO "{runtime_role}";'
            "ALTER DEFAULT PRIVILEGES IN SCHEMA public "
            f'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO "{runtime_role}";'
            "ALTER DEFAULT PRIVILEGES IN SCHEMA public "
            f'GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO "{runtime_role}";'
        )
        psql(psql_path, admin, setup_sql, "postgres_runtime_role_setup_failed")
        runtime_role_created = True
        migrate = run_json(
            [
                str(control),
                "postgres",
                "migrate",
                "--dsn-file",
                str(args.admin_dsn_file),
            ],
            "packaged_ctl_postgres_migrate_failed",
            environment=minimal_child_environment(),
        )
        verify = run_json(
            [
                str(control),
                "postgres",
                "verify",
                "--dsn-file",
                str(args.admin_dsn_file),
            ],
            "packaged_ctl_postgres_verify_failed",
            environment=minimal_child_environment(),
        )
        abort_migrate = run_json(
            [
                str(control),
                "postgres",
                "migrate",
                "--dsn-file",
                str(abort_dsn),
            ],
            "packaged_ctl_abort_postgres_migrate_failed",
            environment=minimal_child_environment(),
        )
        abort_verify = run_json(
            [
                str(control),
                "postgres",
                "verify",
                "--dsn-file",
                str(abort_dsn),
            ],
            "packaged_ctl_abort_postgres_verify_failed",
            environment=minimal_child_environment(),
        )
        require(
            migrate
            == {
                "status": "ok",
                "operation": "migrate",
                "connection": "direct-admin",
                "release_evidence": False,
            }
            and verify
            == {
                "status": "ok",
                "operation": "verify",
                "connection": "direct-admin",
                "release_evidence": False,
            }
            and abort_migrate == migrate
            and abort_verify == verify,
            "packaged_ctl_postgres_admin_contract_drift",
        )
        grants_sql = (
            "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public "
            f'TO "{runtime_role}";'
            "GRANT USAGE, SELECT, UPDATE ON ALL SEQUENCES IN SCHEMA public "
            f'TO "{runtime_role}";'
            "REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON TABLE "
            "public.worldstream_schema_migrations,"
            "public.worldstream_transfer_imports,"
            "public.worldstream_transfer_chunks,"
            "public.worldstream_transfer_target_fence,"
            "public.worldstream_transfer_stream_imports_v2,"
            "public.worldstream_transfer_stream_chunks_v2,"
            f'public.worldstream_transfer_stream_records_v2 FROM "{runtime_role}";'
            "REVOKE DELETE ON TABLE public.worldstream_frames FROM "
            f'"{runtime_role}";'
        )
        psql(psql_path, admin, grants_sql, "postgres_runtime_grants_failed")
        runtime_connection = {
            **admin,
            "user": runtime_role,
            "password": runtime_password,
        }
        validate_runtime_role_admission(
            psql(
                psql_path,
                runtime_connection,
                RUNTIME_ROLE_ADMISSION_SQL,
                "postgres_runtime_role_admission_failed",
            )
        )
        runtime_dsn = root / "postgres-runtime.dsn"
        encoded_user = urllib.parse.quote(runtime_role, safe="")
        encoded_password = urllib.parse.quote(runtime_password, safe="")
        runtime_scheme = "postgresql"
        runtime_dsn_text = (
            f"{runtime_scheme}://{encoded_user}:{encoded_password}@{admin['host']}:"
            f"{admin['port']}/{urllib.parse.quote(admin['dbname'], safe='')}"
        )
        write_secret(
            runtime_dsn,
            f"{runtime_dsn_text}\n".encode(),
        )
        forbidden = (admin["password"].encode(), runtime_password.encode())
        sqlite = run_profile(
            "sqlite-bundled",
            daemon,
            control,
            manifest,
            root,
            None,
            forbidden,
            binding["source_revision"],
        )
        postgres = run_profile(
            "postgres-primary",
            daemon,
            control,
            manifest,
            root,
            runtime_dsn,
            forbidden,
            binding["source_revision"],
        )
        transfer_operator = run_packaged_transfer_roundtrip(
            control,
            root / "sqlite-bundled" / "data" / "worldstream.sqlite3",
            args.admin_dsn_file,
            abort_dsn,
            root / "packaged-transfer",
            psql_path,
            admin,
            abort_admin,
            binding,
            forbidden,
        )
        psql(
            psql_path,
            admin,
            f'DROP DATABASE "{abort_database}" WITH (FORCE);',
            "postgres_abort_database_cleanup_failed",
        )
        abort_database_created = False
        psql(
            psql_path,
            admin,
            f'DROP OWNED BY "{runtime_role}"; DROP ROLE "{runtime_role}";',
            "postgres_runtime_role_cleanup_failed",
        )
        runtime_role_created = False
        control_output = None
        if args.control_output is not None:
            control_output = persist_control_output(
                control, args.control_output, binding
            )
        result = {
            "schema": SCHEMA,
            "status": "pass",
            "release_evidence": False,
            "secrets_emitted": False,
            "platform": {"system": platform.system(), "machine": platform.machine()},
            "package_binding": binding,
            "profiles": {
                "sqlite-bundled": sqlite,
                "postgres-primary": postgres,
            },
            "postgres_admin": {
                "server_version_num": version_num,
                "dsn_delivery": "owner_only_file",
                "migrate": "pass",
                "verify": "pass",
                "runtime_role": "least_privilege",
            },
            "transfer_operator": transfer_operator,
            "cleanup": "pass",
        }
        if control_output is not None:
            result["control_output"] = control_output
        shutil.rmtree(root)
        require(not root.exists(), "owned_temporary_cleanup_failed")
        return result
    finally:
        if admin is not None and abort_database_created and abort_database is not None:
            try:
                psql(
                    args.psql,
                    admin,
                    f'DROP DATABASE "{abort_database}" WITH (FORCE);',
                    "postgres_abort_database_cleanup_failed",
                )
            except (SmokeError, OSError, subprocess.SubprocessError):
                pass
        if admin is not None and runtime_role_created:
            try:
                psql(
                    args.psql,
                    admin,
                    f'DROP OWNED BY "{runtime_role}"; DROP ROLE IF EXISTS "{runtime_role}";',
                    "postgres_runtime_role_cleanup_failed",
                )
            except (SmokeError, OSError, subprocess.SubprocessError):
                pass
        shutil.rmtree(root, ignore_errors=True)


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--package-archive", type=Path, required=True)
    command.add_argument("--package-report", type=Path, required=True)
    command.add_argument("--admin-dsn-file", type=Path, required=True)
    command.add_argument(
        "--psql", type=Path, default=Path(shutil.which("psql") or "psql")
    )
    command.add_argument("--report", type=Path, required=True)
    command.add_argument(
        "--control-output",
        type=Path,
        help="No-clobber owner-only path for the exact verified packaged worldstreamctl",
    )
    command.add_argument(
        "--manifest-toml", type=Path, default=ROOT / "compatibility.toml"
    )
    command.add_argument(
        "--manifest-json", type=Path, default=ROOT / "compatibility.json"
    )
    command.add_argument(
        "--release-inventory",
        choices=tuple(PACKAGE.INVENTORY.BY_ID),
        help="closed release inventory; omission preserves historical verification",
    )
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        report = run(args)
    except (SmokeError, OSError, UnicodeError, subprocess.SubprocessError) as error:
        reason_code = (
            str(error)
            if isinstance(error, SmokeError)
            else "native_package_runtime_io_failure"
        )
        report = {
            "schema": SCHEMA,
            "status": "unavailable",
            "release_evidence": False,
            "secrets_emitted": False,
            "reason_code": reason_code,
            "platform": {"system": platform.system(), "machine": platform.machine()},
            "cleanup": "attempted",
        }
        atomic_write(args.report, report)
        print(json.dumps(report, sort_keys=True))
        return 1
    atomic_write(args.report, report)
    print(json.dumps(report, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

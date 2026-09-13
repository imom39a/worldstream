#!/usr/bin/env python3
"""Run packaged Counter/Heist parity on SQLite, PostgreSQL, and PgBouncer.

The lane owns pinned disposable PostgreSQL 17.11 and transaction-pooler
containers. Passwords are created in an owner-only temporary directory. The
product daemon receives only ``WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE``;
``psql`` receives a passwordless keyword DSN plus ``PGPASSWORD``. No
password-bearing URL is placed in Docker argv or retained evidence.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import importlib.util
import io
import json
import math
import os
import pathlib
import re
import secrets
import shutil
import signal
import stat
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
from dataclasses import dataclass, field
from typing import Any

import tomllib

SCHEMA = "worldstream/packaged-backend-parity/v1"
ROOT = pathlib.Path(__file__).resolve().parents[1]
SECRET_SCAN_PATH = ROOT / "scripts" / "verify-secret-absence.py"
REFERENCE_HOST_PATH = ROOT / "scripts" / "reference_host_environment.py"
BUILD_IDENTITY_PATH = ROOT / "scripts" / "release_build_identity.py"
PACKAGE_PATH = ROOT / "scripts" / "package.py"
POSTGRES_IMAGE = (
    "postgres:17.11-alpine@"
    "sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
)
POSTGRES_DIGEST = (
    "postgres@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
)
PGBOUNCER_IMAGE = (
    "edoburu/pgbouncer@"
    "sha256:4c1ca296ef525f108f5d3552cc337c0c09587cf8dae7f0067fd93349e47dc1cd"
)
PGBOUNCER_DIGEST = (
    "edoburu/pgbouncer@"
    "sha256:4c1ca296ef525f108f5d3552cc337c0c09587cf8dae7f0067fd93349e47dc1cd"
)
EXPECTED_ENGINE = "postgresql/17.11; server_version_num=170011"
PACKAGE_REPORT_SCHEMA = "worldstream/package-report/v1"
PACKAGE_BINDING_SCHEMA = "worldstream/package-binding/v1"
BROWSER_STORY_SCHEMA = "worldstream/package-browser-heist/v1"
PINNED_BROWSER_IDENTITY = {
    "product": "chrome-for-testing-headless-shell",
    "version": "152.0.7977.54",
    "sha256": "sha256:8a3f72f9676736c45e94ae3279b4e2e6a1e323187f9a5e73c9a760e8cc1296ea",
    "size_bytes": 195_435_856,
    "version_output": "Google Chrome for Testing 152.0.7977.54",
    "distribution": {
        "url": "https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.54/linux64/chrome-headless-shell-linux64.zip",
        "sha256": "sha256:11cedb5568cd374a76eb738e40bd434cd0c9956820fb406b8bd9edca53428d3e",
        "size_bytes": 119_570_919,
    },
}
STORAGE_BINDING_SCHEMA = "worldstream/packaged-storage-bindings/v1"
FROZEN_STORAGE_PROFILE = "frozen_local_ext4"
POSTGRES_DATA_DESTINATION = "/var/lib/postgresql/data"
POSTGRES_VOLUME_OWNERSHIP_LABEL = "io.worldstream.packaged-parity.owner"
PRIVACY_CHANNEL_CONTRACT = (
    "every classified child stdout/stderr/safe invocation config, PostgreSQL "
    "and PgBouncer provider stdout/stderr, raw cell report, daemon log, and "
    "aggregate report candidate"
)
PRIVACY_CHANNEL_CLASS_SCHEMA = "worldstream/privacy-channel-class-inventory/v1"
REQUIRED_PRIVACY_CHANNEL_CLASSES = (
    "process.stdout",
    "process.stderr",
    "process.config",
    "provider.postgresql.stdout",
    "provider.postgresql.stderr",
    "provider.pgbouncer.stdout",
    "provider.pgbouncer.stderr",
    "report.cell",
    "daemon.log",
    "report.aggregate",
)
SHA256_PREFIX = "sha256:"
MAX_ARCHIVE_MEMBERS = 20_000
MAX_ARCHIVE_COMPRESSED_BYTES = 4 * 1024 * 1024 * 1024
MAX_ARCHIVE_UNCOMPRESSED_BYTES = 4 * 1024 * 1024 * 1024
MAX_CONTROL_FILE_BYTES = 16 * 1024 * 1024
MAX_CELL_REPORT_BYTES = 64 * 1024 * 1024
MAX_PROVIDER_CHANNEL_BYTES = 16 * 1024 * 1024
MAX_DOCKER_CONTROL_BYTES = 1024 * 1024
MAX_MOUNTINFO_BYTES = 4 * 1024 * 1024
PROVIDER_CAPTURE_TIMEOUT_SECONDS = 30.0
DOCKER_CONTROL_TIMEOUT_SECONDS = 30.0
PROVIDER_CAPTURE_DRAIN_GRACE_SECONDS = 5.0
CHECKSUM_LINE = re.compile(r"^([0-9a-f]{64})  ([^\\\r\n]+)$")
DATABASES = {
    "counter_postgres_direct": "counter_direct",
    "heist_postgres_direct": "heist_direct",
    "counter_transaction_pooler": "counter_pooler",
    "heist_transaction_pooler": "heist_pooler",
}
SQLITE_REFERENCE_SETTINGS = {
    "journal_mode": "wal",
    "synchronous": "full",
    "foreign_keys": "on",
    "busy_timeout_ms": 5000,
    "reader_query_only": "on",
}
POSTGRESQL_CONTRACT_SETTINGS = {
    "isolation_contract": "read_committed",
    "runtime_connection_modes": ["direct", "session_pool", "transaction_pool"],
    "observed_connection_modes": ["direct", "transaction_pool"],
    "maintenance_connection_mode": "direct_admin_offline",
    "transaction_pooler_safe": True,
    "correctness_dependencies_forbidden": [
        "extensions",
        "session_state",
        "named_prepared_statements",
        "connection_affinity",
        "replica_reads",
        "provider_api",
        "provider_failover",
    ],
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


class LaneFailure(Exception):
    """A closed failure safe to retain in a credential-free report."""

    def __init__(self, code: str) -> None:
        super().__init__(code)
        self.code = code


def _validate_runtime_role_admission(witness: str) -> None:
    if witness != RUNTIME_ROLE_ADMISSION_EXPECTED:
        raise LaneFailure("postgres_runtime_role_not_least_privileged")


def _load_secret_scan() -> Any:
    spec = importlib.util.spec_from_file_location(
        "worldstream_postgres_packaged_secret_scan", SECRET_SCAN_PATH
    )
    if spec is None or spec.loader is None:
        raise RuntimeError("secret-absence scanner unavailable")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _load_reference_host() -> Any:
    spec = importlib.util.spec_from_file_location(
        "worldstream_reference_host_environment", REFERENCE_HOST_PATH
    )
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {REFERENCE_HOST_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _load_build_identity() -> Any:
    name = "worldstream_postgres_packaged_strict_json"
    existing = sys.modules.get(name)
    if existing is not None:
        return existing
    spec = importlib.util.spec_from_file_location(name, BUILD_IDENTITY_PATH)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {BUILD_IDENTITY_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def _load_package_verifier() -> Any:
    name = "worldstream_postgres_packaged_package_verifier"
    spec = importlib.util.spec_from_file_location(name, PACKAGE_PATH)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {PACKAGE_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


REFERENCE_HOST = _load_reference_host()
BUILD_IDENTITY = _load_build_identity()
PACKAGE = _load_package_verifier()


SECRET_SCAN = _load_secret_scan()


def _strict_json(content: bytes, code: str) -> dict[str, Any]:
    try:
        return BUILD_IDENTITY.strict_json(content, code)
    except BUILD_IDENTITY.IdentityError as error:
        raise LaneFailure(code) from error


def _regular_file(path: pathlib.Path, code: str) -> pathlib.Path:
    try:
        mode = path.lstat().st_mode
    except OSError as error:
        raise LaneFailure(code) from error
    if stat.S_ISLNK(mode) or not stat.S_ISREG(mode):
        raise LaneFailure(code)
    return path


def _file_identity(metadata: os.stat_result) -> tuple[int, int, int, int, int, int]:
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_mode,
        metadata.st_size,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
    )


@contextlib.contextmanager
def _stable_regular_file(
    path: pathlib.Path,
    code: str,
    *,
    maximum: int,
    expected_size: int | None = None,
):
    """Open one bounded file and reject path or inode changes through its use."""

    if maximum <= 0 or expected_size is not None and not (0 < expected_size <= maximum):
        raise LaneFailure(code)
    try:
        before = path.lstat()
        if (
            stat.S_ISLNK(before.st_mode)
            or not stat.S_ISREG(before.st_mode)
            or not (0 < before.st_size <= maximum)
            or expected_size is not None
            and before.st_size != expected_size
        ):
            raise LaneFailure(code)
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(path, flags)
        with os.fdopen(descriptor, "rb") as source:
            opened = os.fstat(source.fileno())
            identity = _file_identity(before)
            if not stat.S_ISREG(opened.st_mode) or _file_identity(opened) != identity:
                raise LaneFailure(code)
            try:
                yield source, opened
            finally:
                after = os.fstat(source.fileno())
                final_path = path.lstat()
                if (
                    _file_identity(after) != identity
                    or _file_identity(final_path) != identity
                ):
                    raise LaneFailure(code)
    except LaneFailure:
        raise
    except OSError as error:
        raise LaneFailure(code) from error


def _stable_regular_bytes(
    path: pathlib.Path,
    code: str,
    *,
    maximum: int = MAX_CONTROL_FILE_BYTES,
) -> bytes:
    with _stable_regular_file(path, code, maximum=maximum) as (source, opened):
        content = source.read(maximum + 1)
        if len(content) != opened.st_size or len(content) > maximum:
            raise LaneFailure(code)
        return content


def _sha256_record(
    path: pathlib.Path,
    code: str,
    *,
    maximum: int = MAX_CONTROL_FILE_BYTES,
) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    with _stable_regular_file(path, code, maximum=maximum) as (source, opened):
        while chunk := source.read(1024 * 1024):
            size += len(chunk)
            if size > maximum:
                raise LaneFailure(code)
            digest.update(chunk)
        if size != opened.st_size:
            raise LaneFailure(code)
    return digest.hexdigest(), size


def _sha256_file(path: pathlib.Path) -> str:
    return _sha256_record(path, "file_digest_invalid")[0]


@contextlib.contextmanager
def _verified_archive_copy(
    source_path: pathlib.Path,
    staging_parent: pathlib.Path,
    *,
    expected_size: int,
    expected_digest: str,
):
    """Yield one private stable copy of the exact report-bound archive bytes."""

    code = "package_archive_changed_during_verification"
    try:
        parent = staging_parent.lstat()
    except OSError as error:
        raise LaneFailure(code) from error
    if (
        stat.S_ISLNK(parent.st_mode)
        or not stat.S_ISDIR(parent.st_mode)
        or parent.st_uid != os.geteuid()
        or stat.S_IMODE(parent.st_mode) & 0o077
    ):
        raise LaneFailure(code)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=".worldstream-verified-package-", dir=staging_parent
    )
    temporary = pathlib.Path(temporary_name)
    created = os.fstat(descriptor)
    try:
        with os.fdopen(descriptor, "w+b") as stable_copy:
            digest = hashlib.sha256()
            copied = 0
            with _stable_regular_file(
                source_path,
                code,
                maximum=MAX_ARCHIVE_COMPRESSED_BYTES,
                expected_size=expected_size,
            ) as (source, opened):
                while chunk := source.read(1024 * 1024):
                    copied += len(chunk)
                    if copied > MAX_ARCHIVE_COMPRESSED_BYTES:
                        raise LaneFailure(code)
                    digest.update(chunk)
                    stable_copy.write(chunk)
                if copied != opened.st_size:
                    raise LaneFailure(code)
            observed_digest = digest.hexdigest()
            if copied != expected_size or observed_digest != expected_digest:
                raise LaneFailure("package_report_archive_binding_failed")
            stable_copy.flush()
            os.fsync(stable_copy.fileno())
            os.fchmod(stable_copy.fileno(), 0o400)
            stable_copy.seek(0)
            opened_copy = os.fstat(stable_copy.fileno())
            copy_identity = _file_identity(opened_copy)
            try:
                yield stable_copy, observed_digest, copied
            finally:
                after_copy = os.fstat(stable_copy.fileno())
                if _file_identity(after_copy) != copy_identity:
                    raise LaneFailure(code)
    except LaneFailure:
        raise
    except OSError as error:
        raise LaneFailure(code) from error
    finally:
        try:
            temporary_metadata = temporary.lstat()
        except FileNotFoundError:
            pass
        except OSError as error:
            raise LaneFailure(code) from error
        else:
            if not stat.S_ISREG(temporary_metadata.st_mode) or (
                temporary_metadata.st_dev,
                temporary_metadata.st_ino,
            ) != (created.st_dev, created.st_ino):
                raise LaneFailure(code)
            temporary.unlink()


def _safe_archive_member(member: tarfile.TarInfo) -> bool:
    path = pathlib.PurePosixPath(member.name)
    return (
        bool(member.name)
        and not path.is_absolute()
        and "\\" not in member.name
        and all(part not in {"", ".", ".."} for part in path.parts)
        and (member.isfile() or member.isdir())
        and member.size >= 0
    )


def _member_bytes(
    archive: tarfile.TarFile,
    member: tarfile.TarInfo,
    *,
    limit: int = MAX_CONTROL_FILE_BYTES,
) -> bytes:
    if not member.isfile() or member.size > limit:
        raise LaneFailure("package_control_file_invalid")
    source = archive.extractfile(member)
    if source is None:
        raise LaneFailure("package_control_file_unreadable")
    try:
        value = source.read(limit + 1)
    finally:
        source.close()
    if len(value) != member.size or len(value) > limit:
        raise LaneFailure("package_control_file_invalid")
    return value


def _parse_package_report(path: pathlib.Path) -> dict[str, Any]:
    content = _stable_regular_bytes(path, "package_report_invalid")
    return _strict_json(content, "package_report_invalid")


def _parse_checksums(value: bytes) -> dict[str, str]:
    try:
        text = value.decode("utf-8")
    except UnicodeError as error:
        raise LaneFailure("package_checksums_invalid") from error
    if not text.endswith("\n") or "\r" in text:
        raise LaneFailure("package_checksums_invalid")
    checksums: dict[str, str] = {}
    for line in text.splitlines():
        match = CHECKSUM_LINE.fullmatch(line)
        if match is None or match.group(2) in checksums:
            raise LaneFailure("package_checksums_invalid")
        relative = pathlib.PurePosixPath(match.group(2))
        if relative.is_absolute() or any(
            part in {"", ".", ".."} for part in relative.parts
        ):
            raise LaneFailure("package_checksums_invalid")
        checksums[match.group(2)] = match.group(1)
    if not checksums:
        raise LaneFailure("package_checksums_invalid")
    return checksums


def _hash_member(archive: tarfile.TarFile, member: tarfile.TarInfo) -> str:
    source = archive.extractfile(member)
    if source is None:
        raise LaneFailure("package_member_unreadable")
    digest = hashlib.sha256()
    size = 0
    try:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
            size += len(chunk)
    finally:
        source.close()
    if size != member.size:
        raise LaneFailure("package_member_size_mismatch")
    return digest.hexdigest()


def _extract_binary(
    archive: tarfile.TarFile, member: tarfile.TarInfo, output: pathlib.Path
) -> dict[str, Any]:
    if not member.isfile() or member.mode & 0o111 == 0:
        raise LaneFailure("package_binary_not_executable")
    source = archive.extractfile(member)
    if source is None:
        raise LaneFailure("package_binary_unreadable")
    output.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o700)
    digest = hashlib.sha256()
    size = 0
    try:
        with os.fdopen(descriptor, "wb") as destination:
            while chunk := source.read(1024 * 1024):
                destination.write(chunk)
                digest.update(chunk)
                size += len(chunk)
            destination.flush()
            os.fsync(destination.fileno())
    except BaseException:
        output.unlink(missing_ok=True)
        raise
    finally:
        source.close()
    if size != member.size:
        output.unlink(missing_ok=True)
        raise LaneFailure("package_binary_size_mismatch")
    os.chmod(output, 0o700)
    return {
        "archive_path": "/".join(pathlib.PurePosixPath(member.name).parts[1:]),
        "sha256": SHA256_PREFIX + digest.hexdigest(),
        "size_bytes": size,
        "executable": True,
    }


def _extract_tree(
    archive: tarfile.TarFile,
    relative: dict[str, tarfile.TarInfo],
    *,
    prefix: str,
    output: pathlib.Path,
    required: set[str],
) -> dict[str, Any]:
    selected = {
        path.removeprefix(prefix + "/"): member
        for path, member in relative.items()
        if path.startswith(prefix + "/")
    }
    if not selected or not required.issubset(selected):
        raise LaneFailure("package_browser_asset_tree_incomplete")
    records: list[dict[str, Any]] = []
    for path in sorted(selected):
        member = selected[path]
        pure = pathlib.PurePosixPath(path)
        destination = output.joinpath(*pure.parts)
        destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        source = archive.extractfile(member)
        if source is None:
            raise LaneFailure("package_browser_asset_unreadable")
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
        flags |= getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(destination, flags, 0o600)
        digest = hashlib.sha256()
        size = 0
        try:
            with os.fdopen(descriptor, "wb") as target:
                while chunk := source.read(1024 * 1024):
                    target.write(chunk)
                    digest.update(chunk)
                    size += len(chunk)
                target.flush()
                os.fsync(target.fileno())
        except BaseException:
            destination.unlink(missing_ok=True)
            raise
        finally:
            source.close()
        if size != member.size:
            destination.unlink(missing_ok=True)
            raise LaneFailure("package_browser_asset_size_mismatch")
        records.append(
            {
                "path": path,
                "sha256": SHA256_PREFIX + digest.hexdigest(),
                "size_bytes": size,
            }
        )
    canonical = (
        json.dumps(records, sort_keys=True, separators=(",", ":")) + "\n"
    ).encode()
    return {
        "tree_sha256": SHA256_PREFIX + hashlib.sha256(canonical).hexdigest(),
        "file_count": len(records),
        "total_bytes": sum(record["size_bytes"] for record in records),
    }


def _bind_package(
    archive_path: pathlib.Path,
    report_path: pathlib.Path,
    extraction_root: pathlib.Path,
    release_inventory: str | None = None,
) -> tuple[pathlib.Path, pathlib.Path, dict[str, Any]]:
    """Validate exact identity and extract only the accepted runtime surface."""

    if release_inventory is not None:
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                PACKAGE.verify_archive(archive_path, release_inventory)
        except (PACKAGE.PackageError, OSError, ValueError) as error:
            raise LaneFailure("canonical_package_verification_failed") from error

    package_report_raw = _stable_regular_bytes(report_path, "package_report_invalid")
    package_report = _strict_json(package_report_raw, "package_report_invalid")
    identity = package_report.get("identity")
    inventory = package_report.get("inventory")
    archive_size = package_report.get("size_bytes")
    archive_digest_reference = package_report.get("sha256")
    if (
        package_report.get("schema") != PACKAGE_REPORT_SCHEMA
        or package_report.get("kind") != "archive"
        or package_report.get("artifact") != archive_path.name
        or package_report.get("path") != archive_path.name
        or type(archive_size) is not int
        or not (0 < archive_size <= MAX_ARCHIVE_COMPRESSED_BYTES)
        or not isinstance(archive_digest_reference, str)
        or re.fullmatch(r"sha256:[0-9a-f]{64}", archive_digest_reference) is None
        or not isinstance(inventory, dict)
        or inventory.get("archive_verified") is not True
        or inventory.get("manifest_source") != "compatibility.toml"
        or inventory.get("manifest_mirror") != "compatibility.json"
        or inventory.get("release_evidence") is not False
        or not isinstance(identity, dict)
        or identity.get("target") != "linux-x86_64"
        or not isinstance(identity.get("version"), str)
        or not identity["version"]
    ):
        raise LaneFailure("package_report_archive_binding_failed")

    try:
        with contextlib.ExitStack() as stack:
            archive_stream, archive_digest, archive_size = stack.enter_context(
                _verified_archive_copy(
                    archive_path,
                    extraction_root.parent,
                    expected_size=archive_size,
                    expected_digest=archive_digest_reference.removeprefix(
                        SHA256_PREFIX
                    ),
                )
            )
            archive = stack.enter_context(
                tarfile.open(fileobj=archive_stream, mode="r:gz")
            )
            members = archive.getmembers()
            if (
                not members
                or len(members) > MAX_ARCHIVE_MEMBERS
                or sum(member.size for member in members)
                > MAX_ARCHIVE_UNCOMPRESSED_BYTES
                or any(not _safe_archive_member(member) for member in members)
            ):
                raise LaneFailure("package_archive_member_safety_failed")
            names = [member.name for member in members]
            if len(names) != len(set(names)):
                raise LaneFailure("package_archive_duplicate_member")
            roots = {pathlib.PurePosixPath(member.name).parts[0] for member in members}
            expected_root = f"worldstream-{identity['version']}-linux-x86_64"
            if (
                roots != {expected_root}
                or archive_path.name != expected_root + ".tar.gz"
            ):
                raise LaneFailure("package_archive_root_identity_mismatch")
            files = {member.name: member for member in members if member.isfile()}
            relative = {
                name.removeprefix(expected_root + "/"): member
                for name, member in files.items()
            }
            required = {
                "bin/worldstreamd",
                "bin/worldstreamctl",
                "manifest/compatibility.toml",
                "manifest/compatibility.json",
                "metadata/release.json",
                "checksums.sha256",
            }
            if not required.issubset(relative):
                raise LaneFailure("package_archive_required_member_missing")
            manifest_toml = _member_bytes(
                archive, relative["manifest/compatibility.toml"]
            )
            manifest_json = _member_bytes(
                archive, relative["manifest/compatibility.json"]
            )
            json_digest = hashlib.sha256(manifest_json).hexdigest()
            toml_digest = hashlib.sha256(manifest_toml).hexdigest()
            if (
                identity.get("manifest_sha256") != json_digest
                or identity.get("manifest_json_sha256") != json_digest
                or identity.get("manifest_toml_sha256") != toml_digest
            ):
                raise LaneFailure("package_manifest_report_binding_failed")
            try:
                authored = tomllib.loads(manifest_toml.decode("utf-8"))
            except (UnicodeError, ValueError, tomllib.TOMLDecodeError) as error:
                raise LaneFailure("package_manifest_pair_invalid") from error
            mirrored = _strict_json(manifest_json, "package_manifest_pair_invalid")
            canonical = (
                json.dumps(authored, ensure_ascii=False, indent=2, sort_keys=True)
                + "\n"
            ).encode()
            if authored != mirrored or manifest_json != canonical:
                raise LaneFailure("package_manifest_pair_not_canonical")
            metadata = _strict_json(
                _member_bytes(archive, relative["metadata/release.json"]),
                "package_metadata_invalid",
            )
            metadata_manifest = (
                metadata.get("manifest") if isinstance(metadata, dict) else None
            )
            if (
                not isinstance(metadata, dict)
                or metadata.get("target") != "linux-x86_64"
                or metadata.get("version") != identity["version"]
                or not isinstance(metadata_manifest, dict)
                or metadata_manifest.get("schema")
                != "worldstream/storage-compatibility-manifest/v1"
                or metadata_manifest.get("file") != "manifest/compatibility.json"
                or metadata_manifest.get("sha256") != json_digest
            ):
                raise LaneFailure("package_metadata_identity_mismatch")
            checksums = _parse_checksums(
                _member_bytes(archive, relative["checksums.sha256"])
            )
            expected_checksums = set(relative) - {"checksums.sha256"}
            if set(checksums) != expected_checksums:
                raise LaneFailure("package_checksum_inventory_mismatch")
            for path, expected_digest in checksums.items():
                if _hash_member(archive, relative[path]) != expected_digest:
                    raise LaneFailure("package_checksum_mismatch")

            daemon = extraction_root / "bin" / "worldstreamd"
            ctl = extraction_root / "bin" / "worldstreamctl"
            daemon_record = _extract_binary(
                archive, relative["bin/worldstreamd"], daemon
            )
            ctl_record = _extract_binary(archive, relative["bin/worldstreamctl"], ctl)
            ui_record = _extract_tree(
                archive,
                relative,
                prefix="ui",
                output=extraction_root / "ui",
                required={"index.html", "compatibility-identity.json"},
            )
            ui_record["index_sha256"] = SHA256_PREFIX + checksums["ui/index.html"]
            sdk_record = _extract_tree(
                archive,
                relative,
                prefix="sdk/python/src",
                output=extraction_root / "sdk/python/src",
                required={"worldstream_sdk/__init__.py"},
            )
            heist_record = _extract_tree(
                archive,
                relative,
                prefix="examples/heist",
                output=extraction_root / "examples/heist",
                required={
                    "wave10_live/seed_browser_room.py",
                    "wave10_live/run_browser_story.py",
                    "wave10_live/run_absent_broker_live.py",
                    "wave10_live/browser_trace_init.js",
                },
            )
    except (OSError, tarfile.TarError) as error:
        raise LaneFailure("package_archive_unreadable") from error

    binding = {
        "schema": PACKAGE_BINDING_SCHEMA,
        "status": "pass",
        "artifact": archive_path.name,
        "archive_sha256": SHA256_PREFIX + archive_digest,
        "archive_size_bytes": archive_size,
        "package_report_sha256": SHA256_PREFIX
        + hashlib.sha256(package_report_raw).hexdigest(),
        "archive_root": expected_root,
        "identity": {
            "target": "linux-x86_64",
            "version": identity["version"],
            "manifest_sha256": json_digest,
            "manifest_json_sha256": json_digest,
            "manifest_toml_sha256": toml_digest,
        },
        "manifests": {
            "semantic_pair_equal": True,
            "canonical_json": True,
            "exact_archive_bytes_bound": True,
        },
        "archive": {
            "safe_regular_or_directory_members": True,
            "member_count": len(members),
            "checksums_exact": True,
        },
        "binaries": {
            "worldstreamd": daemon_record,
            "worldstreamctl": ctl_record,
        },
        "runtime_assets": {
            "ui": ui_record,
            "sdk_python_source": sdk_record,
            "heist_reference_clients": heist_record,
        },
    }
    return daemon, ctl, binding


def _owner_file(path: pathlib.Path, content: str) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        path.unlink(missing_ok=True)
        raise
    if stat.S_IMODE(path.stat().st_mode) != 0o600:
        raise LaneFailure("owner_only_file_mode_failed")


def _owner_bytes(path: pathlib.Path, content: bytes) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        path.unlink(missing_ok=True)
        raise
    if stat.S_IMODE(path.stat().st_mode) != 0o600:
        raise LaneFailure("owner_only_file_mode_failed")


class PrivacyCapture:
    """Retain bounded child channels until the multi-sentinel scan completes."""

    def __init__(self, root: pathlib.Path) -> None:
        self.root = root / "privacy-channels"
        self.root.mkdir(mode=0o700)
        self.sentinels: dict[str, bytes] = {}
        self.channels: dict[str, pathlib.Path] = {}
        self.channel_classes: dict[str, str] = {}
        self._sequence = 0
        self._sentinel_sequence: dict[str, int] = {}
        self._authority_inodes: set[tuple[int, int]] = set()
        self._live_logs: dict[
            tuple[str, int, int], tuple[Any, Any, pathlib.Path, str]
        ] = {}

    def register_sentinel(self, kind: str, value: bytes) -> None:
        if not isinstance(value, bytes) or not (16 <= len(value) <= 512):
            raise LaneFailure("privacy_sentinel_invalid")
        sequence = self._sentinel_sequence.get(kind, 0) + 1
        self._sentinel_sequence[kind] = sequence
        self.sentinels[f"{kind}-{sequence:02d}"] = value

    def _channel_name(self, suffix: str) -> str:
        self._sequence += 1
        return f"child-{self._sequence:04d}-{suffix}"

    def _register_channel(
        self, name: str, path: pathlib.Path, channel_class: str
    ) -> None:
        if (
            channel_class not in REQUIRED_PRIVACY_CHANNEL_CLASSES
            or name in self.channels
            or name in self.channel_classes
        ):
            raise LaneFailure("privacy_channel_class_invalid")
        self.channels[name] = path
        self.channel_classes[name] = channel_class

    def _record_bytes(self, suffix: str, content: bytes, *, channel_class: str) -> None:
        if not isinstance(content, bytes) or len(content) > MAX_CONTROL_FILE_BYTES:
            raise LaneFailure("privacy_channel_oversized")
        name = self._channel_name(suffix)
        path = self.root / name
        _owner_bytes(path, content)
        self._register_channel(name, path, channel_class)

    def _record_config(
        self,
        *,
        argv: list[str],
        cwd: pathlib.Path,
        environment: dict[str, str] | None,
        input_supplied: bool,
    ) -> None:
        configuration = {
            "argv": argv,
            "cwd": str(cwd),
            "environment_keys": sorted(environment) if environment is not None else [],
            "environment_values_retained": False,
            "stdin_supplied": input_supplied,
            "secret_source_values_retained": False,
        }
        self._record_bytes(
            "config",
            (
                json.dumps(configuration, sort_keys=True, separators=(",", ":")) + "\n"
            ).encode("utf-8"),
            channel_class="process.config",
        )

    def record_process(
        self,
        *,
        argv: list[str],
        cwd: pathlib.Path,
        environment: dict[str, str] | None,
        input_supplied: bool,
        stdout: str,
        stderr: str,
    ) -> None:
        self._record_bytes(
            "stdout", stdout.encode("utf-8"), channel_class="process.stdout"
        )
        self._record_bytes(
            "stderr", stderr.encode("utf-8"), channel_class="process.stderr"
        )
        self._record_config(
            argv=argv,
            cwd=cwd,
            environment=environment,
            input_supplied=input_supplied,
        )

    def record_provider_process(
        self,
        *,
        provider_class: str,
        argv: list[str],
        cwd: pathlib.Path,
        stdout: bytes,
        stderr: bytes,
    ) -> None:
        if provider_class not in {"postgresql", "pgbouncer"}:
            raise LaneFailure("privacy_channel_class_invalid")
        self._record_bytes(
            f"provider-{provider_class}-stdout",
            stdout,
            channel_class=f"provider.{provider_class}.stdout",
        )
        self._record_bytes(
            f"provider-{provider_class}-stderr",
            stderr,
            channel_class=f"provider.{provider_class}.stderr",
        )
        self._record_config(
            argv=argv,
            cwd=cwd,
            environment=None,
            input_supplied=False,
        )

    def add_report(self, label: str, path: pathlib.Path, *, channel_class: str) -> None:
        if path.is_symlink() or not path.is_file():
            raise LaneFailure("privacy_report_channel_invalid")
        name = self._channel_name(label)
        self._register_channel(name, path, channel_class)

    def discover_daemon_channels(
        self, workspace: pathlib.Path, cell_label: str
    ) -> None:
        for secret_path in workspace.rglob("authority.secret"):
            try:
                metadata = secret_path.lstat()
            except OSError:
                continue
            key = (metadata.st_dev, metadata.st_ino)
            if (
                key in self._authority_inodes
                or not stat.S_ISREG(metadata.st_mode)
                or stat.S_ISLNK(metadata.st_mode)
                or metadata.st_size != 32
            ):
                continue
            try:
                value = _stable_regular_bytes(
                    secret_path,
                    "privacy_secret_channel_invalid",
                    maximum=32,
                )
            except LaneFailure:
                continue
            if len(value) != 32:
                continue
            self._authority_inodes.add(key)
            self.register_sentinel("authority-secret", value)
            self.register_sentinel(
                "operator-capability", b"wsb1:" + value.hex().encode()
            )
        for log_path in workspace.rglob("worldstreamd.log"):
            try:
                metadata = log_path.lstat()
            except OSError:
                continue
            key = (cell_label, metadata.st_dev, metadata.st_ino)
            if (
                key in self._live_logs
                or not stat.S_ISREG(metadata.st_mode)
                or stat.S_ISLNK(metadata.st_mode)
            ):
                continue
            source = log_path.open("rb")
            name = self._channel_name(f"{cell_label}-daemon-log")
            destination_path = self.root / name
            descriptor = os.open(
                destination_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600
            )
            destination = os.fdopen(descriptor, "wb")
            self._live_logs[key] = (source, destination, destination_path, name)
        self.drain_daemon_logs()

    def drain_daemon_logs(self) -> None:
        for source, destination, _path, _name in self._live_logs.values():
            while chunk := source.read(1024 * 1024):
                destination.write(chunk)

    def finish_daemon_logs(self) -> None:
        self.drain_daemon_logs()
        for source, destination, path, name in self._live_logs.values():
            source.close()
            destination.flush()
            os.fsync(destination.fileno())
            destination.close()
            self._register_channel(name, path, "daemon.log")
        self._live_logs.clear()

    def scan(self) -> dict[str, Any]:
        required_kinds = (
            "authority-secret-",
            "operator-capability-",
            "postgres-admin-password-",
            "postgres-runtime-password-",
            "postgres-admin-dsn-",
            "postgres-runtime-dsn-",
        )
        if any(
            not any(name.startswith(prefix) for name in self.sentinels)
            for prefix in required_kinds
        ):
            raise LaneFailure("privacy_sentinel_class_incomplete")
        if set(self.channels) != set(self.channel_classes):
            raise LaneFailure("privacy_channel_class_invalid")
        classes = {
            channel_class: sorted(
                name
                for name, observed_class in self.channel_classes.items()
                if observed_class == channel_class
            )
            for channel_class in REQUIRED_PRIVACY_CHANNEL_CLASSES
        }
        if any(not names for names in classes.values()):
            raise LaneFailure("privacy_channel_class_incomplete")
        try:
            result = SECRET_SCAN.scan_sentinels(self.sentinels, self.channels)
        except SECRET_SCAN.ScanError as error:
            raise LaneFailure("privacy_secret_absence_scan_failed") from error
        result["channel_class_inventory"] = {
            "schema": PRIVACY_CHANNEL_CLASS_SCHEMA,
            "status": "complete",
            "required": list(REQUIRED_PRIVACY_CHANNEL_CLASSES),
            "classes": [
                {
                    "class": channel_class,
                    "channel_count": len(classes[channel_class]),
                    "channels": classes[channel_class],
                }
                for channel_class in REQUIRED_PRIVACY_CHANNEL_CLASSES
            ],
        }
        return result


ACTIVE_PRIVACY_CAPTURE: PrivacyCapture | None = None


def _safe_run(
    argv: list[str],
    *,
    cwd: pathlib.Path,
    environment: dict[str, str] | None = None,
    input_text: str | None = None,
    timeout: float = 120.0,
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        argv,
        cwd=cwd,
        env=environment,
        input=input_text,
        text=True,
        capture_output=True,
        check=False,
        timeout=timeout,
    )
    if ACTIVE_PRIVACY_CAPTURE is not None:
        ACTIVE_PRIVACY_CAPTURE.record_process(
            argv=argv,
            cwd=cwd,
            environment=environment,
            input_supplied=input_text is not None,
            stdout=completed.stdout,
            stderr=completed.stderr,
        )
    return completed


@dataclass(frozen=True)
class BoundedProcessOutput:
    """Complete bounded byte output from one provider capture command."""

    returncode: int
    stdout: bytes
    stderr: bytes


def _kill_process(process: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except (OSError, ProcessLookupError):
        try:
            process.kill()
        except OSError:
            pass


def _bounded_provider_capture(
    argv: list[str],
    *,
    cwd: pathlib.Path,
    maximum_bytes: int | None = None,
    timeout_seconds: float | None = None,
) -> BoundedProcessOutput:
    """Capture complete provider streams without unbounded memory or disk use."""

    maximum = MAX_PROVIDER_CHANNEL_BYTES if maximum_bytes is None else maximum_bytes
    timeout = (
        PROVIDER_CAPTURE_TIMEOUT_SECONDS if timeout_seconds is None else timeout_seconds
    )

    try:
        process = subprocess.Popen(
            argv,
            cwd=cwd,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
    except OSError as error:
        raise LaneFailure("provider_capture_unavailable") from error
    if process.stdout is None or process.stderr is None:
        _kill_process(process)
        raise LaneFailure("provider_capture_unavailable")

    stream_states: dict[str, dict[str, Any]] = {
        "stdout": {
            "content": bytearray(),
            "complete": False,
            "error": False,
            "size": 0,
        },
        "stderr": {
            "content": bytearray(),
            "complete": False,
            "error": False,
            "size": 0,
        },
    }

    def drain(label: str, source: Any) -> None:
        state = stream_states[label]
        try:
            while chunk := source.read(64 * 1024):
                state["size"] += len(chunk)
                remaining = maximum - len(state["content"])
                if remaining > 0:
                    state["content"].extend(chunk[:remaining])
            state["complete"] = True
        except OSError:
            state["error"] = True
        finally:
            try:
                source.close()
            except OSError:
                state["error"] = True

    threads = [
        threading.Thread(
            target=drain,
            args=("stdout", process.stdout),
            name="worldstream-provider-stdout-capture",
            daemon=True,
        ),
        threading.Thread(
            target=drain,
            args=("stderr", process.stderr),
            name="worldstream-provider-stderr-capture",
            daemon=True,
        ),
    ]
    for thread in threads:
        thread.start()

    timed_out = False
    try:
        returncode = process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        _kill_process(process)
        try:
            returncode = process.wait(timeout=PROVIDER_CAPTURE_DRAIN_GRACE_SECONDS)
        except subprocess.TimeoutExpired:
            returncode = -int(signal.SIGKILL)
    for thread in threads:
        thread.join(timeout=PROVIDER_CAPTURE_DRAIN_GRACE_SECONDS)

    if any(state["size"] > maximum for state in stream_states.values()):
        raise LaneFailure("provider_capture_oversized")
    if (
        timed_out
        or any(thread.is_alive() for thread in threads)
        or any(
            state["error"] or not state["complete"] for state in stream_states.values()
        )
    ):
        raise LaneFailure("provider_capture_truncated")
    return BoundedProcessOutput(
        returncode=returncode,
        stdout=bytes(stream_states["stdout"]["content"]),
        stderr=bytes(stream_states["stderr"]["content"]),
    )


def _mount_identity(
    path: pathlib.Path,
    mountinfo_path: pathlib.Path = pathlib.Path("/proc/self/mountinfo"),
) -> dict[str, Any]:
    target = path.resolve()
    try:
        with mountinfo_path.open("rb") as source:
            raw = source.read(MAX_MOUNTINFO_BYTES + 1)
    except OSError as error:
        raise REFERENCE_HOST.ReferenceHostError(
            "Linux mount identity was unavailable"
        ) from error
    if not raw or len(raw) > MAX_MOUNTINFO_BYTES or b"\0" in raw:
        raise REFERENCE_HOST.ReferenceHostError("Linux mount identity was invalid")
    try:
        lines = raw.decode("utf-8").splitlines()
    except UnicodeError as error:
        raise REFERENCE_HOST.ReferenceHostError(
            "Linux mount identity was not UTF-8"
        ) from error
    selected: tuple[int, int, str] | None = None
    for line in lines:
        before, separator, after = line.partition(" - ")
        left = before.split()
        right = after.split()
        if not separator or len(left) < 6 or len(right) < 3:
            raise REFERENCE_HOST.ReferenceHostError("Linux mountinfo row was malformed")
        mount_point = pathlib.Path(REFERENCE_HOST.unescape_mount_path(left[4]))
        try:
            target.relative_to(mount_point)
        except ValueError:
            continue
        try:
            mount_id = int(left[0])
        except ValueError as error:
            raise REFERENCE_HOST.ReferenceHostError(
                "Linux mount identity was invalid"
            ) from error
        if (
            mount_id <= 0
            or re.fullmatch(r"[0-9]+:[0-9]+", left[2]) is None
            or not mount_point.is_absolute()
        ):
            raise REFERENCE_HOST.ReferenceHostError("Linux mount identity was invalid")
        candidate = (len(mount_point.parts), mount_id, left[2])
        if selected is None or candidate[0] > selected[0]:
            selected = candidate
    if selected is None:
        raise REFERENCE_HOST.ReferenceHostError("Linux mount identity was unavailable")
    return {"mount_id": selected[1], "device_id": selected[2]}


def _require_frozen_ext4(host: dict[str, Any], *, database: bool) -> None:
    filesystem = host.get("filesystem")
    valid = (
        isinstance(filesystem, dict)
        and set(filesystem) == {"type", "mount_options", "storage_class"}
        and filesystem.get("type") == "ext4"
        and filesystem.get("storage_class") == "local_ssd_or_nvme"
        and isinstance(filesystem.get("mount_options"), list)
        and bool(filesystem["mount_options"])
        and all(
            isinstance(option, str) and bool(option.strip())
            for option in filesystem["mount_options"]
        )
    )
    if not valid:
        raise LaneFailure(
            "reference_database_storage_not_frozen_local_ext4"
            if database
            else "reference_workload_storage_not_frozen_local_ext4"
        )


def _reference_environment(
    workload_storage: pathlib.Path,
    database_storage: pathlib.Path,
    provider_storage: dict[str, Any],
) -> dict[str, Any]:
    try:
        workload_host = REFERENCE_HOST.observe(workload_storage)
    except REFERENCE_HOST.ReferenceHostError as error:
        raise LaneFailure(
            "reference_workload_storage_observation_unavailable"
        ) from error
    try:
        database_host = REFERENCE_HOST.observe(database_storage)
    except REFERENCE_HOST.ReferenceHostError as error:
        raise LaneFailure(
            "reference_database_storage_observation_unavailable"
        ) from error
    _require_frozen_ext4(workload_host, database=False)
    _require_frozen_ext4(database_host, database=True)
    if workload_host.get("platform") != database_host.get(
        "platform"
    ) or workload_host.get("hardware") != database_host.get("hardware"):
        raise LaneFailure("reference_storage_host_identity_mismatch")
    try:
        workload_mount = _mount_identity(workload_storage)
    except REFERENCE_HOST.ReferenceHostError as error:
        raise LaneFailure("reference_workload_mount_identity_unavailable") from error
    try:
        database_mount = _mount_identity(database_storage)
    except REFERENCE_HOST.ReferenceHostError as error:
        raise LaneFailure("reference_database_mount_identity_unavailable") from error
    return {
        "platform": workload_host["platform"],
        "hardware": workload_host["hardware"],
        # Retained as the workload-path summary for existing consumers. The
        # exact provider database observation is independently disclosed below.
        "filesystem": workload_host["filesystem"],
        "storage_bindings": {
            "schema": STORAGE_BINDING_SCHEMA,
            "status": "verified",
            "profile": FROZEN_STORAGE_PROFILE,
            "reference_environment_filesystem_role": "workload_owned_runtime_parent",
            "layout": (
                "same_host_mount"
                if workload_mount == database_mount
                else "split_host_mounts"
            ),
            "raw_paths_retained": False,
            "workload": {
                "role": "workload_owned_runtime_parent",
                "filesystem": workload_host["filesystem"],
                "mount_identity": workload_mount,
            },
            "postgresql_database": {
                "role": "provider_owned_database_volume",
                "filesystem": database_host["filesystem"],
                "mount_identity": database_mount,
                "docker_volume": provider_storage,
            },
        },
        "engines": {
            "postgresql": None,
            "sqlite": None,
        },
    }


def _bounded_docker_text(argv: list[str], *, cwd: pathlib.Path, code: str) -> str:
    completed = _bounded_provider_capture(
        argv,
        cwd=cwd,
        maximum_bytes=MAX_DOCKER_CONTROL_BYTES,
        timeout_seconds=DOCKER_CONTROL_TIMEOUT_SECONDS,
    )
    try:
        stdout = completed.stdout.decode("utf-8")
        stderr = completed.stderr.decode("utf-8")
    except UnicodeError as error:
        raise LaneFailure(code) from error
    if ACTIVE_PRIVACY_CAPTURE is not None:
        ACTIVE_PRIVACY_CAPTURE.record_process(
            argv=argv,
            cwd=cwd,
            environment=None,
            input_supplied=False,
            stdout=stdout,
            stderr=stderr,
        )
    if completed.returncode != 0:
        raise LaneFailure(code)
    return stdout


def _bounded_docker_object(
    argv: list[str], *, cwd: pathlib.Path, code: str
) -> dict[str, Any]:
    return _strict_json(_bounded_docker_text(argv, cwd=cwd, code=code).encode(), code)


def _owned_local_volume_mountpoint(
    volume_record: dict[str, Any],
    expected_volume: str,
    expected_owner: str,
) -> pathlib.Path:
    labels = volume_record.get("Labels")
    options = volume_record.get("Options")
    mountpoint = volume_record.get("Mountpoint")
    if (
        volume_record.get("Name") != expected_volume
        or volume_record.get("Driver") != "local"
        or volume_record.get("Scope") != "local"
        or labels != {POSTGRES_VOLUME_OWNERSHIP_LABEL: expected_owner}
        or options not in (None, {})
        or not isinstance(mountpoint, str)
        or not (0 < len(mountpoint) <= 4096)
        or any(ord(character) < 32 for character in mountpoint)
    ):
        raise LaneFailure("postgres_database_volume_not_owned_local")
    path = pathlib.Path(mountpoint)
    if not path.is_absolute() or ".." in path.parts:
        raise LaneFailure("postgres_database_volume_not_owned_local")
    return path


def _provider_database_storage_binding(
    volume_record: dict[str, Any],
    container_record: dict[str, Any],
    expected_volume: str,
    expected_owner: str,
) -> tuple[pathlib.Path, dict[str, Any]]:
    path = _owned_local_volume_mountpoint(
        volume_record, expected_volume, expected_owner
    )
    mountpoint = str(path)
    mounts = container_record.get("Mounts")
    if not isinstance(mounts, list) or not all(
        isinstance(mount, dict) for mount in mounts
    ):
        raise LaneFailure("postgres_database_volume_mount_unverified")
    matching_destination = [
        mount
        for mount in mounts
        if mount.get("Destination") == POSTGRES_DATA_DESTINATION
    ]
    matching_volume = [
        mount for mount in mounts if mount.get("Name") == expected_volume
    ]
    if (
        len(matching_destination) != 1
        or len(matching_volume) != 1
        or matching_destination[0] is not matching_volume[0]
    ):
        raise LaneFailure("postgres_database_volume_mount_unverified")
    mount = matching_destination[0]
    if (
        mount.get("Type") != "volume"
        or mount.get("Driver") != "local"
        or mount.get("Source") != mountpoint
        or mount.get("RW") is not True
    ):
        raise LaneFailure("postgres_database_volume_mount_unverified")
    return path, {
        "driver": "local",
        "scope": "local",
        "driver_options": {},
        "container_destination": POSTGRES_DATA_DESTINATION,
        "container_mount_type": "volume",
        "read_write": True,
        "source_matches_volume_mountpoint": True,
        "run_unique_ownership_label_verified": True,
    }


def _process_tree_rss_kib(root_pid: int) -> int:
    completed = subprocess.run(
        ["ps", "-axo", "pid=,ppid=,rss="],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if completed.returncode != 0:
        return 0
    rows: list[tuple[int, int, int]] = []
    for line in completed.stdout.splitlines():
        fields = line.split()
        if len(fields) != 3 or any(not field.isdigit() for field in fields):
            continue
        rows.append(tuple(map(int, fields)))
    descendants = {root_pid}
    changed = True
    while changed:
        changed = False
        for pid, parent, _rss in rows:
            if parent in descendants and pid not in descendants:
                descendants.add(pid)
                changed = True
    return sum(rss for pid, _parent, rss in rows if pid in descendants)


def _nearest_rank(samples: list[float], percentile: float) -> float | None:
    if not samples:
        return None
    ordered = sorted(samples)
    index = max(0, math.ceil(len(ordered) * percentile) - 1)
    return ordered[min(index, len(ordered) - 1)]


def _latency_triplet(report: dict[str, Any]) -> dict[str, Any]:
    measurements = report.get("measurements")
    samples = measurements.get("latency_ms") if isinstance(measurements, dict) else None
    if (
        not isinstance(samples, list)
        or not samples
        or any(
            not isinstance(value, (int, float)) or isinstance(value, bool) or value < 0
            for value in samples
        )
    ):
        return {"status": "unavailable", "sample_count": 0}
    numeric = [float(value) for value in samples]
    return {
        "status": "measured",
        "definition": "nearest-rank",
        "sample_count": len(numeric),
        "samples_ms": numeric,
        "p50_ms": _nearest_rank(numeric, 0.50),
        "p95_ms": _nearest_rank(numeric, 0.95),
        "p99_ms": _nearest_rank(numeric, 0.99),
    }


def _measured_process(
    argv: list[str],
    *,
    cwd: pathlib.Path,
    environment: dict[str, str],
    timeout: float,
    privacy_workspace: pathlib.Path | None = None,
    privacy_label: str | None = None,
) -> dict[str, Any]:
    started = time.monotonic()
    process = subprocess.Popen(
        argv,
        cwd=cwd,
        env=environment,
        text=True,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    rss_samples: list[int] = []
    timed_out = False
    while process.poll() is None:
        if (
            ACTIVE_PRIVACY_CAPTURE is not None
            and privacy_workspace is not None
            and privacy_label is not None
        ):
            ACTIVE_PRIVACY_CAPTURE.discover_daemon_channels(
                privacy_workspace, privacy_label
            )
        rss_samples.append(_process_tree_rss_kib(process.pid))
        if time.monotonic() - started >= timeout:
            timed_out = True
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            break
        time.sleep(0.25)
    stdout, stderr = process.communicate()
    if ACTIVE_PRIVACY_CAPTURE is not None:
        if privacy_workspace is not None and privacy_label is not None:
            ACTIVE_PRIVACY_CAPTURE.discover_daemon_channels(
                privacy_workspace, privacy_label
            )
            ACTIVE_PRIVACY_CAPTURE.drain_daemon_logs()
        ACTIVE_PRIVACY_CAPTURE.record_process(
            argv=argv,
            cwd=cwd,
            environment=environment,
            input_supplied=False,
            stdout=stdout,
            stderr=stderr,
        )
    elapsed_ms = round((time.monotonic() - started) * 1000)
    return {
        "exit_code": 124 if timed_out else process.returncode,
        "elapsed_ms": elapsed_ms,
        "process_tree_rss_kib": {
            "sampling_interval_ms": 250,
            "sample_count": len(rss_samples),
            "samples": rss_samples,
            "p50": _nearest_rank([float(value) for value in rss_samples], 0.50),
            "p95": _nearest_rank([float(value) for value in rss_samples], 0.95),
            "p99": _nearest_rank([float(value) for value in rss_samples], 0.99),
            "peak": max(rss_samples, default=0),
        },
    }


def _docker_port(docker: str, name: str) -> int | None:
    completed = _safe_run(
        [docker, "port", name, "5432/tcp"], cwd=pathlib.Path.cwd(), timeout=10
    )
    if completed.returncode != 0:
        return None
    last = completed.stdout.strip().rsplit(":", maxsplit=1)[-1]
    return int(last) if last.isdigit() else None


@dataclass
class Provider:
    root: pathlib.Path
    repository: pathlib.Path
    docker: str
    psql: str
    worldstreamctl: pathlib.Path
    admin_password: str
    runtime_password: str
    network: str
    postgres_name: str
    pooler_name: str
    postgres_volume: str
    volume_ownership_id: str
    postgres_port: int | None = None
    pooler_port: int | None = None
    network_started: bool = False
    postgres_started: bool = False
    pooler_started: bool = False
    volume_started: bool = False
    volume_creation_attempted: bool = False
    _removed_containers: set[str] = field(default_factory=set, init=False, repr=False)

    @classmethod
    def create(
        cls,
        root: pathlib.Path,
        repository: pathlib.Path,
        docker: str,
        psql: str,
        worldstreamctl: pathlib.Path,
    ) -> Provider:
        suffix = f"{os.getpid()}-{secrets.token_hex(4)}"
        provider = cls(
            root=root,
            repository=repository,
            docker=docker,
            psql=psql,
            worldstreamctl=worldstreamctl,
            admin_password=secrets.token_hex(24),
            runtime_password=secrets.token_hex(24),
            network=f"worldstream-packaged-parity-{suffix}",
            postgres_name=f"worldstream-packaged-parity-db-{suffix}",
            pooler_name=f"worldstream-packaged-parity-pooler-{suffix}",
            postgres_volume=f"worldstream-packaged-parity-data-{suffix}",
            volume_ownership_id=secrets.token_hex(16),
        )
        if ACTIVE_PRIVACY_CAPTURE is not None:
            ACTIVE_PRIVACY_CAPTURE.register_sentinel(
                "postgres-admin-password", provider.admin_password.encode("ascii")
            )
            ACTIVE_PRIVACY_CAPTURE.register_sentinel(
                "postgres-runtime-password", provider.runtime_password.encode("ascii")
            )
        return provider

    def _docker(self, *arguments: str, timeout: float = 180.0) -> None:
        completed = _safe_run(
            [self.docker, *arguments], cwd=self.repository, timeout=timeout
        )
        if completed.returncode != 0:
            raise LaneFailure("docker_operation_failed")

    def _psql(
        self,
        database: str,
        sql: str,
        *,
        user: str = "admin",
        password: str | None = None,
        port: int | None = None,
    ) -> str:
        target_port = port or self.postgres_port
        if target_port is None:
            raise LaneFailure("postgres_port_unavailable")
        environment = {
            **os.environ,
            "PGCONNECT_TIMEOUT": "5",
            "PGPASSWORD": password or self.admin_password,
        }
        dsn = f"host=127.0.0.1 port={target_port} dbname={database} user={user}"
        completed = _safe_run(
            [
                self.psql,
                dsn,
                "--no-psqlrc",
                "--quiet",
                "--no-align",
                "--tuples-only",
                "--no-password",
                "--set=ON_ERROR_STOP=1",
            ],
            cwd=self.repository,
            environment=environment,
            input_text=sql + "\n",
            timeout=60,
        )
        if completed.returncode != 0:
            raise LaneFailure("postgres_sql_failed")
        return completed.stdout.strip()

    def start(self) -> None:
        self._docker("pull", POSTGRES_IMAGE, timeout=300)
        postgres_digest = _safe_run(
            [
                self.docker,
                "image",
                "inspect",
                POSTGRES_IMAGE,
                "--format",
                "{{index .RepoDigests 0}}",
            ],
            cwd=self.repository,
        ).stdout.strip()
        if postgres_digest != POSTGRES_DIGEST:
            raise LaneFailure("postgres_image_digest_mismatch")
        self._docker("pull", PGBOUNCER_IMAGE, timeout=300)
        pooler_digest = _safe_run(
            [
                self.docker,
                "image",
                "inspect",
                PGBOUNCER_IMAGE,
                "--format",
                "{{index .RepoDigests 0}}",
            ],
            cwd=self.repository,
        ).stdout.strip()
        if pooler_digest != PGBOUNCER_DIGEST:
            raise LaneFailure("pgbouncer_image_digest_mismatch")

        password_file = self.root / "postgres-admin-password"
        _owner_file(password_file, self.admin_password + "\n")
        self._create_database_volume()
        self._docker("network", "create", self.network)
        self.network_started = True
        self._docker(
            "run",
            "--detach",
            "--network",
            self.network,
            "--name",
            self.postgres_name,
            "--network-alias",
            "postgres",
            "--env",
            "POSTGRES_USER=admin",
            "--env",
            "POSTGRES_PASSWORD_FILE=/run/secrets/postgres-password",
            "--env",
            "POSTGRES_DB=postgres",
            "--volume",
            f"{password_file}:/run/secrets/postgres-password:ro",
            "--mount",
            (
                f"type=volume,source={self.postgres_volume},"
                f"destination={POSTGRES_DATA_DESTINATION}"
            ),
            "--publish",
            "127.0.0.1::5432",
            POSTGRES_IMAGE,
        )
        self.postgres_started = True
        for _ in range(120):
            self.postgres_port = _docker_port(self.docker, self.postgres_name)
            if self.postgres_port is not None:
                try:
                    if self._psql("postgres", "SELECT 1") == "1":
                        break
                except LaneFailure:
                    pass
            time.sleep(0.5)
        else:
            raise LaneFailure("postgres_17_11_not_ready")
        if self._psql("postgres", "SHOW server_version_num") != "170011":
            raise LaneFailure("postgres_server_version_mismatch")

        self._psql(
            "postgres",
            "CREATE ROLE runtime LOGIN PASSWORD "
            f"'{self.runtime_password}' NOSUPERUSER NOCREATEDB NOCREATEROLE "
            "NOINHERIT NOREPLICATION NOBYPASSRLS;",
        )
        for database in DATABASES.values():
            self._psql("postgres", f"CREATE DATABASE {database} OWNER admin;")
            self._prepare_database(database)
        self._start_pooler()

    def _create_database_volume(self) -> None:
        existing = _bounded_docker_text(
            [
                self.docker,
                "volume",
                "ls",
                "--quiet",
                "--filter",
                f"name=^{self.postgres_volume}$",
            ],
            cwd=self.repository,
            code="postgres_database_volume_absence_unverified",
        )
        if existing.strip():
            raise LaneFailure("postgres_database_volume_name_collision")
        self.volume_creation_attempted = True
        self._docker(
            "volume",
            "create",
            "--driver",
            "local",
            "--label",
            f"{POSTGRES_VOLUME_OWNERSHIP_LABEL}={self.volume_ownership_id}",
            self.postgres_volume,
        )
        volume_record = _bounded_docker_object(
            [
                self.docker,
                "volume",
                "inspect",
                self.postgres_volume,
                "--format",
                "{{json .}}",
            ],
            cwd=self.repository,
            code="postgres_database_volume_inspect_failed",
        )
        # Docker volume create is idempotent. Cleanup ownership begins only
        # after post-create metadata proves this invocation's unguessable label.
        if volume_record.get("Name") != self.postgres_volume or volume_record.get(
            "Labels"
        ) != {POSTGRES_VOLUME_OWNERSHIP_LABEL: self.volume_ownership_id}:
            raise LaneFailure("postgres_database_volume_not_owned_local")
        _owned_local_volume_mountpoint(
            volume_record, self.postgres_volume, self.volume_ownership_id
        )
        self.volume_started = True

    def database_storage_binding(self) -> tuple[pathlib.Path, dict[str, Any]]:
        if not self.postgres_started or not self.volume_started:
            raise LaneFailure("postgres_database_volume_mount_unverified")
        volume_record = _bounded_docker_object(
            [
                self.docker,
                "volume",
                "inspect",
                self.postgres_volume,
                "--format",
                "{{json .}}",
            ],
            cwd=self.repository,
            code="postgres_database_volume_inspect_failed",
        )
        container_record = _bounded_docker_object(
            [
                self.docker,
                "inspect",
                self.postgres_name,
                "--format",
                "{{json .}}",
            ],
            cwd=self.repository,
            code="postgres_database_container_inspect_failed",
        )
        return _provider_database_storage_binding(
            volume_record,
            container_record,
            self.postgres_volume,
            self.volume_ownership_id,
        )

    def _prepare_database(self, database: str) -> None:
        self._psql(
            database,
            "REVOKE CREATE ON SCHEMA public FROM PUBLIC;"
            "GRANT CONNECT ON DATABASE "
            f"{database} TO runtime;"
            "GRANT USAGE ON SCHEMA public TO runtime;"
            "ALTER DEFAULT PRIVILEGES FOR ROLE admin IN SCHEMA public "
            "GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO runtime;"
            "ALTER DEFAULT PRIVILEGES FOR ROLE admin IN SCHEMA public "
            "GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO runtime;",
        )
        admin_dsn_file = self.root / f"{database}-admin.dsn"
        admin_dsn = (
            "host=127.0.0.1 "
            f"port={self.postgres_port} dbname={database} user=admin "
            f"password={self.admin_password}"
        )
        _owner_file(admin_dsn_file, admin_dsn + "\n")
        if ACTIVE_PRIVACY_CAPTURE is not None:
            ACTIVE_PRIVACY_CAPTURE.register_sentinel(
                "postgres-admin-dsn", admin_dsn.encode("ascii")
            )
        completed = _safe_run(
            [
                str(self.worldstreamctl),
                "postgres",
                "migrate",
                "--dsn-file",
                str(admin_dsn_file),
            ],
            cwd=self.repository,
            timeout=180,
        )
        if completed.returncode != 0:
            raise LaneFailure("postgres_admin_migration_failed")
        completed = _safe_run(
            [
                str(self.worldstreamctl),
                "postgres",
                "verify",
                "--dsn-file",
                str(admin_dsn_file),
            ],
            cwd=self.repository,
            timeout=180,
        )
        if completed.returncode != 0:
            raise LaneFailure("postgres_admin_verification_failed")
        self._psql(
            database,
            "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public "
            "TO runtime;"
            "GRANT USAGE, SELECT, UPDATE ON ALL SEQUENCES IN SCHEMA public "
            "TO runtime;"
            "REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON TABLE "
            "public.worldstream_schema_migrations,"
            "public.worldstream_transfer_imports,"
            "public.worldstream_transfer_chunks,"
            "public.worldstream_transfer_target_fence,"
            "public.worldstream_transfer_stream_imports_v2,"
            "public.worldstream_transfer_stream_chunks_v2,"
            "public.worldstream_transfer_stream_records_v2 FROM runtime;"
            "REVOKE DELETE ON TABLE public.worldstream_frames FROM runtime;",
        )
        _validate_runtime_role_admission(
            self._psql(
                database,
                RUNTIME_ROLE_ADMISSION_SQL,
                user="runtime",
                password=self.runtime_password,
            )
        )

    def _start_pooler(self) -> None:
        auth_file = self.root / "pgbouncer-userlist.txt"
        config_file = self.root / "pgbouncer.ini"
        _owner_file(
            auth_file,
            f'"admin" "{self.admin_password}"\n"runtime" "{self.runtime_password}"\n',
        )
        _owner_file(
            config_file,
            "[databases]\n"
            "* = host=postgres port=5432\n"
            "[pgbouncer]\n"
            "listen_addr = 0.0.0.0\n"
            "listen_port = 5432\n"
            "auth_type = plain\n"
            "auth_file = /etc/pgbouncer/userlist.txt\n"
            "pool_mode = transaction\n"
            "admin_users = admin\n"
            "max_client_conn = 200\n"
            "default_pool_size = 40\n"
            "ignore_startup_parameters = extra_float_digits\n"
            "pidfile = /tmp/pgbouncer.pid\n",
        )
        self._docker(
            "run",
            "--detach",
            "--network",
            self.network,
            "--name",
            self.pooler_name,
            "--publish",
            "127.0.0.1::5432",
            "--user",
            f"{os.getuid()}:{os.getgid()}",
            "--entrypoint",
            "/usr/bin/pgbouncer",
            "--volume",
            f"{auth_file}:/etc/pgbouncer/userlist.txt:ro",
            "--volume",
            f"{config_file}:/etc/pgbouncer/pgbouncer.ini:ro",
            PGBOUNCER_IMAGE,
            "/etc/pgbouncer/pgbouncer.ini",
        )
        self.pooler_started = True
        for _ in range(120):
            self.pooler_port = _docker_port(self.docker, self.pooler_name)
            if self.pooler_port is not None:
                try:
                    if (
                        self._psql(
                            "counter_pooler",
                            "SELECT 1",
                            user="runtime",
                            password=self.runtime_password,
                            port=self.pooler_port,
                        )
                        == "1"
                    ):
                        return
                except LaneFailure:
                    pass
            time.sleep(0.5)
        raise LaneFailure("transaction_pooler_not_ready")

    def runtime_dsn_file(self, database: str, *, pooler: bool) -> pathlib.Path:
        port = self.pooler_port if pooler else self.postgres_port
        if port is None:
            raise LaneFailure("runtime_dsn_port_unavailable")
        path = self.root / f"{database}-{'pooler' if pooler else 'direct'}.dsn"
        runtime_dsn = (
            "host=127.0.0.1 "
            f"port={port} dbname={database} user=runtime "
            f"password={self.runtime_password} connect_timeout=5"
        )
        _owner_file(path, runtime_dsn + "\n")
        if ACTIVE_PRIVACY_CAPTURE is not None:
            ACTIVE_PRIVACY_CAPTURE.register_sentinel(
                "postgres-runtime-dsn", runtime_dsn.encode("ascii")
            )
        return path

    def measurement(self, database: str) -> dict[str, int]:
        value = self._psql(
            database,
            "SELECT pg_database_size(current_database()),"
            "(SELECT COALESCE(sum(size), 0) FROM pg_ls_waldir()),"
            "(SELECT count(*) FROM worldstream_room_roots),"
            "(SELECT count(*) FROM worldstream_transitions),"
            "(SELECT count(*) FROM worldstream_frames),"
            "(SELECT count(*) FROM worldstream_activation_operation_receipts);",
        )
        fields = value.split("|")
        if len(fields) != 6:
            raise LaneFailure("postgres_measurement_invalid")
        try:
            numbers = [int(field) for field in fields]
        except ValueError as error:
            raise LaneFailure("postgres_measurement_invalid") from error
        return dict(
            zip(
                (
                    "database_bytes",
                    "cluster_wal_bytes",
                    "active_rooms",
                    "transitions",
                    "observation_frames",
                    "activation_receipts",
                ),
                numbers,
                strict=True,
            )
        )

    def reference_engine(self) -> dict[str, Any]:
        value = self._psql(
            "postgres",
            "SELECT current_setting('server_version_num'),"
            "current_setting('synchronous_commit'),"
            "current_setting('transaction_isolation');",
        )
        fields = value.split("|")
        if len(fields) != 3 or fields[0] != "170011":
            raise LaneFailure("postgres_reference_settings_invalid")
        return {
            "version": "17.11",
            "settings": {
                "server_version_num": fields[0],
                "synchronous_commit": fields[1],
                "transaction_isolation": fields[2],
                **POSTGRESQL_CONTRACT_SETTINGS,
            },
            "connection_mode": "direct_and_transaction_pooler",
        }

    def _capture_container_output(self, name: str, provider_class: str) -> None:
        if name in self._removed_containers:
            raise LaneFailure("provider_capture_after_removal")
        capture = ACTIVE_PRIVACY_CAPTURE
        if capture is None:
            raise LaneFailure("provider_capture_unavailable")
        argv = [self.docker, "logs", "--timestamps", name]
        completed = _bounded_provider_capture(argv, cwd=self.repository)
        capture.record_provider_process(
            provider_class=provider_class,
            argv=argv,
            cwd=self.repository,
            stdout=completed.stdout,
            stderr=completed.stderr,
        )
        if completed.returncode != 0:
            raise LaneFailure("provider_capture_unavailable")

    def _remove_container(self, name: str) -> bool:
        if name in self._removed_containers:
            return False
        # Mark the destructive boundary before invoking Docker. Even if the
        # command result cannot be retained, no later capture may race behind
        # an rm attempt that might already have removed the provider.
        self._removed_containers.add(name)
        try:
            completed = _safe_run(
                [self.docker, "rm", "-f", name],
                cwd=self.repository,
                timeout=30,
            )
        except (LaneFailure, OSError, subprocess.SubprocessError):
            return False
        return completed.returncode == 0

    def cleanup(self) -> str:
        failed = False
        containers = []
        if self.pooler_started:
            containers.append((self.pooler_name, "pgbouncer"))
        if self.postgres_started:
            containers.append((self.postgres_name, "postgresql"))
        for name, provider_class in containers:
            try:
                self._capture_container_output(name, provider_class)
            except (LaneFailure, OSError, subprocess.SubprocessError):
                failed = True
        for name, _provider_class in containers:
            failed = not self._remove_container(name) or failed
        self.pooler_started = False
        self.postgres_started = False
        if self.volume_started or self.volume_creation_attempted:
            try:
                existing = _bounded_docker_text(
                    [
                        self.docker,
                        "volume",
                        "ls",
                        "--quiet",
                        "--filter",
                        f"name=^{self.postgres_volume}$",
                    ],
                    cwd=self.repository,
                    code="postgres_database_volume_cleanup_inspect_failed",
                )
                candidates = existing.splitlines()
                if not candidates:
                    failed = self.volume_started or failed
                elif candidates == [self.postgres_volume]:
                    volume_record = _bounded_docker_object(
                        [
                            self.docker,
                            "volume",
                            "inspect",
                            self.postgres_volume,
                            "--format",
                            "{{json .}}",
                        ],
                        cwd=self.repository,
                        code="postgres_database_volume_cleanup_inspect_failed",
                    )
                    _owned_local_volume_mountpoint(
                        volume_record,
                        self.postgres_volume,
                        self.volume_ownership_id,
                    )
                    completed = _safe_run(
                        [self.docker, "volume", "rm", self.postgres_volume],
                        cwd=self.repository,
                        timeout=30,
                    )
                    failed = completed.returncode != 0 or failed
                else:
                    failed = True
            except (LaneFailure, OSError, subprocess.SubprocessError):
                failed = True
            self.volume_started = False
            self.volume_creation_attempted = False
        if self.network_started:
            try:
                completed = _safe_run(
                    [self.docker, "network", "rm", self.network],
                    cwd=self.repository,
                    timeout=30,
                )
                failed = completed.returncode != 0 or failed
            except (LaneFailure, OSError, subprocess.SubprocessError):
                failed = True
            self.network_started = False
        return "failed" if failed else "pass"


def _load_report(path: pathlib.Path) -> dict[str, Any]:
    content = _stable_regular_bytes(
        path,
        "cell_report_invalid",
        maximum=MAX_CELL_REPORT_BYTES,
    )
    return _strict_json(content, "cell_report_invalid")


def _counter_normalized(report: dict[str, Any]) -> dict[str, Any]:
    criteria = report.get("criteria", {})
    restart = criteria.get("restart_reconnect_hash_parity", {})
    hash_contract: dict[str, Any] = {}
    for audience in ("participant", "spectator"):
        value = restart.get(audience, {})
        before = value.get("before", {})
        after = value.get("after", {})
        head = before.get("head_hashes", {})
        replay = before.get("replay_head_hashes", {})
        stable_head_hashes = {
            key: head.get(key)
            for key in ("pack_digest", "activity_state_hash")
            if isinstance(head, dict)
        }
        hash_contract[audience] = {
            "room_seq": before.get("room_seq"),
            "fields": sorted(head) if isinstance(head, dict) else [],
            "stable_hashes": {
                **stable_head_hashes,
                "projection_hash": before.get("projection_hash"),
                "replay_hash": before.get("replay_hash"),
            },
            "identity_bound_hash_fields": sorted(
                set(head) - {"pack_digest", "activity_state_hash"}
            )
            if isinstance(head, dict)
            else [],
            "head_equals_replay": head == replay,
            "restart_exact": before == after,
            "projection_replay_hash_equal": before.get("projection_hash")
            == before.get("replay_hash"),
        }
    frames = criteria.get("authorized_frames", {})
    return {
        "criteria": {
            key: value.get("status") if isinstance(value, dict) else None
            for key, value in sorted(criteria.items())
        },
        "authority": criteria.get("current_and_historical_authority"),
        "action_contracts": criteria.get("action_contracts"),
        "historical_replay": criteria.get("historical_replay"),
        "frame_scopes": {
            key: {
                "frame_seq": value.get("frame_seq"),
                "cause_room_seq": value.get("cause_room_seq"),
                "observation_keys": value.get("observation_keys"),
            }
            for key, value in sorted(frames.items())
            if key.endswith("_frame") and isinstance(value, dict)
        },
        "private_spectator_frame": frames.get("private_spectator_frame"),
        "hash_contract": hash_contract,
    }


def _activation_normalized(value: Any) -> Any:
    if not isinstance(value, dict):
        return value
    return {
        key: (
            {
                field: receipt.get(field)
                for field in (
                    "code",
                    "state",
                    "lease_generation",
                    "context_present",
                    "context_hash_present",
                )
            }
            if isinstance(receipt, dict)
            else receipt
        )
        for key, receipt in sorted(value.items())
        if key != "activation_id"
    }


def _lost_reply_normalized(value: Any) -> Any:
    if not isinstance(value, dict):
        return value
    return {
        key: value.get(key)
        for key in (
            "status",
            "claim_id",
            "transport_loss_observed",
            "exact_request_retried",
            "durable_duplicate_result_matches",
            "restart_same_data_dir",
            "seams_disabled_after_restart",
            "secrets",
        )
    } | {
        key: _activation_normalized({key: value.get(key)}).get(key)
        for key in ("first_duplicate", "second_duplicate")
    }


def _heist_normalized(report: dict[str, Any]) -> dict[str, Any]:
    final = report.get("final", {})
    parity = final.get("replay_hash_parity", {})
    timers = report.get("timers", [])
    activation = report.get("activation", {})
    return {
        "phase_path": report.get("phase_path"),
        "six_phase_order": report.get("six_phase_order"),
        "final": {
            key: final.get(key)
            for key in (
                "phase",
                "outcome",
                "commitment_count",
                "broker_seat_present",
                "replay_verified",
            )
        },
        "hash_contract": {
            "verified": parity.get("verified"),
            "fields": parity.get("fields"),
            "pack_digest": parity.get("expected", {}).get("pack")
            if isinstance(parity.get("expected"), dict)
            else None,
            "identity_bound_hash_fields": sorted(
                set(parity.get("fields", ())) - {"pack"}
            ),
            "final_equals_replay": parity.get("expected") == parity.get("replayed"),
        },
        "privacy": report.get("privacy"),
        "duplicate_action": report.get("duplicate_action"),
        "lost_claim_reply": _lost_reply_normalized(report.get("lost_claim_reply")),
        "timers": [
            {
                key: timer.get(key)
                for key in (
                    "phase",
                    "timer_id",
                    "generation",
                    "duplicate",
                    "transition_id_present",
                    "room_seq",
                )
            }
            for timer in timers
            if isinstance(timer, dict)
        ],
        "activation": {
            key: _activation_normalized(value)
            for key, value in sorted(activation.items())
        },
    }


def _validate_cell(story: str, backend: str, report: dict[str, Any]) -> None:
    if report.get("status") != "completed":
        raise LaneFailure(f"{story}_{backend}_blocked")
    storage = report.get("storage")
    if not isinstance(storage, dict) or storage.get("status") != "verified":
        raise LaneFailure(f"{story}_{backend}_storage_identity_missing")
    expected_profile = "sqlite-bundled" if backend == "sqlite" else "postgres-primary"
    if storage.get("profile") != expected_profile:
        raise LaneFailure(f"{story}_{backend}_storage_profile_mismatch")
    if backend != "sqlite" and storage.get("exact_identity") != EXPECTED_ENGINE:
        raise LaneFailure(f"{story}_{backend}_postgres_identity_mismatch")
    if report.get("secrets") != "not_emitted":
        raise LaneFailure(f"{story}_{backend}_secret_contract_failed")
    disclosure = report.get("reference_workload")
    measurements = report.get("measurements")
    fan_out = measurements.get("fan_out") if isinstance(measurements, dict) else None
    if not (
        isinstance(disclosure, dict)
        and set(disclosure)
        == {
            "payload_sizes_bytes",
            "pack_id",
            "participants_per_room",
            "fan_out",
            "snapshot_cadence_transitions",
        }
        and isinstance(disclosure.get("payload_sizes_bytes"), list)
        and disclosure["payload_sizes_bytes"]
        and all(
            type(value) is int and value > 0
            for value in disclosure["payload_sizes_bytes"]
        )
        and disclosure.get("pack_id")
        == ("worldstream.counter" if story == "counter" else "worldstream.agent-heist")
        and type(disclosure.get("participants_per_room")) is int
        and disclosure["participants_per_room"] > 0
        and type(disclosure.get("fan_out")) is int
        and isinstance(fan_out, dict)
        and disclosure["fan_out"] == fan_out.get("observation_fan_out")
        and type(disclosure.get("snapshot_cadence_transitions")) is int
        and disclosure["snapshot_cadence_transitions"] > 0
    ):
        raise LaneFailure(f"{story}_{backend}_reference_workload_invalid")
    if story == "counter":
        criteria = report.get("criteria")
        if not isinstance(criteria, dict) or any(
            not isinstance(value, dict) or value.get("status") != "passed"
            for value in criteria.values()
        ):
            raise LaneFailure(f"counter_{backend}_criterion_failed")
        return
    final = report.get("final")
    if (
        report.get("six_phase_order") is not True
        or report.get("private_contexts_emitted") is not False
        or not isinstance(final, dict)
        or final.get("phase") != "complete"
        or final.get("broker_seat_present") is not True
        or final.get("commitment_count") != 2
        or final.get("replay_verified") is not True
    ):
        raise LaneFailure(f"heist_{backend}_story_contract_failed")
    outcome = final.get("outcome")
    checks = outcome.get("checks") if isinstance(outcome, dict) else None
    vote_counts = outcome.get("vote_counts") if isinstance(outcome, dict) else None
    if (
        not isinstance(outcome, dict)
        or outcome.get("outcome") != "success"
        or outcome.get("score") != 5
        or outcome.get("reason") != "scored_selected_plan"
        or outcome.get("missing_roles") != ["broker"]
        or not isinstance(checks, dict)
        or set(checks)
        != {
            "route",
            "entry_window",
            "required_tool",
            "extraction",
            "resource_contributed",
        }
        or any(value is not True for value in checks.values())
        or not isinstance(vote_counts, dict)
        or sorted(vote_counts.values()) != [2]
    ):
        raise LaneFailure(f"heist_{backend}_absent_broker_outcome_failed")
    parity = final.get("replay_hash_parity")
    if (
        not isinstance(parity, dict)
        or parity.get("verified") is not True
        or parity.get("expected") != parity.get("replayed")
        or set(parity.get("fields", ()))
        != {
            "pack",
            "core",
            "activity",
            "aggregate_authoritative",
            "transition",
            "room_id",
            "room_seq",
        }
    ):
        raise LaneFailure(f"heist_{backend}_hash_contract_failed")
    lost = report.get("lost_claim_reply")
    if (
        not isinstance(lost, dict)
        or lost.get("status") != "completed"
        or lost.get("transport_loss_observed") is not True
        or lost.get("durable_duplicate_result_matches") is not True
    ):
        raise LaneFailure(f"heist_{backend}_lost_reply_contract_failed")
    timers = report.get("timers")
    if (
        not isinstance(timers, list)
        or len(timers) != 5
        or any(
            not isinstance(timer, dict)
            or timer.get("transition_id_present") is not True
            or timer.get("duplicate") is not False
            for timer in timers
        )
    ):
        raise LaneFailure(f"heist_{backend}_timer_contract_failed")


def _run_cell(
    *,
    story: str,
    backend: str,
    python: pathlib.Path,
    runner: pathlib.Path,
    repository: pathlib.Path,
    daemon: pathlib.Path,
    report_path: pathlib.Path,
    workload_storage_root: pathlib.Path,
    dsn_file: pathlib.Path | None,
    timeout: float,
    timer_timeout: float,
    offer_timeout: float,
) -> dict[str, Any]:
    arguments = [
        str(python),
        str(runner),
        "--binary",
        str(daemon),
        "--report",
        str(report_path),
    ]
    if story == "heist":
        arguments.extend(
            [
                "--spawn-daemon",
                "--exercise-lost-claim-reply",
                "--timer-timeout",
                str(timer_timeout),
                "--offer-timeout",
                str(offer_timeout),
            ]
        )
    if dsn_file is not None:
        arguments.extend(["--postgresql-dsn-file", str(dsn_file)])
    environment = {**os.environ}
    environment.pop("WORLDSTREAM__STORAGE__POSTGRESQL__DSN", None)
    environment.pop("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_HANDLE", None)
    privacy_label = f"{story}-{backend}"
    privacy_workspace = workload_storage_root / f"{privacy_label}-temporary"
    privacy_workspace.mkdir(mode=0o700)
    environment["TMPDIR"] = str(privacy_workspace)
    sdk_source = repository / "sdk" / "python" / "src"
    environment["PYTHONPATH"] = os.pathsep.join(
        [str(sdk_source), environment.get("PYTHONPATH", "")]
    ).rstrip(os.pathsep)
    measurement = _measured_process(
        arguments,
        cwd=repository,
        environment=environment,
        timeout=timeout,
        privacy_workspace=privacy_workspace,
        privacy_label=privacy_label,
    )
    exit_code = measurement["exit_code"]
    try:
        report = _load_report(report_path)
    except LaneFailure:
        report = {
            "status": "blocked",
            "reason_code": "cell_timed_out"
            if exit_code == 124
            else "cell_report_unavailable",
            "secrets": "not_emitted",
        }
    if ACTIVE_PRIVACY_CAPTURE is not None and report_path.is_file():
        ACTIVE_PRIVACY_CAPTURE.add_report(
            f"{privacy_label}-report", report_path, channel_class="report.cell"
        )
    return {**measurement, "report": report}


def _sha256_reference(value: Any) -> bool:
    return (
        isinstance(value, str)
        and re.fullmatch(r"sha256:[0-9a-f]{64}", value) is not None
    )


def _validate_browser_story(
    value: dict[str, Any], binding: dict[str, Any], browser: dict[str, Any]
) -> None:
    if set(value) != {
        "schema",
        "canonical_encoding",
        "status",
        "release_evidence",
        "source_mode",
        "elapsed_ms",
        "browser",
        "tools",
        "runtime",
        "story",
        "dom_evidence",
        "typed_actions",
        "checks",
        "privacy",
    } or not (
        value.get("schema") == BROWSER_STORY_SCHEMA
        and value.get("canonical_encoding") == "utf8-sorted-key-compact-json-lf"
        and value.get("status") == "pass"
        and value.get("release_evidence") is True
        and value.get("source_mode") == "package-extracted"
        and type(value.get("elapsed_ms")) is int
        and 0 < value["elapsed_ms"] <= 600_000
        and value.get("browser") == browser
    ):
        raise LaneFailure("package_browser_story_identity_invalid")
    expected_checks = {
        "browser_identity_verified",
        "catch_up_or_reset_installed",
        "embedded_ui_loaded",
        "final_reveal_dom_visible",
        "new_session_resynchronized",
        "package_bound_reference_clients",
        "package_bound_runtime",
        "precomplete_reveal_locked",
        "privacy_negative_dom_and_browser_channels",
        "replay_hashes_verified",
        "six_phase_story_complete",
        "stale_head_rejected",
        "typed_actions_accepted_in_dom",
    }
    checks = value.get("checks")
    if not (
        isinstance(checks, dict)
        and set(checks) == expected_checks
        and all(checks[item] is True for item in expected_checks)
    ):
        raise LaneFailure("package_browser_story_check_incomplete")
    runtime = value.get("runtime")
    assets = binding.get("runtime_assets")
    binaries = binding.get("binaries")
    if not (
        isinstance(runtime, dict)
        and isinstance(assets, dict)
        and isinstance(binaries, dict)
    ):
        raise LaneFailure("package_browser_story_runtime_invalid")
    expected_runtime = {
        "worldstreamd": {
            "sha256": binaries["worldstreamd"]["sha256"],
            "size_bytes": binaries["worldstreamd"]["size_bytes"],
            "origin": "package:bin/worldstreamd",
        },
        "ui": {
            **assets["ui"],
            "origin": "package:ui",
        },
        "sdk": {
            **assets["sdk_python_source"],
            "origin": "package:sdk/python/src",
        },
        "heist_reference_clients": {
            **assets["heist_reference_clients"],
            "origin": "package:examples/heist",
        },
    }
    if runtime != expected_runtime:
        raise LaneFailure("package_browser_story_runtime_mismatch")
    story = value.get("story")
    if not (
        isinstance(story, dict)
        and set(story) == {"phase_path", "public_projection", "final_replay"}
        and story.get("phase_path")
        == ["Briefing", "Negotiation", "Commitment", "Resolution", "Result", "Complete"]
        and story.get("public_projection", {}).get("broker_present") is True
        and story.get("public_projection", {}).get("commitment_count") == 2
        and story.get("public_projection", {}).get("aggregate_outcome_present") is True
        and story.get("final_replay", {}).get("verified") is True
        and story.get("final_replay", {}).get("hash_parity", {}).get("verified") is True
    ):
        raise LaneFailure("package_browser_story_result_invalid")
    dom = value.get("dom_evidence")
    actions = value.get("typed_actions")
    if not (
        isinstance(dom, dict)
        and set(dom)
        == {
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
        }
        and all(_sha256_reference(item) for item in dom.values())
        and isinstance(actions, dict)
        and set(actions)
        == {
            "inspect_clue",
            "publish_clue",
            "propose_plan",
            "commit_move",
            "acknowledge_result",
        }
        and all(_sha256_reference(item) for item in actions.values())
        and value.get("privacy")
        == {
            "status": "pass",
            "private_canary_absent": True,
            "credentials_absent": True,
            "private_claim_absent_from_retained_evidence": True,
        }
    ):
        raise LaneFailure("package_browser_story_dom_or_privacy_invalid")
    tools = value.get("tools")
    adapter = tools.get("adapter") if isinstance(tools, dict) else None
    python_identity = tools.get("python") if isinstance(tools, dict) else None
    adapter_digest, adapter_size = _sha256_record(
        ROOT / "scripts/cdp-browser.py",
        "package_browser_tool_identity_invalid",
    )
    if not (
        isinstance(adapter, dict)
        and set(tools) == {"adapter", "python"}
        and set(adapter) == {"name", "protocol", "sha256", "size_bytes"}
        and adapter.get("name") == "worldstream-cdp-browser"
        and adapter.get("protocol") == "Chrome DevTools Protocol"
        and adapter.get("sha256") == SHA256_PREFIX + adapter_digest
        and adapter.get("size_bytes") == adapter_size
        and isinstance(python_identity, dict)
        and python_identity == {"implementation": "cpython", "version": "3.14.7"}
    ):
        raise LaneFailure("package_browser_tool_identity_invalid")


def _run_browser_story(
    *,
    python: pathlib.Path,
    repository: pathlib.Path,
    package_root: pathlib.Path,
    daemon: pathlib.Path,
    binding: dict[str, Any],
    browser_path: pathlib.Path,
    browser: dict[str, Any],
    report_path: pathlib.Path,
    workspace: pathlib.Path,
    timeout: float,
) -> dict[str, Any]:
    cdp_state = workspace / "cdp-state"
    cdp_state.mkdir(mode=0o700)
    environment = {
        **os.environ,
        "TMPDIR": str(workspace),
        "WORLDSTREAM_PYTHON": str(python),
        "WORLDSTREAM_BROWSER_MODE": "cdp",
        "WORLDSTREAM_BROWSER_PACKAGE_MODE": "1",
        "WORLDSTREAM_BROWSER_PACKAGE_ROOT": str(package_root),
        "WORLDSTREAM_BROWSER_WORLDSTREAMD": str(daemon),
        "WORLDSTREAM_BROWSER_UI_DIR": str(package_root / "ui"),
        "WORLDSTREAM_BROWSER_SDK_SRC": str(package_root / "sdk/python/src"),
        "WORLDSTREAM_BROWSER_HEIST_DIR": str(package_root / "examples/heist"),
        "WORLDSTREAM_BROWSER_REPORT": str(report_path),
        "WORLDSTREAM_BROWSER_STORY_TIMEOUT": str(max(1, round(timeout - 30))),
        "WORLDSTREAM_CDP_ADAPTER": str(repository / "scripts/cdp-browser.py"),
        "WORLDSTREAM_CDP_STATE_DIR": str(cdp_state),
        "WORLDSTREAM_BROWSER_BINARY": str(browser_path),
        "WORLDSTREAM_BROWSER_VERSION": browser["version"],
        "WORLDSTREAM_BROWSER_SHA256": browser["sha256"].removeprefix(SHA256_PREFIX),
        "WORLDSTREAM_BROWSER_SIZE_BYTES": str(browser["size_bytes"]),
        "WORLDSTREAM_BROWSER_ARCHIVE_URL": browser["distribution"]["url"],
        "WORLDSTREAM_BROWSER_ARCHIVE_SHA256": browser["distribution"][
            "sha256"
        ].removeprefix(SHA256_PREFIX),
        "WORLDSTREAM_BROWSER_ARCHIVE_SIZE_BYTES": str(
            browser["distribution"]["size_bytes"]
        ),
    }
    measurement = _measured_process(
        ["bash", str(repository / "web/console/live-browser-story.sh")],
        cwd=repository,
        environment=environment,
        timeout=timeout,
        privacy_workspace=workspace,
        privacy_label="package-browser-heist",
    )
    content = _stable_regular_bytes(report_path, "package_browser_report_invalid")
    report = _strict_json(content, "package_browser_report_invalid")
    _validate_browser_story(report, binding, browser)
    if measurement["exit_code"] != 0:
        raise LaneFailure("package_browser_story_process_failed")
    if ACTIVE_PRIVACY_CAPTURE is not None:
        ACTIVE_PRIVACY_CAPTURE.add_report(
            "package-browser-heist-report", report_path, channel_class="report.cell"
        )
    return report


def _write_report(path: pathlib.Path, report: dict[str, Any]) -> None:
    encoded = json.dumps(report, sort_keys=True, separators=(",", ":"))
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(encoded + "\n", encoding="utf-8")
    print(encoded)


def main() -> int:
    global ACTIVE_PRIVACY_CAPTURE
    ACTIVE_PRIVACY_CAPTURE = None

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=pathlib.Path, required=True)
    parser.add_argument("--repository", type=pathlib.Path, default=pathlib.Path.cwd())
    parser.add_argument("--package-archive", type=pathlib.Path)
    parser.add_argument("--package-report", type=pathlib.Path)
    parser.add_argument(
        "--release-inventory",
        choices=tuple(PACKAGE.INVENTORY.BY_ID),
        help="closed release inventory; omission preserves historical verification",
    )
    parser.add_argument(
        "--diagnostic-source-tree",
        action="store_true",
        help="run non-promoting provider diagnostics with source-tree binaries",
    )
    parser.add_argument(
        "--worldstreamd",
        type=pathlib.Path,
        help="diagnostic-only daemon; forbidden for package-bound acceptance",
    )
    parser.add_argument(
        "--worldstreamctl",
        type=pathlib.Path,
        help="diagnostic-only CLI; forbidden for package-bound acceptance",
    )
    parser.add_argument(
        "--python", type=pathlib.Path, default=pathlib.Path(sys.executable)
    )
    parser.add_argument("--docker", default=shutil.which("docker"))
    parser.add_argument("--psql", default=shutil.which("psql"))
    parser.add_argument("--cell-timeout", type=float, default=420.0)
    parser.add_argument("--timer-timeout", type=float, default=240.0)
    parser.add_argument("--offer-timeout", type=float, default=30.0)
    parser.add_argument("--browser", type=pathlib.Path)
    parser.add_argument("--browser-version")
    parser.add_argument("--browser-sha256")
    parser.add_argument("--browser-size-bytes", type=int)
    parser.add_argument("--browser-archive-url")
    parser.add_argument("--browser-archive-sha256")
    parser.add_argument("--browser-archive-size-bytes", type=int)
    parser.add_argument("--browser-timeout", type=float, default=420.0)
    args = parser.parse_args()

    repository = args.repository.resolve()
    # Preserve a virtual-environment executable symlink. Resolving it to the
    # base interpreter would discard the environment's installed SDK deps.
    python = pathlib.Path(os.path.abspath(args.python))
    counter_runner = repository / "examples" / "counter" / "run_live_acceptance.py"
    heist_runner = (
        repository / "examples" / "heist" / "wave10_live" / "run_absent_broker_live.py"
    )
    report: dict[str, Any] = {
        "schema": SCHEMA,
        "canonical_encoding": "utf8-sorted-key-compact-json-lf",
        "status": "unavailable",
        "release_evidence": False,
        "secrets_emitted": False,
        "provider": {
            "postgres_image": POSTGRES_IMAGE,
            "pgbouncer_image": PGBOUNCER_IMAGE,
            "pool_mode": "transaction",
        },
        "reference_environment": None,
        "reference_workloads": {"counter": {}, "heist": {}},
        "cells": {"counter": {}, "heist": {}},
        "comparison": {},
        "package_binding": {"status": "not_verified"},
        "browser_story": {"status": "not_run"},
        "performance": {
            "classification": "measured_non_sla",
            "percentile_definition": "nearest-rank",
            "cells_ms": {},
            "cells": {},
        },
        "cleanup": "not_started",
        "privacy": {
            "status": "pending",
            "channel_contract": PRIVACY_CHANNEL_CONTRACT,
        },
    }
    package_mode = not args.diagnostic_source_tree
    package_inputs_valid = (
        args.package_archive is not None
        and args.package_report is not None
        and args.worldstreamd is None
        and args.worldstreamctl is None
        and args.browser is not None
        and isinstance(args.browser_version, str)
        and re.fullmatch(r"[0-9]+(?:\.[0-9]+){3}", args.browser_version) is not None
        and isinstance(args.browser_sha256, str)
        and re.fullmatch(r"[0-9a-f]{64}", args.browser_sha256) is not None
        and type(args.browser_size_bytes) is int
        and 0 < args.browser_size_bytes <= 1024 * 1024 * 1024
        and isinstance(args.browser_archive_url, str)
        and args.browser_archive_url.startswith(
            "https://storage.googleapis.com/chrome-for-testing-public/"
        )
        and isinstance(args.browser_archive_sha256, str)
        and re.fullmatch(r"[0-9a-f]{64}", args.browser_archive_sha256) is not None
        and type(args.browser_archive_size_bytes) is int
        and 0 < args.browser_archive_size_bytes <= 1024 * 1024 * 1024
        and 30 <= args.browser_timeout <= 600
        and {
            "product": "chrome-for-testing-headless-shell",
            "version": args.browser_version,
            "sha256": SHA256_PREFIX + args.browser_sha256,
            "size_bytes": args.browser_size_bytes,
            "version_output": f"Google Chrome for Testing {args.browser_version}",
            "distribution": {
                "url": args.browser_archive_url,
                "sha256": SHA256_PREFIX + args.browser_archive_sha256,
                "size_bytes": args.browser_archive_size_bytes,
            },
        }
        == PINNED_BROWSER_IDENTITY
    )
    diagnostic_inputs_valid = (
        args.package_archive is None
        and args.package_report is None
        and args.browser is None
        and args.browser_version is None
        and args.browser_sha256 is None
        and args.browser_size_bytes is None
        and args.browser_archive_url is None
        and args.browser_archive_sha256 is None
        and args.browser_archive_size_bytes is None
        and args.release_inventory is None
    )
    if (package_mode and not package_inputs_valid) or (
        not package_mode and not diagnostic_inputs_valid
    ):
        report["reason_code"] = "package_bound_inputs_required"
        _write_report(args.report, report)
        return 10

    root = pathlib.Path(tempfile.mkdtemp(prefix="worldstream-packaged-parity-"))
    root.chmod(0o700)
    workload_storage_root = root / "workload-storage"
    workload_storage_root.mkdir(mode=0o700)
    if package_mode:
        if not sys.platform.startswith("linux"):
            report["reason_code"] = "linux_package_execution_host_required"
            shutil.rmtree(root, ignore_errors=True)
            _write_report(args.report, report)
            return 10
        try:
            package_root = root / "verified-package"
            daemon, ctl, binding = _bind_package(
                args.package_archive,
                args.package_report,
                package_root,
                args.release_inventory,
            )
        except LaneFailure as error:
            report["reason_code"] = error.code
            shutil.rmtree(root, ignore_errors=True)
            _write_report(args.report, report)
            return 10
        report["package_binding"] = binding
        browser_path = pathlib.Path(os.path.abspath(args.browser))
        browser_identity = PINNED_BROWSER_IDENTITY
    else:
        daemon_input = args.worldstreamd or pathlib.Path("target/debug/worldstreamd")
        ctl_input = args.worldstreamctl or pathlib.Path("target/debug/worldstreamctl")
        daemon = (
            (repository / daemon_input).resolve()
            if not daemon_input.is_absolute()
            else daemon_input
        )
        ctl = (
            (repository / ctl_input).resolve()
            if not ctl_input.is_absolute()
            else ctl_input
        )
        report["package_binding"] = {
            "schema": PACKAGE_BINDING_SCHEMA,
            "status": "diagnostic_source_tree",
            "release_eligible": False,
        }
        browser_path = pathlib.Path()
        browser_identity = {}

    prerequisites = [daemon, ctl, python, counter_runner, heist_runner]
    if package_mode:
        prerequisites.append(browser_path)
    if (
        os.name != "posix"
        or not args.docker
        or not args.psql
        or any(not path.is_file() for path in prerequisites)
        or any(not os.access(path, os.X_OK) for path in (daemon, ctl, python))
        or (package_mode and not os.access(browser_path, os.X_OK))
    ):
        report["reason_code"] = "packaged_acceptance_prerequisite_missing"
        shutil.rmtree(root, ignore_errors=True)
        _write_report(args.report, report)
        return 10

    privacy_capture = PrivacyCapture(root)
    ACTIVE_PRIVACY_CAPTURE = privacy_capture
    provider = Provider.create(root, repository, args.docker, args.psql, ctl)
    exit_code = 13
    try:
        print("packaged parity: starting pinned provider", file=sys.stderr, flush=True)
        provider.start()
        database_storage, provider_storage = provider.database_storage_binding()
        report["reference_environment"] = _reference_environment(
            workload_storage_root,
            database_storage,
            provider_storage,
        )
        report["reference_environment"]["engines"]["postgresql"] = (
            provider.reference_engine()
        )
        sequence = [
            ("counter", "sqlite", None, None),
            ("heist", "sqlite", None, None),
            (
                "counter",
                "postgres_direct",
                provider.runtime_dsn_file(
                    DATABASES["counter_postgres_direct"], pooler=False
                ),
                DATABASES["counter_postgres_direct"],
            ),
            (
                "heist",
                "postgres_direct",
                provider.runtime_dsn_file(
                    DATABASES["heist_postgres_direct"], pooler=False
                ),
                DATABASES["heist_postgres_direct"],
            ),
            (
                "counter",
                "transaction_pooler",
                provider.runtime_dsn_file(
                    DATABASES["counter_transaction_pooler"], pooler=True
                ),
                DATABASES["counter_transaction_pooler"],
            ),
            (
                "heist",
                "transaction_pooler",
                provider.runtime_dsn_file(
                    DATABASES["heist_transaction_pooler"], pooler=True
                ),
                DATABASES["heist_transaction_pooler"],
            ),
        ]
        failures: list[str] = []
        if package_mode:
            print(
                "packaged parity: running package-bound real-browser Heist story",
                file=sys.stderr,
                flush=True,
            )
            browser_workspace = root / "browser-story-temporary"
            browser_workspace.mkdir(mode=0o700)
            try:
                report["browser_story"] = _run_browser_story(
                    python=python,
                    repository=repository,
                    package_root=package_root,
                    daemon=daemon,
                    binding=binding,
                    browser_path=browser_path,
                    browser=browser_identity,
                    report_path=root / "package-browser-heist.json",
                    workspace=browser_workspace,
                    timeout=args.browser_timeout,
                )
            except LaneFailure as error:
                report["browser_story"] = {
                    "status": "failed",
                    "reason_code": error.code,
                }
                failures.append(error.code)
        else:
            report["browser_story"] = {
                "status": "not_applicable",
                "release_evidence": False,
            }
        for story, backend, dsn_file, database in sequence:
            print(
                f"packaged parity: running {story}.{backend}",
                file=sys.stderr,
                flush=True,
            )
            cell_report_path = root / f"{story}-{backend}.json"
            database_before = (
                provider.measurement(database) if database is not None else None
            )
            cell = _run_cell(
                story=story,
                backend=backend,
                python=python,
                runner=counter_runner if story == "counter" else heist_runner,
                repository=repository,
                daemon=daemon,
                report_path=cell_report_path,
                workload_storage_root=workload_storage_root,
                dsn_file=dsn_file,
                timeout=args.cell_timeout,
                timer_timeout=args.timer_timeout,
                offer_timeout=args.offer_timeout,
            )
            database_after = (
                provider.measurement(database) if database is not None else None
            )
            rss = cell["process_tree_rss_kib"]
            cell_measurements = cell["report"].get("measurements")
            if not isinstance(cell_measurements, dict):
                cell_measurements = {}
            cell["performance"] = {
                "latency_ms": _latency_triplet(cell["report"]),
                "memory": {
                    "status": "measured",
                    "peak_rss_bytes": rss["peak"] * 1024,
                    "process_tree_rss_kib": rss,
                },
                "story_duration_ms": cell["elapsed_ms"],
                "load": cell_measurements.get("load"),
                "fan_out": cell_measurements.get("fan_out"),
                "recovery": cell_measurements.get("recovery"),
            }
            if database_before is not None and database_after is not None:
                database_growth = (
                    database_after["database_bytes"] - database_before["database_bytes"]
                )
                if database_growth < 0:
                    failures.append(f"{story}_{backend}_database_size_regressed")
                cell["performance"]["database_growth"] = {
                    "status": "measured",
                    "database": database,
                    "before": database_before,
                    "after": database_after,
                    "growth_bytes": database_growth,
                    "cluster_wal_growth_bytes": (
                        database_after["cluster_wal_bytes"]
                        - database_before["cluster_wal_bytes"]
                    ),
                    "transition_delta": (
                        database_after["transitions"] - database_before["transitions"]
                    ),
                    "active_rooms_delta": (
                        database_after["active_rooms"] - database_before["active_rooms"]
                    ),
                    "observation_frames_delta": (
                        database_after["observation_frames"]
                        - database_before["observation_frames"]
                    ),
                    "activation_receipts_delta": (
                        database_after["activation_receipts"]
                        - database_before["activation_receipts"]
                    ),
                }
            report["cells"][story][backend] = cell
            report["performance"]["cells_ms"][f"{story}.{backend}"] = cell["elapsed_ms"]
            report["performance"]["cells"][f"{story}.{backend}"] = cell["performance"]
            report["reference_workloads"][story][backend] = {
                "load": cell["performance"]["load"],
                "fan_out": cell["performance"]["fan_out"],
                "observed_transitions": cell_measurements.get("observed_transitions"),
                "story_duration_ms": cell_measurements.get("story_duration_ms"),
                "disclosure": cell["report"].get("reference_workload"),
            }
            print(
                f"packaged parity: {story}.{backend} exit={cell['exit_code']}",
                file=sys.stderr,
                flush=True,
            )
            try:
                _validate_cell(story, backend, cell["report"])
            except LaneFailure as error:
                failures.append(error.code)

        sqlite_identity = (
            report["cells"]
            .get("counter", {})
            .get("sqlite", {})
            .get("report", {})
            .get("storage", {})
            .get("exact_identity")
        )
        sqlite_storage = (
            report["cells"]
            .get("counter", {})
            .get("sqlite", {})
            .get("report", {})
            .get("storage", {})
        )
        sqlite_settings = sqlite_storage.get("settings")
        sqlite_connection_mode = sqlite_storage.get("connection_mode")
        if (
            not isinstance(sqlite_identity, str)
            or not sqlite_identity.startswith("sqlite/3.53.4;")
            or sqlite_settings != {"journal_mode": "wal", "synchronous": "full"}
            or sqlite_connection_mode != "embedded"
        ):
            failures.append("sqlite_reference_settings_invalid")
        report["reference_environment"]["engines"]["sqlite"] = {
            "version": "3.53.4",
            "settings": SQLITE_REFERENCE_SETTINGS,
            "connection_mode": sqlite_connection_mode,
        }

        for story, normalizer in (
            ("counter", _counter_normalized),
            ("heist", _heist_normalized),
        ):
            normalized = {
                backend: normalizer(cell["report"])
                for backend, cell in report["cells"][story].items()
            }
            parity = (
                normalized.get("sqlite")
                == normalized.get("postgres_direct")
                == normalized.get("transaction_pooler")
            )
            report["comparison"][story] = {
                "status": "pass" if parity else "drift",
                "normalized": normalized,
            }
            if not parity:
                failures.append(f"{story}_normalized_backend_drift")
        if failures:
            report["status"] = "incomplete"
            report["reason_codes"] = sorted(set(failures))
        else:
            report["status"] = "pass" if package_mode else "diagnostic_pass"
            report["release_evidence"] = package_mode
            exit_code = 0
    except LaneFailure as error:
        report["status"] = "unavailable"
        report["reason_code"] = error.code
        exit_code = 10
    finally:
        report["cleanup"] = provider.cleanup()
        if report["cleanup"] != "pass":
            report["status"] = "incomplete"
            report.setdefault("reason_codes", []).append(
                "owned_resource_cleanup_failed"
            )
            exit_code = 14
        try:
            privacy_capture.finish_daemon_logs()
            candidate = privacy_capture.root / "packaged-report-candidate.json"
            candidate_report = {**report, "secrets_emitted": "pending_scan"}
            _owner_bytes(
                candidate,
                (
                    json.dumps(candidate_report, sort_keys=True, separators=(",", ":"))
                    + "\n"
                ).encode("utf-8"),
            )
            privacy_capture.add_report(
                "packaged-report", candidate, channel_class="report.aggregate"
            )
            secret_scan = privacy_capture.scan()
            report["privacy"] = {
                "status": "pass",
                "channel_contract": report["privacy"]["channel_contract"],
                "secret_scan": secret_scan,
            }
            report["secrets_emitted"] = False
        except LaneFailure as error:
            report["status"] = "incomplete"
            report["release_evidence"] = False
            report["secrets_emitted"] = True
            report["privacy"] = {
                "status": "failed",
                "reason_code": error.code,
            }
            report.setdefault("reason_codes", []).append(error.code)
            exit_code = 13
        encoded = json.dumps(report, sort_keys=True, separators=(",", ":"))
        if provider.admin_password in encoded or provider.runtime_password in encoded:
            report = {
                "schema": SCHEMA,
                "status": "incomplete",
                "reason_code": "secret_material_in_report",
                "release_evidence": False,
                "secrets_emitted": True,
                "cleanup": report["cleanup"],
            }
            exit_code = 13
        shutil.rmtree(root, ignore_errors=True)
        ACTIVE_PRIVACY_CAPTURE = None
    _write_report(args.report, report)
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())

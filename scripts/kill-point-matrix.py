#!/usr/bin/env python3
"""Exercise the frozen Linux SIGKILL matrix through public WorldStream APIs.

Every cell starts the packaged ``worldstreamd`` against a private SQLite or
PostgreSQL store, waits for the daemon's exact opt-in boundary marker, sends
SIGKILL from this harness, restarts the same store, and resolves the original
operation identity. PostgreSQL is exercised through both its direct runtime
path and its pinned transaction-pool path; migrations and observations always
use the direct-admin path. The guarded marker seam is inert without the full
test configuration and cannot turn an abort, timeout, or graceful exit into a
pass.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import importlib.util
import json
import math
import os
import pathlib
import platform
import secrets
import shutil
import signal
import socket
import sqlite3
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Awaitable
from dataclasses import dataclass
from typing import Any

import tomllib
from worldstream_sdk import Client, LostActionReply, LostRunnerReply, ProtocolError

ROOT = pathlib.Path(__file__).resolve().parents[1]
SECRET_SCAN_PATH = ROOT / "scripts" / "verify-secret-absence.py"
BUILD_IDENTITY_PATH = ROOT / "scripts" / "release_build_identity.py"
POSTGRES_HARNESS_PATH = ROOT / "scripts" / "postgres-packaged-acceptance.py"
SCHEMA = "worldstream/kill-point-evidence/v1"
MARKER_SCHEMA = "worldstream/test-crash-boundary-ready/v1"
ENABLE_VALUE = "worldstream-linux-process-evidence-v1"
EVIDENCE_ID = "failure-fuzz-resource-and-one-hour-sqlite-soak"
BOUNDARIES = (
    "before_commit",
    "after_commit_before_publication",
    "after_publication_before_reply",
)
BACKEND_PROFILES = (
    ("sqlite", "embedded"),
    ("postgresql", "direct"),
    ("postgresql", "transaction_pool"),
)
EXPECTED_CELL_COUNT = 36
EXPECTED_OUTCOME = {
    "before_commit": "no_commit",
    "after_commit_before_publication": "original_result",
    "after_publication_before_reply": "original_result",
}
COUNTER_PACK = {
    "id": "worldstream.counter",
    "version": "2.0.0",
    "digest": "blake3:1c5f75068220f65f9017a062dbe40203c540108b572284d9446a329914008a92",
}
HEIST_PACK = {
    "id": "worldstream.agent-heist",
    "version": "0.1.0",
    "digest": "blake3:b05a682f0923001914a800072ee68348e68b979453c93e033cba7916b99a4407",
}
HEIST_TIMER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FH0"
HEIST_CONFIG = {
    "pack_id": "worldstream.agent-heist",
    "pack_schema": 1,
    "roles": ["navigator", "insider", "broker"],
    "briefing_duration_seconds": 30,
    "negotiation_duration_seconds": 90,
    "commitment_duration_seconds": 30,
    "commitment_reminder_seconds_before_deadline": 10,
    "result_duration_seconds": 20,
    "maximum_plans": 12,
    "maximum_open_offers_per_role": 4,
}
ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
MAX_HTTP_JSON_BYTES = 1024 * 1024
MAX_MARKER_JSON_BYTES = 64 * 1024
MAX_PACKAGE_REPORT_BYTES = 8 * 1024 * 1024
MAX_MANIFEST_JSON_BYTES = 8 * 1024 * 1024


class EvidenceFailure(RuntimeError):
    """A closed, non-secret failure in the process evidence harness."""


def load_build_identity() -> Any:
    spec = importlib.util.spec_from_file_location(
        "worldstream_kill_point_strict_json", BUILD_IDENTITY_PATH
    )
    if spec is None or spec.loader is None:
        raise EvidenceFailure("strict JSON parser could not be loaded")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


BUILD_IDENTITY = load_build_identity()


def load_postgres_harness() -> Any:
    spec = importlib.util.spec_from_file_location(
        "worldstream_kill_point_postgres_harness", POSTGRES_HARNESS_PATH
    )
    if spec is None or spec.loader is None:
        raise EvidenceFailure("PostgreSQL provider harness could not be loaded")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


POSTGRES_HARNESS = load_postgres_harness()


def strict_json_bytes(raw: bytes, label: str, maximum: int) -> dict[str, Any]:
    if not (0 < len(raw) <= maximum):
        raise EvidenceFailure(f"{label} exceeded its bounded JSON size")
    try:
        return BUILD_IDENTITY.strict_json(raw, label)
    except BUILD_IDENTITY.IdentityError as error:
        raise EvidenceFailure(f"{label} was not strict JSON") from error


def load_secret_scan() -> Any:
    spec = importlib.util.spec_from_file_location(
        "worldstream_kill_point_secret_scan", SECRET_SCAN_PATH
    )
    if spec is None or spec.loader is None:
        raise EvidenceFailure("secret-absence scanner could not be loaded")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


SECRET_SCAN = load_secret_scan()


def new_ulid() -> str:
    value = (int(time.time_ns() // 1_000_000) << 80) | int.from_bytes(
        secrets.token_bytes(10), "big"
    )
    return "".join(ALPHABET[(value >> shift) & 31] for shift in range(125, -1, -5))[
        -26:
    ]


def canonical_hash(value: Any) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return "sha256:" + hashlib.sha256(encoded).hexdigest()


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return "sha256:" + digest.hexdigest()


def distribution_identity(
    binary: pathlib.Path,
    package_archive: pathlib.Path | None,
    package_report: pathlib.Path | None,
) -> dict[str, Any]:
    if binary.is_symlink() or not binary.is_file():
        raise EvidenceFailure("daemon identity input must be a regular file")
    binary_digest = sha256_file(binary)
    base: dict[str, Any] = {
        "binary_sha256": binary_digest,
        "binary_size_bytes": binary.stat().st_size,
        "packaged_artifact_bound": False,
        "reference_class": "source_build_diagnostic",
    }
    if package_archive is None and package_report is None:
        return base
    if package_archive is None or package_report is None:
        raise EvidenceFailure(
            "package archive and package report must be supplied together"
        )
    if (
        package_archive.is_symlink()
        or package_report.is_symlink()
        or not package_archive.is_file()
        or not package_report.is_file()
    ):
        raise EvidenceFailure("package identity inputs must not be symlinks")
    try:
        report = strict_json_bytes(
            package_report.read_bytes(),
            "package report",
            MAX_PACKAGE_REPORT_BYTES,
        )
    except OSError as error:
        raise EvidenceFailure("package report is unavailable or invalid") from error
    identity = report.get("identity")
    inventory = report.get("inventory")
    archive_digest = sha256_file(package_archive)
    if (
        report.get("schema") != "worldstream/package-report/v1"
        or report.get("kind") != "archive"
        or report.get("artifact") != package_archive.name
        or report.get("path") != package_archive.name
        or report.get("sha256") != archive_digest
        or report.get("size_bytes") != package_archive.stat().st_size
        or not isinstance(identity, dict)
        or identity.get("target") != "linux-x86_64"
        or not isinstance(identity.get("version"), str)
        or not identity["version"]
        or not isinstance(inventory, dict)
        or inventory.get("archive_verified") is not True
        or inventory.get("manifest_source") != "compatibility.toml"
        or inventory.get("manifest_mirror") != "compatibility.json"
    ):
        raise EvidenceFailure("package report does not bind a verified Linux archive")
    try:
        with tarfile.open(package_archive, "r:gz") as archive:
            members = archive.getmembers()
            names = [member.name for member in members]
            if len(names) != len(set(names)):
                raise EvidenceFailure("package archive contains duplicate members")
            for member in members:
                parts = pathlib.PurePosixPath(member.name).parts
                if (
                    not member.name
                    or member.name.startswith("/")
                    or "\\" in member.name
                    or any(part in {"", ".", ".."} for part in parts)
                    or not (member.isfile() or member.isdir())
                ):
                    raise EvidenceFailure("package archive contains an unsafe member")
            binary_members = [
                member
                for member in members
                if member.isfile() and member.name.endswith("/bin/worldstreamd")
            ]
            manifest_toml_members = [
                member
                for member in members
                if member.isfile()
                and member.name.endswith("/manifest/compatibility.toml")
            ]
            manifest_json_members = [
                member
                for member in members
                if member.isfile()
                and member.name.endswith("/manifest/compatibility.json")
            ]
            if (
                len(binary_members) != 1
                or len(manifest_toml_members) != 1
                or len(manifest_json_members) != 1
            ):
                raise EvidenceFailure(
                    "archive has no unique daemon and compatibility manifest pair"
                )
            binary_source = archive.extractfile(binary_members[0])
            manifest_toml_source = archive.extractfile(manifest_toml_members[0])
            manifest_json_source = archive.extractfile(manifest_json_members[0])
            if (
                binary_source is None
                or manifest_toml_source is None
                or manifest_json_source is None
            ):
                raise EvidenceFailure("archive identity members could not be read")
            packaged_binary_digest = (
                "sha256:" + hashlib.sha256(binary_source.read()).hexdigest()
            )
            manifest_toml_bytes = manifest_toml_source.read()
            manifest_json_bytes = manifest_json_source.read()
            manifest_digest = hashlib.sha256(manifest_toml_bytes).hexdigest()
            mirror_digest = hashlib.sha256(manifest_json_bytes).hexdigest()
            manifest = tomllib.loads(manifest_toml_bytes.decode("utf-8"))
            mirror = strict_json_bytes(
                manifest_json_bytes,
                "packaged compatibility mirror",
                MAX_MANIFEST_JSON_BYTES,
            )
            canonical_mirror = (
                json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True)
                + "\n"
            ).encode()
    except (OSError, UnicodeDecodeError, ValueError, tarfile.TarError) as error:
        raise EvidenceFailure(
            "verified package archive could not be inspected"
        ) from error
    if packaged_binary_digest != binary_digest:
        raise EvidenceFailure(
            "extracted daemon differs from the verified package archive"
        )
    if (
        identity.get("manifest_sha256") != mirror_digest
        or identity.get("manifest_json_sha256") != mirror_digest
        or identity.get("manifest_toml_sha256") != manifest_digest
    ):
        raise EvidenceFailure(
            "package report manifest identities differ from archive bytes"
        )
    if manifest != mirror or manifest_json_bytes != canonical_mirror:
        raise EvidenceFailure(
            "archived compatibility manifest pair is not semantically canonical"
        )
    root_toml = ROOT / "compatibility.toml"
    root_json = ROOT / "compatibility.json"
    if (
        root_toml.is_symlink()
        or root_json.is_symlink()
        or not root_toml.is_file()
        or not root_json.is_file()
        or root_toml.read_bytes() != manifest_toml_bytes
        or root_json.read_bytes() != manifest_json_bytes
    ):
        raise EvidenceFailure(
            "archived compatibility pair differs from the workspace release root"
        )
    return {
        "binary_sha256": binary_digest,
        "binary_size_bytes": binary.stat().st_size,
        "packaged_artifact_bound": True,
        "reference_class": "fresh_packaged_linux_x86_64",
        "archive_sha256": archive_digest,
        "archive_size_bytes": package_archive.stat().st_size,
        "package_report_sha256": sha256_file(package_report),
        "manifest_sha256": "sha256:" + mirror_digest,
        "manifest_json_sha256": "sha256:" + mirror_digest,
        "manifest_toml_sha256": "sha256:" + manifest_digest,
        "target": identity["target"],
        "version": identity.get("version"),
    }


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        raise EvidenceFailure("latency sample is empty")
    ordered = sorted(values)
    rank = max(1, min(len(ordered), math.ceil(len(ordered) * fraction)))
    return round(ordered[rank - 1], 3)


def private_root(prefix: str, authority_secret: bytes | None = None) -> pathlib.Path:
    root = pathlib.Path(tempfile.mkdtemp(prefix=prefix))
    root.chmod(0o700)
    secret = os.urandom(32) if authority_secret is None else authority_secret
    if not isinstance(secret, bytes) or len(secret) != 32:
        shutil.rmtree(root, ignore_errors=True)
        raise EvidenceFailure("authority secret clone input was invalid")
    secret_path = root / "authority.secret"
    descriptor = os.open(secret_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as output:
        output.write(secret)
        output.flush()
        os.fsync(output.fileno())
    return root


def clone_root(source: pathlib.Path, prefix: str) -> pathlib.Path:
    root = private_root(prefix, (source / "authority.secret").read_bytes())
    shutil.copytree(source / "data", root / "data")
    return root


def owner_text(path: pathlib.Path, value: str) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            output.write(value)
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        path.unlink(missing_ok=True)
        raise
    if stat.S_IMODE(path.stat().st_mode) != 0o600:
        raise EvidenceFailure("owner-only runtime file mode was not preserved")


@dataclass
class BackendProfile:
    storage_backend: str
    connection_mode: str
    provider: Any | None = None
    privacy_capture: Any | None = None
    empty_database: str | None = None
    sequence: int = 0

    @property
    def profile_id(self) -> str:
        return f"{self.storage_backend}_{self.connection_mode}"

    def database(self, root: pathlib.Path) -> str:
        path = root / "postgresql-database"
        if (
            self.storage_backend != "postgresql"
            or path.is_symlink()
            or not path.is_file()
        ):
            raise EvidenceFailure("PostgreSQL root database identity was unavailable")
        database = path.read_text(encoding="ascii").strip()
        if not database or not database.replace("_", "").isalnum():
            raise EvidenceFailure("PostgreSQL root database identity was invalid")
        return database

    def _next_database(self) -> str:
        self.sequence += 1
        token = "direct" if self.connection_mode == "direct" else "pool"
        database = f"kill_{token}_{os.getpid()}_{self.sequence}"
        if len(database) > 63 or not database.replace("_", "").isalnum():
            raise EvidenceFailure("generated PostgreSQL database name was invalid")
        return database

    def _clone_database(self, source_database: str, target_database: str) -> None:
        if self.provider is None:
            raise EvidenceFailure("PostgreSQL provider was unavailable")
        self.provider._psql(
            "postgres",
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity "
            f"WHERE datname = '{source_database}' AND pid <> pg_backend_pid();",
        )
        self.provider._psql(
            "postgres",
            f'CREATE DATABASE "{target_database}" WITH TEMPLATE '
            f'"{source_database}" OWNER admin;',
        )

    def _write_runtime_identity(self, root: pathlib.Path, database: str) -> None:
        if self.provider is None:
            raise EvidenceFailure("PostgreSQL provider was unavailable")
        pooler = self.connection_mode == "transaction_pool"
        port = self.provider.pooler_port if pooler else self.provider.postgres_port
        if port is None:
            raise EvidenceFailure("PostgreSQL runtime port was unavailable")
        dsn = (
            "host=127.0.0.1 "
            f"port={port} dbname={database} user=runtime "
            f"password={self.provider.runtime_password} connect_timeout=5"
        )
        owner_text(root / "postgresql-runtime.dsn", dsn + "\n")
        owner_text(root / "postgresql-database", database + "\n")
        if self.privacy_capture is not None:
            self.privacy_capture.register_sentinel(
                f"postgres-runtime-dsn-{self.profile_id}-{self.sequence:03d}",
                dsn.encode("ascii"),
            )

    def new_root(
        self, prefix: str, *, source: pathlib.Path | None = None
    ) -> pathlib.Path:
        if self.storage_backend == "sqlite":
            return (
                private_root(prefix) if source is None else clone_root(source, prefix)
            )
        if self.storage_backend != "postgresql" or self.empty_database is None:
            raise EvidenceFailure("unknown or incomplete storage profile")
        authority_secret = (
            None if source is None else (source / "authority.secret").read_bytes()
        )
        root = private_root(prefix, authority_secret)
        try:
            source_database = (
                self.empty_database if source is None else self.database(source)
            )
            target_database = self._next_database()
            self._clone_database(source_database, target_database)
            self._write_runtime_identity(root, target_database)
            return root
        except BaseException:
            shutil.rmtree(root, ignore_errors=True)
            raise

    def store_identity(self, daemon: Daemon) -> tuple[Any, ...]:
        if self.storage_backend == "sqlite":
            value = daemon.database_path.stat()
            return "sqlite", value.st_dev, value.st_ino
        if self.provider is None:
            raise EvidenceFailure("PostgreSQL provider was unavailable")
        database = self.database(daemon.root)
        oid = self.provider._psql(
            "postgres", f"SELECT oid FROM pg_database WHERE datname = '{database}';"
        )
        if not oid.isdigit():
            raise EvidenceFailure("PostgreSQL store identity was unavailable")
        return "postgresql", database, int(oid)

    def creation_witness(self, daemon: Daemon) -> dict[str, Any]:
        if self.storage_backend == "sqlite":
            return offline_create_witness(daemon.database_path)
        if self.provider is None:
            raise EvidenceFailure("PostgreSQL provider was unavailable")
        database = self.database(daemon.root)
        value = self.provider._psql(
            database,
            "BEGIN READ ONLY;"
            "SELECT (SELECT count(*) FROM worldstream_room_roots),"
            "(SELECT count(*) FROM worldstream_genesis),"
            "(SELECT count(*) FROM worldstream_semantic_receipts "
            "WHERE resolution_kind = 'genesis_created' AND transition_seq IS NULL);"
            "ROLLBACK;",
        )
        rows = [line for line in value.splitlines() if line]
        if len(rows) != 1:
            raise EvidenceFailure("PostgreSQL creation witness was invalid")
        fields = rows[0].split("|")
        if len(fields) != 3 or any(not field.isdigit() for field in fields):
            raise EvidenceFailure("PostgreSQL creation witness was invalid")
        return {
            "room_count": int(fields[0]),
            "genesis_count": int(fields[1]),
            "creation_receipt_count": int(fields[2]),
            "observer": "postgresql_direct_admin_read_only_transaction",
        }


@dataclass
class BackendProviderContext:
    root: pathlib.Path
    provider: Any
    privacy_capture: Any
    profiles: tuple[BackendProfile, ...]
    package_binding: dict[str, Any]
    reference_engine: dict[str, Any]
    cleanup_status: str = "not_started"

    def cleanup(self) -> str:
        if self.cleanup_status == "not_started":
            self.cleanup_status = self.provider.cleanup()
            POSTGRES_HARNESS.ACTIVE_PRIVACY_CAPTURE = None
        return self.cleanup_status

    def privacy_evidence(self) -> dict[str, Any]:
        if self.cleanup_status != "pass":
            raise EvidenceFailure("PostgreSQL provider cleanup did not pass")
        if not self.privacy_capture.channels:
            raise EvidenceFailure("PostgreSQL provider privacy channels were empty")
        try:
            scan = SECRET_SCAN.scan_sentinels(
                self.privacy_capture.sentinels, self.privacy_capture.channels
            )
        except SECRET_SCAN.ScanError as error:
            raise EvidenceFailure(
                "PostgreSQL provider channel secret scan failed closed"
            ) from error
        return {
            "status": "pass",
            "secret_scan": scan,
            "channel_classes": [
                {
                    "channel": channel,
                    "class": self.privacy_capture.channel_classes[channel],
                }
                for channel in sorted(self.privacy_capture.channels)
            ],
        }

    def evidence(self) -> dict[str, Any]:
        binaries = self.package_binding.get("binaries", {})
        control = (
            binaries.get("worldstreamctl", {}) if isinstance(binaries, dict) else {}
        )
        postgres_profiles = [
            profile
            for profile in self.profiles
            if profile.storage_backend == "postgresql"
        ]
        expected_sentinels = {
            f"postgres-runtime-dsn-{profile.profile_id}-{sequence:03d}-01"
            for profile in postgres_profiles
            for sequence in range(1, profile.sequence + 1)
        }
        observed_sentinels = {
            name
            for name in self.privacy_capture.sentinels
            if name.startswith("postgres-runtime-dsn-postgresql_")
        }
        if (
            any(profile.sequence != 16 for profile in postgres_profiles)
            or len(expected_sentinels) != 32
            or observed_sentinels != expected_sentinels
        ):
            raise EvidenceFailure(
                "PostgreSQL runtime DSN sentinel coverage was incomplete"
            )
        return {
            "status": "passed",
            "postgres_image": POSTGRES_HARNESS.POSTGRES_IMAGE,
            "postgres_digest": POSTGRES_HARNESS.POSTGRES_DIGEST,
            "pgbouncer_image": POSTGRES_HARNESS.PGBOUNCER_IMAGE,
            "pgbouncer_digest": POSTGRES_HARNESS.PGBOUNCER_DIGEST,
            "engine_identity": POSTGRES_HARNESS.EXPECTED_ENGINE,
            "server_version_num": "170011",
            "pool_mode": "transaction",
            "migration_connection_mode": "direct_admin_offline",
            "observation_connection_mode": "direct_admin",
            "control_binary_sha256": control.get("sha256"),
            "runtime_dsn_sentinel_coverage": {
                "status": "complete",
                "expected_count": len(expected_sentinels),
                "observed_count": len(observed_sentinels),
                "profiles": [
                    {
                        "storage_backend": profile.storage_backend,
                        "connection_mode": profile.connection_mode,
                        "dsn_count": profile.sequence,
                    }
                    for profile in postgres_profiles
                ],
            },
            "cleanup": self.cleanup_status,
            "privacy": self.privacy_evidence(),
        }


def prepare_backend_provider(
    args: argparse.Namespace, distribution: dict[str, Any]
) -> BackendProviderContext:
    if args.package_archive is None or args.package_report is None:
        raise EvidenceFailure(
            "the 36-cell matrix requires a verified packaged Linux archive"
        )
    if not args.docker or not args.psql:
        raise EvidenceFailure("Docker and psql are required for PostgreSQL kill cells")
    root = pathlib.Path(tempfile.mkdtemp(prefix="worldstream-kill-provider-"))
    root.chmod(0o700)
    provider: Any | None = None
    try:
        try:
            extracted_daemon, ctl, binding = POSTGRES_HARNESS._bind_package(
                args.package_archive, args.package_report, root / "verified-package"
            )
        except POSTGRES_HARNESS.LaneFailure as error:
            raise EvidenceFailure(
                "PostgreSQL provider could not bind the packaged binaries"
            ) from error
        if sha256_file(extracted_daemon) != distribution.get("binary_sha256"):
            raise EvidenceFailure(
                "PostgreSQL provider daemon differs from the kill matrix package"
            )
        privacy_capture = POSTGRES_HARNESS.PrivacyCapture(root)
        POSTGRES_HARNESS.ACTIVE_PRIVACY_CAPTURE = privacy_capture
        provider = POSTGRES_HARNESS.Provider.create(
            root, ROOT, args.docker, args.psql, ctl
        )
        provider.start()
        reference_engine = provider.reference_engine()
        if (
            reference_engine.get("version") != "17.11"
            or reference_engine.get("connection_mode")
            != "direct_and_transaction_pooler"
        ):
            raise EvidenceFailure("PostgreSQL reference engine contract was not frozen")
        profiles = (
            BackendProfile("sqlite", "embedded"),
            BackendProfile(
                "postgresql",
                "direct",
                provider,
                privacy_capture,
                POSTGRES_HARNESS.DATABASES["counter_postgres_direct"],
            ),
            BackendProfile(
                "postgresql",
                "transaction_pool",
                provider,
                privacy_capture,
                POSTGRES_HARNESS.DATABASES["counter_transaction_pooler"],
            ),
        )
        return BackendProviderContext(
            root, provider, privacy_capture, profiles, binding, reference_engine
        )
    except BaseException:
        if provider is not None:
            try:
                provider.cleanup()
            except (
                POSTGRES_HARNESS.LaneFailure,
                OSError,
                subprocess.SubprocessError,
            ):
                POSTGRES_HARNESS.ACTIVE_PRIVACY_CAPTURE = None
        POSTGRES_HARNESS.ACTIVE_PRIVACY_CAPTURE = None
        shutil.rmtree(root, ignore_errors=True)
        raise


def loopback_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def status_code(url: str) -> int:
    try:
        with urllib.request.urlopen(url, timeout=1) as response:
            return response.status
    except urllib.error.HTTPError as error:
        return error.code
    except OSError:
        return 0


def post_json(
    base_url: str, bearer: str, path: str, body: dict[str, Any]
) -> dict[str, Any]:
    payload = json.dumps(body, sort_keys=True, separators=(",", ":")).encode()
    request = urllib.request.Request(
        f"{base_url}{path}",
        data=payload,
        method="POST",
        headers={
            "Accept": "application/json",
            "Authorization": f"Bearer {bearer}",
            "Content-Type": "application/json",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            value = strict_json_bytes(
                response.read(MAX_HTTP_JSON_BYTES + 1),
                "public HTTP response",
                MAX_HTTP_JSON_BYTES,
            )
    except urllib.error.HTTPError as error:
        try:
            value = strict_json_bytes(
                error.read(MAX_HTTP_JSON_BYTES + 1),
                "public HTTP error response",
                MAX_HTTP_JSON_BYTES,
            )
        except (OSError, EvidenceFailure) as parse_error:
            raise EvidenceFailure(
                f"public HTTP failed with status {error.code}"
            ) from parse_error
        code = value.get("error", {}).get("code") if isinstance(value, dict) else None
        raise EvidenceFailure(
            f"public HTTP rejected the operation with {code or error.code}"
        ) from error
    return value


def issue_member_capability(
    base_url: str,
    operator_bearer: str,
    room_id: str,
    member_id: str,
    principal_id: str,
) -> str:
    response = post_json(
        base_url,
        operator_bearer,
        "/v1/operator/member-capabilities",
        {
            "room_id": room_id,
            "member_id": member_id,
            "principal_id": principal_id,
            "scopes": ["room:attach", "room:observe_member", "room:act", "room:replay"],
            "idempotency_key": new_ulid(),
            "expires_at": None,
        },
    )
    bearer = response.get("bearer")
    if not isinstance(bearer, str) or not bearer.startswith("wsb1:"):
        raise EvidenceFailure("member capability was not issued")
    return bearer


def issue_runner_capability(
    base_url: str,
    operator_bearer: str,
    room_id: str,
    member_id: str,
    principal_id: str,
    runner_id: str,
) -> str:
    response = post_json(
        base_url,
        operator_bearer,
        "/v1/operator/runner-capabilities",
        {
            "runner_id": runner_id,
            "owner_principal_id": principal_id,
            "permitted_memberships": [{"room_id": room_id, "member_id": member_id}],
            "scopes": [
                "activation:offer_receive",
                "activation:claim",
                "activation:complete",
            ],
            "principal_idempotency_key": new_ulid(),
            "runner_idempotency_key": new_ulid(),
            "capability_idempotency_key": new_ulid(),
            "expires_at": None,
        },
    )
    bearer = response.get("bearer")
    if not isinstance(bearer, str) or not bearer.startswith("wsb1:"):
        raise EvidenceFailure("runner capability was not issued")
    return bearer


@dataclass(frozen=True)
class CrashSpec:
    operation: str
    boundary: str
    match_id: str


class Daemon:
    def __init__(
        self,
        root: pathlib.Path,
        binary: pathlib.Path,
        *,
        startup_timeout: float,
        shutdown_timeout: float,
    ) -> None:
        self.root = root
        self.binary = binary
        self.data_dir = root / "data"
        self.port = loopback_port()
        self.startup_timeout = startup_timeout
        self.shutdown_timeout = shutdown_timeout
        self.process: subprocess.Popen[bytes] | None = None
        self.log_handle: Any = None
        self.start_count = 0

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    @property
    def operator_bearer(self) -> str:
        return "wsb1:" + (self.root / "authority.secret").read_bytes().hex()

    @property
    def database_path(self) -> pathlib.Path:
        return self.data_dir / "worldstream.sqlite3"

    def start(self, crash: CrashSpec | None = None) -> None:
        if self.process is not None and self.process.poll() is None:
            raise EvidenceFailure("attempted to start an already running daemon")
        self.data_dir.mkdir(mode=0o700, exist_ok=True)
        self.start_count += 1
        log_path = self.root / f"worldstreamd-{self.start_count}.log"
        self.log_handle = log_path.open("ab")
        environment = {
            **os.environ,
            "WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE": str(
                self.root / "authority.secret"
            ),
            "RUST_LOG": "warn",
        }
        environment.pop("WORLDSTREAM__STORAGE__POSTGRESQL__DSN", None)
        environment.pop("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_HANDLE", None)
        runtime_dsn = self.root / "postgresql-runtime.dsn"
        if runtime_dsn.exists():
            if runtime_dsn.is_symlink() or not runtime_dsn.is_file():
                raise EvidenceFailure("PostgreSQL runtime DSN source was invalid")
            environment["WORLDSTREAM__STORAGE__PROFILE"] = "postgres-primary"
            environment["WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE"] = str(runtime_dsn)
        else:
            environment["WORLDSTREAM__STORAGE__PROFILE"] = "sqlite-bundled"
            environment.pop("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE", None)
        for name in (
            "WORLDSTREAM_TEST_CRASH_EVIDENCE_ENABLE",
            "WORLDSTREAM_TEST_CRASH_EVIDENCE_POINT",
            "WORLDSTREAM_TEST_CRASH_EVIDENCE_MATCH_ID",
            "WORLDSTREAM_TEST_CRASH_EVIDENCE_MARKER",
        ):
            environment.pop(name, None)
        if crash is not None:
            marker = self.root / "crash-marker.json"
            marker.unlink(missing_ok=True)
            environment.update(
                {
                    "WORLDSTREAM_TEST_CRASH_EVIDENCE_ENABLE": ENABLE_VALUE,
                    "WORLDSTREAM_TEST_CRASH_EVIDENCE_POINT": (
                        f"{crash.operation}:{crash.boundary}"
                    ),
                    "WORLDSTREAM_TEST_CRASH_EVIDENCE_MATCH_ID": crash.match_id,
                    "WORLDSTREAM_TEST_CRASH_EVIDENCE_MARKER": str(marker),
                }
            )
        self.process = subprocess.Popen(
            [
                str(self.binary),
                "--data-dir",
                str(self.data_dir),
                "--bind",
                f"127.0.0.1:{self.port}",
            ],
            cwd=ROOT,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=self.log_handle,
            stderr=subprocess.STDOUT,
        )
        deadline = time.monotonic() + self.startup_timeout
        while time.monotonic() < deadline:
            if status_code(f"{self.base_url}/readyz") == 200:
                return
            if self.process.poll() is not None:
                raise EvidenceFailure("worldstreamd exited before readiness")
            time.sleep(0.05)
        raise EvidenceFailure(
            "worldstreamd did not become ready within the startup bound"
        )

    def kill_at_marker(self, crash: CrashSpec, timeout: float) -> dict[str, Any]:
        process = self.process
        if process is None:
            raise EvidenceFailure("no daemon is running for the crash boundary")
        marker_path = self.root / "crash-marker.json"
        deadline = time.monotonic() + timeout
        marker: dict[str, Any] | None = None
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise EvidenceFailure("daemon exited before the exact crash marker")
            try:
                marker_stat = marker_path.lstat()
                marker_bytes = marker_path.read_bytes()
                if len(marker_bytes) > MAX_MARKER_JSON_BYTES:
                    raise EvidenceFailure("crash marker exceeded its JSON bound")
                try:
                    value = strict_json_bytes(
                        marker_bytes,
                        "crash marker",
                        MAX_MARKER_JSON_BYTES,
                    )
                except EvidenceFailure:
                    time.sleep(0.01)
                    continue
            except (FileNotFoundError, OSError):
                time.sleep(0.01)
                continue
            if (
                not stat.S_ISREG(marker_stat.st_mode)
                or stat.S_IMODE(marker_stat.st_mode) != 0o600
            ):
                raise EvidenceFailure("crash marker was not an owner-only regular file")
            if marker_stat.st_uid != os.getuid():
                raise EvidenceFailure("crash marker owner did not match the harness")
            marker = value if isinstance(value, dict) else None
            break
        if marker is None:
            raise EvidenceFailure(
                "exact crash marker did not appear within the request bound"
            )
        expected = {
            "schema": MARKER_SCHEMA,
            "operation": crash.operation,
            "boundary": crash.boundary,
            "match_id": crash.match_id,
            "publication_contract": {
                "room_create": "telemetry_only_no_preexisting_room_observer",
                "action": "telemetry_and_live_room_frames",
                "timer": "telemetry_and_live_room_frames",
                "activation_lease": "activation_telemetry_no_room_frame",
            }[crash.operation],
        }
        if marker != expected:
            raise EvidenceFailure(
                "crash marker identity did not match the requested cell"
            )
        os.kill(process.pid, signal.SIGKILL)
        try:
            exit_code = process.wait(timeout=self.shutdown_timeout)
        except subprocess.TimeoutExpired as error:
            raise EvidenceFailure("daemon did not exit after SIGKILL") from error
        if exit_code != -signal.SIGKILL:
            raise EvidenceFailure("daemon exit status was not SIGKILL")
        self.process = None
        self._close_log()
        return {
            "signal": "SIGKILL",
            "signal_sent": True,
            "process_exit_observed": True,
            "marker_verified": True,
            "marker_sha256": canonical_hash(marker),
        }

    def stop(self) -> None:
        process = self.process
        self.process = None
        if process is not None and process.poll() is None:
            process.send_signal(signal.SIGTERM)
            try:
                process.wait(timeout=self.shutdown_timeout)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=self.shutdown_timeout)
        self._close_log()

    def _close_log(self) -> None:
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None


@dataclass(frozen=True)
class CounterTemplate:
    root: pathlib.Path
    room_id: str
    member_id: str
    member_bearer: str


@dataclass(frozen=True)
class HeistTemplate:
    root: pathlib.Path
    room_id: str
    member_ids: tuple[str, str, str]
    member_bearers: tuple[str, str]
    runner_id: str
    runner_bearer: str
    route_claim: str
    entry_claim: str
    timer_due_monotonic: float


async def projection_with_retry(
    client: Client, room_id: str, timeout: float
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while True:
        try:
            return await client.projection(room_id)
        except ProtocolError as error:
            if (
                error.code != "room_busy"
                or not error.retryable
                or time.monotonic() >= deadline
            ):
                raise
            await asyncio.sleep(0.1)


async def open_room_with_retry(
    client: Client, room_id: str, member_id: str, timeout: float
) -> Any:
    deadline = time.monotonic() + timeout
    while True:
        try:
            room = await client.open_room(room_id, member_id)
            await room.sync()
            return room
        except ProtocolError as error:
            if (
                error.code != "room_busy"
                or not error.retryable
                or time.monotonic() >= deadline
            ):
                raise
            await asyncio.sleep(0.1)


async def act_once(
    client: Client,
    room_id: str,
    member_id: str,
    action_type: str,
    payload: dict[str, Any],
) -> dict[str, Any]:
    action_id = new_ulid()
    projection = await projection_with_retry(client, room_id, 15)
    expected_room_seq = projection.get("room_head", {}).get("room_seq")
    if not isinstance(expected_room_seq, int):
        raise EvidenceFailure("activation setup projection had no Room sequence")
    deadline = time.monotonic() + 15
    while True:
        room = await open_room_with_retry(client, room_id, member_id, 15)
        try:
            return await room.act(
                action_type,
                payload,
                action_id=action_id,
                expected_room_seq=expected_room_seq,
            )
        except ProtocolError as error:
            if (
                error.code not in {"room_busy", "internal"}
                or time.monotonic() >= deadline
            ):
                raise
        finally:
            await room.close()
        await asyncio.sleep(0.1)


def private_claim(projection: dict[str, Any], clue_id: str) -> str:
    clues = projection.get("projection", {}).get("activity", {}).get("private_clues")
    if not isinstance(clues, list):
        raise EvidenceFailure("private clue projection was unavailable")
    for clue in clues:
        if isinstance(clue, dict) and clue.get("clue_id") == clue_id:
            value = clue.get("claim_code")
            if isinstance(value, str) and value:
                return value
    raise EvidenceFailure("expected private clue was unavailable")


async def prepare_counter_template(
    profile: BackendProfile,
    binary: pathlib.Path,
    startup: float,
    shutdown: float,
) -> CounterTemplate:
    root = profile.new_root(f"worldstream-kill-{profile.profile_id}-counter-template-")
    daemon = Daemon(root, binary, startup_timeout=startup, shutdown_timeout=shutdown)
    try:
        daemon.start()
        operator = Client(daemon.base_url, daemon.operator_bearer)
        principal_id = new_ulid()
        created = await operator.create_room(
            {
                "pack": COUNTER_PACK,
                "configuration": {"initial_value": 0, "maximum_value": 16},
                "members": [
                    {
                        "principal_id": principal_id,
                        "principal_kind": "agent",
                        "role": "counter",
                        "access_mode": "participant",
                    }
                ],
                "idempotency_key": new_ulid(),
            }
        )
        member_id = created["member_ids"][0]
        member_bearer = await asyncio.to_thread(
            issue_member_capability,
            daemon.base_url,
            daemon.operator_bearer,
            created["room_id"],
            member_id,
            principal_id,
        )
        daemon.stop()
        return CounterTemplate(root, created["room_id"], member_id, member_bearer)
    except BaseException:
        daemon.stop()
        shutil.rmtree(root, ignore_errors=True)
        raise


async def prepare_heist_template(
    profile: BackendProfile,
    binary: pathlib.Path,
    startup: float,
    shutdown: float,
) -> HeistTemplate:
    root = profile.new_root(f"worldstream-kill-{profile.profile_id}-heist-template-")
    daemon = Daemon(root, binary, startup_timeout=startup, shutdown_timeout=shutdown)
    try:
        daemon.start()
        operator = Client(daemon.base_url, daemon.operator_bearer)
        principals = (new_ulid(), new_ulid(), new_ulid())
        created = await operator.create_room(
            {
                "pack": HEIST_PACK,
                "configuration": HEIST_CONFIG,
                "members": [
                    {
                        "principal_id": principals[index],
                        "principal_kind": "agent",
                        "role": role,
                        "access_mode": "participant",
                    }
                    for index, role in enumerate(("navigator", "insider", "broker"))
                ],
                "idempotency_key": new_ulid(),
            }
        )
        timer_due = time.monotonic() + 30.25
        members = tuple(created["member_ids"])
        if len(members) != 3:
            raise EvidenceFailure("Heist template did not create three memberships")
        navigator_bearer = await asyncio.to_thread(
            issue_member_capability,
            daemon.base_url,
            daemon.operator_bearer,
            created["room_id"],
            members[0],
            principals[0],
        )
        insider_bearer = await asyncio.to_thread(
            issue_member_capability,
            daemon.base_url,
            daemon.operator_bearer,
            created["room_id"],
            members[1],
            principals[1],
        )
        runner_id = new_ulid()
        runner_bearer = await asyncio.to_thread(
            issue_runner_capability,
            daemon.base_url,
            daemon.operator_bearer,
            created["room_id"],
            members[2],
            principals[2],
            runner_id,
        )
        navigator = Client(daemon.base_url, navigator_bearer)
        insider = Client(daemon.base_url, insider_bearer)
        await act_once(
            navigator,
            created["room_id"],
            members[0],
            "inspect_clue",
            {"clue_id": "route"},
        )
        await act_once(
            insider,
            created["room_id"],
            members[1],
            "inspect_clue",
            {"clue_id": "entry_window"},
        )
        nav_projection = await navigator.projection(created["room_id"])
        insider_projection = await insider.projection(created["room_id"])
        route_claim = private_claim(nav_projection, "route")
        entry_claim = private_claim(insider_projection, "entry_window")
        daemon.stop()
        return HeistTemplate(
            root,
            created["room_id"],
            (members[0], members[1], members[2]),
            (navigator_bearer, insider_bearer),
            runner_id,
            runner_bearer,
            route_claim,
            entry_claim,
            timer_due,
        )
    except BaseException:
        daemon.stop()
        shutil.rmtree(root, ignore_errors=True)
        raise


async def crash_request(
    request: Awaitable[Any],
    daemon: Daemon,
    crash: CrashSpec,
    timeout: float,
    expected_exception: type[BaseException] | tuple[type[BaseException], ...],
) -> tuple[BaseException, dict[str, Any]]:
    task = asyncio.ensure_future(request)
    kill_result = await asyncio.to_thread(daemon.kill_at_marker, crash, timeout)
    try:
        value = await asyncio.wait_for(task, timeout)
    except expected_exception as error:
        return error, kill_result
    except BaseException:
        raise
    raise EvidenceFailure(
        f"{crash.operation} returned a reply before the configured SIGKILL: {value!r}"
    )


def store_identity(
    profile: BackendProfile | Daemon, daemon: Daemon | None = None
) -> tuple[Any, ...]:
    """Return durable identity while preserving the SQLite helper call shape."""

    if daemon is None:
        daemon = profile
        if not isinstance(daemon, Daemon):
            raise EvidenceFailure("durable store identity input was invalid")
        return BackendProfile("sqlite", "embedded").store_identity(daemon)
    if not isinstance(profile, BackendProfile):
        raise EvidenceFailure("durable store profile was invalid")
    return profile.store_identity(daemon)


def offline_create_witness(database_path: pathlib.Path) -> dict[str, int]:
    """Observe the closed store read-only without claiming engine conformance."""

    wal_path = pathlib.Path(f"{database_path}-wal")
    protected_paths = [path for path in (database_path, wal_path) if path.exists()]
    before = {str(path): sha256_file(path) for path in protected_paths}
    encoded_path = urllib.parse.quote(database_path.resolve().as_posix(), safe="/")
    uri = f"file:{encoded_path}?mode=ro"
    connection = sqlite3.connect(uri, uri=True)
    try:
        connection.execute("PRAGMA query_only = ON")
        room_count = int(connection.execute("SELECT count(*) FROM rooms").fetchone()[0])
        genesis_count = int(
            connection.execute("SELECT count(*) FROM room_genesis").fetchone()[0]
        )
        receipt_count = int(
            connection.execute(
                "SELECT count(*) FROM semantic_receipts "
                "WHERE resolution_kind = 'genesis_created' AND transition_seq IS NULL"
            ).fetchone()[0]
        )
    finally:
        connection.close()
    after = {str(path): sha256_file(path) for path in protected_paths}
    if before != after:
        raise EvidenceFailure(
            "read-only create witness changed SQLite main or WAL bytes"
        )
    return {
        "room_count": room_count,
        "genesis_count": genesis_count,
        "creation_receipt_count": receipt_count,
    }


def receipt_core(value: dict[str, Any], operation: str) -> dict[str, Any]:
    if operation == "room_create":
        return value
    if operation == "action":
        return {
            "action_id": value.get("action_id"),
            "transition_id": value.get("transition_id"),
            "room_head": value.get("room_head"),
        }
    if operation == "timer":
        return {
            "room_id": value.get("room_id"),
            "timer_id": value.get("timer_id"),
            "generation": value.get("generation"),
            "transition_id": value.get("transition_id"),
            "room_head": value.get("room_head"),
        }
    if operation == "activation_lease":
        return value
    raise EvidenceFailure("unknown operation receipt")


def completed_cell(
    profile: BackendProfile,
    crash: CrashSpec,
    kill_result: dict[str, Any],
    before_store: tuple[Any, ...],
    after_store: tuple[Any, ...],
    verification: dict[str, Any],
) -> dict[str, Any]:
    if before_store != after_store:
        raise EvidenceFailure("restart did not preserve the durable store identity")
    if not all(
        value is True for value in verification.values() if isinstance(value, bool)
    ):
        raise EvidenceFailure(
            f"{crash.operation}/{crash.boundary} outcome verification failed"
        )
    return {
        "storage_backend": profile.storage_backend,
        "connection_mode": profile.connection_mode,
        "operation": crash.operation,
        "name": crash.boundary,
        "signal": kill_result["signal"],
        "status": "passed",
        "signal_sent": kill_result["signal_sent"],
        "process_exit_observed": kill_result["process_exit_observed"],
        "marker_verified": kill_result["marker_verified"],
        "marker_sha256": kill_result["marker_sha256"],
        "restart_status": "passed",
        "same_data_directory": True,
        "expected_outcome": EXPECTED_OUTCOME[crash.boundary],
        "outcome_verified": True,
        "reply_resolution_status": "passed",
        "verification": verification,
    }


async def room_create_cell(
    profile: BackendProfile,
    boundary: str,
    binary: pathlib.Path,
    startup: float,
    request_timeout: float,
    shutdown: float,
) -> tuple[dict[str, Any], float, pathlib.Path]:
    root = profile.new_root(f"worldstream-kill-{profile.profile_id}-create-{boundary}-")
    daemon = Daemon(root, binary, startup_timeout=startup, shutdown_timeout=shutdown)
    request = {
        "pack": COUNTER_PACK,
        "configuration": {"initial_value": 0, "maximum_value": 16},
        "members": [
            {
                "principal_id": new_ulid(),
                "principal_kind": "agent",
                "role": "counter",
                "access_mode": "participant",
            }
        ],
        "idempotency_key": new_ulid(),
    }
    crash = CrashSpec("room_create", boundary, request["idempotency_key"])
    started = time.monotonic()
    succeeded = False
    try:
        daemon.start(crash)
        before_store = store_identity(profile, daemon)
        operator = Client(daemon.base_url, daemon.operator_bearer)
        _, kill_result = await crash_request(
            operator.create_room(request),
            daemon,
            crash,
            request_timeout,
            ProtocolError,
        )
        # Reopen once to let the production adapter perform normal WAL
        # recovery, then close it before the offline read-only SQLite observer.
        daemon.start()
        after_store = store_identity(profile, daemon)
        daemon.stop()
        witness = profile.creation_witness(daemon)
        expected_count = 0 if boundary == "before_commit" else 1
        daemon.start()
        operator = Client(daemon.base_url, daemon.operator_bearer)
        first = await operator.create_room(request)
        second = await operator.create_room(request)
        verification = {
            "exact_request_retried": True,
            "two_retry_results_equal": first == second,
            "genesis_room_seq_zero": first.get("room_head", {}).get("room_seq") == 0,
            "pre_retry_room_count_matches_boundary": witness["room_count"]
            == expected_count,
            "pre_retry_genesis_count_matches_boundary": witness["genesis_count"]
            == expected_count,
            "pre_retry_creation_receipt_count_matches_boundary": witness[
                "creation_receipt_count"
            ]
            == expected_count,
            "offline_observer_nonmutation_hash_check": True,
            "offline_observer": witness.get(
                "observer", f"sqlite/{sqlite3.sqlite_version}/read_only"
            ),
            "receipt_hash": canonical_hash(first),
            "boundary_marker_precedes_signal": True,
        }
        cell = completed_cell(
            profile, crash, kill_result, before_store, after_store, verification
        )
        succeeded = True
        return cell, (time.monotonic() - started) * 1000, root
    finally:
        daemon.stop()
        if not succeeded:
            shutil.rmtree(root, ignore_errors=True)


async def action_cell(
    profile: BackendProfile,
    boundary: str,
    template: CounterTemplate,
    binary: pathlib.Path,
    startup: float,
    request_timeout: float,
    shutdown: float,
) -> tuple[dict[str, Any], float, pathlib.Path]:
    root = profile.new_root(
        f"worldstream-kill-{profile.profile_id}-action-{boundary}-",
        source=template.root,
    )
    daemon = Daemon(root, binary, startup_timeout=startup, shutdown_timeout=shutdown)
    action_id = new_ulid()
    crash = CrashSpec("action", boundary, action_id)
    started = time.monotonic()
    room = None
    succeeded = False
    try:
        daemon.start(crash)
        before_store = store_identity(profile, daemon)
        client = Client(daemon.base_url, template.member_bearer)
        before = await client.projection(template.room_id)
        room = await open_room_with_retry(
            client, template.room_id, template.member_id, 15
        )
        lost, kill_result = await crash_request(
            room.act("increment", {}, action_id=action_id, expected_room_seq=0),
            daemon,
            crash,
            request_timeout,
            LostActionReply,
        )
        if not isinstance(lost, LostActionReply):
            raise EvidenceFailure("lost Action did not retain its exact request")
        await room.close()
        room = None
        daemon.start()
        after_store = store_identity(profile, daemon)
        client = Client(daemon.base_url, template.member_bearer)
        before_retry = await projection_with_retry(client, template.room_id, 15)
        retry_room = await open_room_with_retry(
            client, template.room_id, template.member_id, 15
        )
        try:
            first = await retry_room.retry_action(lost.request)
            second = await retry_room.retry_action(lost.request)
        finally:
            await retry_room.close()
        final = await client.projection(template.room_id)
        expected_committed = boundary != "before_commit"
        verification = {
            "exact_request_retried": True,
            "pre_retry_state_matches_boundary": (
                before_retry["room_head"]["room_seq"]
                == before["room_head"]["room_seq"] + int(expected_committed)
            ),
            "first_retry_duplicate_matches_boundary": first.get("duplicate")
            is expected_committed,
            "second_retry_is_duplicate": second.get("duplicate") is True,
            "durable_receipt_equal": receipt_core(first, "action")
            == receipt_core(second, "action"),
            "final_transition_applied_once": final["room_head"]["room_seq"]
            == before["room_head"]["room_seq"] + 1,
            "receipt_hash": canonical_hash(receipt_core(first, "action")),
        }
        cell = completed_cell(
            profile, crash, kill_result, before_store, after_store, verification
        )
        succeeded = True
        return cell, (time.monotonic() - started) * 1000, root
    finally:
        if room is not None:
            await room.close()
        daemon.stop()
        if not succeeded:
            shutil.rmtree(root, ignore_errors=True)


async def timer_cell(
    profile: BackendProfile,
    boundary: str,
    template: HeistTemplate,
    binary: pathlib.Path,
    startup: float,
    request_timeout: float,
    shutdown: float,
) -> tuple[dict[str, Any], float, pathlib.Path]:
    root = profile.new_root(
        f"worldstream-kill-{profile.profile_id}-timer-{boundary}-",
        source=template.root,
    )
    daemon = Daemon(root, binary, startup_timeout=startup, shutdown_timeout=shutdown)
    match_id = f"{HEIST_TIMER_ID}#1"
    crash = CrashSpec("timer", boundary, match_id)
    request = {"timer_id": HEIST_TIMER_ID, "generation": 1}
    started = time.monotonic()
    succeeded = False
    try:
        daemon.start(crash)
        before_store = store_identity(profile, daemon)
        member = Client(daemon.base_url, template.member_bearers[0])
        before = await member.projection(template.room_id)
        # The before-commit seam precedes the adapter's due-time validation,
        # so that cell can be killed and restarted before the obligation is
        # due. Post-commit seams must wait for the real scheduled deadline.
        if boundary != "before_commit":
            remaining = template.timer_due_monotonic - time.monotonic()
            if remaining > 0:
                await asyncio.sleep(remaining)
        operation = asyncio.to_thread(
            post_json,
            daemon.base_url,
            daemon.operator_bearer,
            f"/v1/operator/rooms/{template.room_id}/timers/fire",
            request,
        )
        _, kill_result = await crash_request(
            operation,
            daemon,
            crash,
            request_timeout,
            (urllib.error.URLError, OSError, EvidenceFailure),
        )
        daemon.start()
        after_store = store_identity(profile, daemon)
        member = Client(daemon.base_url, template.member_bearers[0])
        before_retry = await projection_with_retry(member, template.room_id, 15)
        remaining = template.timer_due_monotonic - time.monotonic()
        if remaining > 0:
            await asyncio.sleep(remaining)
        first = await asyncio.to_thread(
            post_json,
            daemon.base_url,
            daemon.operator_bearer,
            f"/v1/operator/rooms/{template.room_id}/timers/fire",
            request,
        )
        second = await asyncio.to_thread(
            post_json,
            daemon.base_url,
            daemon.operator_bearer,
            f"/v1/operator/rooms/{template.room_id}/timers/fire",
            request,
        )
        final = await member.projection(template.room_id)
        expected_committed = boundary != "before_commit"
        verification = {
            "exact_request_retried": True,
            "pre_retry_state_matches_boundary": (
                before_retry["room_head"]["room_seq"]
                == before["room_head"]["room_seq"] + int(expected_committed)
            ),
            "first_retry_duplicate_matches_boundary": first.get("duplicate")
            is expected_committed,
            "second_retry_is_duplicate": second.get("duplicate") is True,
            "durable_receipt_equal": receipt_core(first, "timer")
            == receipt_core(second, "timer"),
            "final_transition_applied_once": final["room_head"]["room_seq"]
            == before["room_head"]["room_seq"] + 1,
            "receipt_hash": canonical_hash(receipt_core(first, "timer")),
        }
        cell = completed_cell(
            profile, crash, kill_result, before_store, after_store, verification
        )
        succeeded = True
        return cell, (time.monotonic() - started) * 1000, root
    finally:
        daemon.stop()
        if not succeeded:
            shutil.rmtree(root, ignore_errors=True)


async def prepare_activation_template(
    profile: BackendProfile,
    template: HeistTemplate,
    binary: pathlib.Path,
    startup: float,
    shutdown: float,
) -> pathlib.Path:
    root = profile.new_root(
        f"worldstream-kill-{profile.profile_id}-activation-template-",
        source=template.root,
    )
    daemon = Daemon(root, binary, startup_timeout=startup, shutdown_timeout=shutdown)
    try:
        daemon.start()
        remaining = template.timer_due_monotonic - time.monotonic()
        if remaining > 0:
            await asyncio.sleep(remaining)
        await asyncio.to_thread(
            post_json,
            daemon.base_url,
            daemon.operator_bearer,
            f"/v1/operator/rooms/{template.room_id}/timers/fire",
            {"timer_id": HEIST_TIMER_ID, "generation": 1},
        )
        navigator = Client(daemon.base_url, template.member_bearers[0])
        insider = Client(daemon.base_url, template.member_bearers[1])
        await act_once(
            navigator,
            template.room_id,
            template.member_ids[0],
            "publish_clue",
            {"clue_id": "route", "claim_code": template.route_claim},
        )
        await act_once(
            insider,
            template.room_id,
            template.member_ids[1],
            "publish_clue",
            {"clue_id": "entry_window", "claim_code": template.entry_claim},
        )
        await act_once(
            navigator,
            template.room_id,
            template.member_ids[0],
            "propose_plan",
            {
                "route": "service",
                "entry_window": "early",
                "required_tool": "thermal_key",
                "extraction": "boat",
            },
        )
        runner_client = Client(daemon.base_url, template.runner_bearer)
        runner = await runner_client.open_runner(
            template.runner_id, 1, [HEIST_PACK["id"]]
        )
        offers = await runner.poll_offers(template.room_id, template.member_ids[2])
        await runner.close()
        if not offers.get("offers"):
            raise EvidenceFailure("activation template did not create a durable offer")
        daemon.stop()
        return root
    except BaseException:
        daemon.stop()
        shutil.rmtree(root, ignore_errors=True)
        raise


async def verify_overdue_timer_restart(
    profile: BackendProfile,
    template: HeistTemplate,
    binary: pathlib.Path,
    startup: float,
    shutdown: float,
) -> tuple[dict[str, Any], pathlib.Path]:
    """Prove a closed overdue store starts gated and drains exactly once."""

    root = profile.new_root(
        f"worldstream-{profile.profile_id}-overdue-recovery-", source=template.root
    )
    daemon = Daemon(root, binary, startup_timeout=startup, shutdown_timeout=shutdown)
    succeeded = False
    try:
        remaining = template.timer_due_monotonic - time.monotonic()
        if remaining > 0:
            await asyncio.sleep(remaining)
        before_store = store_identity(profile, daemon)
        daemon.start()
        member = Client(daemon.base_url, template.member_bearers[0])
        try:
            await member.projection(template.room_id)
        except ProtocolError as error:
            ordinary_work_gated = error.code == "room_busy" and error.retryable
        else:
            ordinary_work_gated = False
        if not ordinary_work_gated:
            raise EvidenceFailure(
                "overdue restart did not gate ordinary Room work during recovery"
            )
        request = {"timer_id": HEIST_TIMER_ID, "generation": 1}
        first = await asyncio.to_thread(
            post_json,
            daemon.base_url,
            daemon.operator_bearer,
            f"/v1/operator/rooms/{template.room_id}/timers/fire",
            request,
        )
        second = await asyncio.to_thread(
            post_json,
            daemon.base_url,
            daemon.operator_bearer,
            f"/v1/operator/rooms/{template.room_id}/timers/fire",
            request,
        )
        first_receipt = receipt_core(first, "timer")
        second_receipt = receipt_core(second, "timer")
        if (
            first.get("duplicate") is not False
            or second.get("duplicate") is not True
            or first_receipt != second_receipt
        ):
            raise EvidenceFailure(
                "overdue recovery Timer did not preserve its exact retry outcome"
            )
        recovered = await projection_with_retry(member, template.room_id, 15)
        recovered_hash = canonical_hash(recovered)
        daemon.stop()
        daemon.start()
        after_store = store_identity(profile, daemon)
        restarted_member = Client(daemon.base_url, template.member_bearers[0])
        restarted = await projection_with_retry(restarted_member, template.room_id, 15)
        restarted_hash = canonical_hash(restarted)
        if before_store != after_store or recovered_hash != restarted_hash:
            raise EvidenceFailure(
                "overdue Timer recovery changed store identity or replay projection"
            )
        result = {
            "status": "passed",
            "storage_backend": profile.storage_backend,
            "connection_mode": profile.connection_mode,
            "durable_state": "catching_up_with_overdue_timer",
            "daemon_ready": True,
            "ordinary_work_gated_before_drain": True,
            "exact_timer_retry_drained": True,
            "first_result_duplicate": False,
            "second_result_duplicate": True,
            "receipt_hash_equal": True,
            "receipt_hash": canonical_hash(first_receipt),
            "projection_hash_equal_after_restart": True,
            "projection_hash": recovered_hash,
            "same_data_directory": True,
        }
        succeeded = True
        return result, root
    finally:
        daemon.stop()
        if not succeeded:
            shutil.rmtree(root, ignore_errors=True)


async def runner_retry_with_room_busy(
    runner: Any, request: dict[str, Any]
) -> dict[str, Any]:
    deadline = time.monotonic() + 15
    while True:
        try:
            return await runner.retry(request)
        except ProtocolError as error:
            if (
                error.code != "room_busy"
                or not error.retryable
                or time.monotonic() >= deadline
            ):
                raise
            await asyncio.sleep(0.1)


async def activation_cell(
    profile: BackendProfile,
    boundary: str,
    activation_template: pathlib.Path,
    template: HeistTemplate,
    binary: pathlib.Path,
    startup: float,
    request_timeout: float,
    shutdown: float,
) -> tuple[dict[str, Any], float, pathlib.Path]:
    root = profile.new_root(
        f"worldstream-kill-{profile.profile_id}-activation-{boundary}-",
        source=activation_template,
    )
    daemon = Daemon(root, binary, startup_timeout=startup, shutdown_timeout=shutdown)
    claim_id = new_ulid()
    crash = CrashSpec("activation_lease", boundary, claim_id)
    started = time.monotonic()
    runner = None
    succeeded = False
    try:
        daemon.start(crash)
        before_store = store_identity(profile, daemon)
        runner_client = Client(daemon.base_url, template.runner_bearer)
        runner = await runner_client.open_runner(
            template.runner_id, 1, [HEIST_PACK["id"]]
        )
        offers = await runner.poll_offers(template.room_id, template.member_ids[2])
        pending = offers.get("offers")
        if not isinstance(pending, list) or not pending:
            raise EvidenceFailure(
                "activation offer was unavailable before the claim cell"
            )
        activation_id = pending[0].get("activation_id")
        if not isinstance(activation_id, str):
            raise EvidenceFailure("activation offer identity was unavailable")
        lost, kill_result = await crash_request(
            runner.claim(activation_id, 30_000, claim_id=claim_id),
            daemon,
            crash,
            request_timeout,
            LostRunnerReply,
        )
        if not isinstance(lost, LostRunnerReply):
            raise EvidenceFailure(
                "lost Activation claim did not retain its exact request"
            )
        await runner.close()
        runner = None
        daemon.start()
        after_store = store_identity(profile, daemon)
        runner_client = Client(daemon.base_url, template.runner_bearer)
        runner = await runner_client.open_runner(
            template.runner_id, 1, [HEIST_PACK["id"]]
        )
        after_offers = await runner.poll_offers(
            template.room_id, template.member_ids[2]
        )
        offered_ids = {
            item.get("activation_id")
            for item in after_offers.get("offers", [])
            if isinstance(item, dict)
        }
        first = await runner_retry_with_room_busy(runner, lost.request)
        second = await runner_retry_with_room_busy(runner, lost.request)
        expected_committed = boundary != "before_commit"
        verification = {
            "exact_request_retried": True,
            "offer_visibility_matches_boundary": (
                (activation_id not in offered_ids) is expected_committed
            ),
            "claim_granted": first.get("code") == "granted",
            "duplicate_claim_result_equal": receipt_core(first, "activation_lease")
            == receipt_core(second, "activation_lease"),
            "claim_identity_equal": first.get("claim_id") == claim_id,
            "receipt_hash": canonical_hash(receipt_core(first, "activation_lease")),
        }
        cell = completed_cell(
            profile, crash, kill_result, before_store, after_store, verification
        )
        succeeded = True
        return cell, (time.monotonic() - started) * 1000, root
    finally:
        if runner is not None:
            await runner.close()
        daemon.stop()
        if not succeeded:
            shutil.rmtree(root, ignore_errors=True)


def atomic_write(path: pathlib.Path, text: str) -> None:
    path = path.resolve()
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.is_symlink():
        raise EvidenceFailure("output path must not be a symlink")
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            output.write(text)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        pathlib.Path(temporary).unlink(missing_ok=True)


def safe_log(cells: list[dict[str, Any]], durations: list[float]) -> str:
    lines = ["worldstream Linux process kill matrix v1"]
    for cell, duration in zip(cells, durations, strict=True):
        lines.append(
            " ".join(
                (
                    f"storage_backend={cell['storage_backend']}",
                    f"connection_mode={cell['connection_mode']}",
                    f"operation={cell['operation']}",
                    f"boundary={cell['name']}",
                    "signal=SIGKILL",
                    "restart=same_data_directory",
                    f"expected={cell['expected_outcome']}",
                    f"duration_ms={duration:.3f}",
                    "status=passed",
                )
            )
        )
    return "\n".join(lines) + "\n"


def scan_daemon_logs(
    roots: list[pathlib.Path], extra_sentinels: dict[str, bytes] | None = None
) -> dict[str, Any]:
    sentinels: dict[str, bytes] = {}
    channels: dict[str, pathlib.Path] = {}
    unique_roots = sorted({root for root in roots if root.is_dir()})
    for index, root in enumerate(unique_roots, start=1):
        secret = root / "authority.secret"
        if secret.is_symlink() or not secret.is_file() or secret.stat().st_size != 32:
            raise EvidenceFailure("kill-point authority sentinel was unavailable")
        value = secret.read_bytes()
        sentinels[f"authority-secret-{index:02d}"] = value
        sentinels[f"operator-capability-{index:02d}"] = b"wsb1:" + value.hex().encode(
            "ascii"
        )
    for name, value in sorted((extra_sentinels or {}).items()):
        sentinels[f"provider-{name}"] = value
    log_index = 0
    for root in unique_roots:
        for path in sorted(root.glob("worldstreamd-*.log")):
            log_index += 1
            channels[f"daemon-log-{log_index:03d}"] = path
    if not channels:
        raise EvidenceFailure("kill-point daemon log inventory was empty")
    try:
        return SECRET_SCAN.scan_sentinels(sentinels, channels)
    except SECRET_SCAN.ScanError as error:
        raise EvidenceFailure(
            "kill-point daemon log secret scan failed closed"
        ) from error


async def run_profile(
    profile: BackendProfile,
    args: argparse.Namespace,
    templates: list[pathlib.Path],
    cell_roots: list[pathlib.Path],
) -> tuple[list[dict[str, Any]], list[float], dict[str, Any]]:
    cells: list[dict[str, Any]] = []
    durations: list[float] = []
    counter = await prepare_counter_template(
        profile,
        args.daemon_bin,
        args.startup_timeout_seconds,
        args.shutdown_timeout_seconds,
    )
    templates.append(counter.root)
    for boundary in BOUNDARIES:
        cell, duration, root = await room_create_cell(
            profile,
            boundary,
            args.daemon_bin,
            args.startup_timeout_seconds,
            args.request_timeout_seconds,
            args.shutdown_timeout_seconds,
        )
        cells.append(cell)
        durations.append(duration)
        cell_roots.append(root)
    for boundary in BOUNDARIES:
        cell, duration, root = await action_cell(
            profile,
            boundary,
            counter,
            args.daemon_bin,
            args.startup_timeout_seconds,
            args.request_timeout_seconds,
            args.shutdown_timeout_seconds,
        )
        cells.append(cell)
        durations.append(duration)
        cell_roots.append(root)

    # All Timer and Activation precondition clones start before the same public
    # deadline. Each cell receives its own closed SQLite copy or direct-admin
    # PostgreSQL template clone before the runtime connection path is selected.
    heist = await prepare_heist_template(
        profile,
        args.daemon_bin,
        args.startup_timeout_seconds,
        args.shutdown_timeout_seconds,
    )
    templates.append(heist.root)
    timer_and_activation = await asyncio.gather(
        *(
            timer_cell(
                profile,
                boundary,
                heist,
                args.daemon_bin,
                args.startup_timeout_seconds,
                args.request_timeout_seconds,
                args.shutdown_timeout_seconds,
            )
            for boundary in BOUNDARIES
        ),
        prepare_activation_template(
            profile,
            heist,
            args.daemon_bin,
            args.startup_timeout_seconds,
            args.shutdown_timeout_seconds,
        ),
    )
    for result in timer_and_activation[: len(BOUNDARIES)]:
        if not isinstance(result, tuple) or len(result) != 3:
            raise EvidenceFailure("Timer cell result was invalid")
        cell, duration, root = result
        cells.append(cell)
        durations.append(duration)
        cell_roots.append(root)
    activation_template = timer_and_activation[-1]
    if not isinstance(activation_template, pathlib.Path):
        raise EvidenceFailure("Activation template result was invalid")
    templates.append(activation_template)
    overdue_restart, overdue_root = await verify_overdue_timer_restart(
        profile,
        heist,
        args.daemon_bin,
        args.startup_timeout_seconds,
        args.shutdown_timeout_seconds,
    )
    templates.append(overdue_root)
    for boundary in BOUNDARIES:
        cell, duration, root = await activation_cell(
            profile,
            boundary,
            activation_template,
            heist,
            args.daemon_bin,
            args.startup_timeout_seconds,
            args.request_timeout_seconds,
            args.shutdown_timeout_seconds,
        )
        cells.append(cell)
        durations.append(duration)
        cell_roots.append(root)

    expected = {
        (operation, boundary)
        for operation in ("room_create", "action", "timer", "activation_lease")
        for boundary in BOUNDARIES
    }
    observed = {(cell["operation"], cell["name"]) for cell in cells}
    if len(cells) != 12 or observed != expected:
        raise EvidenceFailure(
            f"{profile.profile_id} did not produce its exact 12 kill cells"
        )
    return cells, durations, overdue_restart


async def run(args: argparse.Namespace) -> tuple[dict[str, Any], str]:
    if platform.system() != "Linux" or platform.machine() not in {"x86_64", "amd64"}:
        raise EvidenceFailure(
            "Linux x86-64 is required for process-level SIGKILL evidence"
        )
    if not args.daemon_bin.is_file() or not os.access(args.daemon_bin, os.X_OK):
        raise EvidenceFailure("worldstreamd is unavailable or not executable")
    distribution = distribution_identity(
        args.daemon_bin, args.package_archive, args.package_report
    )
    if distribution.get("packaged_artifact_bound") is not True:
        raise EvidenceFailure("release kill evidence must bind a packaged daemon")
    templates: list[pathlib.Path] = []
    cell_roots: list[pathlib.Path] = []
    cells: list[dict[str, Any]] = []
    durations: list[float] = []
    overdue_profiles: list[dict[str, Any]] = []
    provider_context: BackendProviderContext | None = None
    started = time.monotonic()
    try:
        provider_context = prepare_backend_provider(args, distribution)
        if (
            tuple(
                (profile.storage_backend, profile.connection_mode)
                for profile in provider_context.profiles
            )
            != BACKEND_PROFILES
        ):
            raise EvidenceFailure("backend provider profile inventory was not frozen")
        for profile in provider_context.profiles:
            profile_cells, profile_durations, overdue = await run_profile(
                profile, args, templates, cell_roots
            )
            cells.extend(profile_cells)
            durations.extend(profile_durations)
            overdue_profiles.append(overdue)

        expected_cells = {
            (storage_backend, connection_mode, operation, boundary)
            for storage_backend, connection_mode in BACKEND_PROFILES
            for operation in ("room_create", "action", "timer", "activation_lease")
            for boundary in BOUNDARIES
        }
        observed_cells = {
            (
                cell["storage_backend"],
                cell["connection_mode"],
                cell["operation"],
                cell["name"],
            )
            for cell in cells
        }
        if len(cells) != EXPECTED_CELL_COUNT or observed_cells != expected_cells:
            raise EvidenceFailure(
                "process kill matrix did not produce the exact frozen 36 cells"
            )
        if provider_context.cleanup() != "pass":
            raise EvidenceFailure("PostgreSQL provider cleanup failed closed")
        secret_scan = scan_daemon_logs(
            cell_roots + templates, provider_context.privacy_capture.sentinels
        )
        backend_provider = provider_context.evidence()
        profiles = [
            {
                "storage_backend": storage_backend,
                "connection_mode": connection_mode,
                "cell_count": 12,
                "overdue_timer_restart": "passed",
            }
            for storage_backend, connection_mode in BACKEND_PROFILES
        ]
        report = {
            "schema": SCHEMA,
            "status": "passed",
            "evidence_class": "process_level",
            "linux": True,
            "platform": {"system": platform.system(), "machine": platform.machine()},
            "release_evidence": False,
            "distribution": distribution,
            "named_gate_evidence": {
                "manifest_evidence_id": EVIDENCE_ID,
                "gate_cell": EVIDENCE_ID,
                "release_gate": True,
                "release_evidence": False,
                "handoff_status": "eligible_input_after_strict_producer_validation",
            },
            "backend_matrix": {
                "status": "covered",
                "storage_backend_and_connection_mode_separate": True,
                "profiles": profiles,
                "postgres_provider": backend_provider,
            },
            "operation": {
                "status": "committed_or_verified_absent_per_boundary",
                "commit_boundary": "guarded_backend_durable_result",
                "retry": {"attempted": True, "status": "passed"},
            },
            "kill_points": {
                "status": "covered",
                "process_kill_claim": True,
                "power_loss_claim": False,
                "matrix_complete": True,
                "boundaries": cells,
            },
            "restart": {
                "status": "passed",
                "same_data_directory": True,
                "health_observed": True,
            },
            "overdue_timer_restart_regression": {
                "status": "passed",
                "profiles": overdue_profiles,
            },
            "privacy": {"status": "pass", "secret_scan": secret_scan},
            "comparison": {
                "status": "passed",
                "method": "exact_public_idempotent_receipt_and_projection_or_offer_state",
                "equal": True,
                "retry_response_hash_equal": True,
            },
            "statistics": {
                "cell_count": len(cells),
                "elapsed_seconds": round(time.monotonic() - started, 3),
                "duration_ms": {
                    "p50": percentile(durations, 0.50),
                    "p95": percentile(durations, 0.95),
                    "p99": percentile(durations, 0.99),
                },
            },
            "limits": [
                "This is SIGKILL process evidence, not a physical power-loss claim.",
                "Precondition stores are created through public APIs and cloned only while closed.",
                "PostgreSQL migration, cloning, and observation use the direct-admin path.",
                "Release promotion remains owned by the strict detached failure/soak producer.",
            ],
        }
        return report, safe_log(cells, durations)
    except POSTGRES_HARNESS.LaneFailure as error:
        raise EvidenceFailure("PostgreSQL provider operation failed closed") from error
    finally:
        if (
            provider_context is not None
            and provider_context.cleanup_status == "not_started"
        ):
            try:
                provider_context.cleanup()
            except (
                POSTGRES_HARNESS.LaneFailure,
                OSError,
                subprocess.SubprocessError,
            ):
                POSTGRES_HARNESS.ACTIVE_PRIVACY_CAPTURE = None
        for root in cell_roots + templates:
            shutil.rmtree(root, ignore_errors=True)
        if provider_context is not None:
            shutil.rmtree(provider_context.root, ignore_errors=True)


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument(
        "--daemon-bin",
        type=pathlib.Path,
        default=ROOT / "target" / "debug" / "worldstreamd",
    )
    command.add_argument("--package-archive", type=pathlib.Path)
    command.add_argument("--package-report", type=pathlib.Path)
    command.add_argument("--docker", default=shutil.which("docker"))
    command.add_argument("--psql", default=shutil.which("psql"))
    command.add_argument("--startup-timeout-seconds", type=float, default=30)
    command.add_argument("--request-timeout-seconds", type=float, default=20)
    command.add_argument("--shutdown-timeout-seconds", type=float, default=10)
    command.add_argument("--output", type=pathlib.Path)
    command.add_argument("--log-output", type=pathlib.Path)
    return command


def main() -> int:
    args = parser().parse_args()
    if any(
        not 1 <= value <= 120
        for value in (
            args.startup_timeout_seconds,
            args.request_timeout_seconds,
            args.shutdown_timeout_seconds,
        )
    ):
        print(
            "kill-point matrix: timeouts must be in the closed range 1..120",
            file=sys.stderr,
        )
        return 2
    try:
        report, log = asyncio.run(run(args))
        encoded = json.dumps(report, sort_keys=True, separators=(",", ":")) + "\n"
        if args.output is not None:
            atomic_write(args.output, encoded)
        if args.log_output is not None:
            atomic_write(args.log_output, log)
        sys.stdout.write(encoded)
        print("kill-point matrix: exact 36-cell SIGKILL matrix passed", file=sys.stderr)
        return 0
    except (EvidenceFailure, ProtocolError, OSError, TimeoutError, ValueError) as error:
        report = {
            "schema": SCHEMA,
            "status": "failed",
            "evidence_class": "process_level_incomplete",
            "linux": platform.system() == "Linux",
            "release_evidence": False,
            "reason": type(error).__name__,
            "failure_code": (
                error.code if isinstance(error, ProtocolError) else "closed_failure"
            ),
            "named_gate_evidence": {
                "manifest_evidence_id": EVIDENCE_ID,
                "release_gate": True,
                "release_evidence": False,
            },
            "kill_points": {
                "status": "incomplete",
                "process_kill_claim": False,
                "power_loss_claim": False,
                "matrix_complete": False,
                "boundaries": [],
            },
        }
        encoded = json.dumps(report, sort_keys=True, separators=(",", ":")) + "\n"
        if args.output is not None:
            atomic_write(args.output, encoded)
        if args.log_output is not None:
            atomic_write(
                args.log_output, "worldstream Linux process kill matrix failed closed\n"
            )
        sys.stdout.write(encoded)
        print(
            f"kill-point matrix: failed closed ({type(error).__name__})",
            file=sys.stderr,
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

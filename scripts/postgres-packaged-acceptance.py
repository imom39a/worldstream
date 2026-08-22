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
import hashlib
import importlib.util
import json
import math
import os
import pathlib
import platform
import re
import secrets
import shutil
import signal
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
from dataclasses import dataclass
from typing import Any

import tomllib

SCHEMA = "worldstream/packaged-backend-parity/v1"
ROOT = pathlib.Path(__file__).resolve().parents[1]
SECRET_SCAN_PATH = ROOT / "scripts" / "verify-secret-absence.py"
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
SHA256_PREFIX = "sha256:"
MAX_ARCHIVE_MEMBERS = 20_000
MAX_ARCHIVE_UNCOMPRESSED_BYTES = 4 * 1024 * 1024 * 1024
MAX_CONTROL_FILE_BYTES = 16 * 1024 * 1024
CHECKSUM_LINE = re.compile(r"^([0-9a-f]{64})  ([^\\\r\n]+)$")
DATABASES = {
    "counter_postgres_direct": "counter_direct",
    "heist_postgres_direct": "heist_direct",
    "counter_transaction_pooler": "counter_pooler",
    "heist_transaction_pooler": "heist_pooler",
}


class LaneFailure(Exception):
    """A closed failure safe to retain in a credential-free report."""

    def __init__(self, code: str) -> None:
        super().__init__(code)
        self.code = code


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


SECRET_SCAN = _load_secret_scan()


def _regular_file(path: pathlib.Path, code: str) -> pathlib.Path:
    try:
        mode = path.lstat().st_mode
    except OSError as error:
        raise LaneFailure(code) from error
    if stat.S_ISLNK(mode) or not stat.S_ISREG(mode):
        raise LaneFailure(code)
    return path


def _sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


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
    _regular_file(path, "package_report_invalid")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, ValueError) as error:
        raise LaneFailure("package_report_invalid") from error
    if not isinstance(value, dict):
        raise LaneFailure("package_report_invalid")
    return value


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


def _bind_package(
    archive_path: pathlib.Path,
    report_path: pathlib.Path,
    extraction_root: pathlib.Path,
) -> tuple[pathlib.Path, pathlib.Path, dict[str, Any]]:
    """Validate exact archive/report identity and extract only both binaries."""

    archive_path = _regular_file(archive_path, "package_archive_invalid")
    package_report = _parse_package_report(report_path)
    archive_digest = _sha256_file(archive_path)
    identity = package_report.get("identity")
    inventory = package_report.get("inventory")
    if (
        package_report.get("schema") != PACKAGE_REPORT_SCHEMA
        or package_report.get("kind") != "archive"
        or package_report.get("artifact") != archive_path.name
        or package_report.get("path") != archive_path.name
        or package_report.get("sha256") != SHA256_PREFIX + archive_digest
        or package_report.get("size_bytes") != archive_path.stat().st_size
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
        with tarfile.open(archive_path, "r:gz") as archive:
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
                mirrored = json.loads(manifest_json.decode("utf-8"))
            except (UnicodeError, ValueError, tomllib.TOMLDecodeError) as error:
                raise LaneFailure("package_manifest_pair_invalid") from error
            canonical = (
                json.dumps(authored, ensure_ascii=False, indent=2, sort_keys=True)
                + "\n"
            ).encode()
            if authored != mirrored or manifest_json != canonical:
                raise LaneFailure("package_manifest_pair_not_canonical")
            try:
                metadata = json.loads(
                    _member_bytes(archive, relative["metadata/release.json"]).decode(
                        "utf-8"
                    )
                )
            except (UnicodeError, ValueError) as error:
                raise LaneFailure("package_metadata_invalid") from error
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
    except (OSError, tarfile.TarError) as error:
        raise LaneFailure("package_archive_unreadable") from error

    binding = {
        "schema": PACKAGE_BINDING_SCHEMA,
        "status": "pass",
        "artifact": archive_path.name,
        "archive_sha256": SHA256_PREFIX + archive_digest,
        "archive_size_bytes": archive_path.stat().st_size,
        "package_report_sha256": SHA256_PREFIX + _sha256_file(report_path),
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

    def _record_bytes(self, suffix: str, content: bytes) -> None:
        name = self._channel_name(suffix)
        path = self.root / name
        _owner_bytes(path, content)
        self.channels[name] = path

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
        self._record_bytes("stdout", stdout.encode("utf-8"))
        self._record_bytes("stderr", stderr.encode("utf-8"))
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
        )

    def add_report(self, label: str, path: pathlib.Path) -> None:
        if path.is_symlink() or not path.is_file():
            raise LaneFailure("privacy_report_channel_invalid")
        name = self._channel_name(label)
        self.channels[name] = path

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
                value = secret_path.read_bytes()
            except OSError:
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
            self.channels[name] = path
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
        try:
            return SECRET_SCAN.scan_sentinels(self.sentinels, self.channels)
        except SECRET_SCAN.ScanError as error:
            raise LaneFailure("privacy_secret_absence_scan_failed") from error


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


def _filesystem_type(path: pathlib.Path) -> str:
    arguments = (
        ["stat", "-f", "-c", "%T", str(path)]
        if sys.platform.startswith("linux")
        else ["stat", "-f", "%T", str(path)]
    )
    completed = subprocess.run(
        arguments,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return completed.stdout.strip() if completed.returncode == 0 else "unavailable"


def _reference_environment(repository: pathlib.Path) -> dict[str, Any]:
    try:
        page_size = os.sysconf("SC_PAGE_SIZE")
        physical_pages = os.sysconf("SC_PHYS_PAGES")
        memory_bytes = page_size * physical_pages
    except (AttributeError, OSError, ValueError):
        memory_bytes = 0
    return {
        "schema": "worldstream/reference-environment/v1",
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
        },
        "hardware": {
            "logical_cpus": os.cpu_count() or 0,
            "physical_memory_bytes": memory_bytes,
        },
        "filesystem": {
            "repository": _filesystem_type(repository),
            "temporary": _filesystem_type(pathlib.Path(tempfile.gettempdir())),
        },
        "engines": {
            "postgresql": None,
            "pgbouncer": {
                "image": PGBOUNCER_IMAGE,
                "pool_mode": "transaction",
            },
            "sqlite": None,
        },
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
    postgres_port: int | None = None
    pooler_port: int | None = None
    network_started: bool = False
    postgres_started: bool = False
    pooler_started: bool = False

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
            f"'{self.runtime_password}' NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;",
        )
        for database in DATABASES.values():
            self._psql("postgres", f"CREATE DATABASE {database} OWNER admin;")
            self._prepare_database(database)
        self._start_pooler()

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
            "public.worldstream_schema_migrations FROM runtime;",
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
            },
            "connection_mode": "direct_and_transaction_pooler",
        }

    def cleanup(self) -> str:
        failed = False
        if self.pooler_started:
            completed = _safe_run(
                [self.docker, "rm", "-f", self.pooler_name],
                cwd=self.repository,
                timeout=30,
            )
            failed = failed or completed.returncode != 0
            self.pooler_started = False
        if self.postgres_started:
            completed = _safe_run(
                [self.docker, "rm", "-f", self.postgres_name],
                cwd=self.repository,
                timeout=30,
            )
            failed = failed or completed.returncode != 0
            self.postgres_started = False
        if self.network_started:
            completed = _safe_run(
                [self.docker, "network", "rm", self.network],
                cwd=self.repository,
                timeout=30,
            )
            failed = failed or completed.returncode != 0
            self.network_started = False
        return "failed" if failed else "pass"


def _load_report(path: pathlib.Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise LaneFailure("cell_report_unavailable") from error
    if not isinstance(value, dict):
        raise LaneFailure("cell_report_invalid")
    return value


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
    privacy_workspace = report_path.parent / f"{privacy_label}-temporary"
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
        ACTIVE_PRIVACY_CAPTURE.add_report(f"{privacy_label}-report", report_path)
    return {**measurement, "report": report}


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
        "reference_environment": _reference_environment(repository),
        "reference_workloads": {"counter": {}, "heist": {}},
        "cells": {"counter": {}, "heist": {}},
        "comparison": {},
        "package_binding": {"status": "not_verified"},
        "performance": {
            "classification": "measured_non_sla",
            "percentile_definition": "nearest-rank",
            "cells_ms": {},
            "cells": {},
        },
        "cleanup": "not_started",
        "privacy": {
            "status": "pending",
            "channel_contract": (
                "every captured child stdout/stderr/safe invocation config, raw cell "
                "report, daemon log, and aggregate report candidate"
            ),
        },
    }
    package_mode = not args.diagnostic_source_tree
    package_inputs_valid = (
        args.package_archive is not None
        and args.package_report is not None
        and args.worldstreamd is None
        and args.worldstreamctl is None
    )
    diagnostic_inputs_valid = (
        args.package_archive is None and args.package_report is None
    )
    if (package_mode and not package_inputs_valid) or (
        not package_mode and not diagnostic_inputs_valid
    ):
        report["reason_code"] = "package_bound_inputs_required"
        _write_report(args.report, report)
        return 10

    root = pathlib.Path(tempfile.mkdtemp(prefix="worldstream-packaged-parity-"))
    root.chmod(0o700)
    if package_mode:
        if not sys.platform.startswith("linux"):
            report["reason_code"] = "linux_package_execution_host_required"
            shutil.rmtree(root, ignore_errors=True)
            _write_report(args.report, report)
            return 10
        try:
            daemon, ctl, binding = _bind_package(
                args.package_archive,
                args.package_report,
                root / "verified-package",
            )
        except LaneFailure as error:
            report["reason_code"] = error.code
            shutil.rmtree(root, ignore_errors=True)
            _write_report(args.report, report)
            return 10
        report["package_binding"] = binding
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

    prerequisites = [daemon, ctl, python, counter_runner, heist_runner]
    if (
        os.name != "posix"
        or not args.docker
        or not args.psql
        or any(not path.is_file() for path in prerequisites)
        or any(not os.access(path, os.X_OK) for path in (daemon, ctl, python))
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
            "settings": sqlite_settings,
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
            privacy_capture.add_report("packaged-report", candidate)
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

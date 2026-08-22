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
import stat
import subprocess
import sys
import tempfile
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


def require(condition: bool, code: str) -> None:
    if not condition:
        raise SmokeError(code)


def sha256_bytes(value: bytes) -> str:
    return SHA256_PREFIX + hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
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


def verify_and_extract(
    archive: Path,
    package_report_path: Path,
    manifest_toml: Path,
    manifest_json: Path,
    output_dir: Path,
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
            PACKAGE.verify_archive(archive)
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


def run_json(
    argv: list[str],
    code: str,
    *,
    environment: dict[str, str] | None = None,
    timeout: float = 120,
) -> dict[str, Any]:
    completed = run_process(argv, environment=environment, timeout=timeout)
    require(completed.returncode == 0, code)
    try:
        value = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise SmokeError(code) from error
    require(isinstance(value, dict), code)
    return value


def psql(
    executable: Path,
    connection: dict[str, str],
    sql: str,
    code: str,
) -> str:
    environment = {**os.environ, "PGPASSWORD": connection["password"]}
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
    require(completed.returncode == 0, code)
    return completed.stdout.strip()


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
    try:
        value = json.loads(body.decode("utf-8"))
    except (UnicodeError, json.JSONDecodeError) as error:
        raise SmokeError("runtime_probe_invalid_json") from error
    require(isinstance(value, dict), "runtime_probe_invalid_json")
    return status, value


def daemon_environment(secret_file: Path, dsn_file: Path | None) -> dict[str, str]:
    environment = {**os.environ}
    for name in (
        "WORLDSTREAM_POSTGRES_URL",
        "WORLDSTREAM__STORAGE__POSTGRESQL__DSN",
        "WORLDSTREAM__STORAGE__POSTGRESQL__DSN_HANDLE",
        "WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_HANDLE",
    ):
        environment.pop(name, None)
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
            and ctl_health == {"status": "ok", "probe": "daemon-healthz"}
            and ctl_version == version.get("manifest"),
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
        return {
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
            },
        }
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
    try:
        admin = parse_dsn(args.admin_dsn_file)
        daemon, control, binding, manifest = verify_and_extract(
            args.package_archive,
            args.package_report,
            args.manifest_toml,
            args.manifest_json,
            root / "package",
        )
        psql_path = regular_file(args.psql, "psql_unavailable")
        version_num = psql(
            psql_path,
            admin,
            "SHOW server_version_num;",
            "postgres_version_probe_failed",
        )
        require(version_num == "170011", "postgres_version_is_not_17_11")
        setup_sql = (
            f"CREATE ROLE \"{runtime_role}\" LOGIN PASSWORD '{runtime_password}' "
            "NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;"
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
            },
            "packaged_ctl_postgres_admin_contract_drift",
        )
        grants_sql = (
            "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public "
            f'TO "{runtime_role}";'
            "GRANT USAGE, SELECT, UPDATE ON ALL SEQUENCES IN SCHEMA public "
            f'TO "{runtime_role}";'
            "REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON TABLE "
            f'public.worldstream_schema_migrations FROM "{runtime_role}";'
        )
        psql(psql_path, admin, grants_sql, "postgres_runtime_grants_failed")
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
        psql(
            psql_path,
            admin,
            f'DROP OWNED BY "{runtime_role}"; DROP ROLE "{runtime_role}";',
            "postgres_runtime_role_cleanup_failed",
        )
        runtime_role_created = False
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
            "cleanup": "pass",
        }
        shutil.rmtree(root)
        require(not root.exists(), "owned_temporary_cleanup_failed")
        return result
    finally:
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
        "--manifest-toml", type=Path, default=ROOT / "compatibility.toml"
    )
    command.add_argument(
        "--manifest-json", type=Path, default=ROOT / "compatibility.json"
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

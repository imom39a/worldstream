#!/usr/bin/env python3
"""Start the local Counter Studio browser demo without prebuilding a Task.

The coordinator owns only its fresh state directory and the process groups it
starts. It installs the exact Counter v4 managed-host fixture and named local
credential, but deliberately leaves Agent Profile, Task Template, Room draft,
Room creation, and Task setup for the operator to perform in Studio.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import secrets
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from collections.abc import Callable
from pathlib import Path
from typing import Any

from install_managed_counter_fixture import (
    COUNTER_PACK_DIGEST,
    COUNTER_PACK_ID,
    COUNTER_PACK_REVISION,
    MODEL_CREDENTIAL_ID,
    TEMPLATE_ID,
    TEMPLATE_REVISION,
    install_fixture,
)

REPOSITORY = Path(__file__).resolve().parents[2]
CONTROL_SCHEMA = "worldstream/counter-studio-demo-control/v1"
STARTUP_TIMEOUT_SECONDS = 120.0
MAX_CONTROL_BYTES = 16 * 1024
MANAGED_HOST_HEALTH_PORT = 19_432
ULID_PATTERN = re.compile(r"^[0-7][0-9A-HJKMNP-TV-Z]{25}$")


class DemoConfigurationError(RuntimeError):
    """A safe, local configuration failure before a process is started."""


def validate_ports(
    supervisor_port: int,
    daemon_port: int,
    studio_port: int,
    console_port: int,
    provider_port: int,
) -> None:
    ports = [supervisor_port, daemon_port, studio_port, console_port, provider_port]
    if any(not isinstance(port, int) or not 1 <= port <= 65_535 for port in ports):
        raise DemoConfigurationError("ports must be between 1 and 65535")
    if len(set(ports)) != len(ports):
        raise DemoConfigurationError("demo ports must be distinct")


def require_ports_available(ports: list[int]) -> None:
    """Reject an occupied loopback listener; never displace another process."""

    for port in ports:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
            listener.settimeout(0.1)
            if listener.connect_ex(("127.0.0.1", port)) == 0:
                raise DemoConfigurationError(
                    f"127.0.0.1:{port} is already in use; choose an unused port"
                )
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
            probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            try:
                probe.bind(("127.0.0.1", port))
            except OSError as error:
                raise DemoConfigurationError(
                    f"127.0.0.1:{port} is already in use; choose an unused port"
                ) from error


def owner_only_directory(path: Path, *, create: bool) -> Path:
    if create:
        path.mkdir(mode=0o700, parents=True, exist_ok=False)
    try:
        metadata = path.lstat()
    except OSError as error:
        raise DemoConfigurationError("owner-only directory is unavailable") from error
    if (
        stat.S_ISLNK(metadata.st_mode)
        or not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_mode & 0o077
        or (hasattr(os, "getuid") and metadata.st_uid != os.getuid())
    ):
        raise DemoConfigurationError("directory must be an owner-only directory")
    return path.resolve()


def prepare_state_directory(path: Path | None, retain_state: bool) -> tuple[Path, bool]:
    """Create fresh ephemeral state, or require explicit consent to reuse it."""

    if path is None:
        root = Path(tempfile.mkdtemp(prefix="worldstream-counter-studio-"))
        root.chmod(0o700)
        return owner_only_directory(root, create=False), True
    absolute = Path(os.path.abspath(path))
    if absolute.exists():
        if not retain_state:
            raise DemoConfigurationError(
                "state directory already exists; pass --retain-state to reuse it"
            )
        return owner_only_directory(absolute, create=False), False
    if retain_state:
        raise DemoConfigurationError("--retain-state requires an existing state directory")
    absolute.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    return owner_only_directory(absolute, create=True), False


def write_secret(path: Path, value: bytes) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as output:
        output.write(value)
        output.flush()
        os.fsync(output.fileno())
    path.chmod(0o600)


def read_or_create_secret(path: Path, value: bytes) -> bytes:
    """Reuse only one retained owner-only secret file created by this demo."""

    if not path.exists():
        write_secret(path, value)
        return value
    try:
        metadata = path.lstat()
        retained = path.read_bytes()
    except OSError as error:
        raise DemoConfigurationError("retained demo secret is unavailable") from error
    if (
        stat.S_ISLNK(metadata.st_mode)
        or not stat.S_ISREG(metadata.st_mode)
        or metadata.st_mode & 0o077
        or (hasattr(os, "getuid") and metadata.st_uid != os.getuid())
        or not retained
        or len(retained) > 16 * 1024
    ):
        raise DemoConfigurationError("retained demo secret must be owner-only")
    return retained


def control_payload(
    supervisor_port: int,
    daemon_port: int,
    studio_port: int,
    console_port: int,
    provider_port: int = 19_431,
) -> dict[str, Any]:
    return {
        "schema": CONTROL_SCHEMA,
        "status": "ready",
        "urls": {
            "supervisor": f"http://127.0.0.1:{supervisor_port}",
            "daemon": f"http://127.0.0.1:{daemon_port}",
            "studio": f"http://127.0.0.1:{studio_port}",
            "console": f"http://127.0.0.1:{console_port}",
        },
        "fixture": {
            "activity_pack_id": COUNTER_PACK_ID,
            "activity_pack_revision": COUNTER_PACK_REVISION,
            "activity_pack_digest": COUNTER_PACK_DIGEST,
            "credential_id": MODEL_CREDENTIAL_ID,
            "provider_url": f"http://127.0.0.1:{provider_port}",
            "model_id": "counter-deterministic",
            "runner_template_id": TEMPLATE_ID,
            "runner_template_revision": TEMPLATE_REVISION,
        },
        "operator_work_remaining": [
            "publish_agent_profile",
            "publish_task_template",
            "instantiate_editable_draft",
            "save_review",
            "create_room",
            "provision_task_setup",
        ],
    }


def write_control_file(path: Path, payload: dict[str, Any]) -> None:
    parent = owner_only_directory(path.parent, create=False)
    target = parent / path.name
    if target.exists():
        metadata = target.lstat()
        if (
            stat.S_ISLNK(metadata.st_mode)
            or not stat.S_ISREG(metadata.st_mode)
            or metadata.st_mode & 0o077
            or (hasattr(os, "getuid") and metadata.st_uid != os.getuid())
        ):
            raise DemoConfigurationError("control file must be an owner-only regular file")
    encoded = (json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n").encode()
    if len(encoded) > MAX_CONTROL_BYTES:
        raise DemoConfigurationError("control file payload is unexpectedly large")
    temporary = parent / f".{target.name}.{secrets.token_hex(12)}.tmp"
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(encoded)
            output.flush()
            os.fsync(output.fileno())
        temporary.chmod(0o600)
        os.replace(temporary, target)
        directory_descriptor = os.open(parent, os.O_RDONLY)
        try:
            os.fsync(directory_descriptor)
        finally:
            os.close(directory_descriptor)
    finally:
        temporary.unlink(missing_ok=True)


def safe_environment(supervisor_url: str) -> dict[str, str]:
    environment = dict(os.environ)
    for key in list(environment):
        normalized = key.lower()
        if key.startswith("WORLDSTREAM__AUTHORITY__BOOTSTRAP") or any(
            token in normalized for token in ("token", "secret", "credential", "password")
        ):
            environment.pop(key, None)
    environment["RUST_LOG"] = "warn"
    environment["VITE_WORLDSTREAM_SUPERVISOR_URL"] = supervisor_url
    return environment


def request(base: str, path: str, *, method: str = "GET") -> dict[str, Any] | None:
    try:
        with urllib.request.urlopen(
            urllib.request.Request(base + path, method=method), timeout=1.0
        ) as response:
            value: Any = json.loads(response.read())
    except (OSError, TimeoutError, urllib.error.HTTPError, json.JSONDecodeError):
        return None
    return value if isinstance(value, dict) else None


def http_available(url: str) -> bool:
    try:
        with urllib.request.urlopen(url, timeout=0.5):
            return True
    except (OSError, TimeoutError, urllib.error.HTTPError):
        return False


def provider_status_ready(url: str) -> bool:
    """Accept only the provider's bounded, public readiness/status DTO."""

    try:
        with urllib.request.urlopen(url + "/fixture/status", timeout=0.5) as response:
            value: Any = json.loads(response.read())
    except (OSError, TimeoutError, urllib.error.HTTPError, json.JSONDecodeError):
        return False
    return (
        isinstance(value, dict)
        and value.get("schema") == "worldstream/counter-deterministic-provider-status/v1"
        and isinstance(value.get("received_requests"), int)
        and isinstance(value.get("accepted_requests"), int)
        and value["received_requests"] >= 0
        and value["accepted_requests"] >= 0
    )


def wait_for_process(
    name: str,
    process: subprocess.Popen[bytes],
    stopped: threading.Event,
    timeout: float,
    probe: Callable[[], bool],
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if stopped.is_set():
            raise DemoConfigurationError("demo startup interrupted")
        if process.poll() is not None:
            raise DemoConfigurationError("a demo process exited before becoming ready; inspect its owner-only log")
        if probe():
            return
        stopped.wait(0.1)
    raise DemoConfigurationError(f"{name} startup timed out; inspect its owner-only log")


def stop_process(process: subprocess.Popen[bytes] | None) -> bool:
    if process is None or process.poll() is not None:
        return True
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=10)
    except (OSError, subprocess.TimeoutExpired):
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except OSError:
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            return process.poll() is not None
    return process.poll() is not None


def stop_managed_hosts(supervisor_url: str) -> bool:
    attention = request(supervisor_url, "/api/v1/runner-attention")
    if attention is None:
        return False
    hosts = attention.get("managed_hosts")
    if not isinstance(hosts, list):
        return False
    for host in hosts:
        if not isinstance(host, dict):
            return False
        assignment_id = host.get("assignment_id")
        if not isinstance(assignment_id, str) or ULID_PATTERN.fullmatch(assignment_id) is None:
            return False
        stopped = request(
            supervisor_url,
            f"/api/v1/managed-agent-hosts/{assignment_id}/stop",
            method="POST",
        )
        if not isinstance(stopped, dict) or stopped.get("state") != "stopped":
            return False
    return True


def stop_daemon(supervisor_url: str, timeout: float = 15.0) -> bool:
    stopped = request(supervisor_url, "/api/v1/daemon/stop", method="POST")
    if stopped is not None and stopped.get("state") == "stopped":
        return True
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        lifecycle = request(supervisor_url, "/api/v1/daemon/lifecycle")
        if lifecycle is not None and lifecycle.get("state") == "stopped":
            return True
        time.sleep(0.1)
    return False


def build_dependencies() -> None:
    subprocess.run(
        [
            "cargo", "build", "--locked",
            "-p", "worldstream-server", "--bin", "worldstreamd",
            "-p", "worldstream-studio-supervisor", "--bins",
        ],
        cwd=REPOSITORY,
        check=True,
    )
    subprocess.run(["pnpm", "--dir", "web/studio", "build"], cwd=REPOSITORY, check=True)
    subprocess.run(["pnpm", "--dir", "web/console", "build"], cwd=REPOSITORY, check=True)


def require_binaries(paths: list[Path]) -> None:
    if any(not path.is_file() or not os.access(path, os.X_OK) for path in paths):
        raise DemoConfigurationError("required binaries are missing; omit --skip-build to build them")


def vite_command(directory: str, port: int) -> list[str]:
    """Run Vite itself so its network and strict-port options are not swallowed by pnpm."""

    return [
        "pnpm", "--dir", directory, "exec", "vite",
        "--host", "127.0.0.1", "--port", str(port), "--strictPort",
    ]


def daemon_config(root: Path, daemon_port: int, authority_file: Path) -> Path:
    config = root / "daemon.toml"
    content = "\n".join(
        (
            "config_version = 1",
            "[server]",
            f'bind = "127.0.0.1:{daemon_port}"',
            "[storage]",
            'profile = "sqlite-bundled"',
            f"data_dir = {json.dumps(str(root / 'data'))}",
            'deployment_lineage = "counter/studio-demo"',
            "storage_epoch = 1",
            "[authority.bootstrap]",
            f"secret_file = {json.dumps(str(authority_file))}",
            "",
        )
    )
    if config.exists():
        metadata = config.lstat()
        if (
            stat.S_ISLNK(metadata.st_mode)
            or not stat.S_ISREG(metadata.st_mode)
            or metadata.st_mode & 0o077
            or config.read_text("utf-8") != content
        ):
            raise DemoConfigurationError("retained daemon configuration conflicts with this demo")
        return config
    descriptor = os.open(config, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as output:
        output.write(content)
        output.flush()
        os.fsync(output.fileno())
    config.chmod(0o600)
    return config


def start_demo(args: argparse.Namespace) -> int:
    validate_ports(
        args.supervisor_port, args.daemon_port, args.studio_port,
        args.console_port, args.provider_port,
    )
    if args.provider_port == MANAGED_HOST_HEALTH_PORT:
        raise DemoConfigurationError("provider port conflicts with the fixed managed-host health port")
    require_ports_available([
        args.supervisor_port, args.daemon_port, args.studio_port,
        args.console_port, args.provider_port, MANAGED_HOST_HEALTH_PORT,
    ])
    control = Path(os.path.abspath(args.control_file)) if args.control_file is not None else None
    if control is not None:
        building_payload = control_payload(
            args.supervisor_port, args.daemon_port, args.studio_port, args.console_port,
            args.provider_port,
        )
        building_payload["status"] = "building"
        write_control_file(control, building_payload)
    if not args.skip_build:
        build_dependencies()
    binaries = [
        args.daemon_binary,
        args.supervisor_binary,
        args.assignment_mcp_binary,
        args.host_binary,
    ]
    require_binaries(binaries)

    root, ephemeral = prepare_state_directory(args.state_dir, args.retain_state)
    supervisor = provider = studio = console = None
    supervisor_url = f"http://127.0.0.1:{args.supervisor_port}"
    daemon_url = f"http://127.0.0.1:{args.daemon_port}"
    environment = safe_environment(supervisor_url)
    stopped = threading.Event()
    logs: list[Any] = []
    cleanup_confirmed = False

    def signal_stop(_signum: int, _frame: Any) -> None:
        stopped.set()

    previous_interrupt = signal.signal(signal.SIGINT, signal_stop)
    previous_terminate = signal.signal(signal.SIGTERM, signal_stop)
    try:
        logs_root = owner_only_directory(root / f"logs-{secrets.token_hex(8)}", create=True)
        authority_file = root / "authority.secret"
        model_file = root / "model-token"
        read_or_create_secret(authority_file, secrets.token_bytes(32))
        read_or_create_secret(model_file, secrets.token_hex(32).encode("ascii"))
        config = daemon_config(root, args.daemon_port, authority_file)
        studio_path = root / "studio"
        studio_state = owner_only_directory(studio_path, create=not studio_path.exists())
        install_fixture(
            args.host_binary,
            root / "fixture",
            root / "runner-templates",
            args.provider_port,
            credential_file=model_file,
            supervisor_state_dir=studio_state,
            model_provider_credentials_dir=root / "model-provider-credentials",
        )

        payload = control_payload(
            args.supervisor_port, args.daemon_port, args.studio_port, args.console_port,
            args.provider_port,
        )
        payload["status"] = "starting"
        if control is not None:
            write_control_file(control, payload)

        provider_log = (logs_root / "provider.log").open("ab")
        logs.append(provider_log)
        provider = subprocess.Popen(
            [
                sys.executable,
                str(REPOSITORY / "examples/counter/deterministic_loopback_provider.py"),
                "--bind", "127.0.0.1", "--port", str(args.provider_port),
                "--credential-file", str(model_file),
                "--response-delay-ms", str(args.provider_delay_ms),
            ],
            cwd=REPOSITORY, env=environment, stdout=provider_log,
            stderr=subprocess.STDOUT, start_new_session=True,
        )
        provider_url = f"http://127.0.0.1:{args.provider_port}"
        wait_for_process(
            "provider", provider, stopped, args.startup_timeout,
            lambda: provider_status_ready(provider_url),
        )

        supervisor_log = (logs_root / "supervisor.log").open("ab")
        logs.append(supervisor_log)
        supervisor = subprocess.Popen(
            [
                str(args.supervisor_binary),
                "--bind", f"127.0.0.1:{args.supervisor_port}",
                "--daemon", f"127.0.0.1:{args.daemon_port}",
                "--daemon-executable", str(args.daemon_binary),
                "--daemon-config", str(config),
                "--state-dir", str(studio_state),
                "--runner-templates-dir", str(root / "runner-templates"),
                "--model-provider-credentials-dir", str(root / "model-provider-credentials"),
                "--assignment-mcp-executable", str(args.assignment_mcp_binary),
                "--studio-origin", f"http://127.0.0.1:{args.studio_port}",
                "--participant-console-origin", f"http://127.0.0.1:{args.console_port}",
            ],
            cwd=REPOSITORY, env=environment, stdout=supervisor_log,
            stderr=subprocess.STDOUT, start_new_session=True,
        )
        wait_for_process(
            "Supervisor", supervisor, stopped, args.startup_timeout,
            lambda: http_available(supervisor_url + "/api/v1/daemon/status"),
        )
        if request(supervisor_url, "/api/v1/daemon/start", method="POST") is None:
            raise DemoConfigurationError("Supervisor could not start its configured daemon")
        wait_for_process(
            "daemon", supervisor, stopped, args.startup_timeout,
            lambda: http_available(daemon_url + "/readyz"),
        )

        studio_log = (logs_root / "studio.log").open("ab")
        console_log = (logs_root / "console.log").open("ab")
        logs.extend([studio_log, console_log])
        studio = subprocess.Popen(
            vite_command("web/studio", args.studio_port),
            cwd=REPOSITORY, env=environment, stdout=studio_log,
            stderr=subprocess.STDOUT, start_new_session=True,
        )
        console = subprocess.Popen(
            vite_command("web/console", args.console_port),
            cwd=REPOSITORY, env=environment, stdout=console_log,
            stderr=subprocess.STDOUT, start_new_session=True,
        )
        wait_for_process(
            "Studio", studio, stopped, args.startup_timeout,
            lambda: http_available(f"http://127.0.0.1:{args.studio_port}"),
        )
        wait_for_process(
            "Console", console, stopped, args.startup_timeout,
            lambda: http_available(f"http://127.0.0.1:{args.console_port}"),
        )
        payload["status"] = "ready"
        if control is not None:
            write_control_file(control, payload)
        print(json.dumps(payload, sort_keys=True), flush=True)
        while not stopped.wait(0.5):
            if any(process is not None and process.poll() is not None for process in (provider, supervisor, studio, console)):
                raise DemoConfigurationError("a demo process exited; inspect the owner-only log directory")
    finally:
        managed_hosts_stopped = False
        daemon_stopped = False
        if supervisor is not None and supervisor.poll() is None:
            managed_hosts_stopped = stop_managed_hosts(supervisor_url)
            daemon_stopped = stop_daemon(supervisor_url)
        else:
            managed_hosts_stopped = supervisor is None
            daemon_stopped = supervisor is None
        process_groups_reaped = all(
            stop_process(process) for process in (console, studio, supervisor, provider)
        )
        for log in logs:
            log.close()
        if control is not None:
            stopped_payload = control_payload(
                args.supervisor_port, args.daemon_port, args.studio_port, args.console_port,
                args.provider_port,
            )
            cleanup_confirmed = managed_hosts_stopped and daemon_stopped and process_groups_reaped
            stopped_payload["status"] = "stopped" if cleanup_confirmed else "cleanup_needs_attention"
            try:
                write_control_file(control, stopped_payload)
            except DemoConfigurationError:
                pass
        signal.signal(signal.SIGINT, previous_interrupt)
        signal.signal(signal.SIGTERM, previous_terminate)
        cleanup_confirmed = managed_hosts_stopped and daemon_stopped and process_groups_reaped
        if ephemeral and cleanup_confirmed:
            shutil.rmtree(root, ignore_errors=True)
        elif not cleanup_confirmed:
            print("demo cleanup needs attention; retaining owner-only state", file=sys.stderr)
    return 0 if cleanup_confirmed else 2


def parse_args(arguments: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--state-dir", type=Path)
    parser.add_argument("--retain-state", action="store_true")
    parser.add_argument("--control-file", type=Path)
    parser.add_argument("--startup-timeout", type=float, default=STARTUP_TIMEOUT_SECONDS)
    parser.add_argument("--provider-delay-ms", type=int, default=3_000)
    parser.add_argument("--supervisor-port", type=int, default=9410)
    parser.add_argument("--daemon-port", type=int, default=9420)
    parser.add_argument("--studio-port", type=int, default=5174)
    parser.add_argument("--console-port", type=int, default=5173)
    parser.add_argument("--provider-port", type=int, default=19431)
    parser.add_argument("--daemon-binary", type=Path, default=REPOSITORY / "target/debug/worldstreamd")
    parser.add_argument("--supervisor-binary", type=Path, default=REPOSITORY / "target/debug/worldstream-studio-supervisor")
    parser.add_argument("--assignment-mcp-binary", type=Path, default=REPOSITORY / "target/debug/worldstream-assignment-mcp")
    parser.add_argument("--host-binary", type=Path, default=REPOSITORY / "target/debug/worldstream-managed-agent-host")
    return parser.parse_args(arguments)


def main(arguments: list[str] | None = None) -> int:
    args = parse_args(arguments)
    if args.startup_timeout <= 0 or args.startup_timeout > 120:
        print("startup timeout must be between 0 and 120 seconds", file=sys.stderr)
        return 2
    if not 0 <= args.provider_delay_ms <= 10_000:
        print("provider delay must be between 0 and 10000 milliseconds", file=sys.stderr)
        return 2
    try:
        return start_demo(args)
    except (DemoConfigurationError, subprocess.CalledProcessError) as error:
        print(str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())

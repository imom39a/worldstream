#!/usr/bin/env python3
"""Measure a real SQLite ``worldstreamd`` transition workload on Linux.

The workload uses the public HTTP/WebSocket SDK, verifies public fan-out, and
measures only its own daemon, SQLite, WAL, log/temp, latency, and recovery
effects. Short runs are diagnostic. Only ``--one-hour`` fixes the workload
window at exactly 3600 seconds and can become an input to the strict detached
failure/soak producer.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import importlib.util
import json
import os
import pathlib
import platform
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from dataclasses import dataclass
from typing import Any

from worldstream_sdk import Client, ProtocolError

ROOT = pathlib.Path(__file__).resolve().parents[1]
COMMON_PATH = ROOT / "scripts" / "kill-point-matrix.py"
SECRET_SCAN_PATH = ROOT / "scripts" / "verify-secret-absence.py"
SCHEMA = "worldstream/soak-evidence/v1"
EVIDENCE_ID = "failure-fuzz-resource-and-one-hour-sqlite-soak"
WORKLOAD_BINDING = "worldstream-daemon-transition-soak/v1"
ONE_HOUR_SECONDS = 3600.0
MAX_PEAK_RSS_BYTES = 2 * 1024 * 1024 * 1024
DEFAULT_MAX_DATABASE_GROWTH_BYTES = 256 * 1024 * 1024
DEFAULT_MAX_WAL_GROWTH_BYTES = 256 * 1024 * 1024
DEFAULT_MAX_TEMP_GROWTH_BYTES = 64 * 1024 * 1024
DEFAULT_MAX_OUTPUT_BYTES = 256 * 1024
DEFAULT_MAX_ARTIFACT_GROWTH_BYTES = (
    DEFAULT_MAX_TEMP_GROWTH_BYTES + DEFAULT_MAX_OUTPUT_BYTES
)
DEFAULT_INTERNAL_QUEUE_HARD_LIMIT = 256
MAX_AUXILIARY_REPORT_BYTES = 8 * 1024 * 1024
MAX_OS_RELEASE_BYTES = 64 * 1024
PACKAGED_ACCEPTANCE_SCHEMA = "worldstream/packaged-backend-parity/v1"
COUNTER_ACTIONS_PER_ROOM = 8
COUNTER_PACK = {
    "id": "worldstream.counter",
    "version": "2.0.0",
    "digest": "blake3:1c5f75068220f65f9017a062dbe40203c540108b572284d9446a329914008a92",
}


class SoakFailure(RuntimeError):
    """A bounded, non-secret workload or measurement failure."""


def load_common() -> Any:
    spec = importlib.util.spec_from_file_location(
        "worldstream_process_evidence_common", COMMON_PATH
    )
    if spec is None or spec.loader is None:
        raise SoakFailure("process evidence helpers could not be loaded")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


COMMON = load_common()


def load_secret_scan() -> Any:
    spec = importlib.util.spec_from_file_location(
        "worldstream_process_secret_scan", SECRET_SCAN_PATH
    )
    if spec is None or spec.loader is None:
        raise SoakFailure("secret-absence scanner could not be loaded")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


SECRET_SCAN = load_secret_scan()


def workload_target_seconds(one_hour: bool, short_duration_seconds: float) -> float:
    return ONE_HOUR_SECONDS if one_hour else short_duration_seconds


def release_window_is_complete(
    *, one_hour: bool, target_seconds: float, elapsed_seconds: float
) -> bool:
    return bool(
        one_hour
        and target_seconds == ONE_HOUR_SECONDS
        and elapsed_seconds >= ONE_HOUR_SECONDS
    )


def read_proc_rss(pid: int) -> int | None:
    try:
        status = pathlib.Path(f"/proc/{pid}/status").read_text(encoding="utf-8")
    except (FileNotFoundError, OSError):
        return None
    match = re.search(r"^VmRSS:\s+(\d+)\s+kB$", status, re.MULTILINE)
    return int(match.group(1)) * 1024 if match else None


def process_tree(pid: int) -> set[int]:
    pending = [pid]
    observed: set[int] = set()
    while pending:
        current = pending.pop()
        if current in observed:
            continue
        observed.add(current)
        try:
            children = pathlib.Path(
                f"/proc/{current}/task/{current}/children"
            ).read_text(encoding="utf-8")
        except (FileNotFoundError, OSError):
            continue
        pending.extend(int(value) for value in children.split() if value.isdigit())
    return observed


class RssSampler(threading.Thread):
    def __init__(self, pid: int) -> None:
        super().__init__(daemon=True)
        self.pid = pid
        self.peak_bytes = 0
        self.samples = 0
        self._stop_event = threading.Event()

    def run(self) -> None:
        while not self._stop_event.wait(0.05):
            values = [read_proc_rss(pid) for pid in process_tree(self.pid)]
            total = sum(value for value in values if value is not None)
            if total > 0:
                self.peak_bytes = max(self.peak_bytes, total)
                self.samples += 1

    def finish(self) -> int:
        self._stop_event.set()
        self.join(timeout=2)
        if self.peak_bytes <= 0 or self.samples <= 0:
            raise SoakFailure("worldstreamd process-tree RSS could not be measured")
        return self.peak_bytes

    def cancel(self) -> None:
        self._stop_event.set()
        self.join(timeout=2)


def file_size(path: pathlib.Path) -> int:
    try:
        value = path.lstat()
    except FileNotFoundError:
        return 0
    if not pathlib.Path(path).is_file() or pathlib.Path(path).is_symlink():
        raise SoakFailure("measured storage path was not a regular file")
    return value.st_size


def sqlite_sizes(data_dir: pathlib.Path) -> dict[str, int]:
    main = data_dir / "worldstream.sqlite3"
    return {
        "main_bytes": file_size(main),
        "wal_bytes": file_size(pathlib.Path(f"{main}-wal")),
        "shm_bytes": file_size(pathlib.Path(f"{main}-shm")),
    }


def auxiliary_snapshot(
    root: pathlib.Path, excluded: set[pathlib.Path]
) -> dict[str, Any]:
    """Measure every non-database working file by disjoint category."""

    logs: list[pathlib.Path] = []
    temporary: list[pathlib.Path] = []
    for path in sorted(root.rglob("*")):
        if path in excluded:
            continue
        try:
            path.lstat()
        except OSError as error:
            raise SoakFailure(
                "auxiliary workload path could not be inspected"
            ) from error
        if path.is_symlink():
            raise SoakFailure("auxiliary workload path was a symbolic link")
        if not path.is_file():
            continue
        if (
            path.parent == root
            and path.name.startswith("worldstreamd-")
            and path.suffix == ".log"
        ):
            logs.append(path)
        else:
            temporary.append(path)

    def total(paths: list[pathlib.Path]) -> int:
        return sum(path.stat().st_size for path in paths)

    log_bytes = total(logs)
    temporary_bytes = total(temporary)
    return {
        "temporary_bytes": temporary_bytes,
        "log_bytes": log_bytes,
        "artifact_bytes": temporary_bytes + log_bytes,
        "logs": logs,
        "temporary": temporary,
    }


def metric_value(text: str, name: str) -> int:
    match = re.search(rf"^{re.escape(name)} ([0-9]+)$", text, re.MULTILINE)
    if match is None:
        raise SoakFailure(f"public metric {name} was unavailable")
    return int(match.group(1))


class InternalQueueSampler(threading.Thread):
    """Samples the packaged daemon's public bounded telemetry queue facts."""

    def __init__(self, base_url: str) -> None:
        super().__init__(daemon=True)
        self.base_url = base_url
        self.depths: list[int] = []
        self.capacities: list[int] = []
        self.dropped: list[int] = []
        self._stop_event = threading.Event()
        self._error: BaseException | None = None

    def run(self) -> None:
        while not self._stop_event.wait(0.05):
            try:
                with urllib.request.urlopen(
                    f"{self.base_url}/metrics", timeout=1
                ) as response:
                    raw = response.read(1024 * 1024 + 1)
                if len(raw) > 1024 * 1024:
                    raise SoakFailure("public metrics response exceeded its bound")
                text = raw.decode("utf-8")
                self.depths.append(metric_value(text, "worldstream_telemetry_queued"))
                self.capacities.append(
                    metric_value(text, "worldstream_telemetry_queue_capacity")
                )
                self.dropped.append(
                    metric_value(text, "worldstream_telemetry_dropped_total")
                )
            except (OSError, UnicodeDecodeError, ValueError, SoakFailure) as error:
                self._error = error
                return

    def finish(self) -> dict[str, Any]:
        self._stop_event.set()
        self.join(timeout=2)
        if self.is_alive():
            raise SoakFailure("internal queue sampler did not stop within its bound")
        if self._error is not None:
            raise SoakFailure(
                "internal queue metrics could not be sampled"
            ) from self._error
        if not self.depths or len(self.depths) != len(self.capacities):
            raise SoakFailure("internal queue metrics had no complete samples")
        capacities = set(self.capacities)
        if capacities != {DEFAULT_INTERNAL_QUEUE_HARD_LIMIT}:
            raise SoakFailure(
                "packaged daemon queue capacity differed from the hard limit"
            )
        maximum_depth = max(self.depths)
        if maximum_depth > DEFAULT_INTERNAL_QUEUE_HARD_LIMIT:
            raise SoakFailure("internal queue exceeded its configured hard limit")
        return {
            "status": "measured",
            "name": "telemetry_exporter",
            "measurement_source": "public_prometheus_metrics",
            "depth_metric": "worldstream_telemetry_queued",
            "capacity_metric": "worldstream_telemetry_queue_capacity",
            "configured_hard_limit": DEFAULT_INTERNAL_QUEUE_HARD_LIMIT,
            "maximum_observed_depth": maximum_depth,
            "sample_count": len(self.depths),
            "dropped_total_initial": self.dropped[0],
            "dropped_total_final": self.dropped[-1],
            "dropped_total_delta": self.dropped[-1] - self.dropped[0],
            "bound_status": "pass",
        }

    def cancel(self) -> None:
        self._stop_event.set()
        self.join(timeout=2)


def public_json(base_url: str, path: str) -> dict[str, Any]:
    try:
        with urllib.request.urlopen(f"{base_url}{path}", timeout=5) as response:
            value = json.loads(response.read())
    except (OSError, ValueError) as error:
        raise SoakFailure(f"public {path} disclosure was unavailable") from error
    if not isinstance(value, dict):
        raise SoakFailure(f"public {path} disclosure was not an object")
    return value


def cpu_model() -> str:
    try:
        for line in (
            pathlib.Path("/proc/cpuinfo").read_text(encoding="utf-8").splitlines()
        ):
            if line.startswith("model name") and ":" in line:
                value = line.split(":", 1)[1].strip()
                if value:
                    return value
    except OSError as error:
        raise SoakFailure("Linux CPU model could not be measured") from error
    raise SoakFailure("Linux CPU model could not be measured")


def effective_memory_bytes() -> int:
    candidates: list[int] = []
    try:
        for line in (
            pathlib.Path("/proc/meminfo").read_text(encoding="utf-8").splitlines()
        ):
            if line.startswith("MemTotal:"):
                candidates.append(int(line.split()[1]) * 1024)
                break
    except (OSError, ValueError) as error:
        raise SoakFailure("Linux memory capacity could not be measured") from error
    for path in (
        pathlib.Path("/sys/fs/cgroup/memory.max"),
        pathlib.Path("/sys/fs/cgroup/memory/memory.limit_in_bytes"),
    ):
        try:
            value = path.read_text(encoding="utf-8").strip()
        except OSError:
            continue
        if value != "max":
            try:
                limit = int(value)
            except ValueError as error:
                raise SoakFailure("Linux cgroup memory limit was invalid") from error
            if 0 < limit < 1 << 60:
                candidates.append(limit)
    if not candidates or min(candidates) <= 0:
        raise SoakFailure("Linux memory capacity could not be measured")
    return min(candidates)


def unescape_mount_path(value: str) -> str:
    for encoded, decoded in (
        ("\\040", " "),
        ("\\011", "\t"),
        ("\\012", "\n"),
        ("\\134", "\\"),
    ):
        value = value.replace(encoded, decoded)
    return value


def linux_distribution(
    os_release_path: pathlib.Path = pathlib.Path("/etc/os-release"),
) -> dict[str, str]:
    """Read a bounded os-release file without executing its shell syntax."""

    try:
        raw = os_release_path.read_bytes()
    except OSError as error:
        raise SoakFailure("Linux distribution disclosure was unavailable") from error
    if not raw or len(raw) > MAX_OS_RELEASE_BYTES or b"\0" in raw:
        raise SoakFailure("Linux distribution disclosure was invalid")
    try:
        lines = raw.decode("utf-8").splitlines()
    except UnicodeDecodeError as error:
        raise SoakFailure("Linux distribution disclosure was not UTF-8") from error
    values: dict[str, str] = {}
    for line in lines:
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        key, separator, encoded = stripped.partition("=")
        if not separator or re.fullmatch(r"[A-Z][A-Z0-9_]*", key) is None:
            raise SoakFailure("Linux distribution disclosure was malformed")
        try:
            parsed = shlex.split(encoded, comments=False, posix=True)
        except ValueError as error:
            raise SoakFailure("Linux distribution disclosure was malformed") from error
        if len(parsed) != 1 or any(ord(character) < 32 for character in parsed[0]):
            raise SoakFailure("Linux distribution disclosure was malformed")
        values[key] = parsed[0]
    distribution = values.get("NAME")
    distribution_version = values.get("VERSION_ID")
    if not distribution or not distribution_version:
        raise SoakFailure("Linux distribution identity was incomplete")
    return {
        "distribution": distribution,
        "distribution_version": distribution_version,
    }


def sysfs_block_leaves(
    device_id: str, sys_dev_block: pathlib.Path
) -> list[pathlib.Path]:
    """Resolve a mounted block device through any device-mapper slave graph."""

    if re.fullmatch(r"[0-9]+:[0-9]+", device_id) is None:
        raise SoakFailure("Linux mount block-device identity was invalid")
    try:
        root = (sys_dev_block / device_id).resolve(strict=True)
    except OSError as error:
        raise SoakFailure(
            "Linux mount block-device identity was unavailable"
        ) from error
    pending = [root]
    leaves: list[pathlib.Path] = []
    visited: set[pathlib.Path] = set()
    while pending:
        current = pending.pop()
        if current in visited:
            raise SoakFailure("Linux block-device topology contained a cycle")
        visited.add(current)
        slaves_dir = current / "slaves"
        try:
            slaves = sorted(slaves_dir.iterdir()) if slaves_dir.is_dir() else []
            resolved_slaves = [slave.resolve(strict=True) for slave in slaves]
        except OSError as error:
            raise SoakFailure("Linux block-device topology was unavailable") from error
        if resolved_slaves:
            pending.extend(resolved_slaves)
        else:
            leaves.append(current)
    if not leaves:
        raise SoakFailure("Linux block-device topology had no physical leaves")
    return leaves


def rotational_value(device: pathlib.Path) -> str:
    """Read the nearest queue/rotational disclosure for a device or partition."""

    for candidate in (device, *device.parents):
        value_path = candidate / "queue" / "rotational"
        try:
            if value_path.is_file():
                return value_path.read_text(encoding="utf-8").strip()
        except OSError as error:
            raise SoakFailure(
                "Linux block-device rotation status was unavailable"
            ) from error
        if candidate == pathlib.Path("/sys"):
            break
    raise SoakFailure("Linux block-device rotation status was unavailable")


def storage_class(device_id: str, sys_dev_block: pathlib.Path) -> str:
    leaves = sysfs_block_leaves(device_id, sys_dev_block)
    for leaf in leaves:
        # Loop, RAM, zram, and network block devices can all claim to be
        # non-rotational; none is a certified local SSD/NVMe backing device.
        name = leaf.name.lower()
        if re.match(r"(?:loop|ram|zram|nbd|rbd|drbd)", name):
            raise SoakFailure(
                "daemon data directory was not backed by a local SSD/NVMe"
            )
        if rotational_value(leaf) != "0":
            raise SoakFailure(
                "daemon data directory was not backed by a local SSD/NVMe"
            )
    return "local_ssd_or_nvme"


def filesystem_environment(
    path: pathlib.Path,
    mountinfo_path: pathlib.Path = pathlib.Path("/proc/self/mountinfo"),
    sys_dev_block: pathlib.Path = pathlib.Path("/sys/dev/block"),
) -> dict[str, Any]:
    target = path.resolve()
    selected: tuple[int, str, list[str], str] | None = None
    try:
        lines = mountinfo_path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise SoakFailure("Linux filesystem disclosure was unavailable") from error
    for line in lines:
        before, separator, after = line.partition(" - ")
        left = before.split()
        right = after.split()
        if not separator or len(left) < 6 or len(right) < 3:
            raise SoakFailure("Linux mountinfo row was malformed")
        mount_point = pathlib.Path(unescape_mount_path(left[4]))
        try:
            target.relative_to(mount_point)
        except ValueError:
            continue
        options = sorted(set(left[5].split(",") + right[2].split(",")))
        candidate = (len(mount_point.parts), right[0], options, left[2])
        if selected is None or candidate[0] > selected[0]:
            selected = candidate
    if selected is None or selected[1] not in {"ext4", "xfs"} or not selected[2]:
        raise SoakFailure(
            "daemon data directory requires an ext4 or XFS reference mount"
        )
    return {
        "type": selected[1],
        "mount_options": selected[2],
        "storage_class": storage_class(selected[3], sys_dev_block),
    }


def environment_observation(daemon: Any) -> dict[str, Any]:
    """Disclose only facts this SQLite process workload actually observes."""

    version = public_json(daemon.base_url, "/version")
    engine = version.get("engine")
    if (
        not isinstance(engine, dict)
        or engine.get("status") != "verified"
        or engine.get("profile") != "sqlite-bundled"
        or not isinstance(engine.get("exact_identity"), str)
        or not engine["exact_identity"]
    ):
        raise SoakFailure("runtime-verified SQLite identity was unavailable")
    logical_cpu_count = os.cpu_count()
    if logical_cpu_count is None or logical_cpu_count <= 0:
        raise SoakFailure("Linux logical CPU count could not be measured")
    distribution = linux_distribution()
    machine = platform.machine().lower()
    if machine == "amd64":
        machine = "x86_64"
    return {
        "platform": {
            "system": platform.system(),
            **distribution,
            "machine": machine,
        },
        "hardware": {
            "cpu_model": cpu_model(),
            "logical_cpu_count": logical_cpu_count,
            "memory_bytes": effective_memory_bytes(),
        },
        "filesystem": filesystem_environment(daemon.data_dir),
        "engines": {
            "sqlite": {
                "status": "observed_runtime_verified",
                "profile": engine["profile"],
                "exact_identity": engine["exact_identity"],
            },
            "postgresql": {
                "status": "not_observed_by_sqlite_process_soak",
            },
        },
    }


def issue_capability(
    base_url: str,
    operator_bearer: str,
    room_id: str,
    member_id: str,
    principal_id: str,
    scopes: list[str],
) -> str:
    response = COMMON.post_json(
        base_url,
        operator_bearer,
        "/v1/operator/member-capabilities",
        {
            "room_id": room_id,
            "member_id": member_id,
            "principal_id": principal_id,
            "scopes": scopes,
            "idempotency_key": COMMON.new_ulid(),
            "expires_at": None,
        },
    )
    bearer = response.get("bearer")
    if not isinstance(bearer, str) or not bearer.startswith("wsb1:"):
        raise SoakFailure("workload member capability was unavailable")
    return bearer


def projection_fingerprint(value: dict[str, Any]) -> str:
    return COMMON.canonical_hash(
        {
            "projection_hash": value.get("projection_hash"),
            "room_head": value.get("room_head"),
            "projection": value.get("projection"),
        }
    )


def read_bounded_json_report(
    path: pathlib.Path, label: str
) -> tuple[dict[str, Any], bytes]:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise SoakFailure(f"{label} report was unavailable") from error
    if (
        path.is_symlink()
        or not path.is_file()
        or metadata.st_size > MAX_AUXILIARY_REPORT_BYTES
    ):
        raise SoakFailure(f"{label} report was not a bounded regular file")
    try:
        raw = path.read_bytes()
        value = json.loads(raw.decode("utf-8"))
    except (OSError, UnicodeDecodeError, ValueError) as error:
        raise SoakFailure(f"{label} report was invalid UTF-8 JSON") from error
    if not isinstance(value, dict):
        raise SoakFailure(f"{label} report root was not an object")
    return value, raw


def validate_preflight(report: dict[str, Any]) -> dict[str, Any]:
    """Require the ordinary SQLite conformance fixture preflight to be complete."""

    parsed = report.get("preflight", {}).get("test_list_parse")
    hooks = report.get("fixture_hooks")
    runs = report.get("matrix_runs")
    if (
        report.get("schema") != SCHEMA
        or report.get("status") != "pass"
        or report.get("release_evidence") is not False
        or not isinstance(parsed, dict)
        or parsed.get("status") != "pass"
        or parsed.get("missing_groups") != []
        or not isinstance(hooks, list)
        or not isinstance(runs, list)
        or not runs
        or runs[0].get("test_count_validation", {}).get("status") != "pass"
    ):
        raise SoakFailure("SQLite evidence preflight was incomplete")
    return report


def run_preflight(total_timeout: float, max_output_bytes: int) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(
        prefix="worldstream-daemon-soak-preflight-"
    ) as temporary:
        output = pathlib.Path(temporary) / "preflight.json"
        command = [
            "bash",
            str(ROOT / "scripts" / "soak-smoke.sh"),
            "--iterations",
            "1",
            "--command-timeout-seconds",
            "120",
            "--max-total-seconds",
            str(total_timeout),
            "--max-output-bytes",
            str(max_output_bytes),
            "--output",
            str(output),
        ]
        try:
            result = subprocess.run(
                command,
                cwd=ROOT,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=total_timeout + 10,
                check=False,
            )
        except subprocess.TimeoutExpired as error:
            raise SoakFailure("SQLite evidence preflight exceeded its bound") from error
        if result.returncode != 0 or not output.is_file():
            raise SoakFailure("SQLite evidence preflight did not pass")
        report, _ = read_bounded_json_report(output, "SQLite evidence preflight")
    return validate_preflight(report)


def supplied_preflight(path: pathlib.Path) -> dict[str, Any]:
    report, _ = read_bounded_json_report(path, "SQLite evidence preflight")
    return validate_preflight(report)


def packaged_acceptance_sha256(
    path: pathlib.Path | None, distribution: dict[str, Any]
) -> str | None:
    """Bind exact raw six-cell acceptance bytes only after closed validation."""

    if path is None:
        return None
    report, raw = read_bounded_json_report(path, "packaged acceptance")
    binding = report.get("package_binding")
    cells = report.get("cells")
    comparison = report.get("comparison")
    expected_backends = {"sqlite", "postgres_direct", "transaction_pooler"}
    identity = binding.get("identity") if isinstance(binding, dict) else None
    if (
        report.get("schema") != PACKAGED_ACCEPTANCE_SCHEMA
        or report.get("status") != "pass"
        or report.get("release_evidence") is not True
        or report.get("secrets_emitted") is not False
        or report.get("cleanup") != "pass"
        or not isinstance(binding, dict)
        or binding.get("status") != "pass"
        or binding.get("archive_sha256") != distribution.get("archive_sha256")
        or not isinstance(identity, dict)
        or identity.get("target") != distribution.get("target")
        or identity.get("version") != distribution.get("version")
        or "sha256:" + str(identity.get("manifest_sha256"))
        != distribution.get("manifest_sha256")
        or "sha256:" + str(identity.get("manifest_json_sha256"))
        != distribution.get("manifest_json_sha256")
        or "sha256:" + str(identity.get("manifest_toml_sha256"))
        != distribution.get("manifest_toml_sha256")
        or not isinstance(cells, dict)
        or set(cells) != {"counter", "heist"}
        or not isinstance(comparison, dict)
        or any(
            not isinstance(comparison.get(story), dict)
            or comparison[story].get("status") != "pass"
            for story in ("counter", "heist")
        )
    ):
        raise SoakFailure("packaged acceptance report did not verify this archive")
    for story in ("counter", "heist"):
        story_cells = cells[story]
        if not isinstance(story_cells, dict) or set(story_cells) != expected_backends:
            raise SoakFailure("packaged acceptance report did not contain six cells")
        for cell in story_cells.values():
            if (
                not isinstance(cell, dict)
                or cell.get("exit_code") != 0
                or not isinstance(cell.get("report"), dict)
                or cell["report"].get("status") != "completed"
            ):
                raise SoakFailure("packaged acceptance cell was not complete")
    return "sha256:" + hashlib.sha256(raw).hexdigest()


async def next_frame(iterator: Any, timeout: float = 5) -> dict[str, Any]:
    try:
        value = await asyncio.wait_for(iterator.__anext__(), timeout)
    except asyncio.TimeoutError as error:
        raise SoakFailure("public fan-out frame was not observed") from error
    if not isinstance(value, dict):
        raise SoakFailure("public fan-out frame was invalid")
    return value


@dataclass
class LastRoom:
    room_id: str
    member_bearer: str
    projection_hash: str


async def run_transition_workload(
    daemon: Any, duration_seconds: float
) -> tuple[dict[str, Any], LastRoom, list[bytes]]:
    operator = Client(daemon.base_url, daemon.operator_bearer)
    deadline = time.monotonic() + duration_seconds
    accepted = 0
    room_count = 0
    observed_frames = 0
    latencies: list[float] = []
    payload_sizes: set[int] = set()
    capability_sentinels: list[bytes] = []
    last_room: LastRoom | None = None
    while time.monotonic() < deadline:
        principals = [COMMON.new_ulid(), COMMON.new_ulid(), COMMON.new_ulid()]
        created = await operator.create_room(
            {
                "pack": COUNTER_PACK,
                "configuration": {"initial_value": 0, "maximum_value": 16},
                "members": [
                    {
                        "principal_id": principals[0],
                        "principal_kind": "agent",
                        "role": "counter",
                        "access_mode": "participant",
                    },
                    *[
                        {
                            "principal_id": principals[index],
                            "principal_kind": "human",
                            "role": None,
                            "access_mode": "spectator",
                        }
                        for index in (1, 2)
                    ],
                ],
                "idempotency_key": COMMON.new_ulid(),
            }
        )
        room_id = created["room_id"]
        member_ids = created.get("member_ids")
        if not isinstance(member_ids, list) or len(member_ids) != 3:
            raise SoakFailure("Counter workload room shape was invalid")
        participant_bearer = await asyncio.to_thread(
            issue_capability,
            daemon.base_url,
            daemon.operator_bearer,
            room_id,
            member_ids[0],
            principals[0],
            ["room:attach", "room:observe_member", "room:act", "room:replay"],
        )
        spectator_bearers = [
            await asyncio.to_thread(
                issue_capability,
                daemon.base_url,
                daemon.operator_bearer,
                room_id,
                member_ids[index],
                principals[index],
                ["room:attach", "room:observe_public", "room:replay"],
            )
            for index in (1, 2)
        ]
        if not capability_sentinels:
            capability_sentinels = [
                participant_bearer.encode("ascii"),
                *(bearer.encode("ascii") for bearer in spectator_bearers),
            ]
        participant_client = Client(daemon.base_url, participant_bearer)
        spectator_clients = [
            Client(daemon.base_url, bearer) for bearer in spectator_bearers
        ]
        participant = await participant_client.open_room(room_id, member_ids[0])
        spectators = [
            await client.open_room(room_id, member_ids[index])
            for index, client in zip((1, 2), spectator_clients, strict=True)
        ]
        await participant.sync()
        for spectator in spectators:
            await spectator.sync()
        iterators = [spectator.events() for spectator in spectators]
        try:
            for _ in range(COUNTER_ACTIONS_PER_ROOM):
                if time.monotonic() >= deadline:
                    break
                started = time.perf_counter_ns()
                result = await participant.act(
                    "increment", {}, action_id=COMMON.new_ulid(), timeout=15
                )
                latencies.append((time.perf_counter_ns() - started) / 1_000_000)
                if not isinstance(result.get("transition_id"), str):
                    code = result.get("code")
                    safe_code = code if isinstance(code, str) else "invalid_result"
                    raise SoakFailure(
                        f"Counter workload Action was rejected with {safe_code}"
                    )
                if result.get("duplicate") is not False:
                    raise SoakFailure(
                        "Counter workload first Action identity resolved as duplicate"
                    )
                payload_sizes.add(len(json.dumps({}, separators=(",", ":")).encode()))
                frames = await asyncio.gather(
                    *(next_frame(iterator) for iterator in iterators)
                )
                if any(
                    frame.get("cause_room_seq")
                    != result.get("room_head", {}).get("room_seq")
                    for frame in frames
                ):
                    raise SoakFailure(
                        "public fan-out frame did not match the accepted transition"
                    )
                accepted += 1
                observed_frames += len(frames)
        finally:
            for iterator in iterators:
                await iterator.aclose()
            await participant.close()
            for spectator in spectators:
                await spectator.close()
        room_count += 1
        final_projection = await participant_client.projection(room_id)
        last_room = LastRoom(
            room_id,
            participant_bearer,
            projection_fingerprint(final_projection),
        )
    if accepted <= 0 or last_room is None:
        raise SoakFailure("daemon workload accepted no transitions")
    elapsed = time.monotonic() - (deadline - duration_seconds)
    return (
        {
            "kind": "worldstreamd_transition_workload",
            "database_binding": WORKLOAD_BINDING,
            "accepted_transition_count": accepted,
            "room_count": room_count,
            "observed_public_frame_count": observed_frames,
            "fan_out_memberships_per_room": 3,
            "observer_fan_out_per_transition": 2,
            "maximum_inflight_actions": 1,
            "actions_per_room_bound": COUNTER_ACTIONS_PER_ROOM,
            "load_profile": "sequential_counter_increment_with_two_live_spectators",
            "accepted_transitions_per_second": round(accepted / elapsed, 3),
            "action_payload_sizes_bytes": sorted(payload_sizes),
            "snapshot_cadence_transitions": 1,
            "snapshot_cadence_definition": (
                "one post-commit paired projection snapshot attempt per accepted transition"
            ),
            "latencies_ms": latencies,
        },
        last_room,
        capability_sentinels,
    )


def externally_kill(daemon: Any, timeout: float) -> int:
    process = daemon.process
    if process is None or process.poll() is not None:
        raise SoakFailure("daemon was unavailable for recovery measurement")
    os.kill(process.pid, signal.SIGKILL)
    try:
        exit_code = process.wait(timeout=timeout)
    except subprocess.TimeoutExpired as error:
        raise SoakFailure("daemon did not exit after recovery SIGKILL") from error
    if exit_code != -signal.SIGKILL:
        raise SoakFailure("recovery termination was not SIGKILL")
    daemon.process = None
    daemon._close_log()
    return exit_code


def reference_identity(
    distribution: dict[str, Any], acceptance_sha256: str | None
) -> dict[str, Any]:
    identity = {
        "product": "worldstream",
        "profile": (
            "linux-reference"
            if distribution["packaged_artifact_bound"]
            else "source-build-diagnostic"
        ),
        "version": distribution.get("version", "unversioned-source-build"),
        "artifact_sha256": distribution.get(
            "archive_sha256", distribution["binary_sha256"]
        ),
    }
    if acceptance_sha256 is not None:
        identity["packaged_acceptance_sha256"] = acceptance_sha256
    return identity


def reference_workload_disclosure(workload: dict[str, Any]) -> dict[str, Any]:
    return {
        "payload_sizes_bytes": workload["action_payload_sizes_bytes"],
        "pack_id": COUNTER_PACK["id"],
        "participants_per_room": workload["fan_out_memberships_per_room"],
        "fan_out": workload["observer_fan_out_per_transition"],
        "snapshot_cadence_transitions": workload["snapshot_cadence_transitions"],
    }


async def build_report(args: argparse.Namespace) -> tuple[dict[str, Any], str]:
    if platform.system() != "Linux" or platform.machine().lower() not in {
        "x86_64",
        "amd64",
    }:
        raise SoakFailure("daemon transition soak requires Linux x86-64")
    if not args.daemon_bin.is_file() or not os.access(args.daemon_bin, os.X_OK):
        raise SoakFailure("worldstreamd is unavailable or not executable")
    distribution = COMMON.distribution_identity(
        args.daemon_bin, args.package_archive, args.package_report
    )
    acceptance_sha256 = packaged_acceptance_sha256(
        args.packaged_acceptance_report, distribution
    )
    target_seconds = workload_target_seconds(args.one_hour, args.duration_seconds)
    if args.preflight_report is None:
        preflight = await asyncio.to_thread(
            run_preflight, args.preflight_timeout_seconds, args.max_output_bytes
        )
    else:
        preflight = await asyncio.to_thread(supplied_preflight, args.preflight_report)
    root = COMMON.private_root("worldstream-daemon-transition-soak-")
    daemon = COMMON.Daemon(
        root,
        args.daemon_bin,
        startup_timeout=args.startup_timeout_seconds,
        shutdown_timeout=args.shutdown_timeout_seconds,
    )
    started = time.monotonic()
    sampler: RssSampler | None = None
    queue_sampler: InternalQueueSampler | None = None
    try:
        daemon.start()
        observed_environment = environment_observation(daemon)
        process = daemon.process
        if process is None:
            raise SoakFailure("daemon process identity was unavailable")
        sampler = RssSampler(process.pid)
        sampler.start()
        queue_sampler = InternalQueueSampler(daemon.base_url)
        queue_sampler.start()
        initial_database = sqlite_sizes(daemon.data_dir)
        database_files = {
            daemon.data_dir / "worldstream.sqlite3",
            pathlib.Path(f"{daemon.data_dir / 'worldstream.sqlite3'}-wal"),
            pathlib.Path(f"{daemon.data_dir / 'worldstream.sqlite3'}-shm"),
            root / "authority.secret",
        }
        initial_auxiliary = auxiliary_snapshot(root, database_files)
        workload_started = time.monotonic()
        workload, last_room, capability_sentinels = await run_transition_workload(
            daemon, target_seconds
        )
        workload_elapsed = time.monotonic() - workload_started
        peak_rss = sampler.finish()
        rss_sample_count = sampler.samples
        sampler = None
        internal_queue = queue_sampler.finish()
        queue_sampler = None
        final_database = sqlite_sizes(daemon.data_dir)
        main_growth = final_database["main_bytes"] - initial_database["main_bytes"]
        wal_growth = final_database["wal_bytes"] - initial_database["wal_bytes"]
        database_initial = (
            initial_database["main_bytes"] + initial_database["wal_bytes"]
        )
        database_final = final_database["main_bytes"] + final_database["wal_bytes"]
        database_growth = database_final - database_initial
        if min(main_growth, wal_growth, database_growth) < 0:
            raise SoakFailure("a measured storage growth value was negative")
        if (
            database_growth > args.max_database_growth_bytes
            or wal_growth > args.max_wal_growth_bytes
            or peak_rss > args.max_peak_rss_bytes
        ):
            raise SoakFailure("daemon workload exceeded a configured resource bound")
        before_recovery_store = COMMON.store_identity(daemon)
        externally_kill(daemon, args.shutdown_timeout_seconds)
        recovery_started = time.monotonic()
        daemon.start()
        recovery_ms = (time.monotonic() - recovery_started) * 1000
        after_recovery_store = COMMON.store_identity(daemon)
        if before_recovery_store != after_recovery_store:
            raise SoakFailure("recovery did not reopen the same SQLite file")
        recovery_client = Client(daemon.base_url, last_room.member_bearer)
        recovered_projection = await COMMON.projection_with_retry(
            recovery_client, last_room.room_id, 15
        )
        recovery_equal = (
            projection_fingerprint(recovered_projection) == last_room.projection_hash
        )
        if not recovery_equal:
            raise SoakFailure(
                "post-SIGKILL recovery changed the final public projection"
            )
        daemon.stop()
        final_auxiliary = auxiliary_snapshot(root, database_files)
        temp_growth = (
            final_auxiliary["temporary_bytes"] - initial_auxiliary["temporary_bytes"]
        )
        log_growth = final_auxiliary["log_bytes"] - initial_auxiliary["log_bytes"]
        artifact_growth = (
            final_auxiliary["artifact_bytes"] - initial_auxiliary["artifact_bytes"]
        )
        if min(temp_growth, log_growth, artifact_growth) < 0:
            raise SoakFailure("an auxiliary artifact growth value was negative")
        if artifact_growth != temp_growth + log_growth:
            raise SoakFailure("auxiliary artifact growth categories did not reconcile")
        if (
            temp_growth > args.max_temp_growth_bytes
            or log_growth > args.max_output_bytes
            or artifact_growth > args.max_artifact_growth_bytes
        ):
            raise SoakFailure("daemon auxiliary artifacts exceeded a configured bound")
        output_bytes = final_auxiliary["log_bytes"]
        if output_bytes > args.max_output_bytes:
            raise SoakFailure("daemon logs exceeded the configured output bound")
        secret_sentinels = {
            "authority-secret": (root / "authority.secret").read_bytes(),
            "operator-capability": daemon.operator_bearer.encode("ascii"),
            **{
                f"member-capability-{index:02d}": value
                for index, value in enumerate(capability_sentinels, start=1)
            },
        }
        log_channels = {
            f"daemon-log-{index:02d}": path
            for index, path in enumerate(final_auxiliary["logs"], start=1)
        }
        try:
            secret_scan = SECRET_SCAN.scan_sentinels(secret_sentinels, log_channels)
        except SECRET_SCAN.ScanError as error:
            raise SoakFailure("daemon log secret-absence scan failed closed") from error
        log_hashes = [row["sha256"] for row in secret_scan["channels"]]
        latencies = workload.pop("latencies_ms")
        duration_percentiles = {
            "definition": "nearest-rank accepted Action acknowledgement latency",
            "p50": COMMON.percentile(latencies, 0.50),
            "p95": COMMON.percentile(latencies, 0.95),
            "p99": COMMON.percentile(latencies, 0.99),
        }
        direct_ack_latency = {
            "definition": "nearest-rank",
            "sample_count": len(latencies),
            "p50_ms": duration_percentiles["p50"],
            "p95_ms": duration_percentiles["p95"],
            "p99_ms": duration_percentiles["p99"],
        }
        parsed = preflight["preflight"]["test_list_parse"]
        fixture_hooks = preflight["fixture_hooks"]
        release_window_complete = release_window_is_complete(
            one_hour=args.one_hour,
            target_seconds=target_seconds,
            elapsed_seconds=workload_elapsed,
        )
        identity = reference_identity(distribution, acceptance_sha256)
        reference_workload = reference_workload_disclosure(workload)
        measurements = {
            "latency_ms": direct_ack_latency,
            "load": {
                "connections": workload["fan_out_memberships_per_room"],
                "active_rooms": 1,
                "actions_per_second": workload["accepted_transitions_per_second"],
                "transition_rate_per_second": workload[
                    "accepted_transitions_per_second"
                ],
            },
            "fan_out": {
                "observation_fan_out": workload["observer_fan_out_per_transition"],
                "observers_per_room": workload["observer_fan_out_per_transition"],
                "frames_per_transition": workload["observer_fan_out_per_transition"],
            },
            "memory": {
                "status": "measured",
                "peak_rss_bytes": peak_rss,
                "method": "50ms /proc process-tree VmRSS samples",
            },
            "database_growth": {
                "status": "measured",
                "growth_bytes": database_growth,
                "wal_growth_bytes": wal_growth,
            },
            "temporary_growth": {
                "status": "measured",
                "growth_bytes": temp_growth,
                "configured_hard_limit_bytes": args.max_temp_growth_bytes,
            },
            "log_growth": {
                "status": "measured",
                "growth_bytes": log_growth,
                "configured_hard_limit_bytes": args.max_output_bytes,
            },
            "artifact_growth": {
                "status": "measured",
                "growth_bytes": artifact_growth,
                "configured_hard_limit_bytes": args.max_artifact_growth_bytes,
            },
            "internal_queues": {
                "status": "measured",
                "queues": [internal_queue],
            },
            "recovery": {"durations_ms": [round(recovery_ms, 3)]},
        }
        report = {
            "schema": SCHEMA,
            "status": "pass",
            "release_evidence": False,
            "release_candidate_input": bool(
                release_window_complete and distribution["packaged_artifact_bound"]
            ),
            "evidence_class": "process_level_daemon_workload",
            "performance_class": "reference_non_release",
            "mode": "one_hour" if args.one_hour else "short",
            "platform": {"system": platform.system(), "machine": platform.machine()},
            "identity": identity,
            "environment_observation": observed_environment,
            "reference_workload": reference_workload,
            "measurements": measurements,
            "distribution": distribution,
            "configuration": {
                "max_total_seconds": target_seconds,
                "one_hour_target_seconds": ONE_HOUR_SECONDS if args.one_hour else None,
                "max_output_bytes": args.max_output_bytes,
                "max_database_growth_bytes": args.max_database_growth_bytes,
                "max_wal_growth_bytes": args.max_wal_growth_bytes,
                "max_temp_growth_bytes": args.max_temp_growth_bytes,
                "max_log_growth_bytes": args.max_output_bytes,
                "max_artifact_growth_bytes": args.max_artifact_growth_bytes,
                "max_internal_queue_depth": DEFAULT_INTERNAL_QUEUE_HARD_LIMIT,
                "max_peak_rss_bytes": args.max_peak_rss_bytes,
                "database_workload_binding": WORKLOAD_BINDING,
            },
            "named_gate_evidence": {
                "manifest_evidence_id": EVIDENCE_ID,
                "gate_cell": EVIDENCE_ID,
                "release_gate": True,
                "release_evidence": False,
                "handoff_status": (
                    "eligible_input_after_strict_producer_validation"
                    if release_window_complete
                    and distribution["packaged_artifact_bound"]
                    else "diagnostic_only"
                ),
            },
            "evidence_scope": {
                "fixture_only": False,
                "process_level": True,
                "database_workload_bound": True,
                "process_kill_recovery": True,
            },
            "preflight": {"test_list_parse": parsed},
            "fixture_hooks": fixture_hooks,
            "matrix_runs": [
                {
                    "status": "passed",
                    "failure_class": "none",
                    "kind": "worldstreamd_transition_workload",
                    "output_bytes": output_bytes,
                    "output_truncated": False,
                    "test_count_validation": preflight["matrix_runs"][0][
                        "test_count_validation"
                    ],
                    "accepted_transition_count": workload["accepted_transition_count"],
                }
            ],
            "statistics": {
                "matrix_run_count": 1,
                "command_duration_ms": duration_percentiles,
                "ack_latency_ms": direct_ack_latency,
                "memory": {
                    "status": "measured",
                    "scope": "worldstreamd_process_tree",
                    "peak_rss_bytes": peak_rss,
                    "peak_rss_bytes_per_run": [peak_rss],
                    "observed_peak_delta_bytes": 0,
                    "sample_count": rss_sample_count,
                },
            },
            "database": {
                "status": "measured",
                "workload_binding": WORKLOAD_BINDING,
                "initial_bytes": database_initial,
                "final_bytes": database_final,
                "growth_bytes": database_growth,
                "main_initial_bytes": initial_database["main_bytes"],
                "main_final_bytes": final_database["main_bytes"],
                "main_growth_bytes": main_growth,
                "wal_initial_bytes": initial_database["wal_bytes"],
                "wal_final_bytes": final_database["wal_bytes"],
                "wal_growth_bytes": wal_growth,
                "shm_initial_bytes": initial_database["shm_bytes"],
                "shm_final_bytes": final_database["shm_bytes"],
                "growth_bound_status": "pass",
                "wal_growth_bound_status": "pass",
            },
            "temp_and_artifacts": {
                "status": "measured",
                "temporary": {
                    "initial_bytes": initial_auxiliary["temporary_bytes"],
                    "final_bytes": final_auxiliary["temporary_bytes"],
                    "growth_bytes": temp_growth,
                    "configured_hard_limit_bytes": args.max_temp_growth_bytes,
                    "bound_status": "pass",
                },
                "logs": {
                    "initial_bytes": initial_auxiliary["log_bytes"],
                    "final_bytes": final_auxiliary["log_bytes"],
                    "growth_bytes": log_growth,
                    "configured_hard_limit_bytes": args.max_output_bytes,
                    "bound_status": "pass",
                    "file_count": len(log_hashes),
                    "sha256": log_hashes,
                },
                "artifacts": {
                    "definition": "all non-database temporary-workspace and daemon-log bytes",
                    "initial_bytes": initial_auxiliary["artifact_bytes"],
                    "final_bytes": final_auxiliary["artifact_bytes"],
                    "growth_bytes": artifact_growth,
                    "configured_hard_limit_bytes": args.max_artifact_growth_bytes,
                    "bound_status": "pass",
                },
                "initial_bytes": initial_auxiliary["artifact_bytes"],
                "final_bytes": final_auxiliary["artifact_bytes"],
                "growth_bytes": artifact_growth,
                "growth_bound_status": "pass",
                "daemon_log_count": len(log_hashes),
                "daemon_log_sha256": log_hashes,
            },
            "internal_queues": {
                "status": "measured",
                "configured_hard_limits_enforced": True,
                "queues": [internal_queue],
            },
            "privacy": {
                "status": "pass",
                "secret_scan": secret_scan,
            },
            "workload": workload,
            "recovery": {
                "status": "passed",
                "signal": "SIGKILL",
                "same_data_directory": True,
                "projection_hash_equal": True,
                "recovery_time_ms": round(recovery_ms, 3),
                "durations_ms": [round(recovery_ms, 3)],
            },
            "elapsed_seconds": round(workload_elapsed, 3),
            "total_command_elapsed_seconds": round(time.monotonic() - started, 3),
            "one_hour_window_completed": release_window_complete,
            "reference_measurement": {
                "classification": "measured_linux_reference_data",
                "sla": False,
                "percentiles": "measured_not_promised",
            },
            "limitations": [
                "Measured Linux reference data is not an SLA.",
                "Short mode is diagnostic and never one-hour release evidence.",
                "Release promotion remains owned by the strict detached producer.",
            ],
        }
        log = (
            "worldstream daemon transition soak v1\n"
            f"mode={report['mode']} target_seconds={target_seconds:.3f} "
            f"elapsed_seconds={workload_elapsed:.3f}\n"
            f"accepted_transitions={workload['accepted_transition_count']} "
            f"rooms={workload['room_count']} fan_out=3\n"
            f"p50_ms={duration_percentiles['p50']:.3f} "
            f"p95_ms={duration_percentiles['p95']:.3f} "
            f"p99_ms={duration_percentiles['p99']:.3f}\n"
            f"peak_rss_bytes={peak_rss} database_growth_bytes={database_growth} "
            f"wal_growth_bytes={wal_growth} temp_growth_bytes={temp_growth} "
            f"log_growth_bytes={log_growth} artifact_growth_bytes={artifact_growth}\n"
            f"internal_queue_max_depth={internal_queue['maximum_observed_depth']} "
            f"internal_queue_hard_limit={internal_queue['configured_hard_limit']}\n"
            f"recovery_time_ms={recovery_ms:.3f} status=passed\n"
        )
        return report, log
    finally:
        if sampler is not None:
            sampler.cancel()
        if queue_sampler is not None:
            queue_sampler.cancel()
        daemon.stop()
        shutil.rmtree(root, ignore_errors=True)


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument(
        "--daemon-bin",
        type=pathlib.Path,
        default=ROOT / "target" / "debug" / "worldstreamd",
    )
    command.add_argument("--package-archive", type=pathlib.Path)
    command.add_argument("--package-report", type=pathlib.Path)
    command.add_argument(
        "--packaged-acceptance-report",
        type=pathlib.Path,
        help=(
            "optional exact six-cell packaged acceptance report; its raw hash is "
            "bound only after closed validation"
        ),
    )
    command.add_argument(
        "--preflight-report",
        type=pathlib.Path,
        help="optional previously generated passing soak-smoke report",
    )
    command.add_argument("--one-hour", action="store_true")
    command.add_argument("--duration-seconds", type=float, default=10)
    command.add_argument("--startup-timeout-seconds", type=float, default=30)
    command.add_argument("--shutdown-timeout-seconds", type=float, default=10)
    command.add_argument("--preflight-timeout-seconds", type=float, default=300)
    command.add_argument(
        "--max-output-bytes", type=int, default=DEFAULT_MAX_OUTPUT_BYTES
    )
    command.add_argument(
        "--max-database-growth-bytes",
        type=int,
        default=DEFAULT_MAX_DATABASE_GROWTH_BYTES,
    )
    command.add_argument(
        "--max-wal-growth-bytes", type=int, default=DEFAULT_MAX_WAL_GROWTH_BYTES
    )
    command.add_argument(
        "--max-temp-growth-bytes", type=int, default=DEFAULT_MAX_TEMP_GROWTH_BYTES
    )
    command.add_argument(
        "--max-artifact-growth-bytes",
        type=int,
        default=DEFAULT_MAX_ARTIFACT_GROWTH_BYTES,
    )
    command.add_argument("--max-peak-rss-bytes", type=int, default=MAX_PEAK_RSS_BYTES)
    command.add_argument("--output", type=pathlib.Path)
    command.add_argument("--log-output", type=pathlib.Path)
    return command


def main() -> int:
    args = parser().parse_args()
    if not args.one_hour and not 1 <= args.duration_seconds <= 300:
        print(
            "daemon transition soak: short duration must be in 1..300 seconds",
            file=sys.stderr,
        )
        return 2
    if any(
        value < 1024
        for value in (
            args.max_output_bytes,
            args.max_database_growth_bytes,
            args.max_wal_growth_bytes,
            args.max_temp_growth_bytes,
            args.max_artifact_growth_bytes,
            args.max_peak_rss_bytes,
        )
    ):
        print(
            "daemon transition soak: resource bounds must be at least 1024",
            file=sys.stderr,
        )
        return 2
    if args.max_artifact_growth_bytes < (
        args.max_temp_growth_bytes + args.max_output_bytes
    ):
        print(
            "daemon transition soak: artifact bound must cover temp plus log bounds",
            file=sys.stderr,
        )
        return 2
    try:
        report, log = asyncio.run(build_report(args))
        encoded = json.dumps(report, sort_keys=True, separators=(",", ":")) + "\n"
        if args.output is not None:
            COMMON.atomic_write(args.output, encoded)
        if args.log_output is not None:
            COMMON.atomic_write(args.log_output, log)
        sys.stdout.write(encoded)
        print("daemon transition soak: process workload passed", file=sys.stderr)
        return 0
    except (
        SoakFailure,
        COMMON.EvidenceFailure,
        ProtocolError,
        OSError,
        ValueError,
    ) as error:
        report = {
            "schema": SCHEMA,
            "status": "failed",
            "release_evidence": False,
            "evidence_class": "process_level_daemon_workload_incomplete",
            "mode": "one_hour" if args.one_hour else "short",
            "platform": {"system": platform.system(), "machine": platform.machine()},
            "one_hour_window_completed": False,
            "reason": type(error).__name__,
            "failure_code": (
                str(error)
                if isinstance(error, SoakFailure)
                else "closed_runtime_failure"
            ),
        }
        encoded = json.dumps(report, sort_keys=True, separators=(",", ":")) + "\n"
        if args.output is not None:
            COMMON.atomic_write(args.output, encoded)
        if args.log_output is not None:
            COMMON.atomic_write(
                args.log_output, "worldstream daemon transition soak failed closed\n"
            )
        sys.stdout.write(encoded)
        print(
            f"daemon transition soak: failed closed ({type(error).__name__})",
            file=sys.stderr,
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

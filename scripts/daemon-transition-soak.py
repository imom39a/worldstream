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
import errno
import hashlib
import importlib.util
import itertools
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
BUILD_IDENTITY_PATH = ROOT / "scripts" / "release_build_identity.py"
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
DNS_RESOLVER_QUEUE_HARD_LIMIT = 2
WEBSOCKET_LIVE_PUSH_FRAME_QUEUE_HARD_LIMIT = 256
WEBSOCKET_OUTBOUND_PAYLOAD_BYTES_HARD_LIMIT = 4 * 1024 * 1024
RSS_SAMPLE_INTERVAL_SECONDS = 0.05
STORAGE_SAMPLE_INTERVAL_SECONDS = 0.25
RESOURCE_SAMPLE_MAX_GAP_SECONDS = 1.0
RESOURCE_SAMPLING_SCHEMA = "worldstream/live-resource-sampling/v1"
QUEUE_OBSERVATION_SCOPE = "all_bounded_runtime_queues_public_prometheus"
QUEUE_BOUNDARY_TEST_SCHEMA = "worldstream/queue-boundary-tests/v1"
QUEUE_BOUNDARY_MAX_TOTAL_SECONDS = 300.0
QUEUE_SPECS = (
    {
        "name": "telemetry_exporter",
        "capacity_scope": "global",
        "hard_limit": DEFAULT_INTERNAL_QUEUE_HARD_LIMIT,
        "activity_unit": "events",
    },
    {
        "name": "telemetry_dns_resolver_queue",
        "capacity_scope": "global",
        "hard_limit": DNS_RESOLVER_QUEUE_HARD_LIMIT,
        "activity_unit": "requests",
    },
    {
        "name": "room_admission_lane",
        "capacity_scope": "per_room",
        "hard_limit": DEFAULT_INTERNAL_QUEUE_HARD_LIMIT,
        "activity_unit": "reservations",
    },
    {
        "name": "websocket_live_push_frame_queue",
        "capacity_scope": "per_connection",
        "hard_limit": WEBSOCKET_LIVE_PUSH_FRAME_QUEUE_HARD_LIMIT,
        "activity_unit": "frames",
    },
    {
        "name": "websocket_outbound_payload_bytes",
        "capacity_scope": "per_connection",
        "hard_limit": WEBSOCKET_OUTBOUND_PAYLOAD_BYTES_HARD_LIMIT,
        "activity_unit": "bytes",
    },
)
QUEUE_BOUNDARY_TESTS = (
    (
        "telemetry_exporter",
        "worldstream-server",
        "telemetry::tests::postgres_bridge_is_nonblocking_when_queue_is_saturated",
    ),
    (
        "telemetry_dns_resolver_queue",
        "worldstream-server",
        "telemetry::tests::dns_timeouts_use_a_fixed_worker_and_queue_bound",
    ),
    (
        "room_admission_lane",
        "worldstream-core",
        "semantic_time::tests::coordinated_lane_rejects_full_action_before_sampling",
    ),
    (
        "websocket_live_push_frame_queue",
        "worldstream-server",
        "tests::live_frame_count_overflow_closes_with_typed_error",
    ),
    (
        "websocket_outbound_payload_bytes",
        "worldstream-server",
        "tests::live_payload_byte_overflow_closes_with_typed_error",
    ),
)
TELEMETRY_ENDPOINT_ENV = "WORLDSTREAM__TELEMETRY__OTLP__ENDPOINT"
MAX_AUXILIARY_REPORT_BYTES = 8 * 1024 * 1024
MAX_PUBLIC_JSON_BYTES = 1024 * 1024
MAX_OS_RELEASE_BYTES = 64 * 1024
PACKAGED_ACCEPTANCE_SCHEMA = "worldstream/packaged-backend-parity/v1"
DISK_FULL_SCHEMA = "worldstream/disk-full-evidence/v1"
DISK_FULL_SCENARIO = "packaged_sqlite_bootstrap_on_full_ext4_loopback"
DISK_FULL_EVIDENCE_CLASS = "process_level_daemon_fault_injection"
DISK_FULL_CONTAINER_IMAGE = (
    "docker@sha256:12e683a161823b2a839aeea999b9d960e6e1f9a97b1679ad6b441982e2d9cf07"
)
DISK_FULL_FILESYSTEM_IMAGE_BYTES = 64 * 1024 * 1024
DISK_FULL_BLOCK_SIZE_BYTES = 4096
DISK_FULL_ATTEMPTED_WRITE_BYTES = 4096
DISK_FULL_MAX_DAEMON_SECONDS = 10.0
DISK_FULL_MAX_CONTAINER_SECONDS = 120.0
DISK_FULL_MAX_LOG_BYTES = 64 * 1024
DISK_FULL_MAX_CAPTURE_BYTES = 256 * 1024
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


def load_build_identity() -> Any:
    spec = importlib.util.spec_from_file_location(
        "worldstream_daemon_soak_strict_json", BUILD_IDENTITY_PATH
    )
    if spec is None or spec.loader is None:
        raise SoakFailure("strict release JSON parser could not be loaded")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


BUILD_IDENTITY = load_build_identity()


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


def start_daemon_with_dns_telemetry(daemon: Any) -> None:
    """Start with a hostname OTLP target so the real DNS queue is exercised."""

    endpoint = f"http://localhost:{COMMON.loopback_port()}/v1/logs"
    previous = os.environ.get(TELEMETRY_ENDPOINT_ENV)
    os.environ[TELEMETRY_ENDPOINT_ENV] = endpoint
    try:
        daemon.start()
    finally:
        if previous is None:
            os.environ.pop(TELEMETRY_ENDPOINT_ENV, None)
        else:
            os.environ[TELEMETRY_ENDPOINT_ENV] = previous


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
        self._first_sample_at: float | None = None
        self._last_sample_at: float | None = None
        self._maximum_gap_seconds = 0.0

    def _sample(self) -> None:
        values = [read_proc_rss(pid) for pid in process_tree(self.pid)]
        total = sum(value for value in values if value is not None)
        if total <= 0:
            return
        observed_at = time.monotonic()
        if self._first_sample_at is None:
            self._first_sample_at = observed_at
        if self._last_sample_at is not None:
            self._maximum_gap_seconds = max(
                self._maximum_gap_seconds,
                observed_at - self._last_sample_at,
            )
        self._last_sample_at = observed_at
        self.peak_bytes = max(self.peak_bytes, total)
        self.samples += 1

    def run(self) -> None:
        while not self._stop_event.is_set():
            self._sample()
            if self._stop_event.wait(RSS_SAMPLE_INTERVAL_SECONDS):
                break

    def finish(self) -> dict[str, int | float]:
        self._stop_event.set()
        self.join(timeout=2)
        if self.is_alive():
            raise SoakFailure("worldstreamd RSS sampler did not stop within its bound")
        self._sample()
        if (
            self.peak_bytes <= 0
            or self.samples <= 0
            or self._first_sample_at is None
            or self._last_sample_at is None
        ):
            raise SoakFailure("worldstreamd process-tree RSS could not be measured")
        if self._maximum_gap_seconds > RESOURCE_SAMPLE_MAX_GAP_SECONDS:
            raise SoakFailure("worldstreamd RSS sampling interval exceeded its bound")
        return {
            "sampling_interval_ms": round(RSS_SAMPLE_INTERVAL_SECONDS * 1000),
            "maximum_gap_ms": round(RESOURCE_SAMPLE_MAX_GAP_SECONDS * 1000),
            "observed_max_gap_ms": round(self._maximum_gap_seconds * 1000, 3),
            "coverage_duration_ms": round(
                (self._last_sample_at - self._first_sample_at) * 1000, 3
            ),
            "sample_count": self.samples,
            "peak_bytes": self.peak_bytes,
        }

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


STORAGE_RESOURCE_NAMES = (
    "database_bytes",
    "wal_bytes",
    "temporary_bytes",
    "log_bytes",
    "artifact_bytes",
)


def storage_resource_values(
    root: pathlib.Path,
    data_dir: pathlib.Path,
    excluded: set[pathlib.Path],
) -> dict[str, int]:
    database = sqlite_sizes(data_dir)
    auxiliary = auxiliary_snapshot(root, excluded)
    return {
        "database_bytes": database["main_bytes"] + database["wal_bytes"],
        "wal_bytes": database["wal_bytes"],
        "temporary_bytes": auxiliary["temporary_bytes"],
        "log_bytes": auxiliary["log_bytes"],
        "artifact_bytes": auxiliary["artifact_bytes"],
    }


class LiveStorageSampler(threading.Thread):
    """Retain live file-size peaks without storing an unbounded sample series."""

    def __init__(
        self,
        root: pathlib.Path,
        data_dir: pathlib.Path,
        excluded: set[pathlib.Path],
        baseline: dict[str, int],
        *,
        interval_seconds: float = STORAGE_SAMPLE_INTERVAL_SECONDS,
        maximum_gap_seconds: float = RESOURCE_SAMPLE_MAX_GAP_SECONDS,
    ) -> None:
        super().__init__(daemon=True, name="worldstream-live-storage-sampler")
        if (
            set(baseline) != set(STORAGE_RESOURCE_NAMES)
            or any(type(value) is not int or value < 0 for value in baseline.values())
            or interval_seconds <= 0
            or maximum_gap_seconds < interval_seconds
        ):
            raise SoakFailure("live storage sampler configuration was invalid")
        self.root = root
        self.data_dir = data_dir
        self.excluded = excluded
        self.baseline = dict(baseline)
        self.interval_seconds = interval_seconds
        self.maximum_gap_seconds = maximum_gap_seconds
        self._peaks = dict(baseline)
        self._sample_count = 0
        self._first_sample_at: float | None = None
        self._last_sample_at: float | None = None
        self._maximum_observed_gap_seconds = 0.0
        self._error: BaseException | None = None
        self._lock = threading.Lock()
        self._stop_event = threading.Event()

    def _sample(self) -> None:
        try:
            values = storage_resource_values(self.root, self.data_dir, self.excluded)
        except (OSError, SoakFailure) as error:
            self._error = error
            return
        observed_at = time.monotonic()
        with self._lock:
            if self._first_sample_at is None:
                self._first_sample_at = observed_at
            if self._last_sample_at is not None:
                self._maximum_observed_gap_seconds = max(
                    self._maximum_observed_gap_seconds,
                    observed_at - self._last_sample_at,
                )
            self._last_sample_at = observed_at
            for name, value in values.items():
                self._peaks[name] = max(self._peaks[name], value)
            self._sample_count += 1

    def run(self) -> None:
        while not self._stop_event.is_set():
            self._sample()
            if self._error is not None or self._stop_event.wait(self.interval_seconds):
                return

    def observed_peak(self, resource: str) -> int:
        if resource not in STORAGE_RESOURCE_NAMES:
            raise SoakFailure("unknown live storage resource")
        with self._lock:
            return self._peaks[resource]

    def finish(self) -> dict[str, Any]:
        self._stop_event.set()
        self.join(timeout=2)
        if self.is_alive():
            raise SoakFailure("live storage sampler did not stop within its bound")
        if self._error is None:
            self._sample()
        if self._error is not None:
            raise SoakFailure(
                "live storage resources could not be sampled"
            ) from self._error
        with self._lock:
            if (
                self._sample_count <= 0
                or self._first_sample_at is None
                or self._last_sample_at is None
            ):
                raise SoakFailure("live storage resources had no complete samples")
            if self._maximum_observed_gap_seconds > self.maximum_gap_seconds:
                raise SoakFailure("live storage sampling interval exceeded its bound")
            return {
                "sampling_interval_ms": round(self.interval_seconds * 1000),
                "maximum_gap_ms": round(self.maximum_gap_seconds * 1000),
                "observed_max_gap_ms": round(
                    self._maximum_observed_gap_seconds * 1000, 3
                ),
                "coverage_duration_ms": round(
                    (self._last_sample_at - self._first_sample_at) * 1000, 3
                ),
                "sample_count": self._sample_count,
                "initial": dict(self.baseline),
                "peaks": dict(self._peaks),
            }

    def cancel(self) -> None:
        self._stop_event.set()
        self.join(timeout=2)


def live_resource_sampling(
    rss: dict[str, int | float],
    storage: dict[str, Any],
    *,
    workload_elapsed_seconds: float,
    hard_limits: dict[str, int],
) -> dict[str, Any]:
    """Build exact live peak evidence and reject gaps or exceeded limits."""

    if set(hard_limits) != {"process_tree_rss_bytes", *STORAGE_RESOURCE_NAMES}:
        raise SoakFailure("live resource hard-limit inventory was incomplete")
    if set(storage.get("initial", {})) != set(STORAGE_RESOURCE_NAMES) or set(
        storage.get("peaks", {})
    ) != set(STORAGE_RESOURCE_NAMES):
        raise SoakFailure("live storage resource inventory was incomplete")
    workload_elapsed_ms = workload_elapsed_seconds * 1000
    for label, summary in (("RSS", rss), ("storage", storage)):
        coverage = summary.get("coverage_duration_ms")
        maximum_gap = summary.get("maximum_gap_ms")
        if (
            type(coverage) not in {int, float}
            or type(maximum_gap) not in {int, float}
            or coverage + maximum_gap < workload_elapsed_ms
        ):
            raise SoakFailure(
                f"live {label} sampling did not cover the transition workload"
            )

    rss_peak = rss.get("peak_bytes")
    rss_limit = hard_limits["process_tree_rss_bytes"]
    if type(rss_peak) is not int or rss_peak <= 0 or rss_peak > rss_limit:
        raise SoakFailure("process-tree RSS exceeded its configured live peak bound")
    resources: dict[str, dict[str, Any]] = {
        "process_tree_rss_bytes": {
            "measurement_source": "linux_proc_process_tree_vmrss",
            "measurement_window": "transition_workload",
            "sampling_interval_ms": rss["sampling_interval_ms"],
            "maximum_gap_ms": rss["maximum_gap_ms"],
            "observed_max_gap_ms": rss["observed_max_gap_ms"],
            "coverage_duration_ms": rss["coverage_duration_ms"],
            "sample_count": rss["sample_count"],
            "observed_peak_bytes": rss_peak,
            "configured_hard_limit_bytes": rss_limit,
            "bound_status": "pass",
        }
    }
    storage_sources = {
        "database_bytes": "sqlite_main_plus_wal_regular_file_sizes",
        "wal_bytes": "sqlite_wal_regular_file_size",
        "temporary_bytes": "owned_private_working_tree_regular_file_sizes",
        "log_bytes": "owned_private_working_tree_daemon_log_sizes",
        "artifact_bytes": "owned_private_working_tree_regular_file_sizes",
    }
    for name in STORAGE_RESOURCE_NAMES:
        initial = storage["initial"][name]
        peak = storage["peaks"][name]
        limit = hard_limits[name]
        peak_growth = peak - initial
        if (
            type(initial) is not int
            or type(peak) is not int
            or type(limit) is not int
            or initial < 0
            or peak_growth < 0
            or peak_growth > limit
        ):
            raise SoakFailure(f"{name} exceeded its configured live peak bound")
        resources[name] = {
            "measurement_source": storage_sources[name],
            "measurement_window": "transition_workload_through_recovery",
            "sampling_interval_ms": storage["sampling_interval_ms"],
            "maximum_gap_ms": storage["maximum_gap_ms"],
            "observed_max_gap_ms": storage["observed_max_gap_ms"],
            "coverage_duration_ms": storage["coverage_duration_ms"],
            "sample_count": storage["sample_count"],
            "initial_bytes": initial,
            "observed_peak_bytes": peak,
            "observed_peak_growth_bytes": peak_growth,
            "configured_hard_limit_bytes": limit,
            "bound_status": "pass",
        }
    return {
        "schema": RESOURCE_SAMPLING_SCHEMA,
        "status": "measured",
        "sampling_complete": True,
        "peak_semantics": "maximum_observed_at_bounded_sampling_interval",
        "workload_elapsed_ms": round(workload_elapsed_ms, 3),
        "resources": resources,
    }


def metric_value(text: str, name: str, labels: dict[str, str]) -> int:
    label_text = ",".join(f'{key}="{value}"' for key, value in labels.items())
    match = re.search(
        rf"^{re.escape(name)}\{{{re.escape(label_text)}\}} ([0-9]+)$",
        text,
        re.MULTILINE,
    )
    if match is None:
        raise SoakFailure(
            f"public metric {name} with the required fixed labels was unavailable"
        )
    return int(match.group(1))


class InternalQueueSampler(threading.Thread):
    """Samples every packaged daemon bounded queue through public metrics."""

    def __init__(self, base_url: str) -> None:
        super().__init__(daemon=True)
        self.base_url = base_url
        self.samples: dict[str, list[dict[str, int]]] = {
            str(spec["name"]): [] for spec in QUEUE_SPECS
        }
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
                for spec in QUEUE_SPECS:
                    queue = str(spec["name"])
                    queue_label = {"queue": queue}
                    self.samples[queue].append(
                        {
                            "capacity": metric_value(
                                text,
                                "worldstream_internal_queue_capacity",
                                {
                                    "queue": queue,
                                    "scope": str(spec["capacity_scope"]),
                                },
                            ),
                            "process_current": metric_value(
                                text,
                                "worldstream_internal_queue_process_current",
                                queue_label,
                            ),
                            "process_high_water": metric_value(
                                text,
                                "worldstream_internal_queue_process_high_water",
                                queue_label,
                            ),
                            "unit_high_water": metric_value(
                                text,
                                "worldstream_internal_queue_unit_high_water",
                                queue_label,
                            ),
                            "activity_total": metric_value(
                                text,
                                "worldstream_internal_queue_activity_total",
                                queue_label,
                            ),
                            "completion_total": metric_value(
                                text,
                                "worldstream_internal_queue_completion_total",
                                queue_label,
                            ),
                            "backpressure_total": metric_value(
                                text,
                                "worldstream_internal_queue_backpressure_total",
                                queue_label,
                            ),
                        }
                    )
            except (OSError, UnicodeDecodeError, ValueError, SoakFailure) as error:
                self._error = error
                return

    def finish(self) -> list[dict[str, Any]]:
        self._stop_event.set()
        self.join(timeout=2)
        if self.is_alive():
            raise SoakFailure("internal queue sampler did not stop within its bound")
        if self._error is not None:
            raise SoakFailure(
                "internal queue metrics could not be sampled"
            ) from self._error
        return self.summarize()

    def summarize(self) -> list[dict[str, Any]]:
        """Validate already captured samples and build the closed evidence rows."""

        observed: list[dict[str, Any]] = []
        for spec in QUEUE_SPECS:
            name = str(spec["name"])
            rows = self.samples[name]
            if not rows:
                raise SoakFailure(f"{name} queue metrics had no complete samples")
            hard_limit = int(spec["hard_limit"])
            if {row["capacity"] for row in rows} != {hard_limit}:
                raise SoakFailure(f"{name} capacity differed from its hard limit")
            process_current = [row["process_current"] for row in rows]
            process_high_water = [row["process_high_water"] for row in rows]
            unit_high_water = [row["unit_high_water"] for row in rows]
            activity = [row["activity_total"] for row in rows]
            completion = [row["completion_total"] for row in rows]
            backpressure = [row["backpressure_total"] for row in rows]
            monotonic_series = (
                process_high_water,
                unit_high_water,
                activity,
                completion,
                backpressure,
            )
            if any(
                any(
                    next_value < value
                    for value, next_value in itertools.pairwise(series)
                )
                for series in monotonic_series
            ):
                raise SoakFailure(f"{name} cumulative queue metrics moved backwards")
            if any(
                current > high_water
                for current, high_water in zip(process_current, process_high_water)
            ):
                raise SoakFailure(
                    f"{name} current depth exceeded its process high-water"
                )
            maximum_unit_high_water = max(unit_high_water)
            if maximum_unit_high_water > hard_limit:
                raise SoakFailure(f"{name} exceeded its configured unit hard limit")
            if (
                str(spec["capacity_scope"]) == "global"
                and max(process_high_water) > hard_limit
            ):
                raise SoakFailure(f"{name} exceeded its configured global hard limit")
            activity_delta = activity[-1] - activity[0]
            completion_delta = completion[-1] - completion[0]
            backpressure_delta = backpressure[-1] - backpressure[0]
            if (
                activity_delta <= 0
                or completion_delta <= 0
                or maximum_unit_high_water <= 0
            ):
                raise SoakFailure(f"{name} production queue path was not exercised")
            observed.append(
                {
                    "status": "measured",
                    "name": name,
                    "measurement_source": "public_prometheus_metrics",
                    "capacity_metric": "worldstream_internal_queue_capacity",
                    "process_current_metric": (
                        "worldstream_internal_queue_process_current"
                    ),
                    "process_high_water_metric": (
                        "worldstream_internal_queue_process_high_water"
                    ),
                    "unit_high_water_metric": (
                        "worldstream_internal_queue_unit_high_water"
                    ),
                    "activity_metric": "worldstream_internal_queue_activity_total",
                    "completion_metric": (
                        "worldstream_internal_queue_completion_total"
                    ),
                    "backpressure_metric": (
                        "worldstream_internal_queue_backpressure_total"
                    ),
                    "capacity_scope": spec["capacity_scope"],
                    "activity_unit": spec["activity_unit"],
                    "configured_hard_limit": hard_limit,
                    "maximum_observed_process_current": max(process_current),
                    "maximum_reported_process_high_water": max(process_high_water),
                    "maximum_reported_unit_high_water": maximum_unit_high_water,
                    "sample_count": len(rows),
                    "activity_total_initial": activity[0],
                    "activity_total_final": activity[-1],
                    "activity_total_delta": activity_delta,
                    "completion_total_initial": completion[0],
                    "completion_total_final": completion[-1],
                    "completion_total_delta": completion_delta,
                    "backpressure_total_initial": backpressure[0],
                    "backpressure_total_final": backpressure[-1],
                    "backpressure_total_delta": backpressure_delta,
                    "bound_status": "pass",
                }
            )
        return observed

    def cancel(self) -> None:
        self._stop_event.set()
        self.join(timeout=2)


def public_json(base_url: str, path: str) -> dict[str, Any]:
    try:
        with urllib.request.urlopen(f"{base_url}{path}", timeout=5) as response:
            raw = response.read(MAX_PUBLIC_JSON_BYTES + 1)
        if not (0 < len(raw) <= MAX_PUBLIC_JSON_BYTES):
            raise SoakFailure(f"public {path} disclosure exceeded its JSON bound")
        value = BUILD_IDENTITY.strict_json(raw, f"public {path} disclosure")
    except (OSError, BUILD_IDENTITY.IdentityError) as error:
        raise SoakFailure(f"public {path} disclosure was unavailable") from error
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
        value = BUILD_IDENTITY.strict_json(raw, f"{label} report")
    except (OSError, BUILD_IDENTITY.IdentityError) as error:
        raise SoakFailure(f"{label} report was invalid strict JSON") from error
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


def validate_disk_full_evidence(
    evidence: dict[str, Any], *, binary_sha256: str
) -> dict[str, Any]:
    """Validate the exact bounded packaged-daemon ext4 ENOSPC scenario."""

    expected_top_level = {
        "schema",
        "status",
        "release_evidence",
        "scenario",
        "evidence_class",
        "platform",
        "container",
        "bounds",
        "filesystem",
        "fault",
        "daemon",
        "cleanup",
        "limitations",
    }
    platform_evidence = evidence.get("platform")
    container = evidence.get("container")
    bounds = evidence.get("bounds")
    filesystem = evidence.get("filesystem")
    fault = evidence.get("fault")
    daemon = evidence.get("daemon")
    cleanup = evidence.get("cleanup")
    limitations = evidence.get("limitations")
    if (
        set(evidence) != expected_top_level
        or evidence.get("schema") != DISK_FULL_SCHEMA
        or evidence.get("status") != "pass"
        or evidence.get("release_evidence") is not False
        or evidence.get("scenario") != DISK_FULL_SCENARIO
        or evidence.get("evidence_class") != DISK_FULL_EVIDENCE_CLASS
        or platform_evidence
        != {
            "system": "Linux",
            "machine": "x86_64",
            "filesystem": "ext4",
        }
        or not isinstance(container, dict)
        or set(container)
        != {
            "image",
            "platform",
            "privileged",
            "network",
            "root_filesystem_read_only",
            "daemon_mount_read_only",
            "observed_elapsed_ms",
            "captured_output_bytes",
        }
        or container.get("image") != DISK_FULL_CONTAINER_IMAGE
        or container.get("platform") != "linux/amd64"
        or container.get("privileged") is not True
        or container.get("network") != "none"
        or container.get("root_filesystem_read_only") is not True
        or container.get("daemon_mount_read_only") is not True
        or type(container.get("observed_elapsed_ms")) not in {int, float}
        or not 0
        <= container["observed_elapsed_ms"]
        <= DISK_FULL_MAX_CONTAINER_SECONDS * 1000
        or type(container.get("captured_output_bytes")) is not int
        or not 0 < container["captured_output_bytes"] <= DISK_FULL_MAX_CAPTURE_BYTES
        or bounds
        != {
            "filesystem_image_bytes": DISK_FULL_FILESYSTEM_IMAGE_BYTES,
            "attempted_write_bytes": DISK_FULL_ATTEMPTED_WRITE_BYTES,
            "max_daemon_seconds": DISK_FULL_MAX_DAEMON_SECONDS,
            "max_container_seconds": DISK_FULL_MAX_CONTAINER_SECONDS,
            "max_log_bytes": DISK_FULL_MAX_LOG_BYTES,
            "max_capture_bytes": DISK_FULL_MAX_CAPTURE_BYTES,
        }
        or not isinstance(filesystem, dict)
        or set(filesystem)
        != {
            "type",
            "mount_source_class",
            "image_size_bytes",
            "block_size_bytes",
            "available_kib_after_fill",
            "database_file_type",
            "database_file_mode",
            "fill_bytes_written",
        }
        or filesystem.get("type") != "ext4"
        or filesystem.get("mount_source_class") != "loop_device"
        or filesystem.get("image_size_bytes") != DISK_FULL_FILESYSTEM_IMAGE_BYTES
        or filesystem.get("block_size_bytes") != DISK_FULL_BLOCK_SIZE_BYTES
        or filesystem.get("available_kib_after_fill") != 0
        or filesystem.get("database_file_type") != "regular"
        or filesystem.get("database_file_mode") != "0600"
        or type(filesystem.get("fill_bytes_written")) is not int
        or not 0 < filesystem["fill_bytes_written"] < DISK_FULL_FILESYSTEM_IMAGE_BYTES
        or fault
        != {
            "errno_number": errno.ENOSPC,
            "errno_name": "ENOSPC",
            "attempted_write_bytes": DISK_FULL_ATTEMPTED_WRITE_BYTES,
            "write_returned_bytes": 0,
            "database_size_before_bytes": 0,
            "database_size_after_bytes": 0,
        }
        or not isinstance(daemon, dict)
        or set(daemon)
        != {
            "binary_sha256",
            "started",
            "exit_observed",
            "exit_code",
            "ready_http_200_observed",
            "public_mutation_available",
            "storage_failure_observed",
            "elapsed_ms",
            "diagnostic_bytes",
            "diagnostic_sha256",
        }
        or daemon.get("binary_sha256") != binary_sha256
        or daemon.get("started") is not True
        or daemon.get("exit_observed") is not True
        or daemon.get("exit_code") != 1
        or daemon.get("ready_http_200_observed") is not False
        or daemon.get("public_mutation_available") is not False
        or daemon.get("storage_failure_observed") is not True
        or type(daemon.get("elapsed_ms")) not in {int, float}
        or not 0 <= daemon["elapsed_ms"] <= DISK_FULL_MAX_DAEMON_SECONDS * 1000
        or type(daemon.get("diagnostic_bytes")) is not int
        or not 0 < daemon["diagnostic_bytes"] <= DISK_FULL_MAX_LOG_BYTES
        or not isinstance(daemon.get("diagnostic_sha256"), str)
        or re.fullmatch(r"sha256:[0-9a-f]{64}", daemon["diagnostic_sha256"]) is None
        or cleanup
        != {
            "internal_unmount_observed": True,
            "container_remove_requested": True,
            "container_absent_after_run": True,
        }
        or limitations
        != {
            "runtime_disk_exhaustion_recovery_observed": False,
            "physical_power_loss_observed": False,
        }
    ):
        raise SoakFailure("packaged daemon disk-full evidence was incomplete")
    return evidence


def parse_disk_full_container_output(output: bytes) -> dict[str, str]:
    """Parse the fixed, non-secret container witness without accepting extras."""

    prefix = "WORLDSTREAM_ENOSPC_V1 "
    expected = {
        "filesystem_type",
        "mount_source_class",
        "image_size_bytes",
        "block_size_bytes",
        "available_kib_after_fill",
        "database_file_type",
        "database_file_mode",
        "fill_bytes_written",
        "fault_errno_name",
        "fault_attempted_write_bytes",
        "fault_write_returned_bytes",
        "database_size_before_bytes",
        "database_size_after_bytes",
        "daemon_exit_code",
        "daemon_ready_http_200_observed",
        "daemon_storage_failure_observed",
        "daemon_uptime_start_seconds",
        "daemon_uptime_end_seconds",
        "diagnostic_bytes",
        "diagnostic_sha256",
        "internal_unmount_observed",
    }
    try:
        lines = output.decode("ascii").splitlines()
    except UnicodeDecodeError as error:
        raise SoakFailure("disk-full container output was not ASCII") from error
    values: dict[str, str] = {}
    for line in lines:
        if not line.startswith(prefix) or "=" not in line[len(prefix) :]:
            raise SoakFailure("disk-full container output was malformed")
        name, value = line[len(prefix) :].split("=", 1)
        if name not in expected or name in values or not value:
            raise SoakFailure("disk-full container output was malformed")
        values[name] = value
    if set(values) != expected:
        raise SoakFailure("disk-full container output was incomplete")
    return values


def remove_disk_full_container(docker: str, name: str) -> bool:
    """Force removal and prove that the named temporary container is absent."""

    try:
        subprocess.run(
            [docker, "rm", "--force", name],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=5,
            check=False,
        )
        listed = subprocess.run(
            [docker, "ps", "--all", "--quiet", "--filter", f"name=^/{name}$"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    return listed.returncode == 0 and not listed.stdout.strip()


DISK_FULL_CONTAINER_SCRIPT = r"""
set -eu
mounted=0
cleanup() {
  if [ "$mounted" -eq 1 ]; then
    umount /scratch/disk >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT HUP INT TERM

truncate -s 67108864 /scratch/disk.ext4
[ "$(stat -c %s /scratch/disk.ext4)" -eq 67108864 ]
mkfs.ext4 -q -F -b 4096 /scratch/disk.ext4
mkdir /scratch/disk
mount -o loop /scratch/disk.ext4 /scratch/disk
mounted=1
grep -Eq ' /scratch/disk .* - ext4 /dev/loop[0-9]+ ' /proc/self/mountinfo

mkdir -m 700 /scratch/disk/data
: > /scratch/disk/data/worldstream.sqlite3
chmod 600 /scratch/disk/data/worldstream.sqlite3
[ -f /scratch/disk/data/worldstream.sqlite3 ]
[ ! -L /scratch/disk/data/worldstream.sqlite3 ]
head -c 32 /dev/urandom > /scratch/authority.secret
chmod 600 /scratch/authority.secret
database_size_before=$(stat -c %s /scratch/disk/data/worldstream.sqlite3)

set +e
dd if=/dev/zero of=/scratch/disk/filler bs=1048576 status=none \
  2>/scratch/fill.err
fill_rc=$?
set -e
[ "$fill_rc" -ne 0 ]
grep -q 'No space left on device' /scratch/fill.err
available_kib=$(df -Pk /scratch/disk | tail -1 | tr -s ' ' | cut -d ' ' -f4)
[ "$available_kib" -eq 0 ]
fill_bytes=$(stat -c %s /scratch/disk/filler)

set +e
dd if=/dev/zero of=/scratch/disk/data/worldstream.sqlite3 bs=4096 count=1 \
  conv=notrunc status=none 2>/scratch/fault.err
fault_rc=$?
set -e
[ "$fault_rc" -ne 0 ]
grep -q 'No space left on device' /scratch/fault.err
database_size_after=$(stat -c %s /scratch/disk/data/worldstream.sqlite3)
[ "$database_size_before" -eq 0 ]
[ "$database_size_after" -eq 0 ]

daemon_start=$(cut -d ' ' -f1 /proc/uptime)
set +e
WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE=/scratch/authority.secret \
  RUST_LOG=warn /worldstreamd --data-dir /scratch/disk/data \
  --bind 127.0.0.1:48123 > /scratch/daemon.log 2>&1 &
daemon_pid=$!
set -e
ready_observed=0
exited_in_bound=0
poll=0
while [ "$poll" -lt 40 ]; do
  if ! kill -0 "$daemon_pid" 2>/dev/null; then
    exited_in_bound=1
    break
  fi
  set +e
  timeout 0.2 wget -q -T 1 -O /dev/null http://127.0.0.1:48123/readyz
  ready_rc=$?
  set -e
  if [ "$ready_rc" -eq 0 ]; then
    ready_observed=1
    break
  fi
  poll=$((poll + 1))
  sleep 0.05
done
if kill -0 "$daemon_pid" 2>/dev/null; then
  kill -TERM "$daemon_pid" 2>/dev/null || true
  sleep 0.2
  kill -KILL "$daemon_pid" 2>/dev/null || true
fi
set +e
wait "$daemon_pid"
daemon_rc=$?
set -e
daemon_end=$(cut -d ' ' -f1 /proc/uptime)
[ "$exited_in_bound" -eq 1 ]
[ "$ready_observed" -eq 0 ]
[ "$daemon_rc" -eq 1 ]
grep -q 'SQLite durable store initialization/verification failed' /scratch/daemon.log
diagnostic_bytes=$(wc -c < /scratch/daemon.log | tr -d ' ')
[ "$diagnostic_bytes" -gt 0 ]
[ "$diagnostic_bytes" -le 65536 ]
diagnostic_sha256=$(sha256sum /scratch/daemon.log | cut -d ' ' -f1)
block_size=$(stat -f -c %S /scratch/disk)
[ "$block_size" -eq 4096 ]
[ "$(stat -c %a /scratch/disk/data/worldstream.sqlite3)" -eq 600 ]

printf 'WORLDSTREAM_ENOSPC_V1 filesystem_type=ext4\n'
printf 'WORLDSTREAM_ENOSPC_V1 mount_source_class=loop_device\n'
printf 'WORLDSTREAM_ENOSPC_V1 image_size_bytes=67108864\n'
printf 'WORLDSTREAM_ENOSPC_V1 block_size_bytes=%s\n' "$block_size"
printf 'WORLDSTREAM_ENOSPC_V1 available_kib_after_fill=%s\n' "$available_kib"
printf 'WORLDSTREAM_ENOSPC_V1 database_file_type=regular\n'
printf 'WORLDSTREAM_ENOSPC_V1 database_file_mode=0600\n'
printf 'WORLDSTREAM_ENOSPC_V1 fill_bytes_written=%s\n' "$fill_bytes"
printf 'WORLDSTREAM_ENOSPC_V1 fault_errno_name=ENOSPC\n'
printf 'WORLDSTREAM_ENOSPC_V1 fault_attempted_write_bytes=4096\n'
printf 'WORLDSTREAM_ENOSPC_V1 fault_write_returned_bytes=0\n'
printf 'WORLDSTREAM_ENOSPC_V1 database_size_before_bytes=%s\n' "$database_size_before"
printf 'WORLDSTREAM_ENOSPC_V1 database_size_after_bytes=%s\n' "$database_size_after"
printf 'WORLDSTREAM_ENOSPC_V1 daemon_exit_code=%s\n' "$daemon_rc"
printf 'WORLDSTREAM_ENOSPC_V1 daemon_ready_http_200_observed=%s\n' "$ready_observed"
printf 'WORLDSTREAM_ENOSPC_V1 daemon_storage_failure_observed=1\n'
printf 'WORLDSTREAM_ENOSPC_V1 daemon_uptime_start_seconds=%s\n' "$daemon_start"
printf 'WORLDSTREAM_ENOSPC_V1 daemon_uptime_end_seconds=%s\n' "$daemon_end"
printf 'WORLDSTREAM_ENOSPC_V1 diagnostic_bytes=%s\n' "$diagnostic_bytes"
printf 'WORLDSTREAM_ENOSPC_V1 diagnostic_sha256=%s\n' "$diagnostic_sha256"
umount /scratch/disk
mounted=0
printf 'WORLDSTREAM_ENOSPC_V1 internal_unmount_observed=1\n'
"""


def run_disk_full_scenario(
    daemon_bin: pathlib.Path, *, binary_sha256: str
) -> dict[str, Any]:
    """Run the packaged SQLite bootstrap on a bounded full ext4 filesystem."""

    if platform.system() != "Linux" or platform.machine().lower() not in {
        "x86_64",
        "amd64",
    }:
        raise SoakFailure("packaged daemon disk-full evidence requires Linux x86-64")
    docker = shutil.which("docker")
    if docker is None:
        raise SoakFailure("packaged daemon disk-full evidence requires Docker")
    resolved_daemon = daemon_bin.resolve()
    if ":" in str(resolved_daemon) or "\n" in str(resolved_daemon):
        raise SoakFailure("packaged daemon path cannot be mounted safely")
    container_name = f"worldstream-enospc-{os.getpid()}-{time.time_ns()}"
    started = time.monotonic()
    result: subprocess.CompletedProcess[bytes] | None = None
    run_error: BaseException | None = None
    try:
        result = subprocess.run(
            [
                docker,
                "run",
                "--rm",
                "--name",
                container_name,
                "--platform",
                "linux/amd64",
                "--privileged",
                "--network",
                "none",
                "--read-only",
                "--pids-limit",
                "64",
                "--tmpfs",
                "/scratch:rw,size=96m,mode=700",
                "--volume",
                f"{resolved_daemon}:/worldstreamd:ro",
                DISK_FULL_CONTAINER_IMAGE,
                "sh",
                "-c",
                DISK_FULL_CONTAINER_SCRIPT,
            ],
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=DISK_FULL_MAX_CONTAINER_SECONDS,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        run_error = error
    container_elapsed_ms = round((time.monotonic() - started) * 1000, 3)
    removed = remove_disk_full_container(docker, container_name)
    if not removed:
        raise SoakFailure("disk-full container cleanup could not be verified")
    if run_error is not None:
        raise SoakFailure(
            "disk-full container did not complete within its bound"
        ) from run_error
    if result is None:  # pragma: no cover - guarded by run_error.
        raise SoakFailure("disk-full container did not return a result")
    captured_bytes = len(result.stdout) + len(result.stderr)
    if (
        result.returncode != 0
        or captured_bytes > DISK_FULL_MAX_CAPTURE_BYTES
        or container_elapsed_ms > DISK_FULL_MAX_CONTAINER_SECONDS * 1000
    ):
        raise SoakFailure("packaged daemon disk-full container failed closed")
    values = parse_disk_full_container_output(result.stdout)
    try:
        daemon_elapsed_ms = round(
            (
                float(values["daemon_uptime_end_seconds"])
                - float(values["daemon_uptime_start_seconds"])
            )
            * 1000,
            3,
        )
        fill_bytes = int(values["fill_bytes_written"])
        diagnostic_bytes = int(values["diagnostic_bytes"])
    except (KeyError, ValueError) as error:
        raise SoakFailure(
            "disk-full container emitted invalid numeric evidence"
        ) from error
    evidence = {
        "schema": DISK_FULL_SCHEMA,
        "status": "pass",
        "release_evidence": False,
        "scenario": DISK_FULL_SCENARIO,
        "evidence_class": DISK_FULL_EVIDENCE_CLASS,
        "platform": {
            "system": "Linux",
            "machine": "x86_64",
            "filesystem": "ext4",
        },
        "container": {
            "image": DISK_FULL_CONTAINER_IMAGE,
            "platform": "linux/amd64",
            "privileged": True,
            "network": "none",
            "root_filesystem_read_only": True,
            "daemon_mount_read_only": True,
            "observed_elapsed_ms": container_elapsed_ms,
            "captured_output_bytes": captured_bytes,
        },
        "bounds": {
            "filesystem_image_bytes": DISK_FULL_FILESYSTEM_IMAGE_BYTES,
            "attempted_write_bytes": DISK_FULL_ATTEMPTED_WRITE_BYTES,
            "max_daemon_seconds": DISK_FULL_MAX_DAEMON_SECONDS,
            "max_container_seconds": DISK_FULL_MAX_CONTAINER_SECONDS,
            "max_log_bytes": DISK_FULL_MAX_LOG_BYTES,
            "max_capture_bytes": DISK_FULL_MAX_CAPTURE_BYTES,
        },
        "filesystem": {
            "type": values["filesystem_type"],
            "mount_source_class": values["mount_source_class"],
            "image_size_bytes": int(values["image_size_bytes"]),
            "block_size_bytes": int(values["block_size_bytes"]),
            "available_kib_after_fill": int(values["available_kib_after_fill"]),
            "database_file_type": values["database_file_type"],
            "database_file_mode": values["database_file_mode"],
            "fill_bytes_written": fill_bytes,
        },
        "fault": {
            "errno_number": errno.ENOSPC,
            "errno_name": values["fault_errno_name"],
            "attempted_write_bytes": int(values["fault_attempted_write_bytes"]),
            "write_returned_bytes": int(values["fault_write_returned_bytes"]),
            "database_size_before_bytes": int(values["database_size_before_bytes"]),
            "database_size_after_bytes": int(values["database_size_after_bytes"]),
        },
        "daemon": {
            "binary_sha256": binary_sha256,
            "started": True,
            "exit_observed": True,
            "exit_code": int(values["daemon_exit_code"]),
            "ready_http_200_observed": values["daemon_ready_http_200_observed"] == "1",
            "public_mutation_available": False,
            "storage_failure_observed": values["daemon_storage_failure_observed"]
            == "1",
            "elapsed_ms": daemon_elapsed_ms,
            "diagnostic_bytes": diagnostic_bytes,
            "diagnostic_sha256": "sha256:" + values["diagnostic_sha256"],
        },
        "cleanup": {
            "internal_unmount_observed": values["internal_unmount_observed"] == "1",
            "container_remove_requested": True,
            "container_absent_after_run": True,
        },
        "limitations": {
            "runtime_disk_exhaustion_recovery_observed": False,
            "physical_power_loss_observed": False,
        },
    }
    return validate_disk_full_evidence(evidence, binary_sha256=binary_sha256)


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


def run_queue_boundary_tests(
    total_timeout: float, max_output_bytes: int, *, cargo: str = "cargo"
) -> dict[str, Any]:
    """Run each exact production queue saturation regression within one bound."""

    started = time.monotonic()
    deadline = started + total_timeout
    results: list[dict[str, Any]] = []
    with tempfile.TemporaryDirectory(prefix="worldstream-queue-boundary-") as temporary:
        for index, (queue_class, package, test_name) in enumerate(
            QUEUE_BOUNDARY_TESTS, start=1
        ):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise SoakFailure("queue boundary tests exceeded their total bound")
            output_path = pathlib.Path(temporary) / f"test-{index}.log"
            test_started = time.monotonic()
            with output_path.open("w+b") as output:
                process = subprocess.Popen(
                    [
                        cargo,
                        "test",
                        "--locked",
                        "-p",
                        package,
                        "--lib",
                        test_name,
                        "--",
                        "--exact",
                    ],
                    cwd=ROOT,
                    stdin=subprocess.DEVNULL,
                    stdout=output,
                    stderr=subprocess.STDOUT,
                    env={**os.environ, "CARGO_TERM_COLOR": "never"},
                    start_new_session=True,
                )
                try:
                    returncode = process.wait(timeout=min(remaining, 120.0))
                except subprocess.TimeoutExpired as error:
                    try:
                        os.killpg(process.pid, signal.SIGTERM)
                        process.wait(timeout=1)
                    except (OSError, subprocess.TimeoutExpired):
                        try:
                            os.killpg(process.pid, signal.SIGKILL)
                        except OSError:
                            pass
                        process.wait(timeout=2)
                    raise SoakFailure(
                        f"{queue_class} queue boundary test exceeded its bound"
                    ) from error
                output.flush()
                size_bytes = output.tell()
                if size_bytes > max_output_bytes:
                    raise SoakFailure(
                        f"{queue_class} queue boundary output exceeded its bound"
                    )
                output.seek(0)
                raw = output.read()
            text = raw.decode("utf-8", errors="replace")
            exact_result = re.search(
                rf"^test {re.escape(test_name)} \.\.\. ok$", text, re.MULTILINE
            )
            if returncode != 0 or exact_result is None or "running 1 test" not in text:
                raise SoakFailure(
                    f"{queue_class} exact production queue boundary test did not pass"
                )
            results.append(
                {
                    "queue_class": queue_class,
                    "package": package,
                    "test_name": test_name,
                    "status": "passed",
                    "exact_test_count": 1,
                    "duration_ms": round((time.monotonic() - test_started) * 1000, 3),
                    "output_bytes": size_bytes,
                    "output_sha256": "sha256:" + hashlib.sha256(raw).hexdigest(),
                }
            )
    return {
        "schema": QUEUE_BOUNDARY_TEST_SCHEMA,
        "status": "pass",
        "execution_scope": "same_production_queue_paths",
        "total_duration_ms": round((time.monotonic() - started) * 1000, 3),
        "max_total_seconds": total_timeout,
        "max_output_bytes_per_test": max_output_bytes,
        "tests": results,
    }


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
    if args.package_report is not None:
        read_bounded_json_report(args.package_report, "package identity")
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
    queue_boundary_tests = await asyncio.to_thread(
        run_queue_boundary_tests,
        QUEUE_BOUNDARY_MAX_TOTAL_SECONDS,
        DEFAULT_MAX_OUTPUT_BYTES,
    )
    disk_full = await asyncio.to_thread(
        run_disk_full_scenario,
        args.daemon_bin,
        binary_sha256=distribution["binary_sha256"],
    )
    root = COMMON.private_root("worldstream-daemon-transition-soak-")
    daemon = COMMON.Daemon(
        root,
        args.daemon_bin,
        startup_timeout=args.startup_timeout_seconds,
        shutdown_timeout=args.shutdown_timeout_seconds,
    )
    started = time.monotonic()
    sampler: RssSampler | None = None
    storage_sampler: LiveStorageSampler | None = None
    queue_sampler: InternalQueueSampler | None = None
    try:
        start_daemon_with_dns_telemetry(daemon)
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
        storage_baseline = {
            "database_bytes": (
                initial_database["main_bytes"] + initial_database["wal_bytes"]
            ),
            "wal_bytes": initial_database["wal_bytes"],
            "temporary_bytes": initial_auxiliary["temporary_bytes"],
            "log_bytes": initial_auxiliary["log_bytes"],
            "artifact_bytes": initial_auxiliary["artifact_bytes"],
        }
        storage_sampler = LiveStorageSampler(
            root,
            daemon.data_dir,
            database_files,
            storage_baseline,
        )
        storage_sampler.start()
        workload_started = time.monotonic()
        workload, last_room, capability_sentinels = await run_transition_workload(
            daemon, target_seconds
        )
        workload_elapsed = time.monotonic() - workload_started
        rss_summary = sampler.finish()
        peak_rss = rss_summary["peak_bytes"]
        sampler = None
        internal_queues = queue_sampler.finish()
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
        storage_summary = storage_sampler.finish()
        storage_sampler = None
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
        resource_sampling = live_resource_sampling(
            rss_summary,
            storage_summary,
            workload_elapsed_seconds=workload_elapsed,
            hard_limits={
                "process_tree_rss_bytes": args.max_peak_rss_bytes,
                "database_bytes": args.max_database_growth_bytes,
                "wal_bytes": args.max_wal_growth_bytes,
                "temporary_bytes": args.max_temp_growth_bytes,
                "log_bytes": args.max_output_bytes,
                "artifact_bytes": args.max_artifact_growth_bytes,
            },
        )
        live_resources = resource_sampling["resources"]
        final_storage_values = {
            "database_bytes": database_final,
            "wal_bytes": final_database["wal_bytes"],
            "temporary_bytes": final_auxiliary["temporary_bytes"],
            "log_bytes": final_auxiliary["log_bytes"],
            "artifact_bytes": final_auxiliary["artifact_bytes"],
        }
        if any(
            final_storage_values[name] > live_resources[name]["observed_peak_bytes"]
            for name in STORAGE_RESOURCE_NAMES
        ):
            raise SoakFailure("final storage snapshot exceeded its retained live peak")
        queue_observation = {
            "status": "measured",
            "observation_scope": QUEUE_OBSERVATION_SCOPE,
            "all_internal_queues_observed": True,
            "observed_queues": internal_queues,
        }
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
                "method": "bounded /proc process-tree VmRSS live samples",
                "sampling_interval_ms": rss_summary["sampling_interval_ms"],
                "maximum_gap_ms": rss_summary["maximum_gap_ms"],
                "observed_max_gap_ms": rss_summary["observed_max_gap_ms"],
                "coverage_duration_ms": rss_summary["coverage_duration_ms"],
                "sample_count": rss_summary["sample_count"],
            },
            "database_growth": {
                "status": "measured",
                "measurement_semantics": "final_snapshot_not_peak_bound",
                "growth_bytes": database_growth,
                "wal_growth_bytes": wal_growth,
            },
            "temporary_growth": {
                "status": "measured",
                "measurement_semantics": "final_snapshot_not_peak_bound",
                "growth_bytes": temp_growth,
            },
            "log_growth": {
                "status": "measured",
                "measurement_semantics": "final_snapshot_not_peak_bound",
                "growth_bytes": log_growth,
            },
            "artifact_growth": {
                "status": "measured",
                "measurement_semantics": "final_snapshot_not_peak_bound",
                "growth_bytes": artifact_growth,
            },
            "resource_sampling": resource_sampling,
            "queue_observation": queue_observation,
            "queue_boundary_tests": queue_boundary_tests,
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
            "resource_sampling": resource_sampling,
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
                "internal_queue_hard_limits": {
                    str(spec["name"]): {
                        "capacity_scope": spec["capacity_scope"],
                        "hard_limit": spec["hard_limit"],
                        "activity_unit": spec["activity_unit"],
                    }
                    for spec in QUEUE_SPECS
                },
                "queue_boundary_test_max_total_seconds": (
                    QUEUE_BOUNDARY_MAX_TOTAL_SECONDS
                ),
                "queue_boundary_test_max_output_bytes": DEFAULT_MAX_OUTPUT_BYTES,
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
                "disk_full_fault_injection": True,
            },
            "disk_full": disk_full,
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
                    "sampling_interval_ms": rss_summary["sampling_interval_ms"],
                    "maximum_gap_ms": rss_summary["maximum_gap_ms"],
                    "observed_max_gap_ms": rss_summary["observed_max_gap_ms"],
                    "coverage_duration_ms": rss_summary["coverage_duration_ms"],
                    "sample_count": rss_summary["sample_count"],
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
            "queue_observation": queue_observation,
            "queue_boundary_tests": queue_boundary_tests,
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
                (
                    "Queue backpressure totals are measured outcomes; a zero delta is "
                    "valid when the bounded workload does not saturate a queue."
                ),
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
            f"peak_rss_bytes={peak_rss} "
            f"database_observed_peak_growth_bytes="
            f"{live_resources['database_bytes']['observed_peak_growth_bytes']} "
            f"wal_observed_peak_growth_bytes="
            f"{live_resources['wal_bytes']['observed_peak_growth_bytes']} "
            f"temp_observed_peak_growth_bytes="
            f"{live_resources['temporary_bytes']['observed_peak_growth_bytes']} "
            f"log_observed_peak_growth_bytes="
            f"{live_resources['log_bytes']['observed_peak_growth_bytes']} "
            f"artifact_observed_peak_growth_bytes="
            f"{live_resources['artifact_bytes']['observed_peak_growth_bytes']}\n"
            f"observed_internal_queues={len(internal_queues)} "
            f"queue_samples={min(queue['sample_count'] for queue in internal_queues)} "
            "all_internal_queues_observed=true\n"
            f"queue_boundary_tests={len(queue_boundary_tests['tests'])} "
            "queue_boundary_tests_status=pass\n"
            "disk_full_errno=ENOSPC disk_full_ready_observed=false "
            "disk_full_public_mutation_available=false\n"
            f"recovery_time_ms={recovery_ms:.3f} status=passed\n"
        )
        return report, log
    finally:
        if sampler is not None:
            sampler.cancel()
        if storage_sampler is not None:
            storage_sampler.cancel()
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

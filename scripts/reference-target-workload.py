#!/usr/bin/env python3
"""Run the frozen package-bound Linux reference target workload.

The report retains bounded aggregates and exact source bindings only. Room,
Membership, Action, capability, filesystem-path, and latency-sample inventories
never leave this process.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import importlib.util
import json
import math
import os
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import worldstream_sdk
from worldstream_sdk import Client, ProtocolError

ROOT = Path(__file__).resolve().parents[1]
PRODUCER_PATH = ROOT / "scripts/release-evidence-produce-reference.py"
FAILURE_PRODUCER_PATH = ROOT / "scripts/release-evidence-produce-failure-soak.py"
COMMON_PATH = ROOT / "scripts/kill-point-matrix.py"
HOST_PATH = ROOT / "scripts/reference_host_environment.py"
SNAPSHOT_FIXTURE_SOURCE_PATH = (
    ROOT / "crates/worldstream-sqlite/examples/reference_snapshot_tail_fixture.rs"
)
SNAPSHOT_FIXTURE_SCHEMA = "worldstream/reference-snapshot-tail-fixture/v1"
COUNTER_PACK = {
    "id": "worldstream.counter",
    "version": "2.0.0",
    "digest": "blake3:1c5f75068220f65f9017a062dbe40203c540108b572284d9446a329914008a92",
}
COUNTER_CONFIGURATION = {"initial_value": 0, "maximum_value": 16}
SQLITE_ENGINE = {
    "version": "3.53.4",
    "settings": {
        "journal_mode": "wal",
        "synchronous": "full",
        "foreign_keys": "on",
        "busy_timeout_ms": 5000,
        "reader_query_only": "on",
    },
    "connection_mode": "embedded",
}
MAX_HTTP_BYTES = 64 * 1024
MAX_FIXTURE_REPORT_BYTES = 64 * 1024
ACTION_WORKERS = 128
DISPATCH_RATE_PER_SECOND = 125
TOKEN_QUEUE_BOUND = 512
REFERENCE_JOB_HARD_SECONDS = 14_400
REFERENCE_JOB_MINIMUM_RESERVE_SECONDS = 1_800
FROZEN_FIXTURE_SETUP_TIMEOUT_SECONDS = 10_800.0
FROZEN_STARTUP_TIMEOUT_SECONDS = 30.0
FROZEN_SHUTDOWN_TIMEOUT_SECONDS = 15.0


class TargetFailure(RuntimeError):
    """The genuine target attempt could not produce trustworthy evidence."""


def load_module(name: str, path: Path) -> Any:
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


PRODUCER = load_module("worldstream_reference_target_producer", PRODUCER_PATH)
FAILURE = load_module("worldstream_reference_target_failure", FAILURE_PRODUCER_PATH)
COMMON = load_module("worldstream_reference_target_common", COMMON_PATH)
HOST = load_module("worldstream_reference_target_host", HOST_PATH)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise TargetFailure(message)


def frozen_profile() -> dict[str, Any]:
    return json.loads(json.dumps(PRODUCER.FROZEN_REFERENCE_PROFILE))


def reduced_profile() -> dict[str, Any]:
    """Explicitly non-publishable profile used only by local process tests."""

    return {
        "schema": PRODUCER.REFERENCE_TARGET_PROFILE_SCHEMA,
        "name": "reduced_test_nonpublishable",
        "publishable_candidate": False,
        "stored_room_target": 24,
        "loaded_room_target": 4,
        "idle_websocket_target": 8,
        "sustained_transition_rate_target_per_second": 2,
        "sustained_transition_window_seconds": 2,
        "commit_to_ack_p95_target_ms": 100,
        "history_room_transition_target": 100_000,
        "snapshot_maximum_lag_transitions": 250,
        "snapshot_tail_recovery_target_ms": 5_000,
        "one_hour_soak_target_seconds": 3_600,
        "forced_termination_minimum_count": 2,
    }


def validate_timeout_contract(
    args: argparse.Namespace, profile: dict[str, Any]
) -> None:
    """Keep the frozen workload within the exact four-hour workflow boundary."""

    timeouts = (
        args.startup_timeout_seconds,
        args.shutdown_timeout_seconds,
        args.fixture_setup_timeout_seconds,
    )
    require(
        all(
            type(value) in {int, float} and math.isfinite(value) and value > 0
            for value in timeouts
        ),
        "reference target timeouts must be positive finite seconds",
    )
    if profile["publishable_candidate"] is not True:
        return
    require(
        timeouts
        == (
            FROZEN_STARTUP_TIMEOUT_SECONDS,
            FROZEN_SHUTDOWN_TIMEOUT_SECONDS,
            FROZEN_FIXTURE_SETUP_TIMEOUT_SECONDS,
        ),
        "frozen reference timeout arguments drifted",
    )
    require(
        FROZEN_FIXTURE_SETUP_TIMEOUT_SECONDS
        + profile["sustained_transition_window_seconds"]
        + REFERENCE_JOB_MINIMUM_RESERVE_SECONDS
        <= REFERENCE_JOB_HARD_SECONDS,
        "frozen reference workload exceeds the four-hour job boundary",
    )


def nearest_rank(values: list[float], fraction: float) -> float:
    require(bool(values), "latency percentile requires an accepted sample")
    ordered = sorted(values)
    rank = max(1, min(len(ordered), math.ceil(len(ordered) * fraction)))
    return round(ordered[rank - 1], 3)


def latency_triplet(values: list[float]) -> dict[str, Any]:
    return {
        "definition": "nearest-rank",
        "sample_count": len(values),
        "p50_ms": nearest_rank(values, 0.50),
        "p95_ms": nearest_rank(values, 0.95),
        "p99_ms": nearest_rank(values, 0.99),
    }


def dimension(
    identifier: str,
    *,
    classification: str,
    completed: bool,
    observed: dict[str, Any],
    target: dict[str, Any],
    met: bool,
    method: str,
) -> dict[str, Any]:
    return {
        "id": identifier,
        "classification": classification,
        "attempted": True,
        "completed": completed,
        "observed": observed,
        "target": target,
        "target_met": met,
        "outcome": "met" if met else "missed",
        "method": method,
    }


def canonical_digest(value: object) -> str:
    raw = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def public_json(base_url: str, path: str) -> dict[str, Any]:
    try:
        with urllib.request.urlopen(base_url + path, timeout=15) as response:
            raw = response.read(MAX_HTTP_BYTES + 1)
    except OSError as error:
        raise TargetFailure(f"public {path} observation failed") from error
    require(0 < len(raw) <= MAX_HTTP_BYTES, f"public {path} response was unbounded")
    try:
        return PRODUCER.ADAPTER.COLLECTOR.strict_json_object(raw, f"public {path}")
    except PRODUCER.ADAPTER.COLLECTOR.CollectionError as error:
        raise TargetFailure(f"public {path} response was not strict JSON") from error


def verify_runtime_packaged_sdk(
    sdk_root: Path, expected: dict[str, Any]
) -> dict[str, Any]:
    require(
        sdk_root.is_dir() and not sdk_root.is_symlink(),
        "packaged SDK root must be a real directory",
    )
    root = sdk_root.resolve()
    paths = {
        "pyproject_sha256": root / "pyproject.toml",
        "lock_sha256": root / "uv.lock",
        "module_init_sha256": root / "src/worldstream_sdk/__init__.py",
        "client_module_sha256": root / "src/worldstream_sdk/client.py",
        "compatibility_identity_sha256": (
            root / "src/worldstream_sdk/compatibility_identity.json"
        ),
    }
    client_module = sys.modules.get(Client.__module__)
    client_module_file = getattr(client_module, "__file__", None)
    require(
        Path(worldstream_sdk.__file__).resolve() == paths["module_init_sha256"]
        and isinstance(client_module_file, str)
        and Path(client_module_file).resolve() == paths["client_module_sha256"],
        "runtime worldstream_sdk was not imported from the exact verified archive modules",
    )
    require(
        all(path.is_file() and not path.is_symlink() for path in paths.values()),
        "packaged SDK runtime inputs are incomplete",
    )

    def bounded_identity(path: Path) -> tuple[str, int]:
        size = path.stat().st_size
        require(0 < size <= 16 * 1024 * 1024, "packaged SDK file exceeded its bound")
        digest = hashlib.sha256()
        with path.open("rb") as source:
            for block in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(block)
        return "sha256:" + digest.hexdigest(), size

    observed_files = {field: bounded_identity(path) for field, path in paths.items()}
    observed = {
        "source": "verified_native_archive",
        **{field: value[0] for field, value in observed_files.items()},
        **{
            field.replace("_sha256", "_size_bytes"): value[1]
            for field, value in observed_files.items()
        },
        "runtime_module_under_packaged_sdk_root": True,
    }
    require(observed == expected, "runtime packaged SDK bytes differ from the archive")
    return observed


def atomic_write(path: Path, value: object) -> None:
    require(not path.is_symlink() and not path.is_dir(), "unsafe target output path")
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump(value, output, ensure_ascii=False, indent=2, sort_keys=True)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def atomic_write_exact_bytes(path: Path, raw: bytes, *, maximum: int) -> None:
    """Publish one bounded retained input without reserializing or overwriting."""

    require(
        isinstance(raw, bytes) and 0 < len(raw) <= maximum,
        "retained fixture report bytes exceeded their bound",
    )
    require(
        not path.exists() and not path.is_symlink() and not path.is_dir(),
        "retained fixture report output must be new",
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(name)
    created = False
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(raw)
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o644)
        os.link(temporary, path)
        created = True
        temporary.unlink()
    except BaseException:
        temporary.unlink(missing_ok=True)
        if created:
            path.unlink(missing_ok=True)
        raise


@dataclass(frozen=True)
class RoomRecord:
    room_id: str
    member_id: str
    principal_id: str


class RssSampler:
    def __init__(self) -> None:
        self.peak_bytes = 0
        self.sample_count = 0
        self._stop = asyncio.Event()

    def stop(self) -> None:
        self._stop.set()

    async def run(self, pid: int) -> None:
        while not self._stop.is_set():
            try:
                total = process_tree_rss_bytes(pid)
                if total > 0:
                    self.peak_bytes = max(self.peak_bytes, total)
                    self.sample_count += 1
            except (FileNotFoundError, OSError, ValueError):
                pass
            try:
                await asyncio.wait_for(self._stop.wait(), timeout=0.1)
            except TimeoutError:
                pass


def process_tree_rss_bytes(pid: int, proc_root: Path = Path("/proc")) -> int:
    pending = [pid]
    observed: set[int] = set()
    total = 0
    while pending:
        current = pending.pop()
        if current in observed:
            raise TargetFailure("process tree contained a PID cycle")
        require(len(observed) < 4_096, "process tree exceeded its PID bound")
        observed.add(current)
        status = proc_root / str(current) / "status"
        for line in status.read_text(encoding="utf-8").splitlines():
            if line.startswith("VmRSS:"):
                total += int(line.split()[1]) * 1024
                break
        children = (
            proc_root / str(current) / "task" / str(current) / "children"
        ).read_text(encoding="utf-8")
        pending.extend(int(value) for value in children.split())
    return total


async def create_rooms(
    operator: Client, count: int, concurrency: int = 64
) -> tuple[list[RoomRecord], int]:
    queue: asyncio.Queue[int] = asyncio.Queue()
    for index in range(count):
        queue.put_nowait(index)
    created: list[RoomRecord] = []
    failed = 0

    async def worker() -> None:
        nonlocal failed
        while not queue.empty():
            try:
                queue.get_nowait()
            except asyncio.QueueEmpty:
                return
            principal = COMMON.new_ulid()
            try:
                result = await operator.create_room(
                    {
                        "pack": COUNTER_PACK,
                        "configuration": COUNTER_CONFIGURATION,
                        "members": [
                            {
                                "principal_id": principal,
                                "principal_kind": "agent",
                                "role": "counter",
                                "access_mode": "participant",
                            }
                        ],
                        "idempotency_key": COMMON.new_ulid(),
                    }
                )
                members = result.get("member_ids")
                require(
                    isinstance(result.get("room_id"), str)
                    and isinstance(members, list)
                    and len(members) == 1
                    and isinstance(members[0], str),
                    "Room create receipt was malformed",
                )
                created.append(RoomRecord(result["room_id"], members[0], principal))
            except (ProtocolError, OSError, TimeoutError):
                failed += 1

    await asyncio.gather(*(worker() for _ in range(min(concurrency, count))))
    return created, failed


async def open_room(daemon: Any, record: RoomRecord) -> Any:
    bearer = await asyncio.to_thread(
        COMMON.issue_member_capability,
        daemon.base_url,
        daemon.operator_bearer,
        record.room_id,
        record.member_id,
        record.principal_id,
    )
    room = await Client(daemon.base_url, bearer).open_room(
        record.room_id, record.member_id
    )
    await room.sync()
    return room


def verify_accepted(result: dict[str, Any], seen: set[str]) -> None:
    transition_id = result.get("transition_id")
    require(
        isinstance(transition_id, str)
        and bool(transition_id)
        and result.get("duplicate") is False
        and isinstance(result.get("room_head"), dict),
        "Action did not return a complete original accepted receipt",
    )
    require(transition_id not in seen, "accepted transition identity was duplicated")
    seen.add(transition_id)


def fixture_transition_count(profile: dict[str, Any]) -> int:
    """Keep local process tests bounded while the frozen path attempts all 100k."""

    return 100_000 if profile["publishable_candidate"] is True else 20


def stable_fixture_report_bytes(path: Path) -> bytes:
    """Read one regular report through one no-follow, mutation-checked descriptor."""

    no_follow = getattr(os, "O_NOFOLLOW", None)
    require(
        isinstance(no_follow, int) and no_follow != 0,
        "snapshot-tail fixture report requires no-follow file admission",
    )
    flags = os.O_RDONLY | no_follow
    if hasattr(os, "O_CLOEXEC"):
        flags |= os.O_CLOEXEC
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        raise TargetFailure(
            "snapshot-tail fixture report was unavailable or unsafe"
        ) from error
    try:
        before = os.fstat(descriptor)
        require(
            stat.S_ISREG(before.st_mode)
            and 0 < before.st_size <= MAX_FIXTURE_REPORT_BYTES,
            "snapshot-tail fixture report was unavailable or unbounded",
        )
        chunks: list[bytes] = []
        remaining = MAX_FIXTURE_REPORT_BYTES + 1
        while remaining > 0:
            chunk = os.read(descriptor, remaining)
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        after = os.fstat(descriptor)
    except OSError as error:
        raise TargetFailure("snapshot-tail fixture report read failed") from error
    finally:
        os.close(descriptor)
    raw = b"".join(chunks)
    stable_fields = ("st_dev", "st_ino", "st_mode", "st_nlink", "st_size")
    stable_timestamps = ("st_mtime_ns", "st_ctime_ns")
    require(
        len(raw) == before.st_size
        and len(raw) <= MAX_FIXTURE_REPORT_BYTES
        and all(
            getattr(before, field) == getattr(after, field) for field in stable_fields
        )
        and all(
            getattr(before, field) == getattr(after, field)
            for field in stable_timestamps
        ),
        "snapshot-tail fixture report changed while being read",
    )
    return raw


def _run_snapshot_fixture(
    fixture_bin: Path,
    daemon: Any,
    record: RoomRecord,
    transition_count: int,
    timeout_seconds: float,
) -> tuple[dict[str, Any], bytes]:
    output = daemon.root / "snapshot-tail-fixture.json"
    try:
        result = subprocess.run(
            [
                str(fixture_bin),
                "--database",
                str(daemon.database_path),
                "--authority-secret-file",
                str(daemon.root / "authority.secret"),
                "--output",
                str(output),
                "--room-id",
                record.room_id,
                "--member-id",
                record.member_id,
                "--transition-count",
                str(transition_count),
            ],
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=timeout_seconds,
        )
    except subprocess.TimeoutExpired as error:
        raise TargetFailure(
            "snapshot-tail fixture generator exceeded its setup bound"
        ) from error
    require(result.returncode == 0, "snapshot-tail fixture generator failed")
    raw = stable_fixture_report_bytes(output)
    try:
        report = PRODUCER.ADAPTER.COLLECTOR.strict_json_object(
            raw, "snapshot-tail fixture report"
        )
    except PRODUCER.ADAPTER.COLLECTOR.CollectionError as error:
        raise TargetFailure(
            "snapshot-tail fixture report was not strict JSON"
        ) from error
    return report, raw


async def measure_snapshot_tail_recovery(
    args: argparse.Namespace,
    profile: dict[str, Any],
    expected_binary_sha256: str,
    expected_source_revision: str,
) -> tuple[dict[str, Any], bytes]:
    """Set up history outside the stopwatch, then time exact packaged recovery."""

    fixture_bin = args.snapshot_fixture_bin
    require(
        fixture_bin.is_file()
        and not fixture_bin.is_symlink()
        and os.access(fixture_bin, os.X_OK),
        "snapshot-tail fixture generator is not an executable regular file",
    )
    root = COMMON.private_root("worldstream-reference-target-recovery-")
    daemon = COMMON.Daemon(
        root,
        args.daemon_bin,
        startup_timeout=args.startup_timeout_seconds,
        shutdown_timeout=args.shutdown_timeout_seconds,
    )
    try:
        daemon.start()
        version = public_json(daemon.base_url, "/version")
        product_build = version.get("product_build")
        require(
            isinstance(product_build, dict)
            and product_build.get("binary") == "worldstreamd"
            and product_build.get("source_revision") == expected_source_revision,
            "packaged daemon source revision differs from the verified archive",
        )
        operator = Client(daemon.base_url, daemon.operator_bearer)
        rooms, failures = await create_rooms(operator, 1, concurrency=1)
        require(
            len(rooms) == 1 and failures == 0, "recovery fixture Room create failed"
        )
        record = rooms[0]
        member_bearer = await asyncio.to_thread(
            COMMON.issue_member_capability,
            daemon.base_url,
            daemon.operator_bearer,
            record.room_id,
            record.member_id,
            record.principal_id,
        )
        daemon.stop()

        transition_count = fixture_transition_count(profile)
        fixture, fixture_report_raw = await asyncio.to_thread(
            _run_snapshot_fixture,
            fixture_bin,
            daemon,
            record,
            transition_count,
            args.fixture_setup_timeout_seconds,
        )
        expected_fixture = {
            "schema": SNAPSHOT_FIXTURE_SCHEMA,
            "source_revision": product_build["source_revision"],
            "transition_kind": "alternating_authorized_membership_suspend_resume",
            "requested_transition_count": transition_count,
            "accepted_transition_count": transition_count,
            "setup_elapsed_ms": fixture.get("setup_elapsed_ms"),
            "head_room_seq": transition_count,
            "newest_snapshot_room_seq": transition_count - 2,
            "snapshot_lag_transitions": 2,
            "tail_recovery_transition_count": fixture.get(
                "tail_recovery_transition_count"
            ),
            "tail_recovery_elapsed_ms": fixture.get("tail_recovery_elapsed_ms"),
            "tail_recovery_activity_callbacks": fixture.get(
                "tail_recovery_activity_callbacks"
            ),
            "complete_head": fixture.get("complete_head"),
            "projection_hash": fixture.get("projection_hash"),
            "final_membership_standing": "enabled",
        }
        require(fixture == expected_fixture, "snapshot-tail fixture report drifted")
        require(
            type(fixture["setup_elapsed_ms"]) is int
            and fixture["setup_elapsed_ms"] >= 0
            and isinstance(fixture["complete_head"], dict)
            and fixture["complete_head"].get("room_seq") == transition_count
            and isinstance(fixture["projection_hash"], str),
            "snapshot-tail fixture observations were incomplete",
        )
        require(
            type(fixture["tail_recovery_transition_count"]) is int
            and 0 <= fixture["tail_recovery_transition_count"] <= 250
            and type(fixture["tail_recovery_elapsed_ms"]) is int
            and fixture["tail_recovery_elapsed_ms"] >= 0
            and type(fixture["tail_recovery_activity_callbacks"]) is int
            and fixture["tail_recovery_activity_callbacks"] >= 0,
            "snapshot-tail bounded recovery measurement was incomplete",
        )
        atomic_write_exact_bytes(
            args.snapshot_fixture_report,
            fixture_report_raw,
            maximum=MAX_FIXTURE_REPORT_BYTES,
        )
        fixture_report_sha256 = (
            "sha256:" + hashlib.sha256(fixture_report_raw).hexdigest()
        )

        recovery_started = time.monotonic()
        daemon.start()
        after = await Client(daemon.base_url, member_bearer).projection(record.room_id)
        recovery_ms = round((time.monotonic() - recovery_started) * 1_000, 3)
        before_head_sha256 = canonical_digest(fixture["complete_head"])
        after_head_sha256 = canonical_digest(after.get("room_head"))
        head_equal = fixture["complete_head"] == after.get("room_head")
        projection_equal = fixture["projection_hash"] == after.get("projection_hash")
        require(
            head_equal and projection_equal,
            "fresh packaged recovery changed the verified Head or Projection",
        )
        return {
            "requested_transition_count": 100_000,
            "accepted_transition_count": transition_count,
            "setup": {
                "boundary": "excluded_before_recovery_stopwatch",
                "mode": "source_bound_deterministic_production_core_commits",
                "source_revision": product_build["source_revision"],
                "generator_source_sha256": PRODUCER.sha256(
                    SNAPSHOT_FIXTURE_SOURCE_PATH
                ),
                "generator_binary_sha256": PRODUCER.sha256(fixture_bin),
                "generator_report_sha256": fixture_report_sha256,
                "generator_report_size_bytes": len(fixture_report_raw),
                "generator_report_schema": SNAPSHOT_FIXTURE_SCHEMA,
                "transition_kind": fixture["transition_kind"],
                "setup_elapsed_ms": fixture["setup_elapsed_ms"],
            },
            "snapshot": {
                "head_room_seq": fixture["head_room_seq"],
                "newest_snapshot_room_seq": fixture["newest_snapshot_room_seq"],
                "snapshot_lag_transitions": fixture["snapshot_lag_transitions"],
            },
            "recovery": {
                "boundary": "fresh_packaged_daemon_process_start_through_verified_current_projection",
                "daemon_binary_sha256": expected_binary_sha256,
                "recovery_ms": recovery_ms,
                "before_complete_head_sha256": before_head_sha256,
                "after_complete_head_sha256": after_head_sha256,
                "complete_head_equal": head_equal,
                "before_projection_hash": fixture["projection_hash"],
                "after_projection_hash": after["projection_hash"],
                "projection_hash_equal": projection_equal,
            },
        }, fixture_report_raw
    finally:
        daemon.stop()
        remove_owned_root(root)


async def open_idle_sessions(
    daemon: Any, records: list[RoomRecord]
) -> tuple[list[Any], list[asyncio.Task[None]]]:
    rooms: list[Any] = []
    semaphore = asyncio.Semaphore(64)

    async def one(record: RoomRecord) -> None:
        async with semaphore:
            try:
                rooms.append(await open_room(daemon, record))
            except (ProtocolError, COMMON.EvidenceFailure, OSError, TimeoutError):
                return

    await asyncio.gather(*(one(record) for record in records))

    async def keepalive(room: Any) -> None:
        iterator = room.events()
        try:
            async for _frame in iterator:
                raise TargetFailure("mostly-idle Session received an unexpected frame")
        finally:
            await iterator.aclose()

    tasks = [asyncio.create_task(keepalive(room)) for room in rooms]
    return rooms, tasks


async def close_idle_sessions(
    rooms: list[Any], tasks: list[asyncio.Task[None]]
) -> None:
    premature = [task for task in tasks if task.done()]
    for task in tasks:
        if not task.done():
            task.cancel()
    results = await asyncio.gather(*tasks, return_exceptions=True)
    close_results = await asyncio.gather(
        *(room.close() for room in rooms), return_exceptions=True
    )
    unexpected = [
        result for result in results if not isinstance(result, asyncio.CancelledError)
    ]
    close_failures = [
        result for result in close_results if isinstance(result, BaseException)
    ]
    require(
        not premature and not unexpected and not close_failures,
        "mostly-idle WebSocket Session ended before the measured window completed",
    )


async def run_rate_window(
    daemon: Any,
    records: list[RoomRecord],
    duration_seconds: int,
    seen: set[str],
) -> tuple[dict[str, Any], dict[str, Any]]:
    record_queue: asyncio.Queue[RoomRecord] = asyncio.Queue()
    for record in records:
        record_queue.put_nowait(record)
    opened: list[tuple[Any, int]] = []
    while len(opened) < min(ACTION_WORKERS, len(records)):
        try:
            record = record_queue.get_nowait()
        except asyncio.QueueEmpty:
            break
        try:
            opened.append((await open_room(daemon, record), 0))
        except (ProtocolError, COMMON.EvidenceFailure, OSError, TimeoutError):
            continue
    require(bool(opened), "reference rate attempt opened no Action Session")

    tokens: asyncio.Queue[None] = asyncio.Queue(maxsize=TOKEN_QUEUE_BOUND)
    buckets = [0 for _ in range(duration_seconds)]
    latencies: list[float] = []
    dispatch_attempt_count = 0
    dispatch_queue_full_count = 0
    action_attempt_count = 0
    late_accepted_transition_count = 0
    start_event = asyncio.Event()
    start_holder: list[float] = []

    async def next_session() -> tuple[Any, int] | None:
        while not record_queue.empty():
            try:
                record = record_queue.get_nowait()
            except asyncio.QueueEmpty:
                return None
            try:
                return await open_room(daemon, record), 0
            except (ProtocolError, COMMON.EvidenceFailure, OSError, TimeoutError):
                continue
        return None

    async def worker(initial: tuple[Any, int]) -> None:
        nonlocal action_attempt_count, late_accepted_transition_count
        room, accepted_for_room = initial
        await start_event.wait()
        start = start_holder[0]
        deadline = start + duration_seconds
        try:
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return
                try:
                    await asyncio.wait_for(tokens.get(), timeout=remaining)
                except TimeoutError:
                    return
                action_type = "increment" if accepted_for_room < 16 else "private_ack"
                before = time.perf_counter_ns()
                action_attempt_count += 1
                result = await room.act(
                    action_type, {}, action_id=COMMON.new_ulid(), timeout=15
                )
                after = time.monotonic()
                verify_accepted(result, seen)
                accepted_for_room += 1
                if after < deadline:
                    bucket = int(after - start)
                    if 0 <= bucket < duration_seconds:
                        buckets[bucket] += 1
                        latencies.append((time.perf_counter_ns() - before) / 1_000_000)
                else:
                    late_accepted_transition_count += 1
                if accepted_for_room == 32:
                    await room.close()
                    replacement = await next_session()
                    if replacement is None:
                        return
                    room, accepted_for_room = replacement
        finally:
            await room.close()

    tasks = [asyncio.create_task(worker(initial)) for initial in opened]
    start = time.monotonic()
    start_holder.append(start)
    start_event.set()
    deadline = start + duration_seconds
    interval = 1.0 / DISPATCH_RATE_PER_SECOND
    next_dispatch = start
    while True:
        now = time.monotonic()
        if now >= deadline:
            break
        if now < next_dispatch:
            await asyncio.sleep(min(next_dispatch - now, 0.01))
            continue
        dispatch_attempt_count += 1
        try:
            tokens.put_nowait(None)
        except asyncio.QueueFull:
            dispatch_queue_full_count += 1
        next_dispatch += interval
        if next_dispatch < now - interval:
            next_dispatch = now
    measured_window_elapsed = time.monotonic() - start
    await asyncio.gather(*tasks)
    require(bool(latencies), "reference rate window accepted no transition")
    accepted = sum(buckets)
    rate = {
        "configured_window_seconds": duration_seconds,
        "bucket_definition": (
            f"{duration_seconds} contiguous half-open one-second buckets from a "
            "monotonic start boundary"
        ),
        "window_start_basis": "monotonic_after_worker_readiness",
        "elapsed_seconds": round(measured_window_elapsed, 6),
        "full_second_bucket_count": duration_seconds,
        "dispatch_attempt_count": dispatch_attempt_count,
        "dispatch_queue_full_count": dispatch_queue_full_count,
        "action_attempt_count": action_attempt_count,
        "unconsumed_token_count": tokens.qsize(),
        "minimum_accepted_per_full_second": min(buckets),
        "accepted_transition_count": accepted,
        "late_accepted_transition_count": late_accepted_transition_count,
        "aggregate_accepted_per_second": round(accepted / duration_seconds, 3),
    }
    return rate, latency_triplet(latencies)


def offline_room_count(database: Path) -> int:
    uri = f"file:{database.resolve().as_posix()}?mode=ro"
    connection = sqlite3.connect(uri, uri=True)
    try:
        connection.execute("PRAGMA query_only = ON")
        return int(connection.execute("SELECT count(*) FROM rooms").fetchone()[0])
    finally:
        connection.close()


def database_bytes(database: Path) -> int:
    return sum(
        path.stat().st_size
        for path in database.parent.glob(database.name + "*")
        if path.is_file() and not path.is_symlink()
    )


def remove_owned_root(root: Path) -> None:
    """Remove only the exact private target root and prove it is absent."""

    temporary_parent = Path(tempfile.gettempdir()).resolve()
    require(
        root.is_absolute()
        and root.parent.resolve() == temporary_parent
        and root.name.startswith("worldstream-reference-target-")
        and root != temporary_parent,
        "reference target cleanup root was outside its bounded ownership",
    )
    require(not root.is_symlink(), "reference target cleanup root became a symlink")
    if root.exists():
        shutil.rmtree(root)
    require(
        not root.exists() and not root.is_symlink(),
        "reference target cleanup did not remove the owned root",
    )


async def execute(args: argparse.Namespace) -> dict[str, Any]:
    profile = (
        reduced_profile()
        if args.nonpublishable_reduced_test_profile
        else frozen_profile()
    )
    validate_timeout_contract(args, profile)
    manifest = PRODUCER.ADAPTER.COLLECTOR.load_manifest(
        args.manifest_toml, args.manifest_json
    )
    distribution, _package, _package_raw, package_binding = (
        PRODUCER.verify_packaged_distribution(
            args.package_archive,
            args.package_report,
            args.daemon_bin,
            args.manifest_toml,
            args.manifest_json,
            manifest,
        )
    )
    expected_packaged_sdk = PRODUCER.packaged_sdk_identity(args.package_archive)
    runtime_packaged_sdk = verify_runtime_packaged_sdk(
        args.packaged_sdk_root, expected_packaged_sdk
    )
    acceptance, acceptance_raw, acceptance_sha256 = PRODUCER.verify_packaged_acceptance(
        args.packaged_acceptance_report, distribution, package_binding
    )
    soak, soak_raw = PRODUCER.read_json(args.soak_report, "one-hour soak report")
    kill, kill_raw = PRODUCER.read_json(args.kill_point_report, "kill-point report")
    soak_validated = FAILURE.validate_soak(
        soak,
        version=manifest["release_candidate"],
        manifest_json_sha256=distribution["manifest_json_sha256"],
        manifest_toml_sha256=distribution["manifest_toml_sha256"],
        packaged_acceptance_sha256=acceptance_sha256,
    )
    kill_validated = FAILURE.validate_kill(
        kill,
        version=manifest["release_candidate"],
        manifest_json_sha256=distribution["manifest_json_sha256"],
        manifest_toml_sha256=distribution["manifest_toml_sha256"],
    )
    for validated, label in ((soak_validated, "soak"), (kill_validated, "kill")):
        source_distribution = validated["distribution"]
        require(
            source_distribution
            == {field: distribution[field] for field in source_distribution},
            f"{label} report does not bind the exact target package",
        )
    history, fixture_report_raw = await measure_snapshot_tail_recovery(
        args,
        profile,
        distribution["binary_sha256"],
        distribution["source_revision"],
    )
    bindings = {
        "package_archive": PRODUCER.file_binding(
            args.package_archive, "worldstream/linux-release-archive/v1"
        ),
        "package_report": PRODUCER.file_binding(
            args.package_report, "worldstream/package-report/v1"
        ),
        "daemon": PRODUCER.file_binding(
            args.daemon_bin, "worldstream/worldstreamd-elf/v1"
        ),
        "packaged_acceptance": PRODUCER.exact_binding(
            acceptance_raw, acceptance["schema"]
        ),
        "one_hour_soak": PRODUCER.exact_binding(soak_raw, soak["schema"]),
        "kill_point": PRODUCER.exact_binding(kill_raw, kill["schema"]),
        "manifest_toml": PRODUCER.file_binding(
            args.manifest_toml, "worldstream/storage-compatibility-manifest/toml"
        ),
        "manifest_json": PRODUCER.file_binding(args.manifest_json, manifest["schema"]),
        "snapshot_fixture_binary": PRODUCER.file_binding(
            args.snapshot_fixture_bin,
            "worldstream/reference-snapshot-tail-fixture-elf/v1",
        ),
        "snapshot_fixture_source": PRODUCER.file_binding(
            SNAPSHOT_FIXTURE_SOURCE_PATH,
            "worldstream/reference-snapshot-tail-fixture-source/v1",
        ),
        "snapshot_fixture_report": PRODUCER.exact_binding(
            fixture_report_raw, SNAPSHOT_FIXTURE_SCHEMA
        ),
    }
    external_sources = {
        "packaged_acceptance": {
            "source_attribution": "bound_external_source",
            "report_sha256": bindings["packaged_acceptance"]["sha256"],
            "source_environment": acceptance["reference_environment"],
        },
        "one_hour_soak": {
            "source_attribution": "bound_external_source",
            "report_sha256": bindings["one_hour_soak"]["sha256"],
            "source_environment": soak["environment_observation"],
        },
        "kill_point": {
            "source_attribution": "bound_external_source",
            "report_sha256": bindings["kill_point"]["sha256"],
            "source_environment": {"platform": kill["platform"]},
        },
    }

    root = COMMON.private_root("worldstream-reference-target-")
    daemon = COMMON.Daemon(
        root,
        args.daemon_bin,
        startup_timeout=args.startup_timeout_seconds,
        shutdown_timeout=args.shutdown_timeout_seconds,
    )
    idle_rooms: list[Any] = []
    idle_tasks: list[asyncio.Task[None]] = []
    sampler = RssSampler()
    sampler_task: asyncio.Task[None] | None = None
    seen: set[str] = set()
    try:
        daemon.start()
        require(daemon.process is not None, "packaged daemon process was unavailable")
        sampler_task = asyncio.create_task(sampler.run(daemon.process.pid))
        current_host = HOST.observe(daemon.data_dir)
        version = public_json(daemon.base_url, "/version")
        engine = version.get("engine")
        require(
            isinstance(engine, dict)
            and engine.get("status") == "verified"
            and engine.get("profile") == "sqlite-bundled"
            and isinstance(engine.get("exact_identity"), str)
            and engine["exact_identity"].startswith("sqlite/3.53.4;"),
            "packaged daemon did not expose its verified bundled SQLite identity",
        )
        target_environment = {
            **current_host,
            "engines": {
                "sqlite": SQLITE_ENGINE,
                "postgresql": {"status": "not_observed_by_sqlite_target_workload"},
            },
        }
        PRODUCER.AGGREGATOR._validate_reference_environment(
            "target", {"reference_environment": target_environment}
        )
        operator = Client(daemon.base_url, daemon.operator_bearer)
        rooms, create_failures = await create_rooms(
            operator, profile["stored_room_target"]
        )
        require(
            len(rooms) + create_failures == profile["stored_room_target"],
            "Room create attempt accounting was incomplete",
        )
        require(bool(rooms), "reference workload created no Room")
        idle_target = min(profile["idle_websocket_target"], max(0, len(rooms) - 1))
        idle_records = rooms[1 : 1 + idle_target]
        idle_rooms, idle_tasks = await open_idle_sessions(daemon, idle_records)
        rate_records = rooms[1 + idle_target :]
        rate, latency = await run_rate_window(
            daemon,
            rate_records,
            profile["sustained_transition_window_seconds"],
            seen,
        )
        idle_success = len(idle_rooms)
        await close_idle_sessions(idle_rooms, idle_tasks)
        idle_tasks = []
        idle_rooms = []
        daemon.stop()
        sampler.stop()
        if sampler_task is not None:
            await sampler_task
            sampler_task = None
        stored_count = offline_room_count(daemon.database_path)
        retained_database_bytes = database_bytes(daemon.database_path)
        require(
            stored_count == len(rooms),
            "accepted Room creation was not durably retained offline",
        )

        stored_met = stored_count >= profile["stored_room_target"]
        loaded_met = idle_success >= profile["loaded_room_target"]
        idle_met = idle_success >= profile["idle_websocket_target"]
        rate_met = (
            rate["full_second_bucket_count"]
            == profile["sustained_transition_window_seconds"]
            and rate["minimum_accepted_per_full_second"]
            >= profile["sustained_transition_rate_target_per_second"]
        )
        latency_met = latency["p95_ms"] < profile["commit_to_ack_p95_target_ms"]
        history_met = (
            history["accepted_transition_count"]
            >= profile["history_room_transition_target"]
            and history["snapshot"]["snapshot_lag_transitions"]
            <= profile["snapshot_maximum_lag_transitions"]
            and history["recovery"]["recovery_ms"]
            <= profile["snapshot_tail_recovery_target_ms"]
            and history["recovery"]["complete_head_equal"] is True
            and history["recovery"]["projection_hash_equal"] is True
        )
        dimensions = [
            dimension(
                "stored_passivated_rooms",
                classification="measured_non_sla_performance",
                completed=True,
                observed={
                    "create_attempt_count": profile["stored_room_target"],
                    "created_room_count": len(rooms),
                    "offline_stored_room_count": stored_count,
                },
                target={"minimum_room_count": profile["stored_room_target"]},
                met=stored_met,
                method="public create receipts plus read-only closed SQLite room count",
            ),
            dimension(
                "simultaneously_loaded_rooms",
                classification="measured_non_sla_performance",
                completed=True,
                observed={
                    "open_attempt_count": idle_target,
                    "peak_simultaneously_loaded_room_count": idle_success,
                },
                target={"minimum_loaded_room_count": profile["loaded_room_target"]},
                met=loaded_met,
                method="distinct simultaneously live and sync-acknowledged Room Sessions",
            ),
            dimension(
                "mostly_idle_websocket_sessions",
                classification="measured_non_sla_performance",
                completed=True,
                observed={
                    "open_attempt_count": idle_target,
                    "peak_connected_idle_session_count": idle_success,
                    "observation_window_seconds": profile[
                        "sustained_transition_window_seconds"
                    ],
                },
                target={"minimum_session_count": profile["idle_websocket_target"]},
                met=idle_met,
                method="real SDK WebSockets kept responsive to application heartbeats",
            ),
            dimension(
                "sustained_accepted_transition_rate",
                classification="measured_non_sla_performance",
                completed=True,
                observed=rate,
                target={
                    "minimum_per_second": profile[
                        "sustained_transition_rate_target_per_second"
                    ],
                    "continuous_window_seconds": profile[
                        "sustained_transition_window_seconds"
                    ],
                },
                met=rate_met,
                method="ActionAccepted receipts in exact monotonic half-open buckets",
            ),
            dimension(
                "commit_to_ack_latency",
                classification="measured_non_sla_performance",
                completed=True,
                observed=latency,
                target={
                    "maximum_p95_ms_exclusive": profile["commit_to_ack_p95_target_ms"]
                },
                met=latency_met,
                method="local monotonic Action submit-to-ActionAccepted completion",
            ),
            dimension(
                "one_hour_bounded_soak",
                classification="bound_hard_gate",
                completed=True,
                observed={
                    "elapsed_seconds": soak_validated["elapsed_seconds"],
                    "window_completed": True,
                    "report_sha256": bindings["one_hour_soak"]["sha256"],
                    "source_attribution": "bound_external_source",
                },
                target={"minimum_elapsed_seconds": 3_600},
                met=True,
                method="strict validation of separately produced exact raw soak bytes",
            ),
            dimension(
                "snapshot_tail_recovery",
                classification="measured_non_sla_performance",
                completed=True,
                observed=history,
                target={
                    "minimum_room_transition_count": 100_000,
                    "maximum_snapshot_lag_transitions": 250,
                    "maximum_recovery_ms": 5_000,
                },
                met=history_met,
                method=(
                    "source-bound production Core fixture setup followed by fresh exact "
                    "packaged-daemon recovery through a verified current Projection"
                ),
            ),
            dimension(
                "repeated_forced_termination_no_acknowledged_loss",
                classification="bound_hard_gate",
                completed=True,
                observed={
                    "forced_termination_count": kill_validated["cell_count"],
                    "acknowledged_loss_count": 0,
                    "kill_report_sha256": bindings["kill_point"]["sha256"],
                    "source_attribution": "bound_external_source",
                },
                target={
                    "minimum_forced_termination_count": 2,
                    "maximum_acknowledged_loss_count": 0,
                },
                met=True,
                method="strict validation of separately produced exact kill bytes",
            ),
        ]
        report = {
            "schema": PRODUCER.REFERENCE_TARGET_SCHEMA,
            "status": "completed",
            "release_evidence": False,
            "performance_class": "reference_non_release",
            "execution": {
                "mode": "package_bound_linux_reference",
                "workload_source": (
                    "packaged_worldstreamd_public_api_plus_source_bound_fixture_setup"
                ),
                "storage_profile": "sqlite-bundled",
                "connection_mode": "embedded",
                "public_api_only": False,
                "simulated": False,
                "scaled": args.nonpublishable_reduced_test_profile,
                "profile_args_locked": not args.nonpublishable_reduced_test_profile,
                "runner_sha256": PRODUCER.sha256(Path(__file__)),
            },
            "profile": profile,
            "identity": {
                "product": "worldstream",
                "profile": "linux-reference",
                "version": manifest["release_candidate"],
                "artifact_sha256": distribution["archive_sha256"],
                "binary_sha256": distribution["binary_sha256"],
                "manifest_json_sha256": distribution["manifest_json_sha256"],
                "manifest_toml_sha256": distribution["manifest_toml_sha256"],
                "packaged_acceptance_sha256": acceptance_sha256,
                "packaged_sdk": runtime_packaged_sdk,
            },
            "reference_environment": target_environment,
            "reference_workload": {
                "payload_sizes_bytes": [2],
                "pack_id": COUNTER_PACK["id"],
                "participants_per_room": 1,
                "fan_out": 1,
                "snapshot_cadence_transitions": 1,
            },
            "runtime_observation": {
                "sqlite": {
                    "status": "observed_runtime_verified",
                    "profile": engine["profile"],
                    "exact_identity": engine["exact_identity"],
                },
                "postgresql": {"status": "not_observed_by_sqlite_target_workload"},
            },
            "bindings": bindings,
            "bound_external_sources": external_sources,
            "dimensions": dimensions,
            "correctness": {
                "status": "passed",
                "acknowledged_transition_ids_unique": True,
                "acknowledged_transition_receipts_verified": True,
                "acknowledged_loss_count": 0,
                "hard_gate_inputs_validated": True,
            },
            "measurements": {
                "commit_to_ack_latency": latency,
                "transition_rate": rate,
                "resource_observation": {
                    "peak_process_tree_rss_bytes": sampler.peak_bytes,
                    "process_tree_rss_sample_count": sampler.sample_count,
                    "database_bytes_after_workload": retained_database_bytes,
                },
            },
            "limitations": (
                []
                if history_met
                else [
                    {
                        "code": "snapshot_tail_recovery_target_missed",
                        "dimension": "snapshot_tail_recovery",
                        "publishable_non_sla_target_miss": True,
                    }
                ]
            ),
        }
        if not args.nonpublishable_reduced_test_profile:
            PRODUCER.validate_reference_target(
                report,
                manifest=manifest,
                distribution=distribution,
                packaged_acceptance_sha256=acceptance_sha256,
                expected_bindings=bindings,
                expected_external_sources=external_sources,
                expected_packaged_sdk=expected_packaged_sdk,
                kill_cell_count=kill_validated["cell_count"],
                soak_elapsed_seconds=soak_validated["elapsed_seconds"],
            )
        return report
    finally:
        primary_error = sys.exception()
        cleanup_errors: list[BaseException] = []
        try:
            await close_idle_sessions(idle_rooms, idle_tasks)
        except BaseException as error:  # noqa: BLE001 - later cleanup must still run.
            cleanup_errors.append(error)
        try:
            daemon.stop()
        except BaseException as error:  # noqa: BLE001 - later cleanup must still run.
            cleanup_errors.append(error)
        sampler.stop()
        if sampler_task is not None:
            try:
                await sampler_task
            except BaseException as error:  # noqa: BLE001 - root cleanup must still run.
                cleanup_errors.append(error)
        try:
            remove_owned_root(root)
        except BaseException as error:  # noqa: BLE001 - preserve cleanup failure evidence.
            cleanup_errors.append(error)
        if cleanup_errors:
            if primary_error is None:
                raise cleanup_errors[0]
            primary_error.add_note(
                "reference target cleanup also failed: "
                + ", ".join(type(error).__name__ for error in cleanup_errors)
            )


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--output", type=Path, required=True)
    command.add_argument("--daemon-bin", type=Path, required=True)
    command.add_argument("--package-archive", type=Path, required=True)
    command.add_argument("--package-report", type=Path, required=True)
    command.add_argument("--packaged-acceptance-report", type=Path, required=True)
    command.add_argument("--packaged-sdk-root", type=Path, required=True)
    command.add_argument("--soak-report", type=Path, required=True)
    command.add_argument("--kill-point-report", type=Path, required=True)
    command.add_argument("--snapshot-fixture-bin", type=Path, required=True)
    command.add_argument("--snapshot-fixture-report", type=Path, required=True)
    command.add_argument(
        "--startup-timeout-seconds",
        type=float,
        default=FROZEN_STARTUP_TIMEOUT_SECONDS,
    )
    command.add_argument(
        "--shutdown-timeout-seconds",
        type=float,
        default=FROZEN_SHUTDOWN_TIMEOUT_SECONDS,
    )
    command.add_argument(
        "--fixture-setup-timeout-seconds",
        type=float,
        default=FROZEN_FIXTURE_SETUP_TIMEOUT_SECONDS,
    )
    command.add_argument(
        "--nonpublishable-reduced-test-profile",
        action="store_true",
        help=argparse.SUPPRESS,
    )
    command.add_argument(
        "--manifest-toml",
        type=Path,
        default=PRODUCER.ADAPTER.COLLECTOR.DEFAULT_MANIFEST_TOML,
    )
    command.add_argument(
        "--manifest-json",
        type=Path,
        default=PRODUCER.ADAPTER.COLLECTOR.DEFAULT_MANIFEST_JSON,
    )
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        report = asyncio.run(execute(args))
        atomic_write(args.output, report)
    except (
        TargetFailure,
        PRODUCER.ReferenceError,
        PRODUCER.ADAPTER.ProducerError,
        PRODUCER.ADAPTER.COLLECTOR.CollectionError,
        FAILURE.EvidenceError,
        COMMON.EvidenceFailure,
        HOST.ReferenceHostError,
        OSError,
        ValueError,
    ) as error:
        notes = getattr(error, "__notes__", ())
        suffix = f"; {'; '.join(notes)}" if notes else ""
        print(f"reference target workload failed: {error}{suffix}", file=sys.stderr)
        return 1
    print(f"wrote reference target workload report: {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

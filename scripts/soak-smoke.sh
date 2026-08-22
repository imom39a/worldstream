#!/usr/bin/env bash
set -euo pipefail

# Keep one stable, dependency-light entry point for the evidence command.
workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace_dir"
exec python3 - "$@" <<'PY'
from __future__ import annotations

import argparse
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
from typing import Any, Iterable

try:
    import resource
except ImportError:  # Windows has no resource module; keep evidence machine-readable.
    resource = None  # type: ignore[assignment]


PACKAGES = ("worldstream-sqlite", "worldstream-backup")
MATRIX_COMMAND = [
    "cargo",
    "test",
    "-p",
    PACKAGES[0],
    "-p",
    PACKAGES[1],
    "--locked",
]
SCHEMA = "worldstream/soak-evidence/v1"
DEFAULT_ITERATIONS = 3
DEFAULT_COMMAND_TIMEOUT = 120.0
DEFAULT_TOTAL_TIMEOUT = 300.0
ONE_HOUR_SECONDS = 3600.0
DEFAULT_MAX_OUTPUT_BYTES = 256 * 1024
DEFAULT_MAX_DATABASE_GROWTH = 256 * 1024 * 1024
DEFAULT_MAX_ITERATIONS = 100_000
UNBOUND_DATABASE_WORKLOAD = "observer_only_not_bound_to_cargo_test_databases"

# Pattern-based checks tolerate additive tests while failing closed if a
# required semantic area disappears from the named SQLite matrix.
COVERAGE_GROUPS: dict[str, tuple[str, ...]] = {
    "create": (r"(^|::)create_", r"bootstrap"),
    "action": (r"(^|::)action_", r"advance"),
    "timer": (r"timer",),
    "snapshot": (r"snapshot",),
    "recovery": (r"recovery", r"replay"),
    "resource_or_storage_fault": (
        r"failpoint",
        r"read_only",
        r"writer_lock",
        r"unavailable",
        r"failure",
    ),
    # The compatibility manifest treats fuzz/property evidence as a separate
    # acceptance requirement.  Keep it fail-closed: a deterministic unit test
    # matrix is not fuzz evidence merely because it exercises many branches.
    "fuzz": (r"fuzz", r"proptest", r"quickcheck", r"property[_-]?based"),
    "corruption_or_quarantine": (
        r"corrupt",
        r"corruption",
        r"quarantine",
        r"missing_integrity",
    ),
    "concurrency": (r"contention", r"concurr", r"two_candidates", r"writer_lock"),
    "authority": (r"authority", r"capability", r"revocation", r"principal"),
    "delivery": (r"attach", r"observation", r"visibility", r"frame"),
    "migration": (r"migration",),
    # These groups bind the ordinary bounded conformance report to the exact
    # release producer checks. They remain fixture evidence: the separate
    # failure/soak producer still requires a real process-kill matrix and a
    # workload-bound one-hour database measurement.
    "backup_restore": (r"backup", r"restore"),
    "canonical_hash_parity": (
        r"canonical.*(?:hash|parity|round_trip)",
        r"(?:hash|byte).*parity",
    ),
    "native_restore": (
        r"sealed_native_envelope_constructs_and_verifies_real_online_restore",
    ),
}

# Exact fixture hooks are discovered from `cargo test -- --list`.  They are
# deliberately reported as fixture-only and never promoted to process-level
# crash evidence.
FIXTURE_HOOKS: dict[str, tuple[str, ...]] = {
    "resource": (r"actual_read_only_driver_error", r"writer_lock", r"failpoint"),
    "fault": (r"faulted_replay", r"snapshot_failure", r"unavailable_retained_runtime"),
    "corruption": (
        r"corrupt_paired_snapshot",
        r"recovery_quarantines",
        r"replay_verification_rejects",
        r"missing_integrity",
    ),
    # In-process restart/unknown-commit tests do not match this category.
    "kill_point": (r"(^|::)(kill|process_kill|power_loss)(_|$)",),
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Bounded repeated WorldStream SQLite critical-matrix evidence"
    )
    parser.add_argument("--iterations", type=int, default=DEFAULT_ITERATIONS)
    parser.add_argument(
        "--one-hour",
        action="store_true",
        help="repeat until the bounded one-hour wall-clock window ends",
    )
    parser.add_argument(
        "--command-timeout-seconds", type=float, default=DEFAULT_COMMAND_TIMEOUT
    )
    parser.add_argument(
        "--max-total-seconds",
        type=float,
        default=None,
        help="lower the total bound; no invocation may exceed one hour",
    )
    parser.add_argument("--max-iterations", type=int, default=DEFAULT_MAX_ITERATIONS)
    parser.add_argument("--max-output-bytes", type=int, default=DEFAULT_MAX_OUTPUT_BYTES)
    parser.add_argument(
        "--max-database-growth-bytes",
        type=int,
        default=DEFAULT_MAX_DATABASE_GROWTH,
    )
    parser.add_argument(
        "--database",
        metavar="PATH",
        help="optional SQLite file; -wal and -shm sidecars are included",
    )
    parser.add_argument("--output", metavar="PATH", help="also write the JSON report here")
    return parser.parse_args()


def percentile(values: list[float], fraction: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    rank = max(1, min(len(ordered), math.ceil(fraction * len(ordered))))
    return ordered[rank - 1]


def json_number(value: float | int | None) -> float | int | None:
    if value is None:
        return None
    return round(value, 3) if isinstance(value, float) else value


def redact_text(value: str) -> str:
    value = re.sub(r"/(?:[^\s/]+/)+[^\s/]+", "<redacted-path>", value)
    value = re.sub(r"[A-Za-z]:\\[^\s]+", "<redacted-path>", value)
    value = re.sub(r"(?i)(bearer\s+)[^\s]+", r"\1<redacted>", value)
    return value[:400]


def argv_for_report(argv: Iterable[str]) -> list[str]:
    values = list(argv)
    if values:
        values[0] = Path(values[0]).name
    return [redact_text(value) for value in values]


def read_proc_status_rss(pid: int) -> int | None:
    if not sys.platform.startswith("linux"):
        return None
    try:
        status = Path(f"/proc/{pid}/status").read_text(encoding="utf-8")
    except (FileNotFoundError, OSError):
        return None
    match = re.search(r"^VmRSS:\s+(\d+)\s+kB$", status, re.MULTILINE)
    return int(match.group(1)) * 1024 if match else None


class MemorySampler:
    def __init__(self, pid: int) -> None:
        self.pid = pid
        self.peak_bytes: int | None = None
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    def start(self) -> None:
        if read_proc_status_rss(self.pid) is None:
            return

        def sample() -> None:
            while not self._stop.is_set():
                current = read_proc_status_rss(self.pid)
                if current is not None:
                    self.peak_bytes = max(self.peak_bytes or 0, current)
                self._stop.wait(0.05)

        self._thread = threading.Thread(target=sample, daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=1.0)
        final = read_proc_status_rss(self.pid)
        if final is not None:
            self.peak_bytes = max(self.peak_bytes or 0, final)


class BoundedCapture:
    def __init__(self, stream: Any, limit: int) -> None:
        self.stream = stream
        self.limit = limit
        self.total_bytes = 0
        self.retained = bytearray()
        self.thread = threading.Thread(target=self._drain, daemon=True)

    def start(self) -> None:
        self.thread.start()

    def _drain(self) -> None:
        while True:
            chunk = self.stream.read(64 * 1024)
            if not chunk:
                return
            self.total_bytes += len(chunk)
            if len(self.retained) < self.limit:
                self.retained.extend(chunk[: self.limit - len(self.retained)])

    def join(self) -> None:
        self.thread.join(timeout=2.0)


def terminate_process(process: subprocess.Popen[bytes]) -> bool:
    try:
        if os.name == "posix":
            os.killpg(process.pid, signal.SIGTERM)
        else:  # pragma: no cover - local evidence is primarily POSIX.
            process.terminate()
    except (ProcessLookupError, OSError):
        pass
    try:
        process.wait(timeout=2.0)
        return False
    except subprocess.TimeoutExpired:
        try:
            if os.name == "posix":
                os.killpg(process.pid, signal.SIGKILL)
            else:  # pragma: no cover
                process.kill()
        except (ProcessLookupError, OSError):
            pass
        try:
            process.wait(timeout=2.0)
        except subprocess.TimeoutExpired:
            pass
        return True


def run_command(
    argv: list[str],
    *,
    timeout_seconds: float,
    output_limit: int,
    root: Path,
) -> dict[str, Any]:
    started = time.monotonic()
    env = os.environ.copy()
    env.update(
        {
            "CARGO_TERM_COLOR": "never",
            "RUST_BACKTRACE": "0",
            "CARGO_INCREMENTAL": "0",
        }
    )
    try:
        process = subprocess.Popen(
            argv,
            cwd=root,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            start_new_session=os.name == "posix",
        )
    except (FileNotFoundError, OSError) as exc:
        return {
            "status": "not_started",
            "failure_class": "unavailable",
            "exit_code": None,
            "timed_out": False,
            "duration_ms": json_number((time.monotonic() - started) * 1000),
            "argv": argv_for_report(argv),
            "output_bytes": 0,
            "output_truncated": False,
            "error": redact_text(str(exc)),
            "memory": {"status": "not_measured", "reason": "process_not_started"},
        }

    capture = BoundedCapture(process.stdout, output_limit)
    capture.start()
    memory = MemorySampler(process.pid)
    memory.start()
    timed_out = False
    try:
        process.wait(timeout=timeout_seconds)
    except subprocess.TimeoutExpired:
        timed_out = True
        terminate_process(process)
    finally:
        memory.stop()
        capture.join()
    usage = resource.getrusage(resource.RUSAGE_CHILDREN) if resource is not None else None
    retained_text = bytes(capture.retained).decode("utf-8", errors="replace")
    passed_counts = [
        int(value)
        for value in re.findall(r"test result: ok\. (\d+) passed;", retained_text)
    ]
    failed_counts = [
        int(value)
        for value in re.findall(r"test result: FAILED\. (\d+) passed;", retained_text)
    ]
    if memory.peak_bytes is not None:
        memory_payload = {
            "status": "measured",
            "method": "linux_proc_vmrss_peak",
            "scope": "cargo_process_only",
            "peak_rss_bytes": memory.peak_bytes,
        }
    elif usage is not None:
        unit = 1024 if sys.platform.startswith("linux") else 1
        memory_payload = {
            "status": "measured",
            "method": "child_rusage_cumulative_maxrss",
            "scope": "all_waited_children_cumulative",
            "peak_rss_bytes": int(usage.ru_maxrss * unit),
        }
    else:
        memory_payload = {
            "status": "not_measured",
            "method": "unavailable",
            "scope": "no portable child RSS provider",
            "peak_rss_bytes": None,
        }
    duration_ms = (time.monotonic() - started) * 1000
    if timed_out:
        failure_class = "timeout"
        status = "timeout"
    elif capture.total_bytes > output_limit:
        failure_class = "output_limit"
        status = "failed"
    elif process.returncode == 0:
        failure_class = "none"
        status = "passed"
    elif process.returncode in {126, 127}:
        failure_class = "unavailable"
        status = "failed"
    else:
        failure_class = "test_failure"
        status = "failed"
    return {
        "status": status,
        "failure_class": failure_class,
        "exit_code": process.returncode,
        "timed_out": timed_out,
        "duration_ms": json_number(duration_ms),
        "argv": argv_for_report(argv),
        "output_bytes": capture.total_bytes,
        "output_truncated": capture.total_bytes > output_limit,
        "reported_passed_tests": sum(passed_counts),
        "reported_failed_test_runs": len(failed_counts),
        "memory": memory_payload,
    }


def run_command_with_capture(
    argv: list[str], *, timeout_seconds: float, output_limit: int, root: Path
) -> dict[str, Any]:
    """Run once and retain only bounded bytes for test-list parsing."""
    env = os.environ.copy()
    env.update({"CARGO_TERM_COLOR": "never", "RUST_BACKTRACE": "0", "CARGO_INCREMENTAL": "0"})
    started = time.monotonic()
    try:
        process = subprocess.Popen(
            argv,
            cwd=root,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            start_new_session=os.name == "posix",
        )
    except (FileNotFoundError, OSError) as exc:
        return {"status": "not_started", "output": b"", "error": redact_text(str(exc))}
    capture = BoundedCapture(process.stdout, output_limit)
    capture.start()
    timed_out = False
    try:
        process.wait(timeout=timeout_seconds)
    except subprocess.TimeoutExpired:
        timed_out = True
        terminate_process(process)
    capture.join()
    return {
        "status": "timeout" if timed_out else ("passed" if process.returncode == 0 else "failed"),
        "output": bytes(capture.retained),
        "duration_ms": json_number((time.monotonic() - started) * 1000),
    }


def parse_test_names(output: bytes) -> list[str]:
    pattern = re.compile(r"^\s*([A-Za-z0-9_.:]+): test\s*$")
    return sorted(
        {
            match.group(1)
            for line in output.decode("utf-8", errors="replace").splitlines()
            if (match := pattern.match(line))
        }
    )


def matching_names(names: list[str], patterns: tuple[str, ...]) -> list[str]:
    compiled = [re.compile(pattern, re.IGNORECASE) for pattern in patterns]
    return [name for name in names if any(pattern.search(name) for pattern in compiled)]


def validate_gaps(names: list[str]) -> dict[str, Any]:
    groups: dict[str, Any] = {}
    missing: list[str] = []
    for group, patterns in COVERAGE_GROUPS.items():
        matches = matching_names(names, patterns)
        groups[group] = {
            "status": "covered" if matches else "missing",
            "match_count": len(matches),
            "sample_tests": matches[:5],
        }
        if not matches:
            missing.append(group)
    return {
        "status": "pass" if names and not missing else "fail",
        "listed_test_count": len(names),
        "coverage_groups": groups,
        "missing_groups": missing,
        "method": "semantic_name_coverage",
        "limitation": "Named local fixtures do not prove every acceptance criterion.",
    }


def read_database(path: Path | None) -> dict[str, Any]:
    if path is None:
        return {"status": "not_configured", "bytes": None, "files_present": None}
    total = 0
    present = 0
    for candidate in (path, Path(f"{path}-wal"), Path(f"{path}-shm")):
        try:
            stat = candidate.stat()
        except (FileNotFoundError, OSError):
            continue
        if stat.st_mode and candidate.is_file():
            total += stat.st_size
            present += 1
    return {"status": "measured" if present else "not_present", "bytes": total, "files_present": present}


def cargo_path() -> str | None:
    configured = os.environ.get("WORLDSTREAM_SOAK_CARGO", "cargo")
    if os.path.sep in configured:
        candidate = Path(configured)
        return str(candidate) if candidate.is_file() and os.access(candidate, os.X_OK) else None
    return shutil.which(configured)


def initial_report(args: argparse.Namespace, total_bound: float) -> dict[str, Any]:
    return {
        "schema": SCHEMA,
        "status": "error",
        "release_evidence": False,
        "evidence_class": "bounded_fixture_only",
        "mode": "one_hour" if args.one_hour else "default",
        "configuration": {
            "iterations": None if args.one_hour else args.iterations,
            "command_timeout_seconds": json_number(args.command_timeout_seconds),
            "max_total_seconds": json_number(total_bound),
            "max_iterations": args.max_iterations,
            "max_output_bytes": args.max_output_bytes,
            "max_database_growth_bytes": args.max_database_growth_bytes,
            "one_hour_target_seconds": ONE_HOUR_SECONDS if args.one_hour else None,
            "database_workload_binding": UNBOUND_DATABASE_WORKLOAD,
            "packages": list(PACKAGES),
            "matrix_command": MATRIX_COMMAND,
        },
        "platform": {"system": platform.system(), "machine": platform.machine()},
        "named_gate_evidence": {
            "manifest_evidence_id": "failure-fuzz-resource-and-one-hour-sqlite-soak",
            "gate_cell": "failure-fuzz-resource-and-one-hour-sqlite-soak",
            "release_gate": True,
            "release_evidence": False,
            "handoff_status": "diagnostic_only",
            "process_kill_evidence": "scripts/kill-point-smoke.sh",
        },
        "redaction": {
            "paths": "omitted_or_redacted",
            "secrets": "not_collected",
            "command_output": "not_emitted",
        },
        "evidence_scope": {
            "fixture_only": True,
            "process_level": False,
            "process_kill_claim": False,
            "release_evidence": False,
            "database_workload_bound": False,
            "reason": "The runner executes in-process Cargo fixtures; it does not prove process crash or power-loss recovery.",
        },
        "preflight": None,
        "fixture_hooks": [],
        "matrix_runs": [],
        "statistics": None,
        "database": {
            "status": "not_configured",
            "growth_bytes": None,
            "workload_binding": UNBOUND_DATABASE_WORKLOAD,
        },
        "workload": {
            "kind": "cargo_test_fixture_matrix",
            "database_binding": UNBOUND_DATABASE_WORKLOAD,
            "accepted_transition_count": None,
        },
        "kill_points": {
            "status": "not_exposed",
            "evidence_class": "not_measured",
            "process_kill_claim": False,
            "power_loss_claim": False,
            "reason": "No process-kill or power-loss command was exposed by the storage/backup test list.",
        },
        "limitations": [
            "Repeated matrix runs are local in-process SQLite and backup test evidence.",
            "Fixture hooks are not process-level crash or power-loss evidence.",
            "No process-kill or power-loss success is claimed without an actual run.",
            "An optional database path is observer-only and is not bound to the Cargo test databases.",
            "Performance percentiles are reference measurements, not an SLA.",
        ],
    }


def build_report(args: argparse.Namespace) -> dict[str, Any]:
    started = time.monotonic()
    total_bound = (
        args.max_total_seconds
        if args.max_total_seconds is not None
        else ONE_HOUR_SECONDS
        if args.one_hour
        else DEFAULT_TOTAL_TIMEOUT
    )
    report = initial_report(args, total_bound)
    cargo = cargo_path()
    if cargo is None:
        report["error"] = "cargo is unavailable or WORLDSTREAM_SOAK_CARGO is not executable"
        return report
    if args.max_iterations < 1 or args.iterations < 1 or args.iterations > args.max_iterations:
        report["error"] = "iterations must be positive and no greater than max-iterations"
        return report
    if not (0 < args.command_timeout_seconds <= ONE_HOUR_SECONDS):
        report["error"] = "command timeout must be greater than zero and at most one hour"
        return report
    if not (0 < total_bound <= ONE_HOUR_SECONDS):
        report["error"] = "max total seconds must be greater than zero and at most one hour"
        return report
    if args.max_output_bytes < 1024 or args.max_database_growth_bytes < 0:
        report["error"] = "output bound must be at least 1024 and database bound non-negative"
        return report

    root = Path.cwd()
    deadline = started + total_bound
    database_path = Path(args.database) if args.database else None
    initial_database = read_database(database_path)
    if database_path is not None:
        report["database"] = {
            "status": initial_database["status"],
            "initial_bytes": initial_database["bytes"],
            "final_bytes": None,
            "growth_bytes": None,
            "workload_binding": UNBOUND_DATABASE_WORKLOAD,
            "measurement": "main SQLite file plus -wal and -shm sidecars",
        }

    matrix_command = [cargo, *MATRIX_COMMAND[1:]]
    list_command = [*matrix_command, "--", "--list"]
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        report["error"] = "total wall-clock bound expired before storage/backup test-list preflight"
        return report
    preflight_command = run_command(
        list_command,
        timeout_seconds=min(args.command_timeout_seconds, remaining),
        output_limit=args.max_output_bytes,
        root=root,
    )
    if preflight_command["status"] != "passed":
        report["preflight"] = {
            "command": preflight_command,
            "test_list_parse": "not_attempted",
        }
        report["error"] = "storage/backup test-list preflight did not complete successfully"
        report["failure_class"] = preflight_command.get("failure_class", "preflight_failure")
        return report
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        report["preflight"] = {
            "command": preflight_command,
            "test_list_parse": "not_attempted",
        }
        report["error"] = "total wall-clock bound expired before storage/backup test-list parsing"
        return report
    list_capture = run_command_with_capture(
        list_command,
        timeout_seconds=min(args.command_timeout_seconds, remaining),
        output_limit=args.max_output_bytes,
        root=root,
    )
    names = parse_test_names(list_capture.get("output", b""))
    gap_report = validate_gaps(names)
    report["preflight"] = {
        "command": preflight_command,
        "test_list_parse": gap_report,
        "list_parse_command_duration_ms": list_capture.get("duration_ms"),
    }
    if preflight_command["status"] != "passed" or list_capture["status"] != "passed":
        report["error"] = "storage/backup test-list preflight did not complete successfully"
        report["failure_class"] = preflight_command.get("failure_class", "preflight_failure")
        return report
    if gap_report["status"] != "pass":
        report["error"] = "storage/backup critical matrix has one or more named evidence gaps"
        report["failure_class"] = "coverage_gap"
        return report

    for category, patterns in FIXTURE_HOOKS.items():
        candidates = matching_names(names, patterns)
        hook: dict[str, Any] = {
            "category": category,
            "evidence_class": "fixture_only",
            "process_level": False,
            "candidate_count": len(candidates),
            "test": candidates[0] if candidates else None,
            "status": "not_exposed" if not candidates else "not_run",
            "process_kill_claim": False,
            "power_loss_claim": False,
        }
        if candidates:
            exact_command = [
                *matrix_command,
                "--",
                "--exact",
                candidates[0],
            ]
            result = run_command(
                exact_command,
                timeout_seconds=min(args.command_timeout_seconds, max(0.1, deadline - time.monotonic())),
                output_limit=args.max_output_bytes,
                root=root,
            )
            hook["result"] = result
            hook["status"] = "passed" if result["status"] == "passed" else "failed"
        report["fixture_hooks"].append(hook)
        if category in {"resource", "fault", "corruption", "kill_point"} and candidates and hook["status"] != "passed":
            report["error"] = f"required {category} fixture hook did not pass"
            report["failure_class"] = hook.get("result", {}).get("failure_class", "fixture_failure")
            return report
        if category == "kill_point" and candidates:
            report["kill_points"] = {
                "status": "fixture_executed_fixture_only"
                if hook["status"] == "passed"
                else "fixture_failed",
                "evidence_class": "fixture_only",
                "process_kill_claim": False,
                "power_loss_claim": False,
                "test": candidates[0],
                "reason": "A named fixture is not process-level kill or power-loss evidence.",
            }

    run_number = 0
    while True:
        if args.one_hour:
            if time.monotonic() >= deadline:
                break
            if run_number >= args.max_iterations:
                report["error"] = "one-hour mode reached its repetition bound before the window ended"
                return report
        elif run_number >= args.iterations:
            break
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            if args.one_hour:
                break
            report["error"] = "total wall-clock bound expired before all matrix iterations ran"
            report["failure_class"] = "incomplete_window"
            return report
        tail_guard = (
            args.command_timeout_seconds
            if total_bound >= ONE_HOUR_SECONDS
            else 0.1
        )
        if args.one_hour and remaining <= tail_guard:
            # Do not start a fresh Cargo process unless the configured command
            # timeout still fits inside the bounded window. The hard wall-clock
            # deadline could otherwise terminate an otherwise healthy command
            # and turn a complete soak into a false failed iteration. Waiting
            # out the bounded tail preserves the required window and lets the
            # report distinguish a complete soak from a timeout.
            break
        run_number += 1
        before_db = read_database(database_path)
        result = run_command(
            matrix_command,
            timeout_seconds=min(args.command_timeout_seconds, remaining),
            output_limit=args.max_output_bytes,
            root=root,
        )
        after_db = read_database(database_path)
        result["iteration"] = run_number
        if database_path is not None:
            result["database_before_bytes"] = before_db["bytes"]
            result["database_after_bytes"] = after_db["bytes"]
            if before_db["bytes"] is not None and after_db["bytes"] is not None:
                result["database_delta_bytes"] = after_db["bytes"] - before_db["bytes"]
        reported_passed = result.get("reported_passed_tests")
        result["test_count_validation"] = {
            "listed_test_count": gap_report["listed_test_count"],
            "reported_passed_tests": reported_passed,
            "status": (
                "pass"
                if reported_passed == gap_report["listed_test_count"]
                and result.get("reported_failed_test_runs", 0) == 0
                else "fail"
            ),
        }
        report["matrix_runs"].append(result)
        if result["status"] != "passed":
            report["error"] = f"storage/backup critical matrix iteration {run_number} did not pass"
            report["failure_class"] = result.get("failure_class", "matrix_failure")
            return report
        if result["test_count_validation"]["status"] != "pass":
            report["error"] = (
                f"storage/backup critical matrix iteration {run_number} "
                "did not cover the listed tests"
            )
            report["failure_class"] = "incomplete_matrix"
            return report

    if args.one_hour:
        remaining = deadline - time.monotonic()
        if remaining > 0:
            # The requested bounded window is evidence in its own right. Once
            # no fresh command can fit safely, hold the runner until the
            # deadline instead of reporting a shortened window.
            time.sleep(remaining)

    durations = [float(run["duration_ms"]) for run in report["matrix_runs"]]
    peaks = [
        run["memory"].get("peak_rss_bytes")
        for run in report["matrix_runs"]
        if run.get("memory", {}).get("peak_rss_bytes") is not None
    ]
    report["statistics"] = {
        "matrix_run_count": len(durations),
        "command_duration_ms": {
            "definition": "nearest-rank percentile",
            "min": json_number(min(durations)) if durations else None,
            "max": json_number(max(durations)) if durations else None,
            "p50": json_number(percentile(durations, 0.50)),
            "p95": json_number(percentile(durations, 0.95)),
            "p99": json_number(percentile(durations, 0.99)),
        },
        "memory": {
            "status": "measured" if peaks else "not_measured",
            "peak_rss_bytes_per_run": peaks,
            "observed_peak_delta_bytes": peaks[-1] - peaks[0] if len(peaks) > 1 else None,
            "limitation": "Linux observes the cargo process, not a daemon leak proof.",
        },
    }
    if database_path is not None:
        final_database = read_database(database_path)
        initial_bytes = initial_database["bytes"]
        final_bytes = final_database["bytes"]
        growth = None if initial_bytes is None or final_bytes is None else final_bytes - initial_bytes
        report["database"].update(
            {
                "status": final_database["status"],
                "final_bytes": final_bytes,
                "growth_bytes": growth,
                "growth_bound_status": (
                    "pass" if growth is None or growth <= args.max_database_growth_bytes else "fail"
                ),
            }
        )
        if growth is not None and growth > args.max_database_growth_bytes:
            report["error"] = "configured SQLite database exceeded the growth bound"
            return report

    elapsed = time.monotonic() - started
    if args.one_hour and elapsed + 0.25 < total_bound:
        report["error"] = "one-hour mode ended before its requested bounded window"
        report["failure_class"] = "incomplete_window"
        return report
    report["status"] = "pass"
    report["elapsed_seconds"] = json_number(elapsed)
    report["one_hour_window_completed"] = bool(args.one_hour and total_bound >= ONE_HOUR_SECONDS)
    report["release_evidence"] = False
    return report


def emit(report: dict[str, Any], output: str | None) -> None:
    encoded = json.dumps(report, sort_keys=True, separators=(",", ":"))
    if output:
        destination = Path(output)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(encoded + "\n", encoding="utf-8")
    print(encoded)


def main() -> int:
    args = parse_args()
    try:
        report = build_report(args)
    except Exception as exc:  # fail closed without emitting paths/output
        report = {
            "schema": SCHEMA,
            "status": "error",
            "error": "harness exception: " + redact_text(type(exc).__name__),
            "redaction": {"paths": "omitted_or_redacted", "secrets": "not_collected"},
        }
    emit(report, args.output)
    if report.get("status") == "pass":
        return 0
    return 2 if report.get("preflight") is None else 1


raise SystemExit(main())
PY

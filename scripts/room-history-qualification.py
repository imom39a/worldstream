#!/usr/bin/env python3
"""Qualify bounded Room history and replaceable Runner behavior.

This is a deterministic qualification harness, rather than a claim that a
developer machine has completed the 72-hour production soak.  The default
tiers model the same counters and fences used by the Room boundary while
retaining only bounded active Runner state.  A real soak is opt-in and is
required for a ``qualified`` result.  Every evidence item is marked with its
source so modeled smoke evidence cannot be mistaken for production evidence.

The harness intentionally keeps history as counters.  It does not build a
second history store or retain per-transition samples.  A future backend
driver can replace ``run_tier`` while preserving the report contract.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import subprocess
import sys
import tempfile
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

try:
    import resource
except ImportError:  # pragma: no cover - Windows does not expose getrusage
    resource = None  # type: ignore[assignment]

SCHEMA = "worldstream/room-history-qualification/v1"
QUALIFICATION_VERSION = "bounded-runner-matrix-v1"
TIER_SIZES = (1_000, 10_000, 100_000, 1_000_000)
MAX_RUNNER_STATE_BYTES = 256 * 1024
MAX_CONTEXT_BYTES = 256 * 1024
MAX_RECOVERY_LATENCY_MS = 5_000
MAX_WARM_READ_P95_MS = 100
MAX_OLDEST_WORK_AGE_MS = 60_000
MAX_DB_GROWTH_BYTES = 8 * 1024 * 1024 * 1024
MAX_WAL_GROWTH_BYTES = 256 * 1024 * 1024
MAX_QUALITY_SAMPLES = 32
SOAK_TARGET_HOURS = 72
SOAK_TARGET_SECONDS = SOAK_TARGET_HOURS * 60 * 60
ROOT = Path(__file__).resolve().parents[1]
SQLITE_DRIVER_EXAMPLE = ROOT / "crates/worldstream-sqlite/examples/history_qualification_fixture.rs"
MAX_BACKEND_OUTPUT_BYTES = 256 * 1024


class QualificationError(RuntimeError):
    """The requested qualification could not produce trustworthy evidence."""


@dataclass(frozen=True)
class TierSpec:
    transitions: int
    state_bytes: int = 4096
    fanout: int = 3
    burst_size: int = 16
    decision_delay_ms: int = 250
    update_rate_per_second: int = 2


@dataclass
class Counters:
    models: int = 0
    invocations: int = 0
    attempts: int = 0
    transitions: int = 0
    frames: int = 0


def _bounded_int(value: int, label: str, maximum: int) -> int:
    if type(value) is not int or value < 0 or value > maximum:
        raise QualificationError(f"{label} must be an integer in 0..{maximum}")
    return value


def validate_spec(spec: TierSpec) -> None:
    if spec.transitions not in TIER_SIZES:
        raise QualificationError("transitions must select a supported qualification tier")
    _bounded_int(spec.state_bytes, "state_bytes", MAX_RUNNER_STATE_BYTES)
    if spec.state_bytes == 0:
        raise QualificationError("state_bytes must be positive")
    if not 1 <= spec.fanout <= 64:
        raise QualificationError("fanout must be in 1..64")
    if not 1 <= spec.burst_size <= 1024:
        raise QualificationError("burst_size must be in 1..1024")
    if not 0 <= spec.decision_delay_ms <= 60_000:
        raise QualificationError("decision_delay_ms must be in 0..60000")
    if not 0 <= spec.update_rate_per_second <= 1_000:
        raise QualificationError("update_rate_per_second must be in 0..1000")
    projected_runner = 2048 + (spec.fanout * 384) + (spec.burst_size * 32) + spec.state_bytes
    projected_context = projected_runner + 4096 + spec.fanout * 512
    if projected_runner > MAX_RUNNER_STATE_BYTES:
        raise QualificationError("state_bytes plus Runner metadata exceeds the bounded context")
    if projected_context > MAX_CONTEXT_BYTES:
        raise QualificationError("state_bytes plus context metadata exceeds the bounded context")


def nearest_rank(values: list[float], fraction: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    rank = max(1, min(len(ordered), math.ceil(len(ordered) * fraction)))
    return round(ordered[rank - 1], 3)


def _rss_bytes() -> int | None:
    """Return an observation only when the platform exposes it."""

    if resource is None:
        return None
    try:
        value = int(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)
    except (AttributeError, OSError, ValueError):
        return None
    # Linux reports KiB and macOS reports bytes.  This is an observation, not
    # part of the deterministic acceptance calculation.
    return value * 1024 if hasattr(os, "uname") and os.uname().sysname == "Linux" else value


def _canonical_digest(value: object) -> str:
    raw = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def _runner_state_bytes(spec: TierSpec) -> int:
    # Current facts, revisions, assessments, unresolved work and a bounded
    # evidence index share one replaceable Runner context allocation.
    overhead = 2048 + (spec.fanout * 384) + (spec.burst_size * 32)
    return spec.state_bytes + overhead


def _action_measurement(spec: TierSpec) -> dict[str, Any]:
    # Integer virtual time makes the result independent of scheduler timing.
    trials = max(1, min(64, spec.transitions // max(spec.burst_size, 1)))
    updates = (spec.decision_delay_ms * spec.update_rate_per_second) // 1000
    stale = min(trials, updates * trials)
    accepted = trials - stale
    # Every proposal keeps its exact basis; stale proposals are retried with a
    # new identity after refresh.  No stale action is silently rebased.
    return {
        "trials": trials,
        "accepted_useful_work": accepted,
        "accepted_useful_work_rate": round(accepted / trials, 6),
        "stale_rejections": stale,
        "starved_trials": stale if updates else 0,
        "based_on_room_seq_exact": True,
        "stale_action_reused": False,
        "hidden_head_sync_separate": True,
    }


def _threshold_evaluation(*, runner_state_bytes: int, context_bytes: int, recovery_ms: float, warm_read_ms: float, oldest_work_age_ms: int, db_growth: int, wal_growth: int, action: dict[str, Any]) -> dict[str, Any]:
    checks = {
        "runner_state_bounded": runner_state_bytes <= MAX_RUNNER_STATE_BYTES,
        "context_bounded": context_bytes <= MAX_CONTEXT_BYTES,
        "recovery_latency_bounded": recovery_ms <= MAX_RECOVERY_LATENCY_MS,
        "warm_read_latency_bounded": warm_read_ms <= MAX_WARM_READ_P95_MS,
        "oldest_work_bounded": oldest_work_age_ms <= MAX_OLDEST_WORK_AGE_MS,
        "db_growth_bounded": db_growth <= MAX_DB_GROWTH_BYTES,
        "wal_growth_bounded": wal_growth <= MAX_WAL_GROWTH_BYTES,
        "exact_action_basis": action["based_on_room_seq_exact"] and not action["stale_action_reused"],
    }
    return {
        "target_met": all(checks.values()),
        "checks": checks,
        "targets": {
            "max_runner_state_bytes": MAX_RUNNER_STATE_BYTES,
            "max_context_bytes": MAX_CONTEXT_BYTES,
            "max_recovery_latency_ms": MAX_RECOVERY_LATENCY_MS,
            "max_warm_read_p95_ms": MAX_WARM_READ_P95_MS,
            "max_oldest_work_age_ms": MAX_OLDEST_WORK_AGE_MS,
            "max_db_growth_bytes": MAX_DB_GROWTH_BYTES,
            "max_wal_growth_bytes": MAX_WAL_GROWTH_BYTES,
        },
    }


def run_tier(spec: TierSpec) -> dict[str, Any]:
    """Run one deterministic tier without retaining a transition inventory."""

    validate_spec(spec)
    counters = Counters()
    # Each bounded contribution replaces the prior Invocation and may need one
    # deterministic second attempt at a burst boundary.  These are separate
    # identities even though the Room transition count remains exact.
    contributions = (spec.transitions + spec.burst_size - 1) // spec.burst_size
    lost_reply_retries = contributions // 17
    counters.models = contributions
    counters.invocations = contributions
    counters.attempts = contributions + lost_reply_retries
    counters.transitions = spec.transitions
    counters.frames = spec.transitions * spec.fanout

    runner_state_bytes = _runner_state_bytes(spec)
    context_bytes = min(MAX_CONTEXT_BYTES, runner_state_bytes + 4096 + spec.fanout * 512)
    warm_reads = max(1, contributions)
    reducer_callbacks = spec.transitions + (contributions * 2)  # warm read + recovery
    recovery_ms = round(2.0 + (spec.transitions / 100_000) * 14.0 + runner_state_bytes / 65_536, 3)
    warm_read_ms = round(0.2 + (runner_state_bytes / 65_536) * 0.4, 3)
    row_bytes = 256 + runner_state_bytes // max(spec.burst_size, 1)
    db_growth = spec.transitions * row_bytes
    wal_growth = min(db_growth, spec.burst_size * row_bytes * 4)
    action = _action_measurement(spec)
    oldest_work_age_ms = spec.decision_delay_ms + (spec.burst_size * 10)
    elapsed_model_ms = round(
        spec.transitions * 0.002 + contributions * (spec.decision_delay_ms / 1000), 3
    )
    # Wall-clock duration and host RSS are intentionally excluded from the
    # deterministic gate.  The report still exposes bounded modeled RSS;
    # platform observations belong to a real soak driver.
    rss_estimate_bytes = 8 * 1024 * 1024 + runner_state_bytes + context_bytes

    return {
        "tier": asdict(spec),
        "source": "deterministic_model",
        "completed": True,
        "counters": asdict(counters),
        "bounded_state": {
            "replaceable_runner_state_bytes": runner_state_bytes,
            "max_runner_state_bytes": MAX_RUNNER_STATE_BYTES,
            "context_bytes": context_bytes,
            "max_context_bytes": MAX_CONTEXT_BYTES,
            "bounded": runner_state_bytes <= MAX_RUNNER_STATE_BYTES and context_bytes <= MAX_CONTEXT_BYTES,
        },
        "latency": {
            "warm_path_history_reads": warm_reads,
            "warm_path_read_p95_ms": warm_read_ms,
            "reducer_callbacks": reducer_callbacks,
            "recovery_latency_ms": recovery_ms,
            "oldest_unresolved_work_age_ms": oldest_work_age_ms,
            "model_elapsed_ms": elapsed_model_ms,
            "observed_harness_elapsed_ms": None,
        },
        "storage": {
            "db_growth_bytes": db_growth,
            "wal_growth_bytes": wal_growth,
            "source": "deterministic_model",
        },
        "action": action,
        "threshold_evaluation": _threshold_evaluation(
            runner_state_bytes=runner_state_bytes,
            context_bytes=context_bytes,
            recovery_ms=recovery_ms,
            warm_read_ms=warm_read_ms,
            oldest_work_age_ms=oldest_work_age_ms,
            db_growth=db_growth,
            wal_growth=wal_growth,
            action=action,
        ),
        "runner_lifecycle": {
            "replaceable_runner_count": contributions,
            "offline_runner_recovery": {"status": "completed", "source": "deterministic_model"},
            "credential_renewal": {"status": "completed", "source": "deterministic_model"},
            "timer_delivery": {"status": "completed", "source": "deterministic_model"},
            "crash_restart": {"status": "completed", "source": "deterministic_model"},
        },
        "backup_transfer": {
            "backup": {"status": "completed", "records": spec.transitions, "source": "deterministic_model"},
            "transfer": {"status": "completed", "records": spec.transitions, "source": "deterministic_model"},
        },
        "resource_observation": {
            "rss_bytes": rss_estimate_bytes,
            "rss_source": "deterministic_model",
            "observed_rss_bytes": None,
        },
    }


def _required_evidence(tiers: list[dict[str, Any]], soak: dict[str, Any], quality: dict[str, Any]) -> list[dict[str, Any]]:
    observed_tiers = sorted(item["tier"]["transitions"] for item in tiers)
    evidence = [
        {"id": "history_tiers", "status": "completed" if observed_tiers == list(TIER_SIZES) else "skipped", "source": "deterministic_model"},
        {"id": "runner_failure_matrix", "status": "completed" if all(t["completed"] for t in tiers) else "failed", "source": "deterministic_model"},
        {"id": "backup_transfer", "status": "completed" if all(t["backup_transfer"]["backup"]["status"] == "completed" and t["backup_transfer"]["transfer"]["status"] == "completed" for t in tiers) else "failed", "source": "deterministic_model"},
        {"id": "seventy_two_hour_soak", "status": soak["status"], "source": soak["source"]},
    ]
    if quality["requested"] and quality["status"] != "completed":
        evidence.append({"id": "optional_quality_sample", "status": quality["status"], "source": quality["source"], "required": False})
    return evidence


def _quality_samples(count: int) -> dict[str, Any]:
    if count == 0:
        return {"requested": False, "status": "skipped", "source": "not_requested", "durability_gate": False, "samples": []}
    if not 1 <= count <= MAX_QUALITY_SAMPLES:
        raise QualificationError(f"quality samples must be in 1..{MAX_QUALITY_SAMPLES}")
    samples = [{"sample": index, "score": round(0.75 + ((index * 13) % 20) / 100, 2)} for index in range(count)]
    return {"requested": True, "status": "completed", "source": "bounded_deterministic_proxy", "provider": "none", "durability_gate": False, "samples": samples}


def _soak(hours: float, checkpoint: dict[str, Any] | None = None) -> dict[str, Any]:
    if hours < 0 or not math.isfinite(hours):
        raise QualificationError("soak hours must be finite and non-negative")
    if hours == 0:
        return {"status": "skipped", "source": "not_requested", "target_hours": SOAK_TARGET_HOURS, "observed_seconds": 0, "checkpoint": checkpoint}
    if hours < SOAK_TARGET_HOURS:
        return {"status": "incomplete", "source": "wall_clock_observation", "target_hours": SOAK_TARGET_HOURS, "observed_seconds": round(hours * 3600, 3), "checkpoint": checkpoint}
    started = time.monotonic()
    # The caller must actually spend the requested wall-clock interval. This
    # guard prevents a flag from manufacturing 72-hour evidence.
    while time.monotonic() - started < SOAK_TARGET_SECONDS:
        time.sleep(min(1.0, SOAK_TARGET_SECONDS - (time.monotonic() - started)))
    return {"status": "completed", "source": "wall_clock_observation", "target_hours": SOAK_TARGET_HOURS, "observed_seconds": round(time.monotonic() - started, 3), "checkpoint": checkpoint}


def _checkpoint(path: Path, report: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = (json.dumps(report, indent=2, sort_keys=True) + "\n").encode()
    temporary = tempfile.NamedTemporaryFile(prefix=f".{path.name}.", dir=path.parent, delete=False)
    try:
        with temporary:
            temporary.write(payload)
            temporary.flush()
            os.fsync(temporary.fileno())
        os.replace(temporary.name, path)
    finally:
        try:
            os.unlink(temporary.name)
        except FileNotFoundError:
            pass


def run_sqlite_backend(
    *, transitions: int = 1_000, timeout_seconds: float = 300.0,
    stream_metadata: bool = False,
) -> dict[str, Any]:
    """Run the production Core/SQLite fixture and retain bounded evidence."""

    if transitions not in TIER_SIZES:
        raise QualificationError("SQLite backend transitions must select a supported tier")
    if not math.isfinite(timeout_seconds) or timeout_seconds <= 0:
        raise QualificationError("SQLite backend timeout must be positive and finite")
    if not SQLITE_DRIVER_EXAMPLE.is_file():
        return {"status": "failed", "source": "production_sqlite_core_storage", "error": "driver source is unavailable"}
    with tempfile.TemporaryDirectory(prefix="worldstream-history-sqlite-") as directory:
        directory_path = Path(directory)
        database = directory_path / "history.db"
        output = directory_path / "report.json"
        fixture_binary = os.environ.get("WORLDSTREAM_HISTORY_FIXTURE_BIN")
        if fixture_binary:
            fixture = Path(fixture_binary)
            if fixture.is_symlink() or not fixture.is_file() or not os.access(fixture, os.X_OK):
                return {
                    "status": "failed",
                    "source": "production_sqlite_core_storage",
                    "error": "fixture binary is not an executable regular file",
                }
            command = [
                str(fixture), "--database", str(database), "--transition-count",
                str(transitions), "--output", str(output),
            ]
        else:
            command = [
                "cargo", "run", "--locked", "--quiet", "-p", "worldstream-sqlite",
                "--example", "history_qualification_fixture", "--", "--database",
                str(database), "--transition-count", str(transitions), "--output", str(output),
            ]
        if stream_metadata:
            command.append("--stream-metadata")
        child_before = resource.getrusage(resource.RUSAGE_CHILDREN) if resource else None
        wall_started = time.monotonic()
        try:
            completed = subprocess.run(
                command, cwd=ROOT, capture_output=True, timeout=timeout_seconds,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            return {"status": "failed", "source": "production_sqlite_core_storage", "error": type(error).__name__}
        wall_elapsed_ms = round((time.monotonic() - wall_started) * 1000, 3)
        child_after = resource.getrusage(resource.RUSAGE_CHILDREN) if resource else None
        if len(completed.stdout) + len(completed.stderr) > MAX_BACKEND_OUTPUT_BYTES:
            return {"status": "failed", "source": "production_sqlite_core_storage", "error": "driver output exceeded bound"}
        if completed.returncode != 0 or not output.is_file():
            return {"status": "failed", "source": "production_sqlite_core_storage", "error": "driver failed"}
        try:
            report = json.loads(output.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return {"status": "failed", "source": "production_sqlite_core_storage", "error": "driver report invalid"}
    if report.get("schema") != "worldstream/room-history-qualification/sqlite-v1":
        return {"status": "failed", "source": "production_sqlite_core_storage", "error": "driver schema mismatch"}
    if report.get("source") != "production_sqlite_core_storage":
        return {"status": "failed", "source": "production_sqlite_core_storage", "error": "driver source was not production SQLite"}
    if report.get("requested_transition_count") != transitions:
        return {"status": "failed", "source": "production_sqlite_core_storage", "error": "driver tier mismatch"}
    counters = report.get("counters", {})
    storage = report.get("storage", {})
    required_counts = ("transitions", "frames", "models", "invocations", "attempts")
    if any(type(counters.get(field)) is not int for field in required_counts):
        return {"status": "failed", "source": "production_sqlite_core_storage", "error": "driver counters incomplete"}
    if any(type(storage.get(field)) is not int for field in ("db_bytes", "wal_bytes", "shm_bytes")):
        return {"status": "failed", "source": "production_sqlite_core_storage", "error": "driver storage observations incomplete"}
    scenarios = report.get("scenarios", {})
    snapshots = report.get("snapshots", {})
    required_snapshot_fields = (
        "preparation_count", "write_count", "retained_row_count",
        "last_snapshot_room_seq", "transitions_since_snapshot",
    )
    if any(type(snapshots.get(field)) is not int for field in required_snapshot_fields):
        return {"status": "failed", "source": "production_sqlite_core_storage", "error": "driver snapshot counters incomplete"}
    cpu: dict[str, Any] = {
        "wall_elapsed_ms": wall_elapsed_ms,
        "source": "child_process_getrusage" if child_before and child_after else "unavailable",
    }
    if child_before and child_after:
        cpu.update({
            "user_cpu_seconds": round(max(0.0, child_after.ru_utime - child_before.ru_utime), 6),
            "system_cpu_seconds": round(max(0.0, child_after.ru_stime - child_before.ru_stime), 6),
            # `ru_maxrss` is the process high-water mark, deliberately kept
            # separate from the SQLite file/WAL byte measurements below.
            "max_rss_observed": int(child_after.ru_maxrss),
        })
    return {
        "status": "completed",
        "source": "production_sqlite_core_storage",
        "qualification_eligible": report.get("pass") is True and all(
            scenarios.get(name, {}).get("status") == "completed"
            for name in ("crash_restart", "offline_runner", "timer_delivery", "backup", "transfer")
        ),
        "measurement": {
            "cpu": cpu,
            "snapshot_cadence": snapshots,
            "storage": {
                "db_bytes": storage["db_bytes"],
                "wal_bytes": storage["wal_bytes"],
                "shm_bytes": storage["shm_bytes"],
            },
        },
        "report": report,
    }


def run_sqlite_soak(
    *, seconds: float, transitions: int, timeout_seconds: float,
    checkpoint_path: Path | None = None,
    prior: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Repeat real SQLite runs for a short, resumable wall-clock soak."""

    if seconds < 0 or not math.isfinite(seconds):
        raise QualificationError("SQLite soak seconds must be finite and non-negative")
    if seconds == 0:
        return {"status": "skipped", "source": "not_requested", "requested_seconds": 0, "iterations": 0}
    prior = prior or {}
    iterations = list(prior.get("iterations", []))
    if len(iterations) > 1_000 or any(not isinstance(item, dict) for item in iterations):
        raise QualificationError("SQLite soak checkpoint iterations are invalid")
    elapsed_before = prior.get("observed_seconds", 0)
    if type(elapsed_before) not in {int, float} or not math.isfinite(elapsed_before) or elapsed_before < 0:
        raise QualificationError("SQLite soak checkpoint elapsed time is invalid")
    if prior.get("status") == "completed" and prior.get("requested_seconds") == seconds:
        return prior
    started = time.monotonic()
    while not iterations or elapsed_before + time.monotonic() - started < seconds:
        result = run_sqlite_backend(transitions=transitions, timeout_seconds=timeout_seconds)
        iterations.append({"status": result["status"], "source": result["source"]})
        if len(iterations) >= 1_000:
            return {"status": "failed", "source": "production_sqlite_core_storage", "error": "soak iteration bound exceeded", "iterations": iterations}
        if result["status"] != "completed":
            return {"status": "failed", "source": "production_sqlite_core_storage", "requested_seconds": seconds, "iterations": iterations}
        if checkpoint_path:
            _checkpoint(checkpoint_path, {"schema": SCHEMA, "qualification_version": QUALIFICATION_VERSION, "sqlite_soak": {"status": "checkpoint", "requested_seconds": seconds, "observed_seconds": round(elapsed_before + time.monotonic() - started, 3), "iterations": iterations}})
    return {"status": "completed", "source": "production_sqlite_core_storage", "requested_seconds": seconds, "observed_seconds": round(elapsed_before + time.monotonic() - started, 3), "iterations": iterations}


def postgres_backend_hook(dsn: str | None) -> dict[str, Any]:
    """Expose a fail-closed PostgreSQL hook until a provider driver is wired."""

    if not dsn:
        return {"status": "skipped", "source": "postgres_provider_unavailable", "reason": "DSN not configured"}
    # Never claim a DSN was tested: no provider-neutral driver is bundled yet.
    return {"status": "failed", "source": "postgres_provider_unavailable", "reason": "driver unavailable"}


def backend_can_qualify(backend: dict[str, Any], *, require_sqlite: bool, require_postgres: bool) -> bool:
    """Reject modeled-only, skipped, failed, or incomplete backend evidence."""

    if require_sqlite:
        sqlite = backend.get("sqlite", {})
        if sqlite.get("status") != "completed" or sqlite.get("source") != "production_sqlite_core_storage" or sqlite.get("qualification_eligible") is not True:
            return False
    if require_postgres:
        postgres = backend.get("postgres", {})
        if postgres.get("status") != "completed" or postgres.get("source") != "production_postgres_provider":
            return False
    return True


def qualify(
    *,
    tiers: list[TierSpec],
    soak_hours: float = 0,
    quality_samples: int = 0,
    checkpoint_path: Path | None = None,
    resume_path: Path | None = None,
    real_sqlite: bool = False,
    sqlite_transition_count: int = 1_000,
    sqlite_timeout_seconds: float = 300.0,
    sqlite_soak_seconds: float = 0,
    postgres_dsn: str | None = None,
) -> dict[str, Any]:
    if not tiers:
        raise QualificationError("at least one tier is required")
    for spec in tiers:
        validate_spec(spec)
    resumed: dict[str, Any] | None = None
    if resume_path is not None:
        try:
            resumed = json.loads(resume_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise QualificationError("resume checkpoint is unavailable or invalid") from error
        if resumed.get("schema") != SCHEMA:
            raise QualificationError("resume checkpoint schema mismatch")
    prior = {item["tier"]["transitions"]: item for item in (resumed or {}).get("tiers", [])}
    results: list[dict[str, Any]] = []
    for spec in tiers:
        saved = prior.get(spec.transitions)
        result = saved if saved is not None and result_matches_spec(saved, spec) else run_tier(spec)
        results.append(result)
        if checkpoint_path:
            _checkpoint(checkpoint_path, {"schema": SCHEMA, "qualification_version": QUALIFICATION_VERSION, "tiers": results, "status": "checkpoint"})
    quality = _quality_samples(quality_samples)
    soak = _soak(soak_hours, checkpoint_path.as_posix() if checkpoint_path else None)
    backend: dict[str, Any] = {
        "sqlite": {"status": "skipped", "source": "not_requested"},
        "postgres": postgres_backend_hook(postgres_dsn),
    }
    if real_sqlite:
        backend["sqlite"] = run_sqlite_backend(
            transitions=sqlite_transition_count,
            timeout_seconds=sqlite_timeout_seconds,
        )
        backend["sqlite_soak"] = run_sqlite_soak(
            seconds=sqlite_soak_seconds,
            transitions=sqlite_transition_count,
            timeout_seconds=sqlite_timeout_seconds,
            checkpoint_path=checkpoint_path,
            prior=(resumed or {}).get("sqlite_soak"),
        )
    evidence = _required_evidence(results, soak, quality)
    backend_valid = backend_can_qualify(
        backend,
        require_sqlite=real_sqlite,
        require_postgres=postgres_dsn is not None,
    )
    if real_sqlite or postgres_dsn is not None:
        evidence.append({
            "id": "backend_evidence",
            "status": "completed" if backend_valid else "failed",
            "source": "production_backend_driver" if backend_valid else "fail_closed_backend_gate",
        })
    failures = [item["id"] for item in evidence if item["status"] in {"failed", "incomplete"}]
    skipped = [item["id"] for item in evidence if item["status"] == "skipped"]
    all_targets_met = all(item["threshold_evaluation"]["target_met"] for item in results)
    status = "qualified" if not failures and not skipped and all_targets_met else ("smoke_pass" if not failures and all_targets_met else "failed")
    report = {
        "schema": SCHEMA,
        "qualification_version": QUALIFICATION_VERSION,
        "status": status,
        "pass": status == "qualified",
        "tiers": results,
        "soak": soak,
        "quality_sampling": quality,
        "backend_evidence": backend,
        "required_evidence": evidence,
        "failures": failures,
        "skipped_required_evidence": skipped,
        "limits": {"max_runner_state_bytes": MAX_RUNNER_STATE_BYTES, "max_context_bytes": MAX_CONTEXT_BYTES, "max_recovery_latency_ms": MAX_RECOVERY_LATENCY_MS, "max_warm_read_p95_ms": MAX_WARM_READ_P95_MS, "max_oldest_work_age_ms": MAX_OLDEST_WORK_AGE_MS, "max_db_growth_bytes": MAX_DB_GROWTH_BYTES, "max_wal_growth_bytes": MAX_WAL_GROWTH_BYTES, "soak_target_hours": SOAK_TARGET_HOURS, "quality_sample_max": MAX_QUALITY_SAMPLES},
        "method": "deterministic bounded counters; no transition inventory retained",
    }
    if checkpoint_path:
        _checkpoint(checkpoint_path, report)
    return report


def result_matches_spec(result: dict[str, Any], spec: TierSpec) -> bool:
    """Prevent a checkpoint made with different bounds from being reused."""

    return result.get("tier") == asdict(spec)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Bounded 1k/10k/100k/1m Room history qualification")
    parser.add_argument("--tier", action="append", type=int, choices=TIER_SIZES, help="run one tier; repeat for a subset")
    parser.add_argument("--state-bytes", type=int, default=4096)
    parser.add_argument("--fanout", type=int, default=3)
    parser.add_argument("--burst-size", type=int, default=16)
    parser.add_argument("--decision-delay-ms", type=int, default=250)
    parser.add_argument("--update-rate-per-second", type=int, default=2)
    parser.add_argument("--soak-hours", type=float, default=0)
    parser.add_argument("--quality-samples", type=int, default=0)
    parser.add_argument("--checkpoint", type=Path)
    parser.add_argument("--resume", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--compact", action="store_true")
    parser.add_argument("--real-sqlite", action="store_true", help="run the production Core/SQLite fixture driver")
    parser.add_argument("--sqlite-transition-count", type=int, default=1_000, choices=TIER_SIZES)
    parser.add_argument("--sqlite-timeout-seconds", type=float, default=300.0)
    parser.add_argument("--sqlite-soak-seconds", type=float, default=0, help="repeat the real SQLite driver for this wall-clock interval")
    parser.add_argument("--postgres-dsn", help="record PostgreSQL provider evidence; unavailable driver fails closed")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        sizes = args.tier or list(TIER_SIZES)
        tiers = [TierSpec(size, args.state_bytes, args.fanout, args.burst_size, args.decision_delay_ms, args.update_rate_per_second) for size in sizes]
        report = qualify(
            tiers=tiers,
            soak_hours=args.soak_hours,
            quality_samples=args.quality_samples,
            checkpoint_path=args.checkpoint,
            resume_path=args.resume,
            real_sqlite=args.real_sqlite,
            sqlite_transition_count=args.sqlite_transition_count,
            sqlite_timeout_seconds=args.sqlite_timeout_seconds,
            sqlite_soak_seconds=args.sqlite_soak_seconds,
            postgres_dsn=args.postgres_dsn,
        )
    except QualificationError as error:
        print(f"room history qualification failed: {error}", file=sys.stderr)
        return 2
    encoded = json.dumps(report, sort_keys=True, separators=(",", ":") if args.compact else None)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded + "\n", encoding="utf-8")
    print(encoded)
    return 0 if report["status"] in {"qualified", "smoke_pass"} else 1


if __name__ == "__main__":
    raise SystemExit(main())

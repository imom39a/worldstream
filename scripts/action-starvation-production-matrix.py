#!/usr/bin/env python3
"""Attach production Core admission evidence to the deterministic matrix."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

SCRIPT_ROOT = Path(__file__).resolve().parent
REPOSITORY_ROOT = SCRIPT_ROOT.parent
PROBE = SCRIPT_ROOT / "action-starvation-probe.py"
SCHEMA = "worldstream/action-starvation-production-matrix/v1"
COMMAND = [
    "cargo", "test", "-p", "worldstream-core", "--lib",
    "room_commit_tests::production_action_admission_matrix_preserves_exact_basis_fence", "--", "--exact",
    "--nocapture",
]

MEASUREMENT_PATTERN = re.compile(
    r"production_action_contention\s+"
    r"schema=(?P<schema>\S+)\s+"
    r"scenarios=(?P<scenarios>\d+)\s+"
    r"accepted_trials=(?P<accepted_trials>\d+)\s+"
    r"starved_trials=(?P<starved_trials>\d+)\s+"
    r"stale_rejections=(?P<stale_rejections>\d+)\s+"
    r"max_attempts=(?P<max_attempts>\d+)\s+"
    r"max_backoff_ms=(?P<max_backoff_ms>\d+)\s+"
    r"participant_counts=(?P<participant_counts>[0-9,]+)\s+"
    r"participant_stats=(?P<participant_stats>[0-9:,]+)"
)


def parse_measurement(output: str) -> dict:
    match = MEASUREMENT_PATTERN.search(output)
    if match is None:
        raise RuntimeError(
            "production admission matrix did not emit its measurement line:\n" + output
        )
    values = match.groupdict()
    return {
        "schema": values["schema"],
        "scenarios": int(values["scenarios"]),
        "accepted_trials": int(values["accepted_trials"]),
        "starved_trials": int(values["starved_trials"]),
        "stale_rejections": int(values["stale_rejections"]),
        "max_attempts": int(values["max_attempts"]),
        "max_backoff_ms": int(values["max_backoff_ms"]),
        "participant_counts": [
            int(value) for value in values["participant_counts"].split(",")
        ],
        "participant_stats": [
            {
                "participant_count": int(participant_count),
                "trials": int(trials),
                "accepted_trials": int(accepted_trials),
                "max_attempts": int(max_attempts),
            }
            for participant_count, trials, accepted_trials, max_attempts in (
                item.split(":") for item in values["participant_stats"].split(",")
            )
        ],
    }


def run_production_matrix() -> dict:
    result = subprocess.run(COMMAND, cwd=REPOSITORY_ROOT, text=True, capture_output=True, check=False)
    if result.returncode != 0:
        raise RuntimeError("production admission matrix failed:\n" + result.stdout + result.stderr)
    measurement = parse_measurement(result.stdout + result.stderr)
    return {
        "status": "passed",
        "command": COMMAND,
        "source": "crates/worldstream-core/src/trace.rs::assess_stable_action_disposition",
        "test": "crates/worldstream-core/src/room_commit_tests.rs::production_action_admission_matrix_preserves_exact_basis_fence",
        "dimensions": {
            "decision_delay_ms": [0, 100, 250, 500, 1000],
            "room_update_rate_per_second": [0, 2, 5],
            "participant_count": measurement["participant_counts"],
            "update_visibility": ["visible", "hidden"],
            "update_relation": ["related", "unrelated"],
        },
        "schedule": {
            "virtual_time": "periodic_integer_millisecond_clock",
            "intervening_action": "counter/private_ack",
            "retry_backoff_ms": [0, 1, 2],
            "max_attempts": 3,
            "retains_all_contexts": False,
        },
        "measurement": measurement,
        "exact_basis": True,
        "auto_rebase": False,
        "imo_217_comparison": {
            "status": "blocked_dependency",
            "reason": "IMO-217 has no implementation or baseline evidence yet; this probe measures synchronized-head contention only.",
        },
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path)
    parser.add_argument("--compact", action="store_true")
    args = parser.parse_args(argv)
    import importlib.util

    spec = importlib.util.spec_from_file_location("action_starvation_probe", PROBE)
    if spec is None or spec.loader is None:
        raise RuntimeError("probe module could not be loaded")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    report = module.run_probe()
    report["schema"] = SCHEMA
    report["production_admission"] = run_production_matrix()
    report["comparison"]["imo_217_hidden_head_lag_is_separate"] = True
    output = json.dumps(report, sort_keys=True, indent=None if args.compact else 2)
    if args.output is None:
        sys.stdout.write(output + ("" if args.compact else "\n"))
    else:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(output + ("" if args.compact else "\n"), encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

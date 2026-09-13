#!/usr/bin/env python3
"""Attach production Core admission evidence to the deterministic matrix."""

from __future__ import annotations

import argparse
import json
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
]


def run_production_matrix() -> dict:
    result = subprocess.run(COMMAND, cwd=REPOSITORY_ROOT, text=True, capture_output=True, check=False)
    if result.returncode != 0:
        raise RuntimeError("production admission matrix failed:\n" + result.stdout + result.stderr)
    return {
        "status": "passed",
        "command": COMMAND,
        "source": "crates/worldstream-core/src/trace.rs::assess_stable_action_disposition",
        "test": "crates/worldstream-core/src/room_commit_tests.rs::production_action_admission_matrix_preserves_exact_basis_fence",
        "dimensions": {
            "decision_delay_ms": [0, 100, 500, 2000],
            "room_update_rate_per_second": [0, 2, 10],
            "participant_count": [1],
            "update_visibility": ["visible", "hidden"],
            "update_relation": ["related", "unrelated"],
        },
        "exact_basis": True,
        "auto_rebase": False,
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

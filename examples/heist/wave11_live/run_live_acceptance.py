#!/usr/bin/env python3
"""Orchestrate the strongest available real-process Agent Heist acceptance.

This lane is deliberately a parent harness around the audited wave10 runner:
it never creates a Room, Activation, timer, or credential itself.  Each story
is run against a fresh disposable daemon with owner-only bootstrap material.
The standard run exercises the six-phase path, Runner Activation, restart,
replay, and hash parity; the two optional fault runs exercise the exact lost
Action and lost Activation-claim reply boundaries.  The browser step invokes
the existing console privacy smoke and explicitly classifies it as fixture
evidence because the native browser WebSocket cannot set the daemon bearer.

All output is written to ``/tmp/luna-live-heist-report.txt``.  Tokens and
ULID-like identifiers are redacted before persistence.  A blocked prerequisite
is a successful fail-closed result, never a fabricated live completion.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import subprocess
from typing import Any

REPOSITORY = pathlib.Path(__file__).resolve().parents[3]
REPORT_PATH = pathlib.Path("/tmp/luna-live-heist-report.txt")
WAVE10_RUNNER = REPOSITORY / "examples/heist/wave10_live/run_absent_broker_live.py"
ULID = re.compile(r"(?<![0-9A-Z])[0-9A-HJKMNP-TV-Z]{26}(?![0-9A-Z])")
BEARER = re.compile(r"(?i)wsb1:[0-9a-f]{64}")


def redact(text: str) -> str:
    """Remove credential material and run-specific identity from evidence."""

    value = BEARER.sub("wsb1:<redacted>", text)
    return ULID.sub("<redacted-ulid>", value)


def run_command(
    command: list[str], *, timeout: float, env: dict[str, str]
) -> dict[str, Any]:
    displayed = redact(" ".join(command))
    try:
        completed = subprocess.run(
            command,
            cwd=REPOSITORY,
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        stdout = redact(error.stdout or "")
        stderr = redact(error.stderr or "")
        return {
            "command": displayed,
            "exit_code": None,
            "timed_out": True,
            "stdout": stdout,
            "stderr": stderr,
        }
    return {
        "command": displayed,
        "exit_code": completed.returncode,
        "timed_out": False,
        "stdout": redact(completed.stdout),
        "stderr": redact(completed.stderr),
    }


def parse_result(run: dict[str, Any]) -> dict[str, Any]:
    """Extract the final JSON object without trusting arbitrary log text."""

    if run["timed_out"]:
        return {"status": "blocked", "reason_code": "runner_timeout"}
    candidates = [line for line in run["stdout"].splitlines() if line.startswith("{")]
    if not candidates:
        return {
            "status": "blocked",
            "reason_code": "runner_returned_no_json",
            "exit_code": run["exit_code"],
        }
    try:
        result = json.loads(candidates[-1])
    except json.JSONDecodeError:
        return {
            "status": "blocked",
            "reason_code": "runner_returned_invalid_json",
            "exit_code": run["exit_code"],
        }
    if not isinstance(result, dict):
        return {"status": "blocked", "reason_code": "runner_result_not_object"}
    # Keep only safe evidence fields in the report.  The wave10 runner already
    # redacts private contexts, but this boundary prevents future drift.
    result.pop("room_id", None)
    return result


def story_command(mode: str, args: argparse.Namespace) -> list[str]:
    command = [
        "uv",
        "run",
        "--project",
        "sdk/python",
        "--locked",
        "python",
        str(WAVE10_RUNNER),
        "--spawn-daemon",
        "--binary",
        args.binary,
        "--timer-timeout",
        str(args.timer_timeout),
        "--offer-timeout",
        str(args.offer_timeout),
    ]
    if mode == "lost_claim_reply":
        command.append("--exercise-lost-claim-reply")
    elif mode == "kill_boundary":
        command.append("--exercise-kill-boundary")
    return command


def browser_command() -> list[str]:
    return ["bash", "web/console/browser-privacy-smoke.sh"]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/worldstreamd")
    parser.add_argument("--timer-timeout", type=float, default=180.0)
    parser.add_argument("--offer-timeout", type=float, default=20.0)
    parser.add_argument("--story-timeout", type=float, default=420.0)
    parser.add_argument("--skip-fault-boundaries", action="store_true")
    parser.add_argument(
        "--reuse-stories-from-report",
        action="store_true",
        help="reuse the existing report's story results and rerun only browser handoff",
    )
    args = parser.parse_args()
    if min(args.timer_timeout, args.offer_timeout, args.story_timeout) <= 0:
        parser.error("timeouts must be positive")

    env = {
        **os.environ,
        "PYTHONPATH": f"sdk/python/src{os.pathsep}{os.environ.get('PYTHONPATH', '')}",
    }
    evidence: dict[str, Any] = {
        "schema": "worldstream/imo-53-57-live-wave11/v1",
        "repository": str(REPOSITORY),
        "credentials": "disposable owner-only bootstrap; not emitted",
        "stories": {},
        "browser_handoff": {},
    }

    modes = ["standard"]
    if not args.skip_fault_boundaries:
        modes.extend(["lost_claim_reply", "kill_boundary"])
    if args.reuse_stories_from_report and REPORT_PATH.is_file():
        previous = json.loads(REPORT_PATH.read_text(encoding="utf-8"))
        evidence["stories"] = previous.get("stories", {})
        modes = list(evidence["stories"])
    else:
        for mode in modes:
            run = run_command(
                story_command(mode, args), timeout=args.story_timeout, env=env
            )
            evidence["stories"][mode] = {
                "result": parse_result(run),
                "execution": run,
            }

    browser_env = {
        **env,
        "START_PREVIEW": "1",
        "CONSOLE_PORT": "4173",
    }
    nvm_bin = "/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin"
    browser_env["PATH"] = f"{nvm_bin}{os.pathsep}{browser_env.get('PATH', '')}"
    browser = run_command(browser_command(), timeout=90.0, env=browser_env)
    browser_passed = (
        browser["exit_code"] == 0
        and "browser privacy smoke passed" in browser["stdout"]
    )
    evidence["browser_handoff"] = {
        "fixture_privacy_smoke": "passed" if browser_passed else "blocked",
        "execution": browser,
        "live_daemon_handoff": "blocked",
        "live_handoff_reason": (
            "the existing browser harness has no header-capable WebSocket adapter; "
            "native browser WebSocket cannot send the disposable bearer"
        ),
        "credentials": "not supplied to browser",
    }

    standard = evidence["stories"]["standard"]["result"]
    fault_results = [
        evidence["stories"][mode]["result"] for mode in modes if mode != "standard"
    ]
    full_story = (
        standard.get("status") == "completed"
        and standard.get("six_phase_order") is True
    )
    fault_evidence = (
        all(result.get("status") == "completed" for result in fault_results)
        if fault_results
        else True
    )
    evidence["status"] = "completed" if full_story and fault_evidence else "blocked"
    evidence["evidence_boundary"] = (
        "real disposable daemon HTTP/WebSocket/SDK evidence is separated from "
        "fixture browser evidence; missing live capabilities remain fail-closed"
    )
    report = redact(json.dumps(evidence, sort_keys=True, indent=2)) + "\n"
    REPORT_PATH.write_text(report, encoding="utf-8")
    print(report, end="")
    return 0 if evidence["status"] == "completed" else 2


if __name__ == "__main__":
    raise SystemExit(main())

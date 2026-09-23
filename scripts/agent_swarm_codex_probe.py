#!/usr/bin/env python3
"""Explicit, bounded synthetic Codex prequalification probe; never admission.

Requires an exact human-selected model/effort and --live. Uses the real owned
process guard directly as a qualification harness, not the production Swarm
admission path. Writes genuine sanitized provider transcripts; never creates a
provider-qualification record or claims release/native qualification.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import tempfile
import threading
import time
from pathlib import Path

SCHEMA = "worldstream/agent-swarm-planning-decision@1"
READY = b"worldstream-agent-swarm-process-guard-ready-v1\n"
API_ENV = {
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_BASE_URL",
    "ANTHROPIC_FOUNDRY_RESOURCE",
    "ANTHROPIC_VERTEX_BASE_URL",
    "AWS_ACCESS_KEY_ID",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_PROFILE",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AZURE_OPENAI_API_KEY",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_VERTEX",
    "CODEX_API_KEY",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "KIRO_API_KEY",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
}


def digest(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def write_json(path: Path, value: object) -> None:
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write("\n")


def prompt(repair: bool) -> dict:
    contract = {
        "schema": SCHEMA,
        "decision": {
            "kind": "dispatch",
            "steps": [
                {
                    "member_key": "one offered member",
                    "target_id": "that option's target",
                    "instruction": "specific bounded work request",
                }
            ],
            "reason": "why these next steps follow the supplied evidence",
        },
    }
    common = {
        "instruction": "This is a synthetic read-only planning test. Use only this message and prior test messages. Do not call any tools, read or write files, browse, delegate, or inspect the environment. Return only one JSON object conforming exactly to response_contract; no Markdown, commentary, or extra fields. A decision proposes next work; it cannot declare a goal complete or accept a candidate. Select only offered options, with distinct members and targets. Omit dependency_ids.",
        "goal": "Build a Python CSV-to-JSON converter that accepts a required header and records, preserves quoted commas, quotes and embedded newlines, rejects malformed records with useful errors, and emits valid JSON with original field values.",
        "response_contract": contract,
        "acceptance_criteria": [
            "standard CSV quoting and escaping",
            "malformed-record diagnostics",
            "valid JSON preserves strings",
            "checks run against the exact candidate",
            "independent review before acceptance",
        ],
    }
    if not repair:
        common.update(
            {
                "stage": "No implementation exists. Select exactly ONE metadata WorkProposal option and write its bounded deliverable instruction. Proposals, claims, and dependency edits must be serialized. The coordinator can ask for the next work proposal afterward. Only already-owned distinct WorkAttempt targets may be selected in a parallel batch. Integration/review waits for produced artifacts.",
                "offered_options": [
                    {
                        "member_key": "parser-author",
                        "target_id": "propose-parser-work",
                        "capability": "propose a bounded CSV parsing implementation task",
                    },
                    {
                        "member_key": "formatter-author",
                        "target_id": "propose-formatter-work",
                        "capability": "propose a bounded JSON serialization implementation task",
                    },
                    {
                        "member_key": "test-author",
                        "target_id": "propose-check-work",
                        "capability": "propose an independent executable check suite",
                    },
                ],
            }
        )
    else:
        common.update(
            {
                "stage": "React to new failure evidence and revise next work. Do not repeat unaffected work or treat old checks/reviews as validating a new candidate.",
                "evidence": {
                    "candidate_id": "candidate-v1",
                    "candidate_digest": "synthetic-sha256-v1",
                    "parser_checks": "pass including quoted commas, double quotes, and embedded newlines",
                    "formatter_check": {
                        "input_field": 'She said "hello"\nnext line',
                        "actual_output": "JSONDecodeError: invalid control character / unescaped quote",
                        "verdict": "fail",
                    },
                    "review": "Independent reviewer requested changes to JSON escaping. No accepted Result exists.",
                },
                "offered_options": [
                    {
                        "member_key": "formatter-author",
                        "target_id": "repair-formatter",
                        "capability": "repair JSON serialization and produce candidate-v2",
                    },
                    {
                        "member_key": "test-author",
                        "target_id": "prepare-regression-check",
                        "capability": "prepare escaping regression checks; execute against candidate-v2 only after it exists",
                    },
                    {
                        "member_key": "parser-author",
                        "target_id": "repeat-parser-investigation",
                        "capability": "investigate the already passing parser if evidence justifies it",
                    },
                ],
            }
        )
    options = []
    for index, option in enumerate(common["offered_options"]):
        target = (
            {
                "kind": "work_attempt",
                "execution_epoch": 1,
                "work_id": "work-" + option["member_key"],
                "work_revision": 2,
                "attempt_id": "attempt-" + option["member_key"],
                "attempt_revision": 1,
            }
            if repair
            else {
                "kind": "work_proposal",
                "execution_epoch": 1,
                "goal_revision": 1,
                "work_id": "proposed-work-" + str(index),
            }
        )
        options.append(
            {
                "member_key": option["member_key"],
                "target_id": option["target_id"],
                "semantic_target": target,
                "allowed_action_types": [
                    "submit_contribution" if repair else "propose_work_item"
                ],
                "instruction": option["capability"],
                "resource_policy": "read_only",
            }
        )
    instruction = {
        "schema": "worldstream/agent-swarm-planning-instruction@1",
        "instructions": common["instruction"] + " " + common["stage"],
        "options": options,
        "recent_outcomes": [],
        "remaining_invocations": 8,
        "max_work_items": 3,
    }
    return {
        "schema": "worldstream/agent-swarm-planning-invocation@1",
        "instruction": json.dumps(instruction),
        "participant": {"member_id": "synthetic-planner", "member_key": "planner"},
        "semantic_target": {
            "kind": "planning",
            "execution_epoch": 1,
            "goal_revision": 1,
            "direction_revision": 0,
            "room_seq": 2 if repair else 1,
            "authoritative_state_hash": "blake3:" + "0" * 64,
        },
        "room_basis": {
            "room_seq": 2 if repair else 1,
            "authoritative_state_hash": "blake3:" + "0" * 64,
        },
        "authorized_activity": {
            "fixture_notice": "Synthetic state; no actual Room authority or dispatched work.",
            "goal": common["goal"],
            "acceptance_criteria": common["acceptance_criteria"],
            "evidence": common.get("evidence", {}),
            "owned_work_attempts": options if repair else [],
        },
        "response_contract": {
            "schema": SCHEMA,
            "shape": contract,
            "strict": "Return only one JSON object. Metadata decisions select exactly one option; parallel batches require distinct owned WorkAttempt targets.",
        },
        "notice": "This is a read-only synthetic proposal. No Room Action is authorized, no goal completion may be declared, and no tools may be used.",
    }


def analyze(transcript: dict, request: dict, repair: bool) -> dict:
    text = "\n".join(
        item["text"]
        for item in transcript["messages"]
        if item.get("phase") != "commentary"
    )
    decision = json.loads(text)
    if set(decision) != {"schema", "decision"} or decision["schema"] != SCHEMA:
        raise ValueError("response is outside the planning envelope")
    body = decision["decision"]
    if set(body) != {"kind", "steps", "reason"} or body["kind"] != "dispatch":
        raise ValueError("probe expected a bounded dispatch decision")
    steps = body["steps"]
    if (
        not 1 <= len(steps) <= 16
        or not isinstance(body["reason"], str)
        or not body["reason"].strip()
    ):
        raise ValueError("invalid decision bounds")
    retained_options = json.loads(request["instruction"])["options"]
    options = {(o["member_key"], o["target_id"]) for o in retained_options}
    for step in steps:
        if set(step) != {"member_key", "target_id", "instruction"}:
            raise ValueError("invalid step shape")
        if (step["member_key"], step["target_id"]) not in options:
            raise ValueError("unoffered option selected")
        if (
            not isinstance(step["instruction"], str)
            or not 1 <= len(step["instruction"].encode()) <= 65536
        ):
            raise ValueError("invalid instruction bound")
    if len({s["member_key"] for s in steps}) != len(steps) or len(
        {s["target_id"] for s in steps}
    ) != len(steps):
        raise ValueError("duplicate member or target")
    if len(steps) > 1 and any(
        o["semantic_target"]["kind"] != "work_attempt"
        for o in retained_options
        if (o["member_key"], o["target_id"])
        in {(s["member_key"], s["target_id"]) for s in steps}
    ):
        raise ValueError("metadata dispatch must contain exactly one step")
    selected = {step["target_id"] for step in steps}
    preferred = {"repair-formatter", "prepare-regression-check"}
    observed = set(transcript.get("observed_item_types", []))
    return {
        "strict_planning_shape": True,
        "authorized_distinct_selections": True,
        "selected_targets": sorted(selected),
        "matches_harness_preferred_next_work": selected == preferred
        if repair
        else len(selected) == 1,
        "preference_metric_limit": "A harness preference heuristic, not a correctness or quality oracle. Repair alone is a valid conservative choice; preparing regression checks concurrently is optional.",
        "metadata_or_parallel_attempt_rule": True,
        "observed_item_types": sorted(observed),
        "no_tool_item_observed": observed
        <= {"userMessage", "agentMessage", "reasoning"},
        "assessment_limit": "Shape/selection checks plus retained instructions support a narrow synthetic observation, not general planning quality or enforced no-tool confinement.",
        "decision": decision,
    }


def native_canaries(native: Path, directory: Path, env: dict[str, str]) -> dict:
    inside, outside = directory / "inside", directory / "outside"
    inside.mkdir()
    outside.mkdir()
    (inside / "canary.txt").write_text("SYNTHETIC_INSIDE", encoding="utf-8")
    (outside / "canary.txt").write_text("SYNTHETIC_OUTSIDE", encoding="utf-8")
    profile = (
        "worldstream_probe_" + hashlib.sha256(str(directory).encode()).hexdigest()[:16]
    )
    results = {}
    for access in ("read", "write"):
        args = [
            str(native),
            "sandbox",
            "--permission-profile",
            profile,
            "--cd",
            str(inside),
            "-c",
            f'permissions.{profile}.filesystem={{":minimal"="read",":workspace_roots"={{"."="{access}"}}}}',
            "-c",
            f"permissions.{profile}.network.enabled=false",
        ]
        for label, command in {
            "read_inside": ["/bin/cat", str(inside / "canary.txt")],
            "read_outside": ["/bin/cat", str(outside / "canary.txt")],
            "write_inside": ["/usr/bin/touch", str(inside / (access + "-write.txt"))],
            "write_outside": ["/usr/bin/touch", str(outside / (access + "-write.txt"))],
        }.items():
            run = subprocess.run(
                args + ["--"] + command,
                cwd=inside,
                env=env,
                capture_output=True,
                timeout=10,
                check=False,
            )
            results[access + ":" + label] = {
                "exit_code": run.returncode,
                "in_root_canary_returned": run.stdout == b"SYNTHETIC_INSIDE",
                "out_of_root_canary_returned": run.stdout == b"SYNTHETIC_OUTSIDE",
                "os_denial_reported": b"Operation not permitted" in run.stderr
                or b"Permission denied" in run.stderr,
            }
    return {
        "scope": "standalone Codex sandbox, not app-server/Swarm invocation qualification",
        "results": results,
    }


def process_snapshot() -> dict[int, tuple[int, int, str]]:
    output = subprocess.run(
        ["/bin/ps", "-axo", "pid=,ppid=,pgid=,stat="],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        timeout=5,
        check=True,
    )
    result = {}
    for line in output.stdout.decode().splitlines():
        pid, parent, group, status = line.split(maxsplit=3)
        result[int(pid)] = (int(parent), int(group), status)
    return result


def guarded_turn(
    args: argparse.Namespace,
    cwd: Path,
    env: dict[str, str],
    executable_digest: str,
    payload: dict,
    session: str | None,
    *,
    permission_profile: str | None = None,
    output_schema: dict | None = None,
) -> tuple[dict, dict]:
    selected = (
        {"mode": "resume", "session_id": session}
        if session
        else {"mode": "fresh", "requested_id": None}
    )
    request = {
        "model": args.model,
        "effort": args.effort,
        "cwd": str(cwd),
        "resource_policy": "read_only",
        "session": selected,
        "prompt": json.dumps(payload),
    }
    if permission_profile is not None:
        request["permission_profile"] = permission_profile
    if output_schema is not None:
        request["output_schema"] = output_schema
    argv = ["app-server", "--stdio", "--strict-config"]
    for config in [
        'model_provider="openai"',
        'forced_login_method="chatgpt"',
        'approval_policy="never"',
        "features.multi_agent=false",
        'web_search="disabled"',
        "model_reasoning_effort=" + json.dumps(args.effort),
    ]:
        argv += ["--config", config]
    launch = {
        "program": str(args.codex),
        "expected_executable_digest": executable_digest,
        "arguments": argv,
        "working_area": str(cwd),
        "stdin": json.dumps(request),
        "codex_app_server": True,
        "clear_environment": False,
        "environment": {},
    }
    started = time.monotonic()
    process = subprocess.Popen(
        [str(args.guard)],
        cwd=cwd,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    observed_pids, observed_groups = {process.pid}, {process.pid}
    captured = {}

    def drain(name: str, stream) -> None:
        chunks, size = [], 0
        for chunk in iter(lambda: stream.read(65536), b""):
            size += len(chunk)
            if size <= 8 * 1024 * 1024:
                chunks.append(chunk)
        captured[name] = (b"".join(chunks), size)

    pumps = [
        threading.Thread(target=drain, args=(name, stream), daemon=True)
        for name, stream in (("stdout", process.stdout), ("stderr", process.stderr))
    ]
    for pump in pumps:
        pump.start()
    assert process.stdin is not None
    process.stdin.write((json.dumps(launch) + "\n").encode())
    process.stdin.flush()
    # Keep the ownership pipe open throughout the turn. Closing it requests
    # cancellation; communicate(input=...) would close it too soon.
    timed_out = False
    try:
        deadline = started + args.timeout
        while process.poll() is None:
            snapshot = process_snapshot()
            changed = True
            while changed:
                changed = False
                for pid, (parent, group, _) in snapshot.items():
                    if parent in observed_pids and pid not in observed_pids:
                        observed_pids.add(pid)
                        observed_groups.add(group)
                        changed = True
            if time.monotonic() >= deadline:
                timed_out = True
                process.stdin.close()
                process.stdin = None
                process.wait(timeout=15)
                break
            time.sleep(0.05)
    finally:
        if process.stdin is not None:
            process.stdin.close()
            process.stdin = None
    for pump in pumps:
        pump.join(timeout=5)
        if pump.is_alive():
            raise RuntimeError("guard output did not drain after process exit")
    stdout, stdout_count = captured["stdout"]
    stderr, stderr_count = captured["stderr"]
    if stdout_count != len(stdout) or stderr_count != len(stderr):
        raise RuntimeError("guard output exceeded probe capture bounds")
    observation = {
        "elapsed_seconds": round(time.monotonic() - started, 3),
        "exit_code": process.returncode,
        "timed_out": timed_out,
        "stderr_bytes": len(stderr),
        "stderr_sha256": hashlib.sha256(stderr).hexdigest(),
        "session_mode": selected["mode"],
    }
    snapshot = process_snapshot()
    remaining = [
        pid
        for pid, (_, group, status) in snapshot.items()
        if (pid in observed_pids or group in observed_groups)
        and not status.startswith("Z")
    ]
    observation.update(
        {
            "observed_process_count": len(observed_pids),
            "observed_process_groups_reaped": not remaining,
            "remaining_observed_live_process_count": len(remaining),
        }
    )
    if remaining:
        raise RuntimeError(
            "observed provider process remains after guard exit: "
            + json.dumps(observation)
        )
    if timed_out or process.returncode != 0 or not stdout.startswith(READY):
        raise RuntimeError("guarded turn failed: " + json.dumps(observation))
    transcript = json.loads(stdout[len(READY) :])
    configuration = transcript["configuration"]
    if configuration["model"] != args.model or configuration["effort"] != args.effort:
        raise ValueError("provider configuration substituted")
    if session and configuration["thread_id"] != session:
        raise ValueError("provider resumed another thread")
    return transcript, observation


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codex", type=Path, required=True)
    parser.add_argument("--guard", type=Path, required=True)
    parser.add_argument("--swarm-app", type=Path, required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--effort", required=True)
    parser.add_argument(
        "--acknowledge-moving-alias", action="store_true", required=True
    )
    parser.add_argument("--live", action="store_true", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=120)
    args = parser.parse_args()
    if (
        not 15 <= args.timeout <= 180
        or not args.model.strip()
        or not args.effort.strip()
    ):
        parser.error("explicit model/effort and a 15–180 second timeout are required")
    for key in ("codex", "guard", "swarm_app"):
        setattr(args, key, getattr(args, key).resolve(strict=True))
    args.output.mkdir(parents=True, exist_ok=False)
    env = {key: value for key, value in os.environ.items() if key not in API_ENV}
    env["NO_COLOR"] = "1"
    inspection = subprocess.run(
        [str(args.swarm_app), "providers", "--codex-path", str(args.codex)],
        env=env,
        capture_output=True,
        timeout=45,
        check=True,
    )
    rows = json.loads(inspection.stdout)
    matching = [
        row
        for row in rows
        if row.get("provider") == "codex"
        and row.get("requested_path") == str(args.codex)
    ]
    if len(matching) != 1 or not matching[0].get("capabilities"):
        raise RuntimeError("native executable inspection missing")
    capability = matching[0]["capabilities"]
    report = {
        "schema": "worldstream/codex-local-prequalification-probe@1",
        "status": "incomplete",
        "provider_admission": "blocked",
        "release_qualification": "not_attempted",
        "requested_model": args.model,
        "requested_effort": args.effort,
        "moving_alias_acknowledged": True,
        "cli_version": capability["version"],
        "native_executable_blake3": capability["executable_digest"],
        "native_executable_sha256": digest(args.codex),
        "guard_sha256": digest(args.guard),
        "script_sha256": digest(Path(__file__)),
        "turns": [],
    }
    base = None
    try:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-live-codex-probe-"
        ) as temporary:
            base = Path(temporary).resolve()
            work = base / "work"
            work.mkdir()
            report["standalone_permission_canaries"] = native_canaries(
                args.codex, base, env
            )
            session = None
            for repair in (False, True):
                label = "repair" if repair else "initial"
                payload = prompt(repair)
                write_json(args.output / (label + "-prompt.json"), payload)
                transcript, observation = guarded_turn(
                    args, work, env, capability["executable_digest"], payload, session
                )
                write_json(args.output / (label + "-transcript.json"), transcript)
                assessment = analyze(transcript, payload, repair)
                write_json(args.output / (label + "-assessment.json"), assessment)
                report["turns"].append(
                    {
                        "stage": label,
                        **observation,
                        "transcript_sha256": digest(
                            args.output / (label + "-transcript.json")
                        ),
                        "matches_harness_preferred_next_work": assessment[
                            "matches_harness_preferred_next_work"
                        ],
                        "no_tool_item_observed": assessment["no_tool_item_observed"],
                    }
                )
                session = transcript["configuration"]["thread_id"]
            report["status"] = "probe_complete"
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
    finally:
        report["disposable_working_directory_removed"] = (
            base is not None and not base.exists()
        )
        identity = {}
        for label, path in (
            ("native_executable", args.codex),
            ("guard", args.guard),
            ("script", Path(__file__)),
        ):
            try:
                after = digest(path)
                identity[label] = {
                    "sha256_after": after,
                    "matches_before_run": after == report[label + "_sha256"],
                }
            except OSError:
                identity[label] = {
                    "matches_before_run": False,
                    "error": "file could not be hashed after run",
                }
        report["post_run_file_identity"] = identity
        if not all(item["matches_before_run"] for item in identity.values()):
            report["status"] = "incomplete"
            report["file_identity_error"] = (
                "At least one measured executable/script changed or became unavailable during the probe."
            )
        write_json(args.output / "report.json", report)
    print(
        json.dumps(
            {
                "status": report["status"],
                "provider_admission": "blocked",
                "report": str(args.output / "report.json"),
            }
        )
    )
    return 0 if report["status"] == "probe_complete" else 1


if __name__ == "__main__":
    raise SystemExit(main())

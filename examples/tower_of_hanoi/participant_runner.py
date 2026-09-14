"""One long-lived local Runner supervisor for an autonomous Hanoi Participant.

The supervisor is operational only.  It claims Pack-issued Attention, starts one
bounded Luna invocation using that Participant's own Membership credential, and
completes or renews the Activation lease.  It never selects an action, computes
a Hanoi path, or submits an Action on the model's behalf.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
import signal
import stat
import subprocess
import tempfile
import threading
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

from worldstream_sdk import LostRunnerReply, ProtocolError

from examples.cli_activity.credentials import (
    CredentialError,
    load_membership,
    load_runner,
    sdk_base_url,
    validate_pair,
)
from examples.tower_of_hanoi.local_harness import (
    codex_command,
    local_worldstream_environment,
)
from examples.tower_of_hanoi.protocol import (
    ACTIONS,
    PACK_ID,
    HanoiProtocolError,
    json_results_in,
    redact_text,
    safe_activity,
)

ATTENTION_REASONS = frozenset(("board_changed", "claim_review_requested"))
LEASE_MS = 30_000
RENEW_INTERVAL_SECONDS = 10.0
POLL_SECONDS = 0.35
MAX_TRANSCRIPT_BYTES = 1_048_576


class ParticipantRunnerError(RuntimeError):
    pass


def participant_prompt(reason: str) -> str:
    """Prompt one owned Participant; the prompt carries no path or authority bytes."""
    if reason not in ATTENTION_REASONS | {"bootstrap"}:
        raise ParticipantRunnerError("activation_reason_invalid")
    return "\n".join(
        (
            "You are an autonomous Tower of Hanoi Participant. Do not edit files.",
            f"Your bounded invocation reason is: {reason}.",
            'Run `"$HANOI_PYTHON" -m examples.tower_of_hanoi.turn snapshot --membership-file "$HANOI_MEMBERSHIP_FILE"` first.',
            "Use only that current Projection and its action_offers. The Pack provides legal move rules, work_revision, open completion claims, and assessments; it does not provide a path or decide whether the board is finished.",
            "The public Participant objective is to move the full tower from A to C. It is guidance for your own strategy only; the Pack never auto-completes based on board equality.",
            "The board arrays are bottom-to-top, so only the final disk on a rod can move. Choose your own strategy.",
            "If you choose to act, call `turn act` yourself with your observed room_seq and exactly one currently offered action: move_disk with a legal move; post_completion_claim with the observed work_revision; or assess_claim with the observed work_revision, current claim_round, and endorse, challenge, or defer.",
            "A move supersedes a claim. A claim is Participant evidence only. If an action returns stale, refresh with snapshot and independently decide again; make at most two action attempts in this invocation and stop after one accepted action.",
            "Do not print, copy, inspect, or describe the credential file or any bearer. Do not write files. End with a concise JSON status object.",
        )
    )


def _append(path: Path, value: dict[str, object]) -> None:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"))
    if "wsb1:" in encoded.lower():
        encoded = redact_text(encoded)
    descriptor = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
    with os.fdopen(descriptor, "a", encoding="utf-8") as output:
        output.write(encoded + "\n")
        output.flush()
        os.fsync(output.fileno())
    if stat.S_IMODE(path.stat().st_mode) != 0o600:
        raise ParticipantRunnerError("evidence_file_unprotected")


def _remaining(deadline_at_ms: int) -> float:
    return max(0.0, (deadline_at_ms - int(time.time() * 1000)) / 1000)


def _append_canvas_receipts(event_file: Path | None, actor: str, output: str) -> None:
    """Forward only bounded, labeled Participant action receipts to the local canvas."""
    if event_file is None:
        return
    try:
        metadata = event_file.lstat()
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077:
            raise ParticipantRunnerError("canvas_sink_unprotected")
        results = json_results_in(output.splitlines())
    except (OSError, HanoiProtocolError) as error:
        raise ParticipantRunnerError("canvas_receipt_invalid") from error
    for result in results:
        status, action_type = result.get("status"), result.get("action_type")
        if (
            status not in {"accepted", "stale", "rejected"}
            or action_type not in ACTIONS
        ):
            continue
        room_seq = (
            result.get("room_seq")
            if status == "accepted"
            else result.get("current_room_seq")
        )
        if isinstance(room_seq, bool) or not isinstance(room_seq, int) or room_seq < 0:
            continue
        work_revision = result.get("work_revision")
        event: dict[str, object] = {
            "schema": "worldstream/tower-of-hanoi-live-receipt/v2",
            "status": status,
            "actor": actor,
            "action_type": action_type,
            "room_seq": room_seq,
        }
        if (
            isinstance(work_revision, int)
            and not isinstance(work_revision, bool)
            and 0 <= work_revision <= 10_000
        ):
            event["work_revision"] = work_revision
        encoded = (
            json.dumps(event, separators=(",", ":"), sort_keys=True).encode("utf-8")
            + b"\n"
        )
        if len(encoded) > 2048 or b"wsb1:" in encoded.lower():
            raise ParticipantRunnerError("canvas_receipt_invalid")
        descriptor = os.open(event_file, os.O_WRONLY | os.O_APPEND)
        try:
            os.write(descriptor, encoded)
        finally:
            os.close(descriptor)


def _terminate_process_group(process: subprocess.Popen[str]) -> str:
    """Stop a short-lived Codex process and its tool children before evidence writes."""
    if process.poll() is not None:
        return "already_exited"
    try:
        if os.name == "posix":
            os.killpg(process.pid, signal.SIGTERM)
        else:
            process.terminate()
        try:
            process.wait(timeout=3)
            return "terminated"
        except subprocess.TimeoutExpired:
            if os.name == "posix":
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
            process.wait(timeout=3)
            return "killed"
    except (OSError, subprocess.TimeoutExpired):
        return "cleanup_failed"


def _as_text(value: str | bytes | None) -> str:
    if value is None:
        return ""
    return value.decode("utf-8", "replace") if isinstance(value, bytes) else value


def _invoke(
    *,
    codex: Path,
    python: Path,
    repo: Path,
    membership_file: Path,
    effort: str,
    timeout_seconds: float,
    reason: str,
    evidence: Path,
    invocation_name: str,
    seat: str,
    event_file: Path | None = None,
    cancelled: threading.Event | None = None,
) -> bool:
    """Run one short-lived Codex child and retain a redacted bounded transcript.

    A fresh process group lets lease loss and wall-clock expiry stop Codex plus every
    tool subprocess. The Codex child itself still inherits no ambient shell values.
    """
    workspace = Path(
        tempfile.mkdtemp(prefix="worldstream-hanoi-participant-")
    ).resolve()
    os.chmod(workspace, 0o700)
    process: subprocess.Popen[str] | None = None
    stdout = ""
    stderr = ""
    timed_out = False
    cancelled_run = False
    cleanup = "not_needed"
    try:
        command = codex_command(
            codex,
            workspace,
            effort,
            participant_prompt(reason),
            repo=repo,
            python=python,
            membership_file=membership_file,
        )
        # Codex needs its normal login/auth state. Its launched shell remains explicitly
        # `inherit=none` in codex_command, so Room credentials still enter only by path.
        process = subprocess.Popen(
            command,
            cwd=workspace,
            env=local_worldstream_environment(dict(os.environ)),
            text=True,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        deadline = time.monotonic() + max(1.0, timeout_seconds)
        while process.poll() is None:
            if cancelled is not None and cancelled.is_set():
                cancelled_run = True
                cleanup = _terminate_process_group(process)
                break
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                timed_out = True
                cleanup = _terminate_process_group(process)
                break
            try:
                stdout, stderr = process.communicate(timeout=min(0.5, remaining))
            except subprocess.TimeoutExpired:
                continue
        if process.poll() is not None and not stdout and not stderr:
            stdout, stderr = process.communicate()
    finally:
        if process is not None and process.poll() is None:
            cleanup = _terminate_process_group(process)
        try:
            raw_stdout = _as_text(stdout)
            _append_canvas_receipts(event_file, seat, raw_stdout)
            safe_stdout = redact_text(raw_stdout)[:MAX_TRANSCRIPT_BYTES]
            safe_stderr = redact_text(_as_text(stderr))[:MAX_TRANSCRIPT_BYTES]
            transcript_path = evidence.parent / f"{invocation_name}.jsonl"
            descriptor = os.open(
                transcript_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600
            )
            with os.fdopen(descriptor, "w", encoding="utf-8") as output:
                output.write(safe_stdout)
                if safe_stderr:
                    output.write(
                        json.dumps(
                            {"kind": "stderr", "text": safe_stderr},
                            separators=(",", ":"),
                        )
                        + "\n"
                    )
            _append(
                evidence,
                {
                    "kind": "codex_invocation",
                    "name": invocation_name,
                    "reason": reason,
                    "model": "gpt-5.6-luna",
                    "reasoning_effort": effort,
                    "timeout_seconds": timeout_seconds,
                    "exit_status": None if process is None else process.returncode,
                    "timed_out": timed_out,
                    "cancelled": cancelled_run,
                    "process_group_cleanup": cleanup,
                    "workspace_removed": True,
                },
            )
        finally:
            shutil.rmtree(workspace, ignore_errors=True)
    return (
        process is not None
        and process.returncode == 0
        and not timed_out
        and not cancelled_run
    )


async def _invoke_claimed(
    runner: Any,
    claimed: dict[str, Any],
    invoke: Callable[[threading.Event], bool],
    evidence: Path,
) -> None:
    activation_id, claim_id, generation = (
        claimed.get("activation_id"),
        claimed.get("claim_id"),
        claimed.get("lease_generation"),
    )
    if (
        not isinstance(activation_id, str)
        or not isinstance(claim_id, str)
        or isinstance(generation, bool)
        or not isinstance(generation, int)
    ):
        raise ParticipantRunnerError("activation_claim_invalid")
    cancelled = threading.Event()
    task = asyncio.create_task(asyncio.to_thread(invoke, cancelled))
    lease_lost = False
    try:
        while not task.done():
            try:
                await asyncio.wait_for(
                    asyncio.shield(task), timeout=RENEW_INTERVAL_SECONDS
                )
            except asyncio.TimeoutError:
                try:
                    renewed = await runner.renew(
                        activation_id, claim_id, generation, LEASE_MS
                    )
                except (
                    ProtocolError,
                    LostRunnerReply,
                    OSError,
                    TimeoutError,
                    ValueError,
                ) as error:
                    cancelled.set()
                    lease_lost = True
                    _append(
                        evidence,
                        {
                            "kind": "activation_lease_lost",
                            "activation_id": activation_id,
                            "code": type(error).__name__,
                        },
                    )
                    break
                renewed_generation = renewed.get("lease_generation")
                if (
                    renewed.get("code") != "renewed"
                    or isinstance(renewed_generation, bool)
                    or not isinstance(renewed_generation, int)
                ):
                    cancelled.set()
                    lease_lost = True
                    _append(
                        evidence,
                        {
                            "kind": "activation_lease_lost",
                            "activation_id": activation_id,
                        },
                    )
                    break
                generation = renewed_generation
                _append(
                    evidence,
                    {
                        "kind": "activation_renewed",
                        "activation_id": activation_id,
                        "lease_generation": generation,
                    },
                )
        invocation_ok = await task
        if lease_lost:
            return
        try:
            completed = await runner.complete(
                activation_id, claim_id, generation, "handled"
            )
        except Exception as error:
            raise ParticipantRunnerError("activation_complete_failed") from error
        _append(
            evidence,
            {
                "kind": "activation_completed",
                "activation_id": activation_id,
                "claim_id": claim_id,
                "lease_generation": generation,
                "invocation_ok": invocation_ok,
                "code": completed.get("code")
                if isinstance(completed.get("code"), str)
                else "unknown",
            },
        )
    finally:
        if not task.done():
            cancelled.set()
            await task


async def run(arguments: argparse.Namespace) -> dict[str, object]:
    """Bootstrap once, then handle only Pack-issued Activation offers until deadline."""
    from worldstream_sdk import Client

    membership = load_membership(arguments.membership_file)
    runner_doc = load_runner(arguments.runner_file)
    validate_pair(membership, runner_doc)
    if membership["pack"]["id"] != PACK_ID or membership["role"] != "solver":
        raise CredentialError("tower_runner_membership_required")
    member_client = Client(sdk_base_url(membership), membership["bearer"])
    runner_client = Client(sdk_base_url(runner_doc), runner_doc["bearer"])
    room = await member_client.open_room(membership["room_id"], membership["member_id"])
    runner = await runner_client.open_runner(
        runner_doc["runner_id"], 1, [PACK_ID], [membership["pack"]]
    )
    evidence = arguments.evidence_file
    try:
        await room.sync()
        _append(
            evidence,
            {
                "kind": "supervisor_started",
                "seat": membership["seat"],
                "runner_id_present": True,
            },
        )
        bootstrap_timeout = min(
            arguments.codex_timeout_seconds,
            max(1.0, _remaining(arguments.deadline_at_ms)),
        )
        _invoke(
            codex=arguments.codex,
            python=arguments.python,
            repo=arguments.repo,
            membership_file=arguments.membership_file,
            effort=arguments.solver_effort,
            timeout_seconds=bootstrap_timeout,
            reason="bootstrap",
            evidence=evidence,
            invocation_name="bootstrap",
            seat=membership["seat"],
            event_file=arguments.event_file,
        )
        invocation_count = 1
        while _remaining(arguments.deadline_at_ms) > 0:
            projection = await member_client.projection(membership["room_id"])
            activity = safe_activity(
                projection.get("projection", {}).get(
                    "activity", projection.get("projection", {})
                )
            )
            if activity["outcome"]["status"] == "participant_accepted_completion":
                return {"status": "complete", "invocations": invocation_count}
            try:
                offers = await runner.poll_offers(
                    membership["room_id"], membership["member_id"], timeout=5
                )
            except ProtocolError as error:
                if not error.retryable:
                    raise
                await asyncio.sleep(POLL_SECONDS)
                continue
            raw_offers = offers.get("offers")
            if not isinstance(raw_offers, list):
                raise ParticipantRunnerError("activation_offers_invalid")
            offer = next(
                (
                    candidate
                    for candidate in raw_offers
                    if isinstance(candidate, dict)
                    and candidate.get("reason_code") in ATTENTION_REASONS
                ),
                None,
            )
            if offer is None:
                await asyncio.sleep(POLL_SECONDS)
                continue
            activation_id = offer.get("activation_id")
            reason = offer.get("reason_code")
            if not isinstance(activation_id, str) or not isinstance(reason, str):
                raise ParticipantRunnerError("activation_offer_invalid")
            try:
                claimed = await runner.claim(activation_id, LEASE_MS)
            except LostRunnerReply as error:
                claimed = await error.retry()
            if claimed.get("code") != "granted":
                await asyncio.sleep(POLL_SECONDS)
                continue
            invocation_count += 1
            timeout = min(
                arguments.codex_timeout_seconds,
                max(1.0, _remaining(arguments.deadline_at_ms)),
            )

            def invoke_activation(
                cancelled: threading.Event,
                timeout_seconds: float = timeout,
                activation_reason: str = reason,
                name: str = f"activation-{invocation_count:03d}",
            ) -> bool:
                return _invoke(
                    codex=arguments.codex,
                    python=arguments.python,
                    repo=arguments.repo,
                    membership_file=arguments.membership_file,
                    effort=arguments.solver_effort,
                    timeout_seconds=timeout_seconds,
                    reason=activation_reason,
                    evidence=evidence,
                    invocation_name=name,
                    seat=membership["seat"],
                    event_file=arguments.event_file,
                    cancelled=cancelled,
                )

            await _invoke_claimed(runner, claimed, invoke_activation, evidence)
        return {"status": "deadline", "invocations": invocation_count}
    finally:
        await runner.close()
        await room.close()


def _arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--membership-file", type=Path, required=True)
    parser.add_argument("--runner-file", type=Path, required=True)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--python", type=Path, required=True)
    parser.add_argument("--codex", type=Path, required=True)
    parser.add_argument("--solver-effort", choices=("low", "medium"), default="medium")
    parser.add_argument("--codex-timeout-seconds", type=float, default=120.0)
    parser.add_argument("--deadline-at-ms", type=int, required=True)
    parser.add_argument("--evidence-file", type=Path, required=True)
    parser.add_argument("--event-file", type=Path)
    arguments = parser.parse_args()
    if not 1 <= arguments.codex_timeout_seconds <= 600 or arguments.deadline_at_ms <= 0:
        parser.error("timeout or deadline is outside the bounded range")
    return arguments


def main() -> int:
    arguments = _arguments()
    try:
        result = asyncio.run(run(arguments))
    except (
        CredentialError,
        HanoiProtocolError,
        ParticipantRunnerError,
        ProtocolError,
        LostRunnerReply,
        OSError,
        TimeoutError,
        ValueError,
    ) as error:
        result = {"status": "error", "code": type(error).__name__}
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0 if result.get("status") in {"complete", "deadline"} else 3


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Native, isolated Agent Swarm Controller/Runtime acceptance smoke."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import socket
import subprocess
import sys
import time
from pathlib import Path

UPDATE_STATE_SCHEMA = "worldstream/agent-swarm-managed-update-state/v1"
UPDATE_STATE_FILE = "managed-update-state.json"
ACCEPTANCE_CRITERION = (
    "The exact deliverable content passes its declared deterministic check."
)


def run(
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str] | None = None,
    allow_failure: bool = False,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        command, check=False, capture_output=True, text=True, env=env, cwd=cwd
    )
    if result.returncode and not allow_failure:
        # These commands promise secret-safe diagnostics. Never print stdout: some
        # successful application documents contain local paths and Room details.
        diagnostic = result.stderr.strip().splitlines()[-4:] or [
            "command failed without a diagnostic"
        ]
        executable = Path(command[0]).name
        operation = next(
            (value for value in command[1:3] if value and not value.startswith("-")),
            "operation",
        )
        raise RuntimeError(f"{executable} {operation} failed: {' | '.join(diagnostic)}")
    return result


def document(result: subprocess.CompletedProcess[str]) -> object:
    return json.loads(result.stdout)


def finish_cleanup(*, root: Path, marker: Path, cleanup_failures: list[str]) -> None:
    """Report cleanup trouble without replacing an exception already in flight."""

    primary_error = sys.exception()
    if cleanup_failures:
        message = f"managed cleanup was not proven; retain {root}: " + "; ".join(
            cleanup_failures
        )
        if primary_error is not None:
            primary_error.add_note(message)
            return
        raise RuntimeError(message)
    try:
        marker.touch(exist_ok=False)
    except OSError as error:
        message = f"managed cleanup marker could not be written for {root}: {error}"
        if primary_error is not None:
            primary_error.add_note(message)
            return
        raise RuntimeError(message) from error


def retained_managed_stop_proven(result: subprocess.CompletedProcess[str]) -> bool:
    """Accept only exact records proving both managed processes cleanly stopped."""

    try:
        receipt = json.loads(result.stdout)
    except (json.JSONDecodeError, TypeError):
        return False
    retained = receipt.get("retained_server") if isinstance(receipt, dict) else None
    controller = retained.get("controller") if isinstance(retained, dict) else None
    runtime = retained.get("runtime") if isinstance(retained, dict) else None
    # The lifecycle checkpoint can retain the preceding Start when shutdown
    # completes after a reply is lost. Each process record independently marks
    # an explicit clean stop, while the released leases close present ownership.
    return (
        receipt.get("status") == "unavailable"
        and receipt.get("code") == "controller_unavailable"
        and isinstance(controller, dict)
        and controller.get("availability") == "available"
        and controller.get("phase") == "stopped"
        and controller.get("lease") == "released"
        and isinstance(runtime, dict)
        and runtime.get("availability") == "available"
        and runtime.get("phase") == "stopped"
        and runtime.get("lease") == "released"
    )


def managed_stop_proven(result: subprocess.CompletedProcess[str]) -> bool:
    """Accept a completed stop command or exact retained stopped evidence."""

    return result.returncode == 0 or retained_managed_stop_proven(result)


def wait_for_retained_managed_stop(
    *,
    control: Path,
    managed: list[str],
    cwd: Path,
    attempts: int = 50,
    interval_seconds: float = 0.1,
) -> bool:
    """Boundedly wait for the asynchronous controller shutdown checkpoint."""

    for attempt in range(attempts):
        retained = run(
            [str(control), "server", "status", *managed, "--json"],
            cwd=cwd,
            allow_failure=True,
        )
        if retained_managed_stop_proven(retained):
            return True
        if attempt + 1 < attempts:
            time.sleep(interval_seconds)
    return False


def guarded_check_evidence_ref(
    *,
    app: Path,
    process_guard: Path,
    root: Path,
    work: Path,
    service_candidate_id: str,
    pack_candidate_id: str,
    pack_candidate_version: int,
    check_id: str,
    check_revision: int,
    criteria_revision: int,
    resource_basis: list[dict[str, object]],
    subject: Path,
    candidate_artifact: dict[str, object],
    expected_text: str,
    expected_status: str = "passed",
) -> dict[str, object]:
    """Execute one exact candidate check and return its canonical Pack reference."""

    try:
        target = subject.relative_to(work).as_posix()
    except ValueError as error:
        raise RuntimeError("guarded Check subject escaped the approved root") from error
    state = root / f"report-check-state-{service_candidate_id}"

    def request(label: str, value: dict[str, object]) -> Path:
        path = root / f"guarded-check-{service_candidate_id}-{label}.json"
        path.write_text(
            json.dumps(value, sort_keys=True, separators=(",", ":")),
            encoding="utf-8",
        )
        return path

    def command(operation: list[str]) -> dict[str, object]:
        value = document(
            run(
                [
                    str(app),
                    "code-change",
                    "--working-area",
                    str(work),
                    "--code-change-state",
                    str(state),
                    *operation,
                ],
                cwd=root,
            )
        )
        if not isinstance(value, dict):
            raise TypeError("guarded Check command returned a non-object receipt")
        return value

    command(
        [
            "prepare",
            "--request",
            str(
                request(
                    "prepare",
                    {"candidate_id": pack_candidate_id, "targets": [target]},
                )
            ),
        ]
    )
    sealed = command(
        [
            "seal",
            "--request",
            str(
                request(
                    "seal",
                    {
                        "acceptance_criteria": [ACCEPTANCE_CRITERION],
                        "authors": ["fixture-a"],
                        "candidate_id": pack_candidate_id,
                        "criteria_revision": criteria_revision,
                        "inputs": [],
                        "pack_candidate": {
                            "candidate_id": pack_candidate_id,
                            "version": pack_candidate_version,
                        },
                        "pack_candidate_artifact": candidate_artifact,
                        "pack_candidate_path": target,
                        "resource_basis": resource_basis,
                    },
                )
            ),
        ]
    )
    revision_digest = sealed.get("revision_digest")
    if not isinstance(revision_digest, str):
        raise TypeError("guarded Check seal omitted its exact revision")
    checked = command(
        [
            "check",
            "--request",
            str(
                request(
                    "run",
                    {
                        "arguments": [
                            "code-change-check-fixture",
                            "--path",
                            target,
                            "--expected",
                            expected_text,
                        ],
                        "candidate_id": pack_candidate_id,
                        "check_id": check_id,
                        "evidence_path": (
                            f"check-evidence-{service_candidate_id}-{check_id}.json"
                        ),
                        "environment": {},
                        "pack_check_revision": check_revision,
                        "program": str(app.resolve()),
                        "revision_digest": revision_digest,
                    },
                )
            ),
            "--process-guard",
            str(process_guard),
        ]
    )
    payload = checked.get("action_payload")
    evidence_refs = payload.get("evidence_refs") if isinstance(payload, dict) else None
    if (
        not isinstance(payload, dict)
        or payload.get("candidate")
        != {
            "candidate_id": pack_candidate_id,
            "version": pack_candidate_version,
        }
        or payload.get("check_id") != check_id
        or payload.get("status") != expected_status
        or not isinstance(evidence_refs, list)
        or len(evidence_refs) != 1
        or not isinstance(evidence_refs[0], dict)
        or evidence_refs[0].get("candidate_artifact") != candidate_artifact
        or evidence_refs[0].get("criterion") != ACCEPTANCE_CRITERION
    ):
        raise RuntimeError("guarded Check did not emit exact canonical Pack evidence")
    return payload


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


class ActionIds:
    """Deterministic valid ULIDs used only inside an isolated smoke Room."""

    alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"

    def __init__(self) -> None:
        self.value = 0

    def next(self) -> str:
        if self.value >= len(self.alphabet) ** 2:
            raise RuntimeError("smoke Action identity capacity exhausted")
        suffix = (
            self.alphabet[self.value // len(self.alphabet)]
            + self.alphabet[self.value % len(self.alphabet)]
        )
        self.value += 1
        return "01ARZ3NDEKTSV4RRFFQ69G5F" + suffix


def tui_reopen_inputs(selected_index: int) -> list[dict[str, object]]:
    """Select one exact listed Swarm before exercising update-time TUI reopen."""

    if isinstance(selected_index, bool) or not 0 <= selected_index < 512:
        raise ValueError("updated TUI Swarm index is invalid")
    return [
        *[{"input": "down"}] * selected_index,
        {"input": "open"},
        {"input": "resize", "width": 72, "height": 20},
        {"input": "refresh"},
        {"input": "quit"},
    ]


def observe_actor(
    app: Path, local: list[str], swarm_id: str, actor: str, cwd: Path
) -> dict[str, object]:
    try:
        value = document(
            run(
                [
                    str(app),
                    "observe",
                    *local,
                    "--swarm-id",
                    swarm_id,
                    "--actor",
                    actor,
                ],
                cwd=cwd,
            )
        )
    except (RuntimeError, ValueError) as error:
        raise RuntimeError(f"{actor} observation failed: {error}") from error
    if not isinstance(value, dict):
        raise TypeError("participant observation was not an object")
    return value


def participant_member_id(observation: dict[str, object], member_key: str) -> str:
    actor = observation.get("actor")
    authenticated_member_id = observation.get("member_id")
    if (
        isinstance(actor, dict)
        and actor.get("kind") == "worker"
        and actor.get("member_key") == member_key
        and isinstance(authenticated_member_id, str)
    ):
        return authenticated_member_id
    activity = observation.get("activity")
    roster = activity.get("roster") if isinstance(activity, dict) else None
    if not isinstance(roster, list):
        raise TypeError("participant observation omitted the Swarm roster")
    matches = [
        item.get("member_id")
        for item in roster
        if isinstance(item, dict) and item.get("member_key") == member_key
    ]
    if len(matches) != 1 or not isinstance(matches[0], str):
        raise RuntimeError(f"could not resolve exact member identity for {member_key}")
    return matches[0]


def reopen_existing_state(
    *,
    root: Path,
    binary_dir: Path,
    config: Path,
) -> int:
    """Reopen an old managed Room and artifact with the selected application."""

    manifest_path = root / UPDATE_STATE_FILE
    try:
        retained = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeError("managed update state is unavailable") from error
    if not isinstance(retained, dict) or retained.get("schema") != UPDATE_STATE_SCHEMA:
        raise RuntimeError("managed update state is invalid")
    required_text = (
        "swarm_id",
        "room_id",
        "goal",
        "pack_id",
        "pack_version",
        "pack_digest",
        "artifact_path",
        "authoritative_artifact_digest",
        "runtime_address",
        "controller_address",
    )
    if any(not isinstance(retained.get(field), str) for field in required_text):
        raise RuntimeError("managed update state is incomplete")
    expected_roster = retained.get("roster")
    if not isinstance(expected_roster, list):
        raise TypeError("managed update roster is unavailable")

    control = binary_dir / "worldstreamctl"
    app = binary_dir / "worldstream-agent-swarm"
    if os.name == "nt":
        control = control.with_suffix(".exe")
        app = app.with_suffix(".exe")
    state = root / "studio"
    data = root / "runtime"
    runtime_address = str(retained["runtime_address"])
    controller_address = str(retained["controller_address"])
    common = [
        "--config",
        str(config),
        "--bind",
        runtime_address,
        "--data-dir",
        str(data),
    ]
    managed = common + [
        "--state-dir",
        str(state),
        "--controller",
        controller_address,
        "--timeout-seconds",
        "30",
    ]
    local = [
        "--state-dir",
        str(state),
        "--controller-address",
        controller_address,
        "--runtime-address",
        runtime_address,
        "--pack-id",
        str(retained["pack_id"]),
        "--pack-version",
        str(retained["pack_version"]),
        "--pack-digest",
        str(retained["pack_digest"]),
        "--timeout-ms",
        "30000",
    ]
    started = False
    start_attempted = False
    try:
        start_attempted = True
        for _ in range(8):
            start = run(
                [str(control), "server", "start", *managed, "--json"],
                cwd=root,
                allow_failure=True,
            )
            if start.returncode == 0:
                started = True
                break
            run(
                [str(control), "server", "status", *managed, "--json"],
                cwd=root,
                allow_failure=True,
            )
            time.sleep(1)
        if not started:
            raise RuntimeError(
                "updated managed Controller/Runtime did not become ready"
            )

        listing = document(run([str(app), "list", *local], cwd=root))
        if not isinstance(listing, list):
            raise TypeError("updated managed list was not an array")
        summary_index = next(
            (
                index
                for index, item in enumerate(listing)
                if isinstance(item, dict)
                and item.get("swarm_id") == retained["swarm_id"]
            ),
            None,
        )
        summary = listing[summary_index] if isinstance(summary_index, int) else None
        if (
            not isinstance(summary, dict)
            or summary.get("room_id") != retained["room_id"]
            or summary.get("goal") != retained["goal"]
        ):
            raise RuntimeError("updated application did not find the retained Swarm")
        reopened = document(
            run(
                [
                    str(app),
                    "open",
                    *local,
                    "--swarm-id",
                    str(retained["swarm_id"]),
                ],
                cwd=root,
            )
        )
        if (
            not isinstance(reopened, dict)
            or reopened.get("room_id") != retained["room_id"]
            or reopened.get("roster") != expected_roster
        ):
            raise RuntimeError(
                "updated application changed Room identity or provider selections"
            )
        observation = observe_actor(
            app, local, str(retained["swarm_id"]), "human", root
        )
        activity = observation.get("activity")
        results = activity.get("results") if isinstance(activity, dict) else None
        contributions = (
            activity.get("contributions") if isinstance(activity, dict) else None
        )
        if (
            not isinstance(activity, dict)
            or activity.get("phase") != "completed"
            or not isinstance(results, list)
            or len(results) < 2
            or not isinstance(contributions, list)
            or not contributions
        ):
            raise RuntimeError(
                "updated application did not retain accepted Results and Contributions"
            )

        artifact = document(
            run(
                [
                    str(app),
                    "artifact",
                    *local,
                    "--swarm-id",
                    str(retained["swarm_id"]),
                    "--path",
                    str(retained["artifact_path"]),
                    "--artifact-state",
                    str(root / "artifact-state"),
                    "--expected-digest",
                    str(retained["authoritative_artifact_digest"]),
                ],
                cwd=root,
            )
        )
        if (
            not isinstance(artifact, dict)
            or artifact.get("authoritative_digest")
            != retained["authoritative_artifact_digest"]
        ):
            raise RuntimeError("updated application did not retain the exact artifact")

        reopen_script = root / "tui-update-reopen.json"
        reopen_script.write_text(
            json.dumps(tui_reopen_inputs(summary_index)),
            encoding="utf-8",
        )
        tui = document(
            run(
                [str(app), "tui", *local, "--script", str(reopen_script)],
                cwd=root,
            )
        )
        if (
            not isinstance(tui, dict)
            or tui.get("status") != "detached"
            or tui.get("swarm_id") != retained["swarm_id"]
            or tui.get("room_id") != retained["room_id"]
            or tui.get("restored") is not True
            or tui.get("cursor_shown") is not True
        ):
            safe_tui = {
                field: tui.get(field) if isinstance(tui, dict) else None
                for field in (
                    "status",
                    "swarm_id",
                    "room_id",
                    "restored",
                    "cursor_shown",
                )
            }
            raise RuntimeError(
                "updated packaged TUI did not reopen and restore: "
                + json.dumps(safe_tui, sort_keys=True, separators=(",", ":"))
            )

        (root / "managed-update-reopened").touch(exist_ok=False)
        print(
            json.dumps(
                {
                    "status": "ok",
                    "swarm_id": retained["swarm_id"],
                    "room_id": retained["room_id"],
                    "accepted_results_preserved": True,
                    "contributions_preserved": True,
                    "artifact_preserved": True,
                    "provider_selections_preserved": True,
                    "tui_restored": True,
                },
                separators=(",", ":"),
            )
        )
        return 0
    finally:
        cleanup_failure: str | None = None
        if start_attempted:
            try:
                stopped = run(
                    [str(control), "server", "stop", *managed, "--json"],
                    cwd=root,
                    allow_failure=True,
                )
                controller_stopped = run(
                    [str(control), "server", "controller-stop", *managed, "--json"],
                    cwd=root,
                    allow_failure=True,
                )
                if (
                    not managed_stop_proven(stopped)
                    or not managed_stop_proven(controller_stopped)
                ) and not wait_for_retained_managed_stop(
                    control=control, managed=managed, cwd=root
                ):
                    cleanup_failure = (
                        "updated managed cleanup was not proven; "
                        "retain state for inspection"
                    )
            except OSError as error:
                cleanup_failure = f"updated managed cleanup could not run: {error}"
        if cleanup_failure is not None:
            primary_error = sys.exception()
            if primary_error is not None:
                primary_error.add_note(cleanup_failure)
            else:
                raise RuntimeError(cleanup_failure)


def exact_action(
    observation: dict[str, object],
    actor: str,
    action_type: str,
    action_id: str,
    payload: dict[str, object],
) -> dict[str, object]:
    offers = observation.get("action_offers")
    if not isinstance(offers, list):
        raise TypeError("participant observation omitted Action offers")
    matching = [
        offer
        for offer in offers
        if isinstance(offer, dict) and offer.get("action_type") == action_type
    ]
    if len(matching) != 1:
        raise RuntimeError(f"expected one {action_type} offer, found {len(matching)}")
    offer = matching[0]
    room_seq = observation.get("room_seq")
    if not isinstance(room_seq, int):
        raise TypeError("participant observation omitted the Room Head")
    actor_value: dict[str, object]
    if actor == "human":
        actor_value = {"kind": "human_coordinator"}
    elif actor.startswith("worker:") and actor.removeprefix("worker:"):
        actor_value = {
            "kind": "worker",
            "member_key": actor.removeprefix("worker:"),
        }
    else:
        raise ValueError("invalid smoke actor")
    return {
        "actor": actor_value,
        "action_id": action_id,
        "based_on_room_seq": room_seq,
        "offer_id": offer.get("offer_id"),
        "action_type": action_type,
        "payload_schema_digest": offer.get("payload_schema_digest"),
        "payload": payload,
    }


def submit_exact(
    app: Path,
    local: list[str],
    swarm_id: str,
    action: dict[str, object],
    root: Path,
    cwd: Path,
) -> dict[str, object]:
    action_id = action.get("action_id")
    if not isinstance(action_id, str):
        raise TypeError("exact Action omitted its identity")
    path = root / f"action-{action_id}.json"
    path.write_text(json.dumps(action), encoding="utf-8")
    fixture_authority: list[str] = []
    actor = action.get("actor")
    if isinstance(actor, dict) and actor.get("kind") == "worker":
        fixture_authority = ["--allow-controlled-worker-fixture"]
    receipt = document(
        run(
            [
                str(app),
                "act",
                *local,
                "--swarm-id",
                swarm_id,
                "--request",
                str(path),
                *fixture_authority,
            ],
            cwd=cwd,
        )
    )
    if not isinstance(receipt, dict):
        raise TypeError("Action receipt was not an object")
    return receipt


def exercise_native_shared_capacity(
    *,
    app: Path,
    execution_control: Path,
    execution_state: Path,
    controlled_worker: Path,
    root: Path,
    work: Path,
    first_swarm: str,
    second_swarm: str,
) -> dict[str, bool]:
    """Run real slow controlled processes through the shared daemon scheduler."""

    inspections = document(
        run(
            [str(app), "providers", "--controlled-path", str(controlled_worker)],
            cwd=root,
        )
    )
    if not isinstance(inspections, list):
        raise TypeError("controlled provider inspection was not a list")
    capabilities = next(
        (
            item.get("capabilities")
            for item in inspections
            if isinstance(item, dict)
            and item.get("provider") == "controlled"
            and isinstance(item.get("capabilities"), dict)
        ),
        None,
    )
    if not isinstance(capabilities, dict):
        raise TypeError("controlled provider capabilities were unavailable")
    program = capabilities.get("executable")
    executable_digest = capabilities.get("executable_digest")
    if not isinstance(program, str) or not isinstance(executable_digest, str):
        raise TypeError("controlled provider identity was incomplete")

    def control(arguments: list[str]) -> dict[str, object]:
        value = document(
            run(
                [str(execution_control), "--state", str(execution_state), *arguments],
                cwd=root,
            )
        )
        if not isinstance(value, dict):
            raise TypeError("execution control response was not an object")
        return value

    def ticket(
        invocation_id: str, swarm_id: str, kind: str, sequence: int
    ) -> dict[str, object]:
        return {
            "invocation_id": invocation_id,
            "swarm_id": swarm_id,
            "member_id": f"capacity-member-{swarm_id[-8:]}",
            "provider": "controlled",
            "configuration_revision": 1,
            "kind": kind,
            "due_sequence": sequence,
        }

    def prepared(item: dict[str, object]) -> dict[str, object]:
        invocation_id = item["invocation_id"]
        return {
            "provider": "controlled",
            "invocation_id": invocation_id,
            "member_id": item["member_id"],
            "configuration_revision": 1,
            "program": program,
            "executable_digest": executable_digest,
            "qualification": None,
            "arguments": [
                "invoke",
                "--invocation-id",
                invocation_id,
                "--model",
                "controlled",
                "--behavior",
                "delayed",
                "--new-session",
                f"session-{invocation_id}",
            ],
            "working_area": str(work.resolve()),
            "environment_remove": [],
            "environment_set": {"WORLDSTREAM_CONTROLLED_PROVIDER": "1"},
            "stdin": f"capacity observation for {invocation_id}",
            "requested_model": "controlled",
            "requested_effort": None,
            "requested_session": {
                "mode": "fresh",
                "requested_id": f"session-{invocation_id}",
            },
            "resolution": {
                "state": "verified",
                "model": "controlled",
                "effort": None,
            },
            "output_contract": "controlled_json_lines",
        }

    def persist(label: str, value: dict[str, object]) -> Path:
        path = root / f"capacity-{label}.json"
        path.write_text(
            json.dumps(value, sort_keys=True, separators=(",", ":")),
            encoding="utf-8",
        )
        return path

    def enqueue(item: dict[str, object]) -> None:
        path = persist(f"ticket-{item['invocation_id']}", item)
        control(["enqueue", "--ticket", str(path)])

    def tick() -> dict[str, object]:
        response = control(["tick"])
        if response.get("kind") != "tick" or not isinstance(
            response.get("value"), dict
        ):
            raise RuntimeError("execution Tick returned the wrong result kind")
        return response["value"]

    def launch_and_settle(item: dict[str, object]) -> None:
        invocation_id = item["invocation_id"]
        ticket_path = persist(f"launch-ticket-{invocation_id}", item)
        prepared_path = persist(f"prepared-{invocation_id}", prepared(item))
        launched = control(
            [
                "launch",
                "--ticket",
                str(ticket_path),
                "--prepared",
                str(prepared_path),
            ]
        )
        if launched.get("kind") != "launch":
            raise RuntimeError("capacity Invocation was not launched")
        active_tick = tick()
        if active_tick.get("admissions") != []:
            raise RuntimeError("global provider cap admitted a second live process")
        for _ in range(40):
            observed = tick()
            completions = observed.get("completions")
            if isinstance(completions, list) and any(
                isinstance(entry, dict) and entry.get("invocation_id") == invocation_id
                for entry in completions
            ):
                break
            time.sleep(0.05)
        else:
            raise RuntimeError("slow controlled capacity Invocation did not complete")
        completed = control(["collect", invocation_id])
        result = completed.get("value")
        if (
            completed.get("kind") != "completion"
            or not isinstance(result, dict)
            or result.get("resolution") != "completed"
        ):
            raise RuntimeError("capacity Invocation completion was not retained")
        control(["acknowledge-completion", invocation_id])

    control(["set-provider-cap", "controlled", "1"])
    control(["set-priority", first_swarm, "1"])
    control(["set-priority", second_swarm, "1"])
    queued = [
        ticket("capacity-first-work", first_swarm, "work", 1),
        ticket("capacity-second-work", second_swarm, "work", 2),
        ticket("capacity-second-review", second_swarm, "progress_review", 1),
    ]
    for item in queued:
        enqueue(item)
    order: list[dict[str, object]] = []
    for _ in queued:
        scheduled = tick().get("admissions")
        if not isinstance(scheduled, list) or len(scheduled) != 1:
            raise RuntimeError("cap-one scheduler did not admit exactly one Invocation")
        decision = scheduled[0]
        admitted = decision.get("ticket") if isinstance(decision, dict) else None
        if not isinstance(admitted, dict):
            raise TypeError("scheduler admission omitted its exact ticket")
        order.append(admitted)
        launch_and_settle(admitted)
    if order[0].get("swarm_id") == order[1].get("swarm_id"):
        raise RuntimeError("equal-priority Swarms did not receive a fair first share")
    ids = [item.get("invocation_id") for item in order]
    if ids.index("capacity-second-review") > ids.index("capacity-second-work"):
        raise RuntimeError("Progress Review did not precede same-Swarm ordinary work")

    control(["set-priority", first_swarm, "2"])
    priority_status = control(["status", "--swarm-id", first_swarm])
    priority_snapshot = priority_status.get("value")
    priority_swarms = (
        priority_snapshot.get("swarms") if isinstance(priority_snapshot, dict) else None
    )
    if (
        not isinstance(priority_swarms, list)
        or len(priority_swarms) != 1
        or priority_swarms[0].get("priority") != 2
    ):
        raise RuntimeError("explicit Swarm priority was not retained")

    second_status = control(["status", "--swarm-id", second_swarm])
    second_snapshot = second_status.get("value")
    second_swarms = (
        second_snapshot.get("swarms") if isinstance(second_snapshot, dict) else None
    )
    if not isinstance(second_swarms, list) or len(second_swarms) != 1:
        raise TypeError("second Swarm execution status was unavailable")
    started = second_swarms[0].get("invocations_started")
    if not isinstance(started, int):
        raise TypeError("second Swarm invocation count was unavailable")
    limit = started + 1
    control(["set-budget", second_swarm, "--invocation-limit", str(limit)])
    budget_ticket = ticket("capacity-budget-final", second_swarm, "work", 3)
    enqueue(budget_ticket)
    admission = tick().get("admissions")
    if not isinstance(admission, list) or len(admission) != 1:
        raise RuntimeError("final budget slot was not admitted")
    admitted_ticket = admission[0].get("ticket")
    if not isinstance(admitted_ticket, dict):
        raise TypeError("final budget admission omitted its ticket")
    launch_and_settle(admitted_ticket)
    if tick().get("admissions") != []:
        raise RuntimeError("budget-exhausted Swarm admitted another Invocation")
    exhausted = control(["status", "--swarm-id", second_swarm]).get("value")
    exhausted_swarms = exhausted.get("swarms") if isinstance(exhausted, dict) else None
    if (
        not isinstance(exhausted_swarms, list)
        or len(exhausted_swarms) != 1
        or exhausted_swarms[0].get("phase") != "paused"
        or exhausted_swarms[0].get("desired") != "paused"
    ):
        raise RuntimeError("budget exhaustion did not durably pause the Swarm")

    first_effect_status = control(["status", "--swarm-id", first_swarm]).get("value")
    first_effect_swarms = (
        first_effect_status.get("swarms")
        if isinstance(first_effect_status, dict)
        else None
    )
    if not isinstance(first_effect_swarms, list) or len(first_effect_swarms) != 1:
        raise TypeError("effect-recovery Swarm status was unavailable")
    execution_epoch = first_effect_swarms[0].get("execution_epoch")
    if not isinstance(execution_epoch, int) or execution_epoch < 1:
        raise TypeError("effect-recovery execution epoch was unavailable")
    effect_intent = {
        "operation_id": "native-effect-recovery",
        "swarm_id": first_swarm,
        "work_id": "integration",
        "work_revision": 1,
        "owner_member_id": "controlled-native-owner",
        "invocation_id": "native-effect-invocation",
        "attempt_id": "native-effect-attempt",
        "configuration_revision": 1,
        "execution_epoch": execution_epoch,
        "target_id": "native-idempotency-target",
        "request_digest": f"blake3:{'a' * 64}",
    }
    effect_path = persist("effect-intent", effect_intent)
    control(["prepare-effect", "--intent", str(effect_path)])
    control(["mark-effect-dispatched", "native-effect-recovery"])
    control(["observe-effect", "native-effect-recovery", "unknown"])
    blocked_effect = control(["status", "--swarm-id", first_swarm]).get("value")
    blocked_effect_swarms = (
        blocked_effect.get("swarms") if isinstance(blocked_effect, dict) else None
    )
    if (
        not isinstance(blocked_effect_swarms, list)
        or len(blocked_effect_swarms) != 1
        or blocked_effect_swarms[0].get("phase") != "blocked_unknown"
        or blocked_effect_swarms[0].get("unknown_effects") != ["native-effect-recovery"]
    ):
        raise RuntimeError("ambiguous external effect did not block the exact Swarm")
    premature_resume = run(
        [
            str(execution_control),
            "--state",
            str(execution_state),
            "resume",
            first_swarm,
        ],
        cwd=root,
        allow_failure=True,
    )
    if premature_resume.returncode == 0:
        raise RuntimeError("unknown external effect allowed an unsafe Resume")
    control(["observe-effect", "native-effect-recovery", "applied"])
    control(["acknowledge-effect", "native-effect-recovery"])
    control(["resume", first_swarm])
    return {
        "native_shared_capacity": True,
        "native_progress_review_priority": True,
        "native_budget_pause": True,
        "native_effect_recovery": True,
    }


def exercise_reviewed_code_change(
    *,
    app: Path,
    process_guard: Path,
    root: Path,
    work: Path,
    local: list[str],
    swarm_id: str,
    action_ids: ActionIds,
) -> dict[str, bool]:
    """Drive one reviewed code result through local effects and the same Room."""

    state = root / "code-change-state"
    evidence_root = work / "code-check-evidence"
    evidence_root.mkdir()

    def request(name: str, value: dict[str, object]) -> Path:
        path = root / f"code-change-{name}.json"
        path.write_text(
            json.dumps(value, sort_keys=True, separators=(",", ":")),
            encoding="utf-8",
        )
        return path

    def command(operation: list[str]) -> dict[str, object]:
        value = document(
            run(
                [
                    str(app),
                    "code-change",
                    "--working-area",
                    str(work),
                    "--code-change-state",
                    str(state),
                    *operation,
                ],
                cwd=root,
            )
        )
        if not isinstance(value, dict):
            raise TypeError("code-change command returned a non-object receipt")
        return value

    def prepare_and_accept(
        *,
        candidate_id: str,
        target: str,
        expected: str,
        work_id: str,
        resource_id: str,
        contribution_id: str,
        check_revision: int,
    ) -> tuple[str, dict[str, object], list[dict[str, object]]]:
        act(
            app,
            local,
            swarm_id,
            "worker:fixture-a",
            "propose_work_item",
            {
                "dependency_ids": [],
                "description": f"Integrate the isolated {candidate_id} code revision.",
                "expected_goal_revision": 1,
                "kind": "integration",
                "title": f"Integrate {candidate_id}",
                "work_id": work_id,
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            swarm_id,
            "worker:fixture-a",
            "claim_work_item",
            {
                "attempt_id": f"attempt-{work_id}",
                "expected_work_revision": 1,
                "work_id": work_id,
            },
            action_ids,
            root,
            root,
        )
        target_path = work / target
        act(
            app,
            local,
            swarm_id,
            "human",
            "record_resource_version",
            {
                "affected_work_ids": [work_id],
                "digest": "sha256:"
                + hashlib.sha256(target_path.read_bytes()).hexdigest(),
                "expected_previous_version": 0,
                "local_path": str(target_path),
                "resource_id": resource_id,
                "version": 1,
            },
            action_ids,
            root,
            root,
        )
        resource_basis = [{"resource_id": resource_id, "version": 1}]
        prepared = command(
            [
                "prepare",
                "--request",
                str(
                    request(
                        f"{candidate_id}-prepare",
                        {"candidate_id": candidate_id, "targets": [target]},
                    )
                ),
            ]
        )
        editable_root = prepared.get("editable_root")
        if not isinstance(editable_root, str):
            raise TypeError("prepared code Candidate omitted its editable root")
        (Path(editable_root) / target).write_text(expected, encoding="utf-8")
        sealed = command(
            [
                "seal",
                "--request",
                str(
                    request(
                        f"{candidate_id}-seal",
                        {
                            "acceptance_criteria": [ACCEPTANCE_CRITERION],
                            "authors": ["fixture-a"],
                            "candidate_id": candidate_id,
                            "criteria_revision": 1,
                            "inputs": [],
                            "pack_candidate": {
                                "candidate_id": candidate_id,
                                "version": 1,
                            },
                            "resource_basis": resource_basis,
                        },
                    )
                ),
            ]
        )
        revision_digest = sealed.get("revision_digest")
        candidate_artifact = sealed.get("candidate_artifact")
        if not isinstance(revision_digest, str) or not isinstance(
            candidate_artifact, dict
        ):
            raise TypeError("sealed code Candidate omitted its exact revision artifact")
        checked = command(
            [
                "check",
                "--request",
                str(
                    request(
                        f"{candidate_id}-check",
                        {
                            "arguments": [
                                "code-change-check-fixture",
                                "--path",
                                target,
                                "--expected",
                                expected,
                            ],
                            "candidate_id": candidate_id,
                            "check_id": "criterion-1",
                            "evidence_path": (
                                f"code-check-evidence/{candidate_id}-criterion-1.json"
                            ),
                            "environment": {},
                            "pack_check_revision": check_revision,
                            "program": str(app.resolve()),
                            "revision_digest": revision_digest,
                        },
                    )
                ),
                "--process-guard",
                str(process_guard),
            ]
        )
        payload = checked.get("action_payload")
        evidence_refs = (
            payload.get("evidence_refs") if isinstance(payload, dict) else None
        )
        if (
            not isinstance(payload, dict)
            or payload.get("check_id") != "criterion-1"
            or payload.get("status") != "passed"
            or not isinstance(evidence_refs, list)
            or len(evidence_refs) != 1
            or not isinstance(evidence_refs[0], dict)
            or evidence_refs[0].get("candidate_artifact") != candidate_artifact
            or evidence_refs[0].get("criterion") != ACCEPTANCE_CRITERION
        ):
            raise RuntimeError(
                "guarded code Check did not emit canonical Pack evidence"
            )
        act(
            app,
            local,
            swarm_id,
            "worker:fixture-a",
            "submit_contribution",
            {
                "artifact": candidate_artifact,
                "completes_work": True,
                "contribution_id": contribution_id,
                "expected_attempt_revision": 1,
                "expected_work_revision": 2,
                "resource_basis": resource_basis,
                "source_refs": [f"code-change:{revision_digest}"],
                "summary": f"Produced isolated reviewed revision {candidate_id}.",
                "work_id": work_id,
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            swarm_id,
            "worker:fixture-a",
            "submit_candidate",
            {
                "artifact": candidate_artifact,
                "candidate_id": candidate_id,
                "contribution_refs": [
                    {"contribution_id": contribution_id, "version": 1}
                ],
                "expected_work_revision": 3,
                "resource_basis": resource_basis,
                "work_id": work_id,
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            swarm_id,
            "worker:fixture-b",
            "record_check",
            payload,
            action_ids,
            root,
            root,
        )
        reviewed = command(
            [
                "review",
                "--request",
                str(
                    request(
                        f"{candidate_id}-review",
                        {
                            "candidate_id": candidate_id,
                            "findings": [],
                            "pack_review_revision": 1,
                            "review_id": f"review-{candidate_id}",
                            "reviewer_id": "fixture-b",
                            "reviewer_kind": "agent",
                            "revision_digest": revision_digest,
                            "verdict": "pass",
                        },
                    )
                ),
            ]
        )
        if reviewed.get("gate") != "accepted" or not isinstance(
            reviewed.get("accepted"), dict
        ):
            raise RuntimeError(
                "independent code review did not accept exact Check evidence"
            )
        act(
            app,
            local,
            swarm_id,
            "worker:fixture-b",
            "record_review",
            {
                "candidate": {"candidate_id": candidate_id, "version": 1},
                "expected_criteria_revision": 1,
                "findings": [],
                "resource_basis": resource_basis,
                "review_id": f"review-{candidate_id}",
                "verdict": "passed",
            },
            action_ids,
            root,
            root,
        )
        return revision_digest, payload, resource_basis

    applied_target = work / "code-answer.txt"
    applied_target.write_text("original code answer\n", encoding="utf-8")
    applied_digest, applied_check, applied_basis = prepare_and_accept(
        candidate_id="code-candidate",
        target="code-answer.txt",
        expected="reviewed code answer\n",
        work_id="code-change-integration",
        resource_id="code-answer-resource",
        contribution_id="contribution-code-candidate",
        check_revision=1,
    )
    before_acceptance = observe_actor(app, local, swarm_id, "human", root).get(
        "activity"
    )
    direction_revision = (
        before_acceptance.get("direction_revision")
        if isinstance(before_acceptance, dict)
        else None
    )
    if not isinstance(direction_revision, int):
        raise TypeError("code-change Room omitted its current Direction revision")
    act(
        app,
        local,
        swarm_id,
        "worker:fixture-a",
        "request_writeback",
        {
            "candidate": {"candidate_id": "code-candidate", "version": 1},
            "expected_resource_version": 1,
            "operation_id": "writeback-code-candidate",
            "resource_id": "code-answer-resource",
        },
        action_ids,
        root,
        root,
    )
    operation = command(
        [
            "writeback",
            "--request",
            str(
                request(
                    "code-candidate-writeback",
                    {
                        "candidate_id": "code-candidate",
                        "operation_id": "writeback-code-candidate",
                        "revision_digest": applied_digest,
                    },
                )
            ),
        ]
    )
    if operation.get("operation_id") != "writeback-code-candidate":
        raise RuntimeError("code write-back did not retain its operation identity")
    applied = command(["reconcile", "--operation-id", "writeback-code-candidate"])
    if (
        applied.get("disposition") != "applied"
        or applied_target.read_text(encoding="utf-8") != "reviewed code answer\n"
    ):
        raise RuntimeError("reviewed code revision was not safely written back")
    act(
        app,
        local,
        swarm_id,
        "worker:fixture-a",
        "record_writeback_outcome",
        {
            "evidence_refs": [f"code-change:{applied_digest}"],
            "expected_writeback_revision": 1,
            "operation_id": "writeback-code-candidate",
            "resulting_version": 2,
            "status": "applied",
        },
        action_ids,
        root,
        root,
    )

    conflict_target = work / "code-conflict.txt"
    conflict_target.write_text("captured baseline\n", encoding="utf-8")
    conflict_digest, _, _ = prepare_and_accept(
        candidate_id="code-conflict",
        target="code-conflict.txt",
        expected="candidate replacement\n",
        work_id="code-conflict-integration",
        resource_id="code-conflict-resource",
        contribution_id="contribution-code-conflict",
        check_revision=2,
    )
    act(
        app,
        local,
        swarm_id,
        "worker:fixture-a",
        "request_writeback",
        {
            "candidate": {"candidate_id": "code-conflict", "version": 1},
            "expected_resource_version": 1,
            "operation_id": "writeback-code-conflict",
            "resource_id": "code-conflict-resource",
        },
        action_ids,
        root,
        root,
    )
    command(
        [
            "writeback",
            "--request",
            str(
                request(
                    "code-conflict-writeback",
                    {
                        "candidate_id": "code-conflict",
                        "operation_id": "writeback-code-conflict",
                        "revision_digest": conflict_digest,
                    },
                )
            ),
        ]
    )
    conflict_target.write_text("newer human edit\n", encoding="utf-8")
    conflict = command(["reconcile", "--operation-id", "writeback-code-conflict"])
    if (
        conflict.get("disposition") != "blocked_conflict"
        or conflict_target.read_text(encoding="utf-8") != "newer human edit\n"
    ):
        raise RuntimeError("code write-back did not preserve the concurrent edit")
    act(
        app,
        local,
        swarm_id,
        "worker:fixture-a",
        "record_writeback_outcome",
        {
            "evidence_refs": [f"code-change:{conflict_digest}"],
            "expected_writeback_revision": 1,
            "operation_id": "writeback-code-conflict",
            "resulting_version": 1,
            "status": "conflicted",
        },
        action_ids,
        root,
        root,
    )
    act(
        app,
        local,
        swarm_id,
        "human",
        "accept_result",
        {
            "candidate": {"candidate_id": "code-candidate", "version": 1},
            "check_refs": [{"id": "criterion-1", "revision": 1}],
            "expected_direction_revision": direction_revision,
            "result_id": "native-code-change",
            "review_refs": [{"id": "review-code-candidate", "revision": 1}],
        },
        action_ids,
        root,
        root,
    )
    room = observe_actor(app, local, swarm_id, "human", root).get("activity")
    room_results = room.get("results") if isinstance(room, dict) else None
    room_checks = room.get("checks") if isinstance(room, dict) else None
    expected_evidence = applied_check.get("evidence_refs")
    result = next(
        (
            item
            for item in room_results or []
            if isinstance(item, dict) and item.get("result_id") == "native-code-change"
        ),
        None,
    )
    recorded_check = next(
        (
            item
            for item in room_checks or []
            if isinstance(item, dict)
            and item.get("candidate")
            == {"candidate_id": "code-candidate", "version": 1}
        ),
        None,
    )
    if (
        not isinstance(result, dict)
        or result.get("candidate") != {"candidate_id": "code-candidate", "version": 1}
        or result.get("check_refs") != [{"id": "criterion-1", "revision": 1}]
        or result.get("review_refs") != [{"id": "review-code-candidate", "revision": 1}]
        or not isinstance(recorded_check, dict)
        or recorded_check.get("evidence_refs") != expected_evidence
        or recorded_check.get("resource_basis") != applied_basis
    ):
        raise RuntimeError(
            "reviewed code Result did not retain the exact guarded Room evidence"
        )
    status = command(["status"])
    candidates = status.get("candidates")
    write_backs = status.get("write_backs")
    if (
        not isinstance(candidates, list)
        or len(candidates) != 2
        or not isinstance(write_backs, list)
        or len(write_backs) != 2
    ):
        raise RuntimeError("code-change durable status omitted retained workflow state")
    return {
        "reviewed_code_change": True,
        "code_change_conflict_preserved": True,
        "room_code_change_result": True,
    }


def act(
    app: Path,
    local: list[str],
    swarm_id: str,
    actor: str,
    action_type: str,
    payload: dict[str, object],
    action_ids: ActionIds,
    root: Path,
    cwd: Path,
    expected_status: str = "accepted",
) -> tuple[dict[str, object], dict[str, object]]:
    for _ in range(3):
        observed = observe_actor(app, local, swarm_id, actor, cwd)
        action = exact_action(observed, actor, action_type, action_ids.next(), payload)
        receipt = submit_exact(app, local, swarm_id, action, root, cwd)
        if (
            receipt.get("status") == "rejected"
            and receipt.get("code") in {"stale_room_state", "stale_room_head"}
            and receipt.get("may_submit_revised_action") is True
        ):
            continue
        if receipt.get("status") != expected_status:
            raise RuntimeError(
                f"{action_type} expected {expected_status}, got "
                f"{receipt.get('status')} ({receipt.get('code')})"
            )
        return receipt, observed
    raise RuntimeError(f"{action_type} could not acquire a stable Room head")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", type=Path)
    parser.add_argument("--root", type=Path)
    parser.add_argument(
        "--validate-problem",
        type=Path,
        help="validate a custom problem without starting services or models",
    )
    parser.add_argument(
        "--binary-dir",
        type=Path,
        help="verified application bin directory; defaults to target/debug",
    )
    parser.add_argument(
        "--pack",
        type=Path,
        help="already-built exact Pack bundle; skips the source Pack build",
    )
    parser.add_argument(
        "--config",
        type=Path,
        help="exact local WorldStream configuration; defaults to the checkout fixture",
    )
    parser.add_argument(
        "--reopen-existing",
        action="store_true",
        help="reopen the retained state produced by a previous full smoke",
    )
    parser.add_argument(
        "--challenge-report",
        type=Path,
        help="run the bounded deterministic parallel CSV challenge and write its sanitized JSON report",
    )
    parser.add_argument(
        "--autonomy-report",
        type=Path,
        help="run the controlled adaptive-planning protocol scenarios and write their sanitized JSON report",
    )
    parser.add_argument(
        "--live-codex-report",
        type=Path,
        help="explicitly run a bounded real subscription-backed Codex Swarm evaluation",
    )
    parser.add_argument("--codex-path", type=Path)
    parser.add_argument("--live-autonomous-delivery", action="store_true")
    parser.add_argument("--live-baseline-recovery", action="store_true")
    parser.add_argument(
        "--live-problem",
        type=Path,
        help="bounded single-file problem JSON for autonomous delivery",
    )
    parser.add_argument("--live-model")
    parser.add_argument("--live-effort")
    parser.add_argument("--acknowledge-live-moving-alias", action="store_true")
    args = parser.parse_args()
    if args.validate_problem is not None:
        if any(
            (
                args.reopen_existing,
                args.challenge_report,
                args.autonomy_report,
                args.live_codex_report,
                args.live_problem,
                args.live_autonomous_delivery,
                args.live_baseline_recovery,
            )
        ):
            raise ValueError("--validate-problem cannot be combined with a run mode")
        from agent_swarm_problem import load_problem

        print(
            json.dumps(
                {
                    "status": "validated_no_model_or_managed_services",
                    **load_problem(args.validate_problem).summary(),
                },
                indent=2,
            )
        )
        return 0
    if args.workspace is None or args.root is None:
        parser.error("--workspace and --root are required outside --validate-problem")
    if args.live_problem is not None and args.live_codex_report is None:
        raise ValueError("--live-problem requires --live-codex-report")
    if args.live_autonomous_delivery and args.live_codex_report is None:
        raise ValueError("--live-autonomous-delivery requires --live-codex-report")
    if args.live_baseline_recovery and args.live_codex_report is None:
        raise ValueError("--live-baseline-recovery requires --live-codex-report")
    if (
        sum(
            (
                args.live_baseline_recovery,
                args.live_autonomous_delivery,
                args.live_problem is not None,
            )
        )
        > 1
    ):
        raise ValueError("select only one live delivery evaluation")
    if (
        sum(
            (
                args.reopen_existing,
                args.challenge_report is not None,
                args.autonomy_report is not None,
                args.live_codex_report is not None,
            )
        )
        > 1
    ):
        raise ValueError(
            "--reopen-existing, --challenge-report and --autonomy-report are mutually exclusive"
        )
    if args.live_codex_report is not None and not all(
        (
            args.codex_path,
            args.live_model,
            args.live_effort,
            args.acknowledge_live_moving_alias,
        )
    ):
        raise ValueError(
            "live evaluation requires exact Codex path, selected model/effort and deliberate moving-alias acknowledgement"
        )
    if args.live_problem is not None:
        from agent_swarm_problem import load_problem

        problem = load_problem(args.live_problem)
    else:
        problem = None
    workspace = args.workspace.resolve()
    root = args.root.resolve()
    if not root.is_dir() or (not args.reopen_existing and any(root.iterdir())):
        raise RuntimeError("smoke root must be an existing empty directory")

    binary_dir = (
        args.binary_dir.resolve()
        if args.binary_dir is not None
        else workspace / "target/debug"
    )
    control = binary_dir / "worldstreamctl"
    app = binary_dir / "worldstream-agent-swarm"
    process_guard = binary_dir / "worldstream-agent-swarm-process-guard"
    execution_daemon = binary_dir / "worldstream-agent-swarmd"
    execution_control = binary_dir / "worldstream-agent-swarmctl"
    controlled_worker = binary_dir / "worldstream-agent-swarm-controlled-worker"
    if os.name == "nt":
        control = control.with_suffix(".exe")
        app = app.with_suffix(".exe")
        process_guard = process_guard.with_suffix(".exe")
        execution_daemon = execution_daemon.with_suffix(".exe")
        execution_control = execution_control.with_suffix(".exe")
        controlled_worker = controlled_worker.with_suffix(".exe")
    bundle = (
        args.pack.resolve()
        if args.pack is not None
        else workspace
        / "packs/agent-swarm/releases/0.2.0/worldstream-agent-swarm-candidate.wspack"
    )
    config = (
        args.config.resolve()
        if args.config is not None
        else workspace / "config/development.toml"
    )
    if not config.is_file():
        raise RuntimeError("managed smoke configuration is unavailable")
    if args.reopen_existing:
        return reopen_existing_state(
            root=root,
            binary_dir=binary_dir,
            config=config,
        )
    state = root / "studio"
    data = root / "runtime"
    execution_state = root / "execution"
    work = root / "work"
    work.mkdir()
    runtime_address = f"127.0.0.1:{free_port()}"
    controller_address = f"127.0.0.1:{free_port()}"
    common = [
        "--config",
        str(config),
        "--bind",
        runtime_address,
        "--data-dir",
        str(data),
    ]
    managed = common + [
        "--state-dir",
        str(state),
        "--controller",
        controller_address,
        "--timeout-seconds",
        "30",
    ]
    started = False
    start_attempted = False
    daemon_process: subprocess.Popen[str] | None = None
    registered_swarm_ids: list[str] = []
    cleanup_marker = root / "managed-processes-stopped"
    challenge_report: dict[str, object] | None = None

    try:
        daemon_process = subprocess.Popen(
            [
                str(execution_daemon),
                "--state",
                str(execution_state),
                "--process-guard",
                str(process_guard),
            ],
            cwd=root,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        publication = execution_state / "execution-control.v1.json"
        for _ in range(100):
            if publication.is_file():
                break
            if daemon_process.poll() is not None:
                diagnostic = (
                    daemon_process.stderr.read().strip().splitlines()[-1:]
                    if daemon_process.stderr is not None
                    else []
                )
                raise RuntimeError(
                    "execution daemon exited before publication"
                    + (f": {diagnostic[0]}" if diagnostic else "")
                )
            time.sleep(0.05)
        if not publication.is_file():
            raise RuntimeError("execution daemon did not publish its control endpoint")

        if args.pack is None:
            prove_env = dict(os.environ)
            prove_env["WORLDSTREAM_PACK_HOST"] = str(control)
            run(
                [
                    "pnpm",
                    "--dir",
                    str(workspace / "packs/agent-swarm"),
                    "pack:prove",
                ],
                cwd=workspace,
                env=prove_env,
            )
        inspected = document(
            run(
                [str(control), "pack", "inspect", "--bundle", str(bundle)],
                cwd=workspace,
            )
        )
        if not isinstance(inspected, dict) or inspected.get("status") != "complete":
            raise RuntimeError("Agent Swarm Pack production inspection did not pass")
        pack_digest = inspected.get("revision_digest") or inspected.get(
            "revisionDigest"
        )
        bundle_digest = inspected.get("bundle_digest") or inspected.get("bundleDigest")
        if not isinstance(pack_digest, str) or not isinstance(bundle_digest, str):
            raise TypeError("Agent Swarm Pack proof omitted exact identities")

        run([str(control), "init", *managed, "--json"], cwd=root)
        run(
            [
                str(control),
                "pack",
                "approve",
                *common,
                "--bundle",
                str(bundle),
                "--operator-id",
                "native-agent-swarm-smoke",
                "--decided-at",
                "2026-09-15T12:00:00Z",
            ],
            cwd=root,
        )
        run(
            [
                str(control),
                "pack",
                "install",
                *common,
                "--bundle",
                str(bundle),
                "--installed-at",
                "2026-09-15T12:00:01Z",
            ],
            cwd=root,
        )
        run(
            [
                str(control),
                "pack",
                "set-selectable",
                *common,
                "--bundle-digest",
                bundle_digest,
                "--selectable",
                "true",
            ],
            cwd=root,
        )
        run([str(control), "pack", "restart-readiness", *common], cwd=root)

        start_attempted = True
        for _ in range(8):
            start = run(
                [str(control), "server", "start", *managed, "--json"],
                cwd=root,
                allow_failure=True,
            )
            if start.returncode == 0:
                started = True
                break
            run(
                [str(control), "server", "status", *managed, "--json"],
                cwd=root,
                allow_failure=True,
            )
            time.sleep(1)
        if not started:
            raise RuntimeError("managed Controller/Runtime did not become ready")

        local = [
            "--state-dir",
            str(state),
            "--controller-address",
            controller_address,
            "--runtime-address",
            runtime_address,
            "--pack-id",
            "worldstream.agent-swarm",
            "--pack-version",
            "0.2.0",
            "--pack-digest",
            pack_digest,
            "--timeout-ms",
            "30000",
            "--execution-state",
            str(execution_state),
        ]
        if args.challenge_report is not None:
            from agent_swarm_challenge import run_challenge

            challenge_report = run_challenge(
                sys.modules[__name__],
                app=app,
                process_guard=process_guard,
                execution_control=execution_control,
                controlled_worker=controlled_worker,
                root=root,
                work=work,
                local=local,
                execution_state=execution_state,
                registered_swarm_ids=registered_swarm_ids,
            )
            return 0
        if args.autonomy_report is not None:
            from agent_swarm_autonomy import run_autonomy

            challenge_report = run_autonomy(
                sys.modules[__name__],
                app=app,
                process_guard=process_guard,
                execution_control=execution_control,
                execution_daemon=execution_daemon,
                controlled_worker=controlled_worker,
                root=root,
                work=work,
                local=local,
                execution_state=execution_state,
                registered_swarm_ids=registered_swarm_ids,
            )
            return 0
        if args.live_codex_report is not None:
            from agent_swarm_live import run_live

            challenge_report = run_live(
                sys.modules[__name__],
                app=app,
                process_guard=process_guard,
                execution_control=execution_control,
                root=root,
                work=work,
                local=local,
                execution_state=execution_state,
                registered_swarm_ids=registered_swarm_ids,
                model=args.live_model,
                effort=args.live_effort,
                codex=args.codex_path,
                autonomous_delivery=args.live_autonomous_delivery,
                baseline_recovery=args.live_baseline_recovery,
                problem=problem,
            )
            return 0
        requests = []
        for ordinal in (1, 2):
            request = root / f"create-{ordinal}.json"
            request_document = {
                "goal": f"Native managed goal {ordinal}",
                "constraints": ["Use only the isolated smoke working area."],
                "acceptance_criteria": [{"text": ACCEPTANCE_CRITERION}],
                "working_area": str(work),
                "roster": [
                    {
                        "member_key": "fixture-a",
                        "label": "Controlled fixture A",
                        "provider": "controlled",
                        "requested_model": "fixture-v1",
                        "requested_effort": "medium",
                        "configuration_state": "fixture_unavailable",
                    },
                    {
                        "member_key": "fixture-b",
                        "label": "Controlled fixture B",
                        "provider": "controlled",
                        "requested_model": "fixture-v1",
                        "requested_effort": "high",
                        "configuration_state": "fixture_unavailable",
                    },
                ],
            }
            if ordinal == 2:
                # Keep the timer-backed native coordinator proof bounded without
                # changing the first Swarm's longer completion story.
                request_document["progress_review_interval_seconds"] = 5
            request.write_text(
                json.dumps(request_document),
                encoding="utf-8",
            )
            requests.append(request)

        first_script = root / "tui-create-first.json"
        first_script.write_text(
            json.dumps(
                [
                    {"input": "new"},
                    {"input": "open"},
                    {"input": "open"},
                    {"input": "resize", "width": 72, "height": 20},
                    {"input": "quit"},
                ]
            ),
            encoding="utf-8",
        )
        reopen_script = root / "tui-reopen.json"
        reopen_script.write_text(
            json.dumps(
                [
                    {"input": "home"},
                    {"input": "open"},
                    {"input": "resize", "width": 100, "height": 30},
                    {"input": "refresh"},
                    {"input": "quit"},
                ]
            ),
            encoding="utf-8",
        )
        second_script = root / "tui-create-second.json"
        second_script.write_text(
            json.dumps(
                [
                    {"input": "new"},
                    {"input": "open"},
                    {"input": "open"},
                    {"input": "quit"},
                ]
            ),
            encoding="utf-8",
        )

        first_receipt = document(
            run(
                [
                    str(app),
                    "tui",
                    *local,
                    "--request",
                    str(requests[0]),
                    "--script",
                    str(first_script),
                ],
                cwd=root,
            )
        )
        reopen_receipt = document(
            run([str(app), "tui", *local, "--script", str(reopen_script)], cwd=root)
        )
        second_receipt = document(
            run(
                [
                    str(app),
                    "tui",
                    *local,
                    "--request",
                    str(requests[1]),
                    "--script",
                    str(second_script),
                ],
                cwd=root,
            )
        )
        for receipt in (first_receipt, reopen_receipt, second_receipt):
            if (
                not isinstance(receipt, dict)
                or receipt.get("status") != "detached"
                or receipt.get("restored") is not True
                or receipt.get("cursor_shown") is not True
                or not isinstance(receipt.get("draws"), int)
                or receipt["draws"] < 1
            ):
                raise RuntimeError("scripted TUI did not restore the terminal contract")
        if (
            first_receipt.get("created") is not True
            or reopen_receipt.get("created") is not False
            or second_receipt.get("created") is not True
        ):
            raise RuntimeError(
                "scripted TUI create/reopen actions disagreed with the script"
            )

        listing = document(run([str(app), "list", *local], cwd=root))
        if not isinstance(listing, list) or len(listing) != 2:
            raise TypeError("managed list did not return exactly two Swarms")
        summaries = {item.get("swarm_id"): item for item in listing}
        first = summaries.get(first_receipt.get("swarm_id"))
        second = summaries.get(second_receipt.get("swarm_id"))
        if not isinstance(first, dict) or not isinstance(second, dict):
            raise TypeError("scripted TUI Swarms were absent from the managed list")
        if first.get("swarm_id") == second.get("swarm_id") or first.get(
            "room_id"
        ) == second.get("room_id"):
            raise RuntimeError(
                "separate goals did not create distinct Swarms and Rooms"
            )
        if reopen_receipt.get("swarm_id") != first.get("swarm_id"):
            raise RuntimeError("scripted TUI did not reopen the first Swarm")
        if (
            first_receipt.get("room_id") != first.get("room_id")
            or reopen_receipt.get("room_id") != first.get("room_id")
            or second_receipt.get("room_id") != second.get("room_id")
        ):
            raise RuntimeError(
                "scripted TUI Room identities disagreed with the managed list"
            )
        registered_swarm_ids = [str(first["swarm_id"]), str(second["swarm_id"])]
        for swarm_id in registered_swarm_ids:
            run(
                [
                    str(execution_control),
                    "--state",
                    str(execution_state),
                    "resume",
                    swarm_id,
                ],
                cwd=root,
            )
        for created in (first, second):
            reopened = document(
                run(
                    [str(app), "open", *local, "--swarm-id", str(created["swarm_id"])],
                    cwd=root,
                )
            )
            if (
                not isinstance(reopened, dict)
                or reopened.get("room_id") != created.get("room_id")
                or reopened.get("goal") != created.get("goal")
            ):
                raise RuntimeError(
                    "scoped participant reopen did not preserve the Room view"
                )

        # Drive a complete authoritative Pack flow through exact participant
        # offers. This covers duplicate lost-reply recovery, dependent Work,
        # competing claims, attributed artifacts, negative validation,
        # independent review, and completion persistence.
        action_ids = ActionIds()
        first_swarm = str(first["swarm_id"])
        human_observation = observe_actor(app, local, first_swarm, "human", root)
        worker_b_member_id = participant_member_id(
            observe_actor(app, local, first_swarm, "worker:fixture-b", root),
            "fixture-b",
        )
        confirmation = exact_action(
            human_observation,
            "human",
            "confirm_initial_setup",
            action_ids.next(),
            {"setup_revision": 2},
        )
        confirmed = submit_exact(app, local, first_swarm, confirmation, root, root)
        if confirmed.get("status") != "accepted" or confirmed.get("duplicate") is True:
            raise RuntimeError("initial setup confirmation was not freshly accepted")
        duplicate = submit_exact(app, local, first_swarm, confirmation, root, root)
        if (
            duplicate.get("status") != "accepted"
            or duplicate.get("duplicate") is not True
        ):
            raise RuntimeError(
                "lost-reply retry did not return a duplicate accepted receipt"
            )

        worker_a = "worker:fixture-a"
        worker_b = "worker:fixture-b"
        for actor, work_id, title in (
            (worker_a, "source-a", "Prepare source A"),
            (worker_b, "source-b", "Prepare source B"),
        ):
            act(
                app,
                local,
                first_swarm,
                actor,
                "propose_work_item",
                {
                    "dependency_ids": [],
                    "description": f"Produce the attributable {work_id} material.",
                    "expected_goal_revision": 1,
                    "kind": "goal",
                    "title": title,
                    "work_id": work_id,
                },
                action_ids,
                root,
                root,
            )
        act(
            app,
            local,
            first_swarm,
            worker_a,
            "propose_work_item",
            {
                "dependency_ids": ["source-a", "source-b"],
                "description": "Integrate both exact source Contribution versions.",
                "expected_goal_revision": 1,
                "kind": "integration",
                "title": "Integrate report",
                "work_id": "integration",
            },
            action_ids,
            root,
            root,
        )

        blocked_claim, _ = act(
            app,
            local,
            first_swarm,
            worker_b,
            "claim_work_item",
            {
                "attempt_id": "attempt-integration-too-early",
                "expected_work_revision": 1,
                "work_id": "integration",
            },
            action_ids,
            root,
            root,
            expected_status="rejected",
        )
        if blocked_claim.get("code") != "work_ineligible":
            raise RuntimeError(
                "dependent Work rejection reported "
                f"{blocked_claim.get('code')!r}, expected 'work_ineligible'"
            )

        claim_a_observation = observe_actor(app, local, first_swarm, worker_a, root)
        claim_b_observation = observe_actor(app, local, first_swarm, worker_b, root)
        claim_a = exact_action(
            claim_a_observation,
            worker_a,
            "claim_work_item",
            action_ids.next(),
            {
                "attempt_id": "attempt-source-a",
                "expected_work_revision": 1,
                "work_id": "source-a",
            },
        )
        claim_b = exact_action(
            claim_b_observation,
            worker_b,
            "claim_work_item",
            action_ids.next(),
            {
                "attempt_id": "attempt-source-a-race",
                "expected_work_revision": 1,
                "work_id": "source-a",
            },
        )
        won = submit_exact(app, local, first_swarm, claim_a, root, root)
        lost = submit_exact(app, local, first_swarm, claim_b, root, root)
        if won.get("status") != "accepted" or lost.get("status") != "rejected":
            raise RuntimeError(
                "competing Work claim did not produce one authoritative owner"
            )
        if lost.get("code") != "stale_room_state":
            raise RuntimeError("losing exact-Head claim did not report staleness")

        act(
            app,
            local,
            first_swarm,
            worker_b,
            "claim_work_item",
            {
                "attempt_id": "attempt-source-b",
                "expected_work_revision": 1,
                "work_id": "source-b",
            },
            action_ids,
            root,
            root,
        )

        act(
            app,
            local,
            first_swarm,
            worker_a,
            "report_work_blocker",
            {
                "blocker_id": "source-a-blocked",
                "evidence_refs": ["fixture:source-a-unavailable"],
                "expected_work_revision": 2,
                "summary": "Source A is temporarily unavailable.",
                "work_id": "source-a",
            },
            action_ids,
            root,
            root,
        )

        artifacts: dict[str, dict[str, object]] = {}
        for name, text in (
            ("source-a", "Source A evidence\n"),
            ("source-b", "Source B evidence\n"),
            ("integration", "Integrated report using source A and source B.\n"),
        ):
            path = work / f"{name}.txt"
            path.write_text(text, encoding="utf-8")
            artifacts[name] = {
                "artifact_id": f"artifact-{name}",
                "digest": "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest(),
                "local_path": str(path),
                "media_type": "text/plain",
            }

        act(
            app,
            local,
            first_swarm,
            worker_b,
            "submit_contribution",
            {
                "artifact": artifacts["source-b"],
                "completes_work": True,
                "contribution_id": "contribution-source-b",
                "expected_attempt_revision": 1,
                "expected_work_revision": 2,
                "resource_basis": [],
                "source_refs": ["supplied:source-b"],
                "summary": "Independent source B completed while source A was blocked.",
                "work_id": "source-b",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_a,
            "resolve_work_blocker",
            {
                "blocker_id": "source-a-blocked",
                "evidence_refs": ["fixture:source-a-now-available"],
                "expected_blocker_revision": 1,
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_b,
            "handoff_work_item",
            {
                "evidence_refs": ["fixture:old-attempt-terminal"],
                "expected_attempt_revision": 3,
                "expected_work_revision": 4,
                "from_attempt_id": "attempt-source-a",
                "handoff_id": "handoff-source-a",
                "reason": "Continue with an existing eligible roster member.",
                "receiving_member_id": worker_b_member_id,
                "reconciliation": "confirmed_terminal",
                "to_attempt_id": "attempt-source-a-receiver",
                "work_id": "source-a",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_b,
            "submit_contribution",
            {
                "artifact": artifacts["source-a"],
                "completes_work": True,
                "contribution_id": "contribution-source-a",
                "expected_attempt_revision": 1,
                "expected_work_revision": 5,
                "resource_basis": [],
                "source_refs": ["supplied:source-a"],
                "summary": "Source A completed after a reconciled ownership handoff.",
                "work_id": "source-a",
            },
            action_ids,
            root,
            root,
        )

        act(
            app,
            local,
            first_swarm,
            worker_a,
            "claim_work_item",
            {
                "attempt_id": "attempt-integration",
                "expected_work_revision": 1,
                "work_id": "integration",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_a,
            "submit_contribution",
            {
                "artifact": artifacts["integration"],
                "completes_work": True,
                "contribution_id": "contribution-integration",
                "expected_attempt_revision": 1,
                "expected_work_revision": 2,
                "resource_basis": [],
                "source_refs": [
                    "contribution-source-a:1",
                    "contribution-source-b:1",
                ],
                "summary": "Integrated both retained source Contribution versions.",
                "work_id": "integration",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_a,
            "submit_candidate",
            {
                "artifact": artifacts["integration"],
                "candidate_id": "report-candidate",
                "contribution_refs": [
                    {"contribution_id": "contribution-source-a", "version": 1},
                    {"contribution_id": "contribution-source-b", "version": 1},
                    {"contribution_id": "contribution-integration", "version": 1},
                ],
                "expected_work_revision": 3,
                "resource_basis": [],
                "work_id": "integration",
            },
            action_ids,
            root,
            root,
        )

        resolved_artifact = document(
            run(
                [
                    str(app),
                    "artifact",
                    *local,
                    "--swarm-id",
                    first_swarm,
                    "--path",
                    "integration.txt",
                    "--artifact-state",
                    str(root / "artifact-state"),
                ],
                cwd=root,
            )
        )
        if (
            not isinstance(resolved_artifact, dict)
            or resolved_artifact.get("swarm_id") != first_swarm
            or not str(resolved_artifact.get("digest", "")).startswith("blake3:")
            or resolved_artifact.get("byte_length")
            != (work / "integration.txt").stat().st_size
        ):
            raise RuntimeError(
                "Room-scoped artifact resolution did not retain exact bytes"
            )

        premature, _ = act(
            app,
            local,
            first_swarm,
            "human",
            "accept_result",
            {
                "candidate": {"candidate_id": "report-candidate", "version": 1},
                "check_refs": [{"id": "missing-check", "revision": 1}],
                "expected_direction_revision": 0,
                "result_id": "native-report",
                "review_refs": [{"id": "missing-review", "revision": 1}],
            },
            action_ids,
            root,
            root,
            expected_status="rejected",
        )
        if premature.get("code") != "validation_incomplete":
            raise RuntimeError("result without exact Check evidence was not rejected")

        failed_guarded_check = guarded_check_evidence_ref(
            app=app,
            process_guard=process_guard,
            root=root,
            work=work,
            service_candidate_id="report-check-negative",
            pack_candidate_id="report-negative",
            pack_candidate_version=1,
            check_id="criterion-1",
            check_revision=1,
            criteria_revision=1,
            resource_basis=[],
            subject=work / "integration.txt",
            candidate_artifact=artifacts["integration"],
            expected_text="This deliberately wrong report must not pass.\n",
            expected_status="failed",
        )
        if failed_guarded_check.get("status") != "failed":
            raise RuntimeError(
                "wrong supplied-material content passed its guarded Check"
            )

        act(
            app,
            local,
            first_swarm,
            worker_b,
            "record_check",
            {
                "candidate": {"candidate_id": "report-candidate", "version": 1},
                "check_id": "criterion-1",
                "evidence_refs": guarded_check_evidence_ref(
                    app=app,
                    process_guard=process_guard,
                    root=root,
                    work=work,
                    service_candidate_id="report-check-v1",
                    pack_candidate_id="report-candidate",
                    pack_candidate_version=1,
                    check_id="criterion-1",
                    check_revision=1,
                    criteria_revision=1,
                    resource_basis=[],
                    subject=work / "integration.txt",
                    candidate_artifact=artifacts["integration"],
                    expected_text=("Integrated report using source A and source B.\n"),
                )["evidence_refs"],
                "expected_criteria_revision": 1,
                "resource_basis": [],
                "status": "passed",
            },
            action_ids,
            root,
            root,
        )
        self_review, _ = act(
            app,
            local,
            first_swarm,
            worker_a,
            "record_review",
            {
                "candidate": {"candidate_id": "report-candidate", "version": 1},
                "expected_criteria_revision": 1,
                "findings": [],
                "resource_basis": [],
                "review_id": "self-review",
                "verdict": "passed",
            },
            action_ids,
            root,
            root,
            expected_status="rejected",
        )
        if self_review.get("code") != "self_review":
            raise RuntimeError("Candidate author was allowed to self-review")
        act(
            app,
            local,
            first_swarm,
            worker_b,
            "record_review",
            {
                "candidate": {"candidate_id": "report-candidate", "version": 1},
                "expected_criteria_revision": 1,
                "findings": [],
                "resource_basis": [],
                "review_id": "independent-review",
                "verdict": "passed",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_b,
            "propose_work_item",
            {
                "dependency_ids": [],
                "description": "Retain attribution for output that arrives after completion.",
                "expected_goal_revision": 1,
                "kind": "goal",
                "title": "Delayed supporting note",
                "work_id": "late-work",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_b,
            "claim_work_item",
            {
                "attempt_id": "attempt-late-work",
                "expected_work_revision": 1,
                "work_id": "late-work",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            "human",
            "accept_result",
            {
                "candidate": {"candidate_id": "report-candidate", "version": 1},
                "check_refs": [{"id": "criterion-1", "revision": 1}],
                "expected_direction_revision": 0,
                "result_id": "native-report",
                "review_refs": [{"id": "independent-review", "revision": 1}],
            },
            action_ids,
            root,
            root,
        )
        completed = observe_actor(app, local, first_swarm, "human", root)
        activity = completed.get("activity")
        if not isinstance(activity, dict) or activity.get("phase") != "completed":
            raise RuntimeError("accepted reviewed Result did not quiesce the Swarm")
        results = activity.get("results")
        if not isinstance(results, list) or len(results) != 1:
            raise RuntimeError("accepted reviewed Result was not retained")

        # Merely reopening the application is inspection, not a new execution
        # epoch. A delayed owned result remains attributable but cannot mutate
        # the accepted Result. Only an explicit Human Action reopens the same
        # Room; changed input then invalidates old validation and requires a
        # fresh integration, Check, and independent Review.
        inspected_completed = document(
            run(
                [str(app), "open", *local, "--swarm-id", first_swarm],
                cwd=root,
            )
        )
        if not isinstance(inspected_completed, dict) or inspected_completed.get(
            "room_id"
        ) != first.get("room_id"):
            raise RuntimeError("completed Swarm inspection changed Room identity")
        still_completed = observe_actor(app, local, first_swarm, "human", root)
        still_activity = still_completed.get("activity")
        if (
            not isinstance(still_activity, dict)
            or still_activity.get("phase") != "completed"
            or still_activity.get("execution_epoch") != 1
        ):
            raise RuntimeError("inspection implicitly reopened a completed Swarm")

        late_path = work / "late-note.txt"
        late_path.write_text(
            "Delayed note retained after acceptance.\n", encoding="utf-8"
        )
        late_artifact = {
            "artifact_id": "artifact-late-note",
            "digest": "sha256:" + hashlib.sha256(late_path.read_bytes()).hexdigest(),
            "local_path": str(late_path),
            "media_type": "text/plain",
        }
        act(
            app,
            local,
            first_swarm,
            worker_b,
            "record_late_contribution",
            {
                "artifact": late_artifact,
                "attempt_id": "attempt-late-work",
                "expected_work_revision": 3,
                "late_id": "late-note-1",
                "summary": "Arrived after the first exact Result was accepted.",
                "work_id": "late-work",
            },
            action_ids,
            root,
            root,
        )
        after_late = observe_actor(app, local, first_swarm, "human", root)
        after_late_activity = after_late.get("activity")
        if (
            not isinstance(after_late_activity, dict)
            or len(after_late_activity.get("late_contributions", [])) != 1
            or len(after_late_activity.get("results", [])) != 1
            or after_late_activity.get("phase") != "completed"
        ):
            raise RuntimeError("late output changed the accepted completed Result")

        act(
            app,
            local,
            first_swarm,
            "human",
            "reopen_swarm",
            {
                "expected_execution_epoch": 1,
                "reason": "A supplied input changed and needs a new reviewed Result.",
            },
            action_ids,
            root,
            root,
        )
        input_path = work / "reopened-input.txt"
        input_path.write_text("Input revision one.\n", encoding="utf-8")
        act(
            app,
            local,
            first_swarm,
            "human",
            "record_resource_version",
            {
                "affected_work_ids": [],
                "digest": "sha256:"
                + hashlib.sha256(input_path.read_bytes()).hexdigest(),
                "expected_previous_version": 0,
                "local_path": str(input_path),
                "resource_id": "reopened-input",
                "version": 1,
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_a,
            "propose_work_item",
            {
                "dependency_ids": [],
                "description": "Integrate the first reopened input revision.",
                "expected_goal_revision": 1,
                "kind": "integration",
                "title": "Reopened integration revision one",
                "work_id": "reopened-integration-v1",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_a,
            "claim_work_item",
            {
                "attempt_id": "attempt-reopened-v1",
                "expected_work_revision": 1,
                "work_id": "reopened-integration-v1",
            },
            action_ids,
            root,
            root,
        )
        reopened_v1_path = work / "reopened-result-v1.txt"
        reopened_v1_path.write_text(
            "Result from input revision one.\n", encoding="utf-8"
        )
        reopened_v1_artifact = {
            "artifact_id": "artifact-reopened-v1",
            "digest": "sha256:"
            + hashlib.sha256(reopened_v1_path.read_bytes()).hexdigest(),
            "local_path": str(reopened_v1_path),
            "media_type": "text/plain",
        }
        resource_v1 = [{"resource_id": "reopened-input", "version": 1}]
        act(
            app,
            local,
            first_swarm,
            worker_a,
            "submit_contribution",
            {
                "artifact": reopened_v1_artifact,
                "completes_work": True,
                "contribution_id": "reopened-contribution",
                "expected_attempt_revision": 1,
                "expected_work_revision": 2,
                "resource_basis": resource_v1,
                "source_refs": ["reopened-input:1"],
                "summary": "Integrated the first reopened input revision.",
                "work_id": "reopened-integration-v1",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_a,
            "submit_candidate",
            {
                "artifact": reopened_v1_artifact,
                "candidate_id": "report-candidate",
                "contribution_refs": [
                    {"contribution_id": "reopened-contribution", "version": 1}
                ],
                "expected_work_revision": 3,
                "resource_basis": resource_v1,
                "work_id": "reopened-integration-v1",
            },
            action_ids,
            root,
            root,
        )

        input_path.write_text("Input revision two.\n", encoding="utf-8")
        act(
            app,
            local,
            first_swarm,
            "human",
            "record_resource_version",
            {
                "affected_work_ids": ["reopened-integration-v1"],
                "digest": "sha256:"
                + hashlib.sha256(input_path.read_bytes()).hexdigest(),
                "expected_previous_version": 1,
                "local_path": str(input_path),
                "resource_id": "reopened-input",
                "version": 2,
            },
            action_ids,
            root,
            root,
        )
        stale_validation, _ = act(
            app,
            local,
            first_swarm,
            worker_b,
            "record_check",
            {
                "candidate": {"candidate_id": "report-candidate", "version": 2},
                "check_id": "criterion-1",
                "evidence_refs": guarded_check_evidence_ref(
                    app=app,
                    process_guard=process_guard,
                    root=root,
                    work=work,
                    service_candidate_id="report-check-v2-stale",
                    pack_candidate_id="report-candidate",
                    pack_candidate_version=2,
                    check_id="criterion-1",
                    check_revision=1,
                    criteria_revision=1,
                    resource_basis=resource_v1,
                    subject=reopened_v1_path,
                    candidate_artifact=reopened_v1_artifact,
                    expected_text="Result from input revision one.\n",
                )["evidence_refs"],
                "expected_criteria_revision": 1,
                "resource_basis": resource_v1,
                "status": "passed",
            },
            action_ids,
            root,
            root,
            expected_status="rejected",
        )
        if stale_validation.get("code") != "stale_resource_basis":
            raise RuntimeError("changed input did not invalidate old Check evidence")

        act(
            app,
            local,
            first_swarm,
            worker_a,
            "propose_work_item",
            {
                "dependency_ids": [],
                "description": "Recompute after the supplied input changed.",
                "expected_goal_revision": 1,
                "kind": "integration",
                "supersedes_work_id": "reopened-integration-v1",
                "title": "Reopened integration revision two",
                "work_id": "reopened-integration-v2",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_a,
            "claim_work_item",
            {
                "attempt_id": "attempt-reopened-v2",
                "expected_work_revision": 1,
                "work_id": "reopened-integration-v2",
            },
            action_ids,
            root,
            root,
        )
        reopened_v2_path = work / "reopened-result-v2.txt"
        reopened_v2_path.write_text(
            "Result from input revision two.\n", encoding="utf-8"
        )
        reopened_v2_artifact = {
            "artifact_id": "artifact-reopened-v2",
            "digest": "sha256:"
            + hashlib.sha256(reopened_v2_path.read_bytes()).hexdigest(),
            "local_path": str(reopened_v2_path),
            "media_type": "text/plain",
        }
        resource_v2 = [{"resource_id": "reopened-input", "version": 2}]
        act(
            app,
            local,
            first_swarm,
            worker_a,
            "submit_contribution",
            {
                "artifact": reopened_v2_artifact,
                "completes_work": True,
                "contribution_id": "reopened-contribution",
                "expected_attempt_revision": 1,
                "expected_work_revision": 2,
                "resource_basis": resource_v2,
                "source_refs": ["reopened-input:2"],
                "summary": "Recomputed against the changed supplied input.",
                "work_id": "reopened-integration-v2",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_a,
            "submit_candidate",
            {
                "artifact": reopened_v2_artifact,
                "candidate_id": "report-candidate",
                "contribution_refs": [
                    {"contribution_id": "reopened-contribution", "version": 2}
                ],
                "expected_work_revision": 3,
                "resource_basis": resource_v2,
                "work_id": "reopened-integration-v2",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_b,
            "propose_work_item",
            {
                "dependency_ids": [],
                "description": "Correct the reopened Candidate after independent review.",
                "expected_goal_revision": 1,
                "kind": "correction",
                "supersedes_work_id": "reopened-integration-v2",
                "title": "Correct reopened review finding",
                "work_id": "reopened-review-correction",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_b,
            "record_review",
            {
                "candidate": {"candidate_id": "report-candidate", "version": 3},
                "expected_criteria_revision": 1,
                "findings": [
                    {
                        "corrective_work_id": "reopened-review-correction",
                        "finding_id": "reopened-blocking-finding",
                        "severity": "blocking",
                        "summary": "The changed input needs an explicit corrected conclusion.",
                    }
                ],
                "resource_basis": resource_v2,
                "review_id": "reopened-changes-review",
                "verdict": "changes_requested",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_b,
            "claim_work_item",
            {
                "attempt_id": "attempt-reopened-correction",
                "expected_work_revision": 1,
                "work_id": "reopened-review-correction",
            },
            action_ids,
            root,
            root,
        )
        corrected_path = work / "reopened-result-corrected.txt"
        corrected_path.write_text(
            "Corrected result from input revision two.\n", encoding="utf-8"
        )
        corrected_artifact = {
            "artifact_id": "artifact-reopened-corrected",
            "digest": "sha256:"
            + hashlib.sha256(corrected_path.read_bytes()).hexdigest(),
            "local_path": str(corrected_path),
            "media_type": "text/plain",
        }
        act(
            app,
            local,
            first_swarm,
            worker_b,
            "submit_contribution",
            {
                "artifact": corrected_artifact,
                "completes_work": True,
                "contribution_id": "reopened-review-correction",
                "expected_attempt_revision": 1,
                "expected_work_revision": 2,
                "resource_basis": resource_v2,
                "source_refs": ["finding:reopened-blocking-finding"],
                "summary": "Addressed the exact blocking review finding.",
                "work_id": "reopened-review-correction",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            "human",
            "resolve_review_finding",
            {
                "evidence_refs": ["review:independent-disagreement"],
                "expected_finding_revision": 1,
                "finding_id": "reopened-blocking-finding",
                "resolution": "disputed",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_a,
            "submit_candidate",
            {
                "artifact": corrected_artifact,
                "candidate_id": "report-candidate",
                "contribution_refs": [
                    {"contribution_id": "reopened-contribution", "version": 2},
                    {"contribution_id": "reopened-review-correction", "version": 1},
                ],
                "expected_work_revision": 3,
                "resource_basis": resource_v2,
                "work_id": "reopened-integration-v2",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_b,
            "record_check",
            {
                "candidate": {"candidate_id": "report-candidate", "version": 4},
                "check_id": "criterion-1",
                "evidence_refs": guarded_check_evidence_ref(
                    app=app,
                    process_guard=process_guard,
                    root=root,
                    work=work,
                    service_candidate_id="report-check-v4",
                    pack_candidate_id="report-candidate",
                    pack_candidate_version=4,
                    check_id="criterion-1",
                    check_revision=2,
                    criteria_revision=1,
                    resource_basis=resource_v2,
                    subject=corrected_path,
                    candidate_artifact=corrected_artifact,
                    expected_text="Corrected result from input revision two.\n",
                )["evidence_refs"],
                "expected_criteria_revision": 1,
                "resource_basis": resource_v2,
                "status": "passed",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_b,
            "record_review",
            {
                "candidate": {"candidate_id": "report-candidate", "version": 4},
                "expected_criteria_revision": 1,
                "findings": [],
                "resource_basis": resource_v2,
                "review_id": "independent-review",
                "verdict": "passed",
            },
            action_ids,
            root,
            root,
        )
        unresolved_finding_acceptance, _ = act(
            app,
            local,
            first_swarm,
            "human",
            "accept_result",
            {
                "candidate": {"candidate_id": "report-candidate", "version": 4},
                "check_refs": [{"id": "criterion-1", "revision": 2}],
                "expected_direction_revision": 0,
                "result_id": "native-report",
                "review_refs": [{"id": "independent-review", "revision": 2}],
            },
            action_ids,
            root,
            root,
            expected_status="rejected",
        )
        if unresolved_finding_acceptance.get("code") != "blocking_finding":
            raise RuntimeError(
                "a later passing review hid an unresolved blocking Finding"
            )
        act(
            app,
            local,
            first_swarm,
            "human",
            "resolve_review_finding",
            {
                "evidence_refs": ["review:correction-independently-reconciled"],
                "expected_finding_revision": 2,
                "finding_id": "reopened-blocking-finding",
                "resolution": "resolved",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_a,
            "request_writeback",
            {
                "candidate": {"candidate_id": "report-candidate", "version": 4},
                "expected_resource_version": 2,
                "operation_id": "reopened-writeback",
                "resource_id": "reopened-input",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            worker_a,
            "record_writeback_outcome",
            {
                "evidence_refs": ["filesystem:newer-content-preserved"],
                "expected_writeback_revision": 1,
                "operation_id": "reopened-writeback",
                "resulting_version": 2,
                "status": "conflicted",
            },
            action_ids,
            root,
            root,
        )
        conflicted_acceptance, _ = act(
            app,
            local,
            first_swarm,
            "human",
            "accept_result",
            {
                "candidate": {"candidate_id": "report-candidate", "version": 4},
                "check_refs": [{"id": "criterion-1", "revision": 2}],
                "expected_direction_revision": 0,
                "result_id": "native-report",
                "review_refs": [{"id": "independent-review", "revision": 2}],
            },
            action_ids,
            root,
            root,
            expected_status="rejected",
        )
        if conflicted_acceptance.get("code") != "resource_conflict":
            raise RuntimeError("unresolved writeback conflict did not block acceptance")
        act(
            app,
            local,
            first_swarm,
            "human",
            "resolve_work_blocker",
            {
                "blocker_id": "writeback-conflict:reopened-writeback",
                "evidence_refs": ["filesystem:manual-merge-preserved-newer-content"],
                "expected_blocker_revision": 1,
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            first_swarm,
            "human",
            "accept_result",
            {
                "candidate": {"candidate_id": "report-candidate", "version": 4},
                "check_refs": [{"id": "criterion-1", "revision": 2}],
                "expected_direction_revision": 0,
                "result_id": "native-report",
                "review_refs": [{"id": "independent-review", "revision": 2}],
            },
            action_ids,
            root,
            root,
        )
        reopened_completed = observe_actor(app, local, first_swarm, "human", root)
        reopened_activity = reopened_completed.get("activity")
        reopened_results = (
            reopened_activity.get("results")
            if isinstance(reopened_activity, dict)
            else None
        )
        if (
            not isinstance(reopened_activity, dict)
            or reopened_activity.get("phase") != "completed"
            or reopened_activity.get("execution_epoch") != 2
            or not isinstance(reopened_results, list)
            or len(reopened_results) != 2
            or reopened_results[-1].get("version") != 2
        ):
            raise RuntimeError(
                "explicit same-Room reopen did not retain and version reviewed Results"
            )

        # Exercise the other Room as an unfinished dependency/supervision
        # story. A Human Direction interrupts only its dependent closure,
        # unrelated work completes, ownership moves only after reconciliation,
        # and three evidence-free corrections escalate and interrupt the
        # affected active attempt without blocking the independent branch.
        second_swarm = str(second["swarm_id"])
        second_human = observe_actor(app, local, second_swarm, "human", root)
        second_worker_b_member_id = participant_member_id(
            observe_actor(app, local, second_swarm, "worker:fixture-b", root),
            "fixture-b",
        )
        second_confirmation = exact_action(
            second_human,
            "human",
            "confirm_initial_setup",
            action_ids.next(),
            {"setup_revision": 2},
        )
        if (
            submit_exact(app, local, second_swarm, second_confirmation, root, root).get(
                "status"
            )
            != "accepted"
        ):
            raise RuntimeError("second Swarm setup was not confirmed")

        coordinator_arguments = [
            *local,
            "--swarm-id",
            second_swarm,
            "--coordinator-state",
            str(root / "coordinator-state"),
            "--controlled-path",
            str(controlled_worker),
        ]

        # Prove the automatic review bridge against a real managed Room and
        # the persistent execution daemon without staging caller-authored plan
        # JSON. A timer can make the review due, but cap zero must keep it from
        # becoming a provider process. Restore the roster-sized cap before
        # allowing the controlled worker to claim and report the same review.
        run(
            [
                str(execution_control),
                "--state",
                str(execution_state),
                "set-provider-cap",
                "controlled",
                "0",
            ],
            cwd=root,
        )
        automatic_review_id: str | None = None
        automatic_session_selection: dict[str, object] | None = None
        try:
            for _ in range(120):
                cycle = document(
                    run(
                        [str(app), "worker-run", *coordinator_arguments, "--once"],
                        cwd=root,
                    )
                )
                service = cycle.get("service") if isinstance(cycle, dict) else None
                reviews = (
                    service.get("progress_reviews")
                    if isinstance(service, dict)
                    else None
                )
                waiting = next(
                    (
                        review
                        for review in reviews or []
                        if isinstance(review, dict)
                        and review.get("authoritative_status") == "due"
                        and review.get("execution_status") == "waiting_for_capacity"
                    ),
                    None,
                )
                if isinstance(waiting, dict) and isinstance(
                    waiting.get("review_id"), str
                ):
                    invocation_id = waiting.get("invocation_id")
                    if not isinstance(invocation_id, str):
                        raise TypeError(
                            "capacity-blocked Progress Review omitted its retained Invocation"
                        )
                    intent = document(
                        run(
                            [
                                str(app),
                                "worker-intent",
                                *coordinator_arguments,
                                "--invocation-id",
                                invocation_id,
                            ],
                            cwd=root,
                        )
                    )
                    session = (
                        intent.get("session") if isinstance(intent, dict) else None
                    )
                    if not isinstance(session, dict) or not isinstance(
                        session.get("mode"), str
                    ):
                        raise TypeError(
                            "capacity-blocked Progress Review omitted its retained session selection"
                        )
                    automatic_review_id = waiting["review_id"]
                    automatic_session_selection = session
                    break
                time.sleep(0.05)
        finally:
            run(
                [
                    str(execution_control),
                    "--state",
                    str(execution_state),
                    "set-provider-cap",
                    "controlled",
                    "2",
                ],
                cwd=root,
            )
        if automatic_review_id is None or automatic_session_selection is None:
            raise RuntimeError(
                "due Progress Review did not wait at zero provider capacity"
            )

        automatic_claim_observed = False
        automatic_report_observed = False
        for _ in range(120):
            document(
                run(
                    [str(app), "worker-run", *coordinator_arguments, "--once"],
                    cwd=root,
                )
            )
            review_activity = observe_actor(
                app, local, second_swarm, "human", root
            ).get("activity")
            reviews = (
                review_activity.get("outstanding_progress_reviews")
                if isinstance(review_activity, dict)
                else None
            )
            if not isinstance(reviews, list):
                raise TypeError("automatic Progress Review state was unavailable")
            matching_review = next(
                (
                    review
                    for review in reviews
                    if isinstance(review, dict)
                    and review.get("review_id") == automatic_review_id
                ),
                None,
            )
            if isinstance(matching_review, dict):
                automatic_claim_observed = (
                    automatic_claim_observed
                    or matching_review.get("status") == "claimed"
                )
            elif automatic_claim_observed:
                automatic_report_observed = True
                break
            time.sleep(0.05)
        if not automatic_claim_observed or not automatic_report_observed:
            raise RuntimeError(
                "controlled coordinator did not automatically claim and report the due review"
            )

        # Prove the production coordinator/daemon/process-guard seam with a
        # real controlled Invocation rather than manufacturing every Action in
        # the smoke driver. The exact Work Attempt target fences the proposal.
        act(
            app,
            local,
            second_swarm,
            worker_a,
            "propose_work_item",
            {
                "dependency_ids": [],
                "description": "Produce one contribution through the persistent coordinator.",
                "expected_goal_revision": 1,
                "kind": "goal",
                "title": "Coordinator-owned contribution",
                "work_id": "coordinator-work",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            second_swarm,
            worker_a,
            "claim_work_item",
            {
                "attempt_id": "attempt-coordinator-work",
                "expected_work_revision": 1,
                "work_id": "coordinator-work",
            },
            action_ids,
            root,
            root,
        )
        coordinator_observation = observe_actor(
            app, local, second_swarm, worker_a, root
        )
        coordinator_activity = coordinator_observation.get("activity")
        if not isinstance(coordinator_activity, dict):
            raise TypeError("coordinator Work observation omitted Pack activity")
        work_item = next(
            (
                item
                for item in coordinator_activity.get("work_items", [])
                if isinstance(item, dict) and item.get("work_id") == "coordinator-work"
            ),
            None,
        )
        attempt = next(
            (
                item
                for item in coordinator_activity.get("work_attempts", [])
                if isinstance(item, dict)
                and item.get("attempt_id") == "attempt-coordinator-work"
            ),
            None,
        )
        roster_member = next(
            (
                item
                for item in coordinator_activity.get("roster", [])
                if isinstance(item, dict) and item.get("member_key") == "fixture-a"
            ),
            None,
        )
        execution_epoch = coordinator_activity.get("execution_epoch")
        if (
            not isinstance(work_item, dict)
            or not isinstance(attempt, dict)
            or not isinstance(roster_member, dict)
            or not isinstance(execution_epoch, int)
        ):
            raise TypeError("coordinator Work target was not authoritative")
        contribution_payload = {
            "artifact": {
                "artifact_id": "artifact-coordinator-work",
                "local_path": "coordinator-work.txt",
                "media_type": "text/plain",
            },
            "action_type": "submit_contribution",
            "payload": {
                "completes_work": True,
                "contribution_id": "contribution-coordinator-work",
                "expected_attempt_revision": attempt["revision"],
                "expected_work_revision": work_item["revision"],
                "resource_basis": [],
                "source_refs": ["controlled:coordinator"],
                "summary": "Controlled worker contribution through the real coordinator.",
                "work_id": "coordinator-work",
            },
            "schema": "worldstream/agent-swarm-action-proposal@1",
        }
        plan = {
            "allowed_action_types": ["submit_contribution"],
            "allowed_tools": [],
            "configuration_revision": roster_member["configuration_revision"],
            "due_sequence": 1,
            "effort": roster_member.get("requested_effort"),
            "instruction": json.dumps(
                contribution_payload, sort_keys=True, separators=(",", ":")
            ),
            "invocation_id": "invocation-coordinator-work",
            "kind": "work",
            "member_key": "fixture-a",
            "model": roster_member["requested_model"],
            "moving_alias_acknowledged": roster_member["moving_alias_acknowledged"],
            "provider": "controlled",
            "resource_policy": "workspace_write",
            "semantic_target": {
                "kind": "work_attempt",
                "execution_epoch": execution_epoch,
                "work_id": "coordinator-work",
                "work_revision": work_item["revision"],
                "attempt_id": "attempt-coordinator-work",
                "attempt_revision": attempt["revision"],
            },
            # Session selection is part of the member configuration revision.
            # Automatic supervision was the first Invocation for this member,
            # so later work at the same revision must reuse its retained local
            # selection rather than inventing a conflicting session identity.
            "session": automatic_session_selection,
            "swarm_id": second_swarm,
        }
        plan_path = root / "coordinator-plan.json"
        plan_path.write_text(
            json.dumps(plan, sort_keys=True, separators=(",", ":")),
            encoding="utf-8",
        )
        run(
            [
                str(app),
                "worker-stage",
                *coordinator_arguments,
                "--plan",
                str(plan_path),
            ],
            cwd=root,
        )
        run(
            [str(app), "worker-dispatch", *coordinator_arguments],
            cwd=root,
        )
        coordinator_settled = False
        for _ in range(40):
            run(
                [str(app), "worker-harvest", *coordinator_arguments],
                cwd=root,
            )
            intent = document(
                run(
                    [
                        str(app),
                        "worker-intent",
                        *coordinator_arguments,
                        "--invocation-id",
                        "invocation-coordinator-work",
                    ],
                    cwd=root,
                )
            )
            if isinstance(intent, dict) and intent.get("state") == "settled":
                submission = intent.get("submission")
                receipt = (
                    submission.get("value") if isinstance(submission, dict) else None
                )
                if (
                    not isinstance(submission, dict)
                    or submission.get("status") != "received"
                    or not isinstance(receipt, dict)
                    or receipt.get("status") != "accepted"
                ):
                    raise RuntimeError(
                        "controlled coordinator Invocation settled without accepted Action"
                    )
                coordinator_settled = True
                break
            time.sleep(0.05)
        if not coordinator_settled:
            raise RuntimeError("controlled coordinator Invocation did not settle")
        coordinator_after = observe_actor(app, local, second_swarm, "human", root).get(
            "activity"
        )
        coordinator_contributions = (
            coordinator_after.get("contributions")
            if isinstance(coordinator_after, dict)
            else None
        )
        if not isinstance(coordinator_contributions, list) or not any(
            isinstance(item, dict)
            and item.get("contribution_id") == "contribution-coordinator-work"
            for item in coordinator_contributions
        ):
            raise RuntimeError(
                "coordinator-produced Contribution was not an authoritative Room fact"
            )

        for work_id, dependencies in (
            ("directed-branch", []),
            ("independent-branch", []),
            ("dependent-deliverable", ["directed-branch"]),
        ):
            act(
                app,
                local,
                second_swarm,
                worker_a,
                "propose_work_item",
                {
                    "dependency_ids": dependencies,
                    "description": f"Native dependency scenario for {work_id}.",
                    "expected_goal_revision": 1,
                    "kind": "integration"
                    if work_id == "dependent-deliverable"
                    else "goal",
                    "title": work_id.replace("-", " ").title(),
                    "work_id": work_id,
                },
                action_ids,
                root,
                root,
            )
        cycle, _ = act(
            app,
            local,
            second_swarm,
            worker_a,
            "revise_work_dependencies",
            {
                "dependency_ids": ["dependent-deliverable"],
                "expected_work_revision": 1,
                "work_id": "directed-branch",
            },
            action_ids,
            root,
            root,
            expected_status="rejected",
        )
        if cycle.get("code") != "dependency_cycle":
            raise RuntimeError("cyclic dependency plan was not rejected")

        act(
            app,
            local,
            second_swarm,
            worker_b,
            "submit_suggestion",
            {
                "suggestion_id": "advisory-shortcut",
                "summary": "Skip evidence collection for the directed branch.",
                "target_scope": "work",
                "target_work_ids": ["directed-branch"],
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            second_swarm,
            "human",
            "record_suggestion_disposition",
            {
                "disposition": "rejected",
                "expected_suggestion_revision": 1,
                "reason": "The accepted evidence requirement remains binding.",
                "suggestion_id": "advisory-shortcut",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            second_swarm,
            worker_a,
            "claim_work_item",
            {
                "attempt_id": "attempt-directed-before-steering",
                "expected_work_revision": 1,
                "work_id": "directed-branch",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            second_swarm,
            "human",
            "issue_direction",
            {
                "direction_id": "revalidate-directed-branch",
                "expected_direction_revision": 0,
                "instruction": "Retain source citations and revalidate this branch.",
                "target_scope": "work",
                "target_work_ids": ["directed-branch"],
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            second_swarm,
            worker_b,
            "claim_work_item",
            {
                "attempt_id": "attempt-independent-branch",
                "expected_work_revision": 1,
                "work_id": "independent-branch",
            },
            action_ids,
            root,
            root,
        )
        independent_path = work / "independent-branch.txt"
        independent_path.write_text(
            "Independent branch completed during steering.\n", encoding="utf-8"
        )
        independent_artifact = {
            "artifact_id": "artifact-independent-branch",
            "digest": "sha256:"
            + hashlib.sha256(independent_path.read_bytes()).hexdigest(),
            "local_path": str(independent_path),
            "media_type": "text/plain",
        }
        act(
            app,
            local,
            second_swarm,
            worker_b,
            "submit_contribution",
            {
                "artifact": independent_artifact,
                "completes_work": True,
                "contribution_id": "contribution-independent-branch",
                "expected_attempt_revision": 1,
                "expected_work_revision": 2,
                "resource_basis": [],
                "source_refs": ["fixture:independent"],
                "summary": "Unaffected work continued while the directed branch stopped.",
                "work_id": "independent-branch",
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            second_swarm,
            "human",
            "resolve_work_blocker",
            {
                "blocker_id": "direction:revalidate-directed-branch",
                "evidence_refs": ["human:direction-acknowledged"],
                "expected_blocker_revision": 1,
            },
            action_ids,
            root,
            root,
        )
        act(
            app,
            local,
            second_swarm,
            worker_b,
            "handoff_work_item",
            {
                "evidence_refs": ["process:prior-owner-terminal"],
                "expected_attempt_revision": 2,
                "expected_work_revision": 4,
                "from_attempt_id": "attempt-directed-before-steering",
                "handoff_id": "handoff-directed-branch",
                "reason": "Resume under the receiving member's retained settings.",
                "receiving_member_id": second_worker_b_member_id,
                "reconciliation": "confirmed_terminal",
                "to_attempt_id": "attempt-directed-receiver",
                "work_id": "directed-branch",
            },
            action_ids,
            root,
            root,
        )

        correction_ids = ["realign-1", "realign-2", "realign-3"]
        for index, correction_id in enumerate(correction_ids, start=1):
            act(
                app,
                local,
                second_swarm,
                worker_a,
                "propose_work_item",
                {
                    "dependency_ids": [],
                    "description": f"Bounded realignment attempt {index}.",
                    "expected_goal_revision": 1,
                    "kind": "correction",
                    "title": f"Realignment {index}",
                    "work_id": correction_id,
                },
                action_ids,
                root,
                root,
            )
            act(
                app,
                local,
                second_swarm,
                worker_a,
                "claim_work_item",
                {
                    "attempt_id": f"attempt-{correction_id}",
                    "expected_work_revision": 1,
                    "work_id": correction_id,
                },
                action_ids,
                root,
                root,
            )
            correction_path = work / f"{correction_id}.txt"
            correction_path.write_text(
                f"Attempt {index} produced no goal-relevant evidence.\n",
                encoding="utf-8",
            )
            act(
                app,
                local,
                second_swarm,
                worker_a,
                "submit_contribution",
                {
                    "artifact": {
                        "artifact_id": f"artifact-{correction_id}",
                        "digest": "sha256:"
                        + hashlib.sha256(correction_path.read_bytes()).hexdigest(),
                        "local_path": str(correction_path),
                        "media_type": "text/plain",
                    },
                    "completes_work": True,
                    "contribution_id": f"contribution-{correction_id}",
                    "expected_attempt_revision": 1,
                    "expected_work_revision": 2,
                    "resource_basis": [],
                    "source_refs": [],
                    "summary": "Correction attempt retained for bounded escalation.",
                    "work_id": correction_id,
                },
                action_ids,
                root,
                root,
            )

        supervision_observation = observe_actor(
            app, local, second_swarm, worker_a, root
        )
        supervision_activity = supervision_observation.get("activity")
        reviews = (
            supervision_activity.get("outstanding_progress_reviews")
            if isinstance(supervision_activity, dict)
            else None
        )
        if not isinstance(reviews, list):
            raise TypeError("Direction did not expose a Progress Review obligation")
        matching_reviews = [
            review
            for review in reviews
            if isinstance(review, dict)
            and review.get("scope") == "work"
            and "directed-branch" in review.get("target_work_ids", [])
        ]
        if len(matching_reviews) != 1:
            raise RuntimeError("Direction did not retain one bounded Progress Review")
        progress_review = matching_reviews[0]
        progress_work_id = progress_review.get("work_id")
        progress_review_id = progress_review.get("review_id")
        if not isinstance(progress_work_id, str) or not isinstance(
            progress_review_id, str
        ):
            raise TypeError("Progress Review omitted stable identities")
        act(
            app,
            local,
            second_swarm,
            worker_a,
            "claim_work_item",
            {
                "attempt_id": "attempt-progress-review",
                "expected_work_revision": 1,
                "work_id": progress_work_id,
            },
            action_ids,
            root,
            root,
        )
        claimed_review_observation = observe_actor(
            app, local, second_swarm, worker_a, root
        )
        claimed_activity = claimed_review_observation.get("activity")
        claimed_reviews = (
            claimed_activity.get("outstanding_progress_reviews")
            if isinstance(claimed_activity, dict)
            else None
        )
        claimed_review = next(
            (
                review
                for review in claimed_reviews or []
                if isinstance(review, dict)
                and review.get("review_id") == progress_review_id
            ),
            None,
        )
        if not isinstance(claimed_review, dict) or not isinstance(
            claimed_review.get("revision"), int
        ):
            raise TypeError("claimed Progress Review revision was unavailable")
        act(
            app,
            local,
            second_swarm,
            worker_a,
            "report_progress_review",
            {
                "assessment": "realignment_required",
                "corrective_work_ids": correction_ids,
                "evidence_refs": [],
                "expected_review_revision": claimed_review["revision"],
                "problem_id": "directed-branch-stall",
                "problem_scope": "work",
                "review_id": progress_review_id,
                "summary": "The directed dependency branch remains unproductive.",
            },
            action_ids,
            root,
            root,
        )
        for expected_revision, correction_id in enumerate(correction_ids, start=1):
            act(
                app,
                local,
                second_swarm,
                worker_a,
                "record_correction_outcome",
                {
                    "evidence_refs": [],
                    "expected_problem_revision": expected_revision,
                    "outcome": "unsuccessful",
                    "problem_id": "directed-branch-stall",
                    "work_id": correction_id,
                },
                action_ids,
                root,
                root,
            )
        escalated = observe_actor(app, local, second_swarm, "human", root)
        escalated_activity = escalated.get("activity")
        escalated_work = (
            escalated_activity.get("work_items")
            if isinstance(escalated_activity, dict)
            else None
        )
        escalated_problems = (
            escalated_activity.get("problems")
            if isinstance(escalated_activity, dict)
            else None
        )
        if not isinstance(escalated_work, list) or not isinstance(
            escalated_problems, list
        ):
            raise TypeError("escalated supervision state was unavailable")
        work_by_id = {
            item.get("work_id"): item
            for item in escalated_work
            if isinstance(item, dict)
        }
        problem = next(
            (
                item
                for item in escalated_problems
                if isinstance(item, dict)
                and item.get("problem_id") == "directed-branch-stall"
            ),
            None,
        )
        if (
            not isinstance(problem, dict)
            or problem.get("status") != "escalated"
            or problem.get("failed_corrections") != 3
            or work_by_id.get("directed-branch", {}).get("status") != "blocked"
            or work_by_id.get("dependent-deliverable", {}).get("status") != "blocked"
            or work_by_id.get("independent-branch", {}).get("status") != "completed"
        ):
            raise RuntimeError(
                "bounded realignment did not isolate and escalate the affected branch"
            )

        inventory = document(
            run([str(control), "room", "list", *managed, "--json"], cwd=root)
        )
        inventory_page = inventory.get("rooms") if isinstance(inventory, dict) else None
        rooms = (
            inventory_page.get("rooms") if isinstance(inventory_page, dict) else None
        )
        if not isinstance(rooms, list) or len(rooms) != 2:
            raise RuntimeError(
                "operator Room inventory did not contain exactly two Rooms"
            )
        if {room.get("room_id") for room in rooms} != {
            first.get("room_id"),
            second.get("room_id"),
        }:
            raise RuntimeError(
                "operator Room inventory identities disagree with Agent Swarm"
            )
        if any(room.get("pack", {}).get("digest") != pack_digest for room in rooms):
            raise RuntimeError(
                "operator Room inventory did not bind the exact Agent Swarm Pack"
            )

        forbidden_keys = {"bearer", "token", "secret"}
        for path in (state / "agent-swarm").glob("*.json"):
            retained = json.loads(path.read_text(encoding="utf-8"))
            pending = [retained]
            while pending:
                value = pending.pop()
                if isinstance(value, dict):
                    if forbidden_keys.intersection(key.lower() for key in value):
                        raise RuntimeError(
                            "application index persisted credential material"
                        )
                    pending.extend(value.values())
                elif isinstance(value, list):
                    pending.extend(value)

        final_view = document(
            run([str(app), "open", *local, "--swarm-id", first_swarm], cwd=root)
        )
        if not isinstance(final_view, dict) or not isinstance(
            final_view.get("roster"), list
        ):
            raise TypeError("final managed Swarm view omitted its roster")
        update_state = {
            "schema": UPDATE_STATE_SCHEMA,
            "swarm_id": first_swarm,
            "room_id": first["room_id"],
            "goal": first["goal"],
            "pack_id": "worldstream.agent-swarm",
            "pack_version": "0.2.0",
            "pack_digest": pack_digest,
            "artifact_path": "integration.txt",
            "authoritative_artifact_digest": resolved_artifact["authoritative_digest"],
            "runtime_address": runtime_address,
            "controller_address": controller_address,
            "roster": final_view["roster"],
        }
        (root / UPDATE_STATE_FILE).write_text(
            json.dumps(update_state, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
        code_change = exercise_reviewed_code_change(
            app=app,
            process_guard=process_guard,
            root=root,
            work=work,
            local=local,
            swarm_id=second_swarm,
            action_ids=action_ids,
        )
        capacity = exercise_native_shared_capacity(
            app=app,
            execution_control=execution_control,
            execution_state=execution_state,
            controlled_worker=controlled_worker,
            root=root,
            work=work,
            first_swarm=first_swarm,
            second_swarm=second_swarm,
        )

        print(
            json.dumps(
                {
                    "status": "ok",
                    "backend": "managed_local",
                    "rooms": 2,
                    "reopened": True,
                    "reviewed_result": True,
                    "reopened_reviewed_result": True,
                    "late_output_fenced": True,
                    "changed_input_revalidated": True,
                    "review_correction_revalidated": True,
                    "review_dispute_visible": True,
                    "resource_conflict_reconciled": True,
                    "human_steering_reassigned": True,
                    "progress_review_claimed": True,
                    "automatic_progress_review": True,
                    "automatic_progress_review_waited_for_capacity": True,
                    "bounded_realignments_escalated": True,
                    "competing_claim": True,
                    "duplicate_retry": True,
                    "artifact_resolved": True,
                    "coordinator_worker_contribution": True,
                    "guarded_report_check": True,
                    **code_change,
                    **capacity,
                },
                separators=(",", ":"),
            )
        )
        return 0
    finally:
        cleanup_failures: list[str] = []
        if daemon_process is not None and daemon_process.poll() is None:
            for swarm_id in registered_swarm_ids:
                try:
                    stopped_swarm = run(
                        [
                            str(execution_control),
                            "--state",
                            str(execution_state),
                            "stop",
                            swarm_id,
                        ],
                        cwd=root,
                        allow_failure=True,
                    )
                except OSError as error:
                    cleanup_failures.append(
                        f"execution Stop could not run for retained Swarm {swarm_id}: {error}"
                    )
                    continue
                if stopped_swarm.returncode:
                    cleanup_failures.append(
                        f"execution Stop failed for retained Swarm {swarm_id}"
                    )
        if start_attempted:
            try:
                stopped = run(
                    [str(control), "server", "stop", *managed, "--json"],
                    cwd=root,
                    allow_failure=True,
                )
                controller_stopped = run(
                    [str(control), "server", "controller-stop", *managed, "--json"],
                    cwd=root,
                    allow_failure=True,
                )
            except OSError as error:
                cleanup_failures.append(
                    f"managed Controller/Runtime cleanup could not run: {error}"
                )
            else:
                if (
                    not managed_stop_proven(stopped)
                    or not managed_stop_proven(controller_stopped)
                ) and not wait_for_retained_managed_stop(
                    control=control, managed=managed, cwd=root
                ):
                    cleanup_failures.append(
                        "managed Controller/Runtime cleanup was not proven"
                    )
        if daemon_process is not None and daemon_process.poll() is None:
            try:
                daemon_process.terminate()
                daemon_process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                try:
                    daemon_process.kill()
                    daemon_process.wait(timeout=10)
                except OSError as error:
                    cleanup_failures.append(
                        f"execution daemon kill failed during cleanup: {error}"
                    )
            except OSError as error:
                cleanup_failures.append(
                    f"execution daemon termination failed during cleanup: {error}"
                )
        if daemon_process is not None and daemon_process.poll() is None:
            cleanup_failures.append("execution daemon cleanup was not proven")
        if daemon_process is not None and daemon_process.stderr is not None:
            try:
                daemon_process.stderr.close()
            except OSError as error:
                cleanup_failures.append(
                    f"execution daemon diagnostics could not be closed: {error}"
                )
        finish_cleanup(
            root=root, marker=cleanup_marker, cleanup_failures=cleanup_failures
        )
        if challenge_report is not None and sys.exception() is None:
            challenge_report["managed_processes_stopped"] = True
            report_path = (
                args.challenge_report or args.autonomy_report or args.live_codex_report
            ).resolve()
            report_path.parent.mkdir(parents=True, exist_ok=True)
            report_path.write_text(
                json.dumps(challenge_report, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            print(json.dumps(challenge_report, separators=(",", ":")))


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, TypeError, ValueError) as error:
        print(f"Agent Swarm managed native smoke failed: {error}", file=sys.stderr)
        for note in getattr(error, "__notes__", ()):
            print(f"Additional cleanup context: {note}", file=sys.stderr)
        raise SystemExit(1)

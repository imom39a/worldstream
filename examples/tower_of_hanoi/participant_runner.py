"""One long-lived local Runner supervisor for autonomous Hanoi Participants.

The supervisor claims Pack-issued Attention and renews the Activation lease. A
Codex Participant runs in its isolated child process; direct JEV/OpenRouter
Participants use the same snapshot and action protocol in this server-side
process. Provider output selects only from code-enumerated candidates, while
WorldStream remains the authority that accepts or rejects each Action.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import math
import os
import random
import shutil
import signal
import stat
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
from collections.abc import Callable
from pathlib import Path
from typing import Any

try:
    import fcntl
except ImportError:  # pragma: no cover - Windows has no fcntl
    fcntl = None

from worldstream_sdk import LostRunnerReply, ProtocolError

from examples.cli_activity.credentials import (
    CredentialError,
    load_membership,
    load_runner,
    sdk_base_url,
    validate_pair,
)
from examples.tower_of_hanoi import turn
from examples.tower_of_hanoi.local_harness import (
    codex_command,
    local_worldstream_environment,
)
from examples.tower_of_hanoi.participant_context import (
    ParticipantContext,
    load_checkpoint,
)
from examples.tower_of_hanoi.protocol import (
    ACTIONS,
    DECISION_REASONS,
    PACK_ID,
    SOLVER_STATE_SCHEMA,
    HanoiProtocolError,
    activity_from_projection,
    board_fingerprint,
    decision_from_candidate,
    json_results_in,
    redact_text,
    resolve_solver_decision,
    safe_activity,
)

ATTENTION_REASONS = frozenset(("board_changed", "claim_review_requested"))
LEASE_MS = 30_000
RENEW_INTERVAL_SECONDS = 10.0
POLL_SECONDS = 0.35
# A failed or rate-limited bootstrap would leave a Room with no first action and
# no later activation to re-wake it, so retry that one turn a bounded few times.
BOOTSTRAP_ATTEMPTS = 3
BOOTSTRAP_RETRY_SECONDS = 1.5
# A mid-run provider error leaves the Room with no action and no new activation
# to re-wake a lone solver, so keep retrying the same head with capped
# exponential backoff and jitter until the run deadline. An upstream outage can
# last minutes, so a fixed small attempt count is not enough.
ACTIVATION_RETRY_SECONDS = 1.0
ACTIVATION_RETRY_MAX_SECONDS = 30.0
# After this many failed attempts on the primary model, fall through to the
# configured fallback models so a sustained provider outage still makes progress.
ACTIVATION_PRIMARY_ATTEMPTS = 3
# A cheap model can be routed to a slow upstream provider and take tens of
# seconds per reply. Ask OpenRouter for its lowest-latency provider and bound
# the call so a hung route becomes a retryable error instead of eating the run.
OPENROUTER_PROVIDER_SORT = "latency"
OPENROUTER_CALL_TIMEOUT_SECONDS = 30.0
JEV_CALL_TIMEOUT_SECONDS = 20.0
# Serialized local action grants: one acting member per Room head.
GRANT_LEASE_MS = 30_000
GRANT_SETTLE_SECONDS = 0.75


class ActionGrant:
    """Elect one acting local supervisor per exact Room head.

    The local harness has no gateway that serializes grants, so every solver can
    claim an activation for the same head, invoke a provider, and lose almost
    all of them as stale. This file lock elects exactly one acting member per
    ``room_seq`` with a lease, so a slow or crashed holder cannot block the Room
    forever. It grants no authority: the elected action still passes ordinary
    Pack admission and may be rejected normally. It is a no-op when ``fcntl`` is
    unavailable.
    """

    def __init__(self, path: Path, *, lease_ms: int = GRANT_LEASE_MS) -> None:
        self.path = path
        self.lease_ms = lease_ms
        self.held_seq: int | None = None

    @staticmethod
    def _now_ms() -> int:
        return int(time.time() * 1000)

    def _lock_descriptor(self) -> int:
        lock_path = self.path.with_name(self.path.name + ".lock")
        return os.open(lock_path, os.O_WRONLY | os.O_CREAT, 0o600)

    def _read(self) -> dict[str, Any] | None:
        try:
            value = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return None
        return value if isinstance(value, dict) else None

    def _write(self, record: dict[str, object]) -> None:
        temporary = self.path.with_name(self.path.name + ".tmp")
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            output.write(json.dumps(record, sort_keys=True, separators=(",", ":")))
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, self.path)

    def acquire(self, room_seq: int, member_id: str) -> bool:
        """Try to become the acting member for ``room_seq``."""
        if fcntl is None:
            return True
        descriptor = self._lock_descriptor()
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            record = self._read()
            now = self._now_ms()
            current = record.get("room_seq") if isinstance(record, dict) else None
            if not isinstance(current, int) or current < room_seq:
                self._write(
                    {
                        "room_seq": room_seq,
                        "member_id": member_id,
                        "state": "leased",
                        "expires_at_ms": now + self.lease_ms,
                    }
                )
                self.held_seq = room_seq
                return True
            if current > room_seq:
                return False
            if record.get("state") == "acted":
                return False
            if record.get("member_id") == member_id:
                self.held_seq = room_seq
                return True
            expires = record.get("expires_at_ms")
            if isinstance(expires, int) and not isinstance(expires, bool) and expires > now:
                return False
            self._write(
                {
                    "room_seq": room_seq,
                    "member_id": member_id,
                    "state": "leased",
                    "expires_at_ms": now + self.lease_ms,
                }
            )
            self.held_seq = room_seq
            return True
        finally:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
            os.close(descriptor)

    def mark_acted(self) -> None:
        """Record that the elected member finished with this head."""
        if fcntl is None or self.held_seq is None:
            return
        descriptor = self._lock_descriptor()
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            record = self._read()
            if isinstance(record, dict) and record.get("room_seq") == self.held_seq:
                record["state"] = "acted"
                record["expires_at_ms"] = self._now_ms() + self.lease_ms
                self._write(record)
        finally:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
            os.close(descriptor)
        self.held_seq = None

    def clear(self) -> None:
        """Drop the grant so another member may try the same head."""
        if fcntl is None:
            return
        descriptor = self._lock_descriptor()
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            try:
                self.path.unlink()
            except OSError:
                pass
        finally:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
            os.close(descriptor)
        self.held_seq = None


class ModelLadder:
    """Shared, file-coordinated model fallback so both Rooms stay in step.

    A per-solver fallback would let one Room drift onto a different model than
    its peer, which breaks the comparison. When a shared path is configured,
    both Rooms read the same ladder index and a promotion on either side moves
    both. The index only ever advances, and an advance is applied once per
    index even if both sides request it.
    """

    def __init__(self, path: Path, models: list[str]) -> None:
        self.path = path
        self.models = [model for model in models if model]

    def _lock_descriptor(self) -> int:
        lock_path = self.path.with_name(self.path.name + ".lock")
        return os.open(lock_path, os.O_WRONLY | os.O_CREAT, 0o600)

    def _read_index(self) -> int:
        try:
            record = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return 0
        index = record.get("index") if isinstance(record, dict) else 0
        if isinstance(index, bool) or not isinstance(index, int) or index < 0:
            return 0
        return min(index, len(self.models) - 1)

    def _write_index(self, index: int) -> None:
        temporary = self.path.with_name(self.path.name + ".tmp")
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            output.write(json.dumps({"index": index}, sort_keys=True))
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, self.path)

    def current(self) -> tuple[str, int]:
        index = self._read_index()
        return self.models[index], index

    def advance(self, expected: int) -> tuple[str, int]:
        """Promote the shared ladder if it is still at ``expected``."""
        if fcntl is None:
            index = min(expected + 1, len(self.models) - 1)
            return self.models[index], index
        descriptor = self._lock_descriptor()
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            index = self._read_index()
            if index == expected and index < len(self.models) - 1:
                index += 1
                self._write_index(index)
            return self.models[index], index
        finally:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
            os.close(descriptor)


MAX_TRANSCRIPT_BYTES = 1_048_576
MAX_PROVIDER_RESPONSE_BYTES = 1_048_576
PROVIDER_ENGINES = frozenset(("codex", "openrouter", "jev"))


class ParticipantRunnerError(RuntimeError):
    pass


def participant_prompt(reason: str, *, context_mode: str = "snapshot") -> str:
    """Prompt one owned Participant; the prompt carries no path or authority bytes."""
    if reason not in ATTENTION_REASONS | {"bootstrap"}:
        raise ParticipantRunnerError("activation_reason_invalid")
    if context_mode not in {"snapshot", "stream"}:
        raise ParticipantRunnerError("context_mode_invalid")
    view_instruction = (
        'Read `$HANOI_CONTEXT_FILE` as the atomic claimed Invocation Context; it is the exact Room Head for this turn.'
        if context_mode == "stream"
        else 'Run `"$HANOI_PYTHON" -m examples.tower_of_hanoi.turn snapshot --membership-file "$HANOI_MEMBERSHIP_FILE"` first.'
    )
    return "\n".join(
        (
            "You are an autonomous Tower of Hanoi Participant. Do not edit files.",
            f"Your bounded invocation reason is: {reason}.",
            view_instruction,
            "Use only that current Projection and its action_offers. The Pack provides legal move rules, work_revision, open completion claims, and assessments; it does not provide a path or decide whether the board is finished.",
            "The public Participant objective is to move the full tower from A to C. It is guidance for your own strategy only; the Pack never auto-completes based on board equality.",
            "The objective is met only when every disk rests in order on rod C; at that point post_completion_claim records your result.",
            "The board arrays are bottom-to-top, so only the final disk on a rod can move. Choose your own strategy.",
            "The Invocation Context marks moves that repeat recent or already-seen states; you decide whether to break a cycle, try a different move, or claim.",
            "This local harness has no scheduler that re-wakes you, so do not idle: on the bootstrap turn choose a concrete opening action.",
            "When the Invocation Context shows target_reached, post a completion claim so the result is recorded.",
            "If you choose to act, call `turn act` yourself with your observed room_seq and exactly one currently offered action: move_disk with a legal move; post_completion_claim with the observed work_revision; or assess_claim with the observed work_revision, current claim_round, and endorse, challenge, or defer.",
            "A move supersedes a claim. A claim is Participant evidence only. If an action returns stale, refresh with snapshot and independently decide again; make at most two action attempts in this invocation and stop after one accepted action.",
            "Do not print, copy, inspect, or describe the credential file or any bearer. Do not write files. End with a concise JSON status object.",
        )
    )


def _dotenv_values(repo: Path) -> dict[str, str]:
    """Read only simple local .env assignments without printing secret values."""
    path = repo / ".env"
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return {}
    values: dict[str, str] = {}
    for line in lines:
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or "=" not in stripped:
            continue
        key, value = stripped.split("=", 1)
        key, value = key.strip(), value.strip()
        if key and value and not value.startswith("${"):
            values[key] = value.strip("'\"")
    return values


def _provider_key(engine: str, repo: Path) -> str | None:
    values = _dotenv_values(repo)
    if engine == "jev":
        names = ("JEV_API_KEY",)
    elif engine == "openrouter":
        # The existing repository uses the historical `openouterkey` spelling.
        names = ("OPENROUTER_API_KEY", "OPENROUTER_KEY", "openouterkey")
    else:
        return None
    for name in names:
        value = os.environ.get(name) or values.get(name)
        if value:
            return value
    return None


def _compact_event(value: object) -> dict[str, object] | None:
    """Reduce one observation to the fields a solver can act on."""
    if not isinstance(value, dict):
        return None
    event: dict[str, object] = {}
    kind = value.get("kind")
    if isinstance(kind, str):
        event["kind"] = kind
    room_seq = value.get("room_seq")
    if isinstance(room_seq, int) and not isinstance(room_seq, bool):
        event["room_seq"] = room_seq
    actor = value.get("actor")
    if isinstance(actor, str) and actor:
        event["actor"] = actor
    move = value.get("move")
    if isinstance(move, dict):
        event["move"] = {
            "from": move.get("from"),
            "to": move.get("to"),
            "disk": move.get("disk"),
        }
    assessment = value.get("assessment")
    if isinstance(assessment, str):
        event["assessment"] = assessment
    claim_open = value.get("claim_open")
    if isinstance(claim_open, bool):
        event["claim_open"] = claim_open
    return event or None


def _compact_decisions(values: object, maximum: int = 4) -> list[dict[str, object]]:
    """Reduce recent own decisions to the label/disposition summary."""
    if not isinstance(values, list):
        return []
    compact: list[dict[str, object]] = []
    for value in values[-maximum:]:
        if not isinstance(value, dict):
            continue
        selected = value.get("selected")
        source = selected if isinstance(selected, dict) else value
        label = source.get("label")
        action_type = source.get("action_type")
        summary: dict[str, object] = {}
        if isinstance(label, str):
            summary["label"] = label
        if isinstance(action_type, str):
            summary["action_type"] = action_type
        disposition = value.get("disposition")
        if isinstance(disposition, str):
            summary["disposition"] = disposition
        if summary:
            compact.append(summary)
    return compact


def _provider_state(
    view: dict[str, object],
    candidates: dict[str, dict[str, object]],
    reason: str,
    *,
    room_seq: int | None,
    context: dict[str, object] | None = None,
) -> dict[str, object]:
    """Build the one bounded public state supplied to every direct solver.

    Both the LLM and JEV paths receive this exact object. It carries the
    authoritative board at the claimed Room Head, the objective, the closed set
    of currently offered choices, bounded recent history, previous own
    decisions, cycle hints, and any open completion claim with quorum status.
    Every legal move stays offered; each move carries facts about whether it
    repeats history so the agent, not the harness, decides what to do.
    """
    activity = {
        key: view[key]
        for key in (
            "board",
            "disks",
            "objective",
            "outcome",
            "phase",
            "round",
            "work_revision",
            "completion",
        )
        if key in view
    }
    completion = view.get("completion")
    open_claim: dict[str, object] | None = None
    if isinstance(completion, dict) and completion.get("claim_open"):
        claim = completion.get("claim")
        approval = completion.get("approval_count")
        quorum = completion.get("quorum")
        open_claim = {
            "claim_round": claim.get("claim_round") if isinstance(claim, dict) else None,
            "work_revision": claim.get("work_revision") if isinstance(claim, dict) else None,
            "electorate_size": claim.get("electorate_size") if isinstance(claim, dict) else None,
            "assessments_by_member": completion.get("assessments_by_member", {}),
            "approval_count": approval,
            "endorsement_count": completion.get("endorsement_count"),
            "quorum": quorum,
            "accepted": (
                isinstance(approval, int)
                and isinstance(quorum, int)
                and approval >= quorum
            ),
        }
    stream: dict[str, object] = {
        "recent_events": [],
        "recent_own_decisions": [],
        "state_visit": {},
        "delivery": {},
    }
    if context is not None:
        raw_events = context.get("events_since_turn", [])
        stream = {
            "recent_events": [
                compact
                for event in (
                    raw_events[-MAX_RECENT_EVENTS:]
                    if isinstance(raw_events, list)
                    else []
                )
                if (compact := _compact_event(event)) is not None
            ],
            "recent_own_decisions": _compact_decisions(
                context.get("recent_own_decisions", []),
                maximum=MAX_RECENT_DECISIONS,
            ),
            "state_visit": context.get("state_visit", {}),
            "delivery": context.get("delivery", {}),
            "cursor": context.get("cursor"),
            "cursor_range": context.get("cursor_range"),
        }
    recent_moves = _recent_move_events(context)
    state_visit = context.get("state_visit") if isinstance(context, dict) else {}
    if not isinstance(state_visit, dict):
        state_visit = {}
    recent_move_labels = {
        f"move:{move['from']}>{move['to']}:{move['disk']}" for move in recent_moves
    }
    undo_label = _immediate_undo_label(candidates, context)
    choice_history = _choice_history(
        view, candidates, context, recent_move_labels, undo_label
    )
    board = activity.get("board")
    disks = activity.get("disks")
    target_reached = _is_target_board(board, disks) if isinstance(disks, int) else False
    claim_open = open_claim is not None
    return {
        "schema": SOLVER_STATE_SCHEMA,
        "activation_reason": reason,
        "based_on_room_seq": room_seq,
        "objective": activity.get("objective"),
        "board": activity.get("board"),
        "disks": activity.get("disks"),
        "phase": activity.get("phase"),
        "round": activity.get("round"),
        "work_revision": activity.get("work_revision"),
        "outcome": activity.get("outcome"),
        "open_completion_claim": open_claim,
        "target_reached": target_reached,
        "claim_open": claim_open,
        "recent_moves": recent_moves,
        "immediate_undo": undo_label,
        "cycle_hint": bool(state_visit.get("cycle_hint", False)),
        "choice_history": choice_history,
        "available_choices": _annotated_choices(
            candidates, choice_history, target_reached, claim_open
        ),
        **stream,
    }


def _choice_history(
    view: dict[str, object],
    candidates: dict[str, dict[str, object]],
    context: dict[str, object] | None,
    recent_move_labels: set[str],
    undo_label: str | None,
) -> dict[str, dict[str, object]]:
    """Describe each offered move against history without choosing for the agent.

    ``visits`` counts how many times the resulting board was already seen,
    ``repeats_recent`` marks a move played in the bounded recent window, and
    ``undoes_last`` marks a move that exactly reverses the most recent move.
    These are facts the solver may weigh; nothing is filtered out.
    """
    visited = context.get("visited_boards") if isinstance(context, dict) else None
    if not isinstance(visited, dict):
        visited = {}
    board = view.get("board")
    disks = view.get("disks")
    history: dict[str, dict[str, object]] = {}
    for label, candidate in candidates.items():
        if candidate.get("action_type") != "move_disk":
            continue
        resulting = _apply_move_to_board(board, candidate.get("payload"))
        visits = 0
        if (
            resulting is not None
            and isinstance(disks, int)
            and not _is_target_board(resulting, disks)
        ):
            count = visited.get(board_fingerprint(resulting))
            visits = count if isinstance(count, int) and count > 0 else 0
        history[label] = {
            "visits": visits,
            "repeats_recent": label in recent_move_labels,
            "undoes_last": label == undo_label,
        }
    return history


def _annotated_choices(
    candidates: dict[str, dict[str, object]],
    history: dict[str, dict[str, object]],
    target_reached: bool,
    claim_open: bool,
) -> dict[str, str]:
    """Describe every offered choice with the same facts for both solvers.

    The annotation is the shared steering layer: history facts plus the
    completion/claim state. Both JEV and the LLM receive these exact strings so
    the comparison is on one scale; only the answer medium differs.
    """
    choices: dict[str, str] = {}
    for label, candidate in candidates.items():
        description = candidate.get("description")
        if not isinstance(description, str):
            continue
        facts = history.get(label)
        if isinstance(facts, dict):
            notes: list[str] = []
            visits = facts.get("visits")
            if isinstance(visits, int) and visits > 0:
                notes.append(f"this board was seen {visits} time(s) before")
            if facts.get("repeats_recent") is True:
                notes.append("already played in the last few moves")
            if facts.get("undoes_last") is True:
                notes.append("reverses the most recent move")
            if notes:
                description = f"{description} ({'; '.join(notes)})."
        action_type = candidate.get("action_type")
        if target_reached:
            if action_type == "post_completion_claim":
                if claim_open:
                    description = (
                        f"{description} A completion claim is already open at this revision; "
                        "posting another will be rejected. If assessing is offered, endorse it; "
                        "otherwise wait for the other participants."
                    )
                else:
                    description = (
                        f"{description} The tower is already complete on the target rod; "
                        "this records the agreed result."
                    )
            elif action_type == "assess_claim":
                payload = candidate.get("payload")
                assessment = payload.get("assessment") if isinstance(payload, dict) else None
                if assessment == "endorse":
                    description = (
                        f"{description} The tower is complete and a claim is open; endorsing "
                        "records the agreed result."
                    )
                elif claim_open:
                    description = f"{description} The tower is complete and a claim is open."
            elif action_type == "move_disk":
                description = f"{description} This would move a disk off the completed tower."
        choices[label] = description
    return choices


def _legal_candidates(view: dict[str, object]) -> dict[str, dict[str, object]]:
    """Enumerate legal actions plus local wait; the provider only selects one."""
    board = view.get("board")
    disks = view.get("disks")
    offers = view.get("action_offers")
    if not isinstance(board, dict) or not isinstance(disks, int) or not isinstance(offers, list):
        return {}
    offered = {
        item if isinstance(item, str) else item.get("action_type")
        for item in offers
        if isinstance(item, (str, dict))
    }
    candidates: dict[str, dict[str, object]] = {}
    if "move_disk" in offered:
        for source in ("A", "B", "C"):
            source_stack = board.get(source)
            if not isinstance(source_stack, list) or not source_stack:
                continue
            disk = source_stack[-1]
            if not isinstance(disk, int):
                continue
            for destination in ("A", "B", "C"):
                if destination == source:
                    continue
                destination_stack = board.get(destination)
                if not isinstance(destination_stack, list):
                    continue
                if destination_stack and destination_stack[-1] < disk:
                    continue
                label = f"move:{source}>{destination}:{disk}"
                candidates[label] = {
                    "action_type": "move_disk",
                    "payload": {"from": source, "to": destination, "disk": disk},
                    "description": f"Move disk {disk} from rod {source} to rod {destination}.",
                }
    if "post_completion_claim" in offered:
        work_revision = view.get("work_revision")
        if isinstance(work_revision, int):
            candidates["post_completion_claim"] = {
                "action_type": "post_completion_claim",
                "payload": {"work_revision": work_revision},
                "description": "Post a completion claim for the current work revision.",
            }
    if "assess_claim" in offered:
        completion = view.get("completion")
        claim = completion.get("claim") if isinstance(completion, dict) else None
        if isinstance(claim, dict):
            work_revision, claim_round = claim.get("work_revision"), claim.get("claim_round")
            if isinstance(work_revision, int) and isinstance(claim_round, int):
                for assessment in ("endorse", "challenge", "defer"):
                    candidates[f"assess:{assessment}"] = {
                        "action_type": "assess_claim",
                        "payload": {
                            "work_revision": work_revision,
                            "claim_round": claim_round,
                            "assessment": assessment,
                        },
                        "description": f"Assess the open completion claim as {assessment}.",
                    }
    candidates["wait"] = {
        "action_type": "wait",
        "payload": None,
        "description": "Take no action and wait for a new observation or request.",
    }
    return candidates


MAX_RECENT_MOVES = 8
MAX_RECENT_EVENTS = 8
MAX_RECENT_DECISIONS = 4


def _actionable_candidates(
    candidates: dict[str, dict[str, object]],
) -> dict[str, dict[str, object]]:
    """Decline to offer "wait" while a concrete action exists.

    The local harness has no scheduler that re-wakes a waiting Participant, so a
    wait on bootstrap, or from the only solver in a Room, stalls the Room with
    no further activation. Removing the no-op option keeps the run live; the
    Pack contract still defines a wait action for callers that can re-wake it.
    """
    concrete = {
        label: candidate
        for label, candidate in candidates.items()
        if candidate.get("action_type") != "wait"
    }
    return concrete or candidates


def _recent_move_events(context: dict[str, object] | None) -> list[dict[str, object]]:
    """Extract the bounded move history the solver may reason from."""
    if not isinstance(context, dict):
        return []
    events = context.get("events_since_turn")
    if not isinstance(events, list):
        return []
    moves: list[dict[str, object]] = []
    for event in events:
        if not isinstance(event, dict):
            continue
        move = event.get("move")
        source, destination = (
            (move.get("from"), move.get("to")) if isinstance(move, dict) else (None, None)
        )
        disk = move.get("disk") if isinstance(move, dict) else None
        if source not in {"A", "B", "C"} or destination not in {"A", "B", "C"}:
            continue
        if isinstance(disk, bool) or not isinstance(disk, int):
            continue
        moves.append(
            {
                "actor": event.get("actor") if isinstance(event.get("actor"), str) else None,
                "from": source,
                "to": destination,
                "disk": disk,
                "room_seq": event.get("room_seq") if isinstance(event.get("room_seq"), int) else None,
            }
        )
    return moves[-MAX_RECENT_MOVES:]


def _immediate_undo_label(
    candidates: dict[str, dict[str, object]],
    context: dict[str, object] | None,
) -> str | None:
    """Return the move label that exactly reverses the most recent move.

    Reversing the same disk is never part of an optimal Tower of Hanoi path, so
    the solver declines to offer it again when a different legal move exists.
    This is solver policy, not a WorldStream rule.
    """
    moves = _recent_move_events(context)
    if not moves:
        return None
    last = moves[-1]
    label = f"move:{last['to']}>{last['from']}:{last['disk']}"
    return label if label in candidates else None


def _apply_move_to_board(
    board: object, move: object
) -> dict[str, list[int]] | None:
    """Return the board after one candidate move, or None if it is malformed."""
    if not isinstance(board, dict) or not isinstance(move, dict):
        return None
    source, destination, disk = move.get("from"), move.get("to"), move.get("disk")
    if source not in {"A", "B", "C"} or destination not in {"A", "B", "C"}:
        return None
    if isinstance(disk, bool) or not isinstance(disk, int):
        return None
    source_stack, destination_stack = board.get(source), board.get(destination)
    if not isinstance(source_stack, list) or not isinstance(destination_stack, list):
        return None
    if not source_stack or source_stack[-1] != disk:
        return None
    resulting = {rod: list(board.get(rod, [])) for rod in ("A", "B", "C")}
    resulting[source] = source_stack[:-1]
    resulting[destination] = [*destination_stack, disk]
    return resulting


def _is_target_board(board: object, disks: int) -> bool:
    """True when every disk rests in order on the objective target rod C."""
    if not isinstance(board, dict):
        return False
    target = list(range(disks, 0, -1))
    return (
        board.get("C") == target
        and not board.get("A")
        and not board.get("B")
    )


def _activation_retry_delay(attempt: int) -> float:
    """Capped exponential backoff with jitter for one failed provider attempt."""
    base = ACTIVATION_RETRY_SECONDS * (2 ** max(0, attempt - 1))
    capped = min(ACTIVATION_RETRY_MAX_SECONDS, base)
    return capped * (0.75 + random.random() * 0.5)


def _http_json(
    url: str, body: dict[str, object], headers: dict[str, str], timeout: float
) -> dict[str, object]:
    encoded = json.dumps(body, separators=(",", ":")).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=encoded,
        headers={"Accept": "application/json", "Content-Type": "application/json", **headers},
        method="POST",
    )
    # A socket timeout is per-read, so a provider that trickles a response can
    # hold a call open far longer than the configured seconds. Run the request
    # on a worker and enforce a hard wall-clock deadline so a slow call becomes
    # a bounded, retryable failure instead of eating the run.
    deadline = max(1.0, timeout)
    outcome: dict[str, object] = {}

    def perform() -> None:
        try:
            with urllib.request.urlopen(request, timeout=deadline) as response:
                outcome["raw"] = response.read(MAX_PROVIDER_RESPONSE_BYTES + 1)
        except urllib.error.HTTPError as error:
            # Surface only the numeric status so operators can tell an account
            # guardrail rejection (404) from a rate limit (429) or a server fault.
            outcome["http"] = error.code
        except (OSError, TimeoutError):
            outcome["failed"] = True

    worker = threading.Thread(target=perform, daemon=True, name="worldstream-provider-call")
    worker.start()
    worker.join(deadline)
    if worker.is_alive():
        raise ParticipantRunnerError("provider_request_timeout")
    if "http" in outcome:
        raise ParticipantRunnerError(f"provider_request_failed_{outcome['http']}")
    if "failed" in outcome:
        raise ParticipantRunnerError("provider_request_failed")
    raw = outcome.get("raw")
    if not isinstance(raw, bytes):
        raise ParticipantRunnerError("provider_request_failed")
    if len(raw) > MAX_PROVIDER_RESPONSE_BYTES:
        raise ParticipantRunnerError("provider_response_oversize")
    try:
        value = json.loads(raw)
    except (UnicodeError, json.JSONDecodeError) as error:
        raise ParticipantRunnerError("provider_response_invalid") from error
    if not isinstance(value, dict):
        raise ParticipantRunnerError("provider_response_invalid")
    return value


def _jev_choice(
    state: dict[str, object], criteria: dict[str, str], model: str, key: str, timeout: float
) -> tuple[str, dict[str, object]]:
    response = _http_json(
        "https://api.typesafe.ai/v1/systemone",
        {
            "state": state,
            "model": model,
            "questions": {
                "action": {
                    "type": "choice",
                    "instructions": (
                        "Choose the single next action that best advances this Tower of Hanoi "
                        "Participant's work. Use the current board, the completion evidence, "
                        "recent_moves, cycle_hint, and choice_history, which marks moves that "
                        "repeat a recent move, reverse the last move, or return to a board that "
                        "was already seen. If target_reached is true the objective board is "
                        "already assembled: post_completion_claim records it when no claim is "
                        "open, and when a claim is already open, assess it (endorse when the "
                        "objective is met) instead of posting another claim. Choose the action "
                        "you judge best, including deliberately breaking a cycle or waiting."
                    ),
                    "criteria": criteria,
                }
            },
        },
        {"Authorization": f"Bearer {key}"},
        min(max(1.0, timeout), JEV_CALL_TIMEOUT_SECONDS),
    )
    answers = response.get("answers")
    answer = answers.get("action") if isinstance(answers, dict) else None
    choice = answer.get("choice") if isinstance(answer, dict) else None
    if not isinstance(choice, str):
        raise ParticipantRunnerError("jev_choice_missing")
    metadata = {
        "provider_model": response.get("model") if isinstance(response.get("model"), str) else model,
        "choice": choice,
        "confidence": answer.get("confidence") if isinstance(answer, dict) else None,
        "probabilities": answer.get("probabilities") if isinstance(answer, dict) else None,
        "usage": response.get("usage") if isinstance(response.get("usage"), dict) else None,
    }
    return choice, metadata


def _jev_staged_choice(
    state: dict[str, object],
    candidates: dict[str, dict[str, object]],
    model: str,
    key: str,
    timeout: float,
) -> tuple[str | None, str, dict[str, object]]:
    """Ask JEV to choose exactly one action from the closed candidate set.

    JEV receives the same state as the LLM, expressed as one closed Choice
    problem. Completion is the ``post_completion_claim`` choice and uncertainty
    is the ``wait`` choice; JEV never answers a separate completion question
    that competes with its action choice.
    """
    criteria = state.get("available_choices")
    if not isinstance(criteria, dict):
        criteria = {
            label: candidate["description"]
            for label, candidate in candidates.items()
            if isinstance(candidate.get("description"), str)
        }
    criteria = {
        label: description
        for label, description in criteria.items()
        if isinstance(label, str) and isinstance(description, str)
    }
    choice, metadata = _jev_choice(state, criteria, model, key, timeout)
    candidate = candidates.get(choice)
    if candidate is None:
        raise ParticipantRunnerError("provider_choice_not_offered")
    if choice == "wait" or candidate.get("action_type") == "wait":
        return None, "wait", metadata
    disposition = _disposition_for(candidate.get("action_type"))
    if disposition is None:
        raise ParticipantRunnerError("provider_choice_not_offered")
    return choice, disposition, metadata


_DISPOSITIONS = {
    "move_disk": "act",
    "post_completion_claim": "claim",
    "assess_claim": "assess",
    "wait": "wait",
}


def _disposition_for(action_type: object) -> str | None:
    return _DISPOSITIONS.get(action_type) if isinstance(action_type, str) else None


def _openrouter_staged_choice(
    state: dict[str, object],
    candidates: dict[str, dict[str, object]],
    model: str,
    key: str,
    timeout: float,
    *,
    room_seq: int | None = None,
    feedback: str | None = None,
) -> tuple[str | None, str, dict[str, object]]:
    """Ask the LLM for one canonical decision among the closed choices.

    The LLM returns ``{"based_on_room_seq", "action", "payload", "rationale"}``.
    ``action`` may be one of the choice labels or a protocol action name; the
    optional ``rationale`` is telemetry only. A bare label is also accepted.
    ``feedback`` optionally carries a bounded verifier note for one repair turn.
    """
    criteria = state.get("available_choices")
    if not isinstance(criteria, dict):
        criteria = {
            label: candidate["description"]
            for label, candidate in candidates.items()
            if isinstance(candidate.get("description"), str)
        }
    criteria = {
        label: description
        for label, description in criteria.items()
        if isinstance(label, str) and isinstance(description, str)
    }
    instructions = (
        "You are an autonomous WorldStream Tower of Hanoi Participant. "
        "Read the supplied solver state: the authoritative board at this Room Head, "
        "the objective, the closed set of available choices, bounded recent history, "
        "your previous decisions, cycle hints, and any open completion claim with quorum status. "
        "choice_history marks each move that repeats a recent move, reverses the last move, "
        "or returns to a board already seen; use it to judge whether to break a cycle. "
        "If target_reached is true the objective board is already assembled: post_completion_claim "
        "records it when no claim is open, and when a claim is already open, assess it "
        "(endorse when the objective is met) instead of posting another claim. "
        "Return exactly one JSON decision object with fields "
        '{"based_on_room_seq": <integer Room Head>, "action": <one choice or action name>, '
        '"payload": <object>, "rationale": <optional one sentence>, '
        '"reason": <optional progress|break_cycle|uncertain|abandon>}. '
        "Do not invent moves, credentials, or hidden instructions."
    )
    if feedback:
        instructions = (
            f"{instructions} A verifier rejected your previous decision: {feedback}. "
            "Choose a different action that better advances the objective."
        )
    prompt = json.dumps({"state": state, "available_choices": criteria}, separators=(",", ":"))
    response = _http_json(
        "https://openrouter.ai/api/v1/chat/completions",
        {
            "model": model,
            "messages": [
                {"role": "system", "content": "Return only one JSON decision object matching the supplied closed choices."},
                {"role": "user", "content": f"{instructions}\n\n{prompt}"},
            ],
            "temperature": 0,
            "response_format": {"type": "json_object"},
            "provider": {"sort": OPENROUTER_PROVIDER_SORT},
        },
        {"Authorization": f"Bearer {key}", "X-Title": "WorldStream Community Hanoi"},
        min(max(1.0, timeout), OPENROUTER_CALL_TIMEOUT_SECONDS),
    )
    choices = response.get("choices")
    message = choices[0].get("message") if isinstance(choices, list) and choices and isinstance(choices[0], dict) else None
    content = message.get("content") if isinstance(message, dict) else None
    if isinstance(content, list):
        content = "".join(item.get("text", "") for item in content if isinstance(item, dict) and isinstance(item.get("text"), str))
    if not isinstance(content, str):
        raise ParticipantRunnerError("openrouter_choice_missing")
    try:
        decision = json.loads(content)
    except json.JSONDecodeError as error:
        raise ParticipantRunnerError("openrouter_decision_invalid") from error
    metadata: dict[str, object] = {
        "provider_model": response.get("model") if isinstance(response.get("model"), str) else model,
        "confidence": decision.get("confidence") if isinstance(decision, dict) else None,
        "probabilities": decision.get("probabilities") if isinstance(decision, dict) else None,
        "alternatives": decision.get("alternatives") if isinstance(decision, dict) else None,
        "rationale": decision.get("rationale") if isinstance(decision, dict) else None,
        "usage": response.get("usage") if isinstance(response.get("usage"), dict) else None,
    }
    try:
        label, canonical = resolve_solver_decision(
            decision, candidates, int(room_seq) if isinstance(room_seq, int) else -1
        )
    except HanoiProtocolError as error:
        # Keep the specific resolution failure (stale head, unoffered action,
        # invalid shape) so a run's evidence says why a valid HTTP reply was
        # still unusable instead of one opaque code.
        raise ParticipantRunnerError(f"openrouter_choice_missing:{error}") from error
    metadata["choice"] = label
    metadata["decision"] = canonical
    if label == "wait":
        return None, "wait", metadata
    disposition = _disposition_for(candidates[label].get("action_type"))
    if disposition is None:
        raise ParticipantRunnerError("openrouter_choice_missing")
    return label, disposition, metadata


def _jev_judge(
    state: dict[str, object],
    criteria: dict[str, str],
    proposed: str,
    model: str,
    key: str,
    timeout: float,
    threshold: float,
) -> tuple[bool, float, dict[str, object]]:
    """Ask JEV one atomic question: is the proposed action the best next action?

    The judge consumes the same shared state the proposer saw plus the proposed
    label and its description. A probability at or above ``threshold`` approves
    the proposal. The judge never submits; code owns the final decision.
    """
    judge_state = dict(state)
    judge_state["proposed_action"] = proposed
    judge_state["proposed_description"] = criteria.get(proposed)
    response = _http_json(
        "https://api.typesafe.ai/v1/systemone",
        {
            "state": judge_state,
            "model": model,
            "questions": {
                "judge": {
                    "type": "noul",
                    "instructions": (
                        "Consider `proposed_action` for this Tower of Hanoi Participant. "
                        "Does it best advance the objective board given the current board, "
                        "recent_moves, cycle_hint, and choice_history? Answer true only when "
                        "you would endorse it as the next action. Answer false when it "
                        "reverses the most recent move, returns to a board already seen, or "
                        "does not advance the objective."
                    ),
                }
            },
        },
        {"Authorization": f"Bearer {key}"},
        min(max(1.0, timeout), JEV_CALL_TIMEOUT_SECONDS),
    )
    answers = response.get("answers")
    answer = answers.get("judge") if isinstance(answers, dict) else None
    probability = None
    if isinstance(answer, dict):
        for field in ("noul", "probability"):
            candidate = answer.get(field)
            if isinstance(candidate, bool):
                continue
            if isinstance(candidate, (int, float)):
                probability = float(candidate)
                break
    if probability is None:
        raise ParticipantRunnerError("jev_judge_missing")
    metadata: dict[str, object] = {
        "engine": "jev",
        "model": response.get("model") if isinstance(response.get("model"), str) else model,
        "proposed": proposed,
        "probability": probability,
        "threshold": threshold,
        "approved": probability >= threshold,
        "usage": response.get("usage") if isinstance(response.get("usage"), dict) else None,
    }
    return metadata["approved"], probability, metadata


def _judged_choice(
    state: dict[str, object],
    candidates: dict[str, dict[str, object]],
    choice: str,
    disposition: str,
    metadata: dict[str, object],
    model: str,
    key: str,
    room_seq: int,
    timeout: float,
    judge_model: str,
    judge_threshold: float,
    judge_key: str | None,
) -> tuple[str | None, str, dict[str, object]]:
    """Verify one LLM proposal with JEV, allowing exactly one bounded repair.

    The proposal is approved when the judge probability reaches the threshold.
    Otherwise the LLM gets one repair turn that carries the verifier note, and
    code keeps whichever proposal the judge scored higher. The judge never
    selects from the open set itself; it only scores the proposer's candidates.
    """
    if not judge_key:
        raise ParticipantRunnerError("provider_key_missing")
    criteria = state.get("available_choices")
    if not isinstance(criteria, dict):
        criteria = {}
    approved, probability, judge_metadata = _jev_judge(
        state, criteria, choice, judge_model, judge_key, timeout, judge_threshold
    )
    if approved:
        metadata["judge"] = judge_metadata
        return choice, disposition, metadata
    repaired_choice, repaired_disposition, repaired_metadata = _openrouter_staged_choice(
        state,
        candidates,
        model,
        key,
        timeout,
        room_seq=room_seq,
        feedback=(
            f"the proposed {choice} scored {probability:.2f} below the "
            f"{judge_threshold:.2f} approval threshold"
        ),
    )
    judge_metadata["repair"] = {"choice": repaired_choice, "disposition": repaired_disposition}
    if repaired_disposition != "wait" and isinstance(repaired_choice, str):
        repaired_approved, repaired_probability, repaired_judge = _jev_judge(
            state, criteria, repaired_choice, judge_model, judge_key, timeout, judge_threshold
        )
        judge_metadata["repair"]["judge"] = repaired_judge
        if repaired_approved or repaired_probability > probability:
            judge_metadata["accepted_repair"] = True
            repaired_metadata["judge"] = judge_metadata
            return repaired_choice, repaired_disposition, repaired_metadata
    metadata["judge"] = judge_metadata
    return choice, disposition, metadata


def _trace_number(value: object) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    if not math.isfinite(float(value)):
        return None
    number = float(value)
    if not 0.0 <= number <= 1.0:
        return None
    return number


def _decision_trace(
    *,
    seat: str,
    engine: str,
    model: str,
    observed_room_seq: object,
    activation_reason: str,
    disposition: str,
    selected: dict[str, object] | None,
    metadata: dict[str, object] | None,
    latency_ms: int,
    context: dict[str, object] | None,
    status: object = None,
) -> dict[str, object]:
    """Build the owner-only, bounded decision record without provider material."""
    if disposition not in {"assess", "claim", "act", "wait"}:
        disposition = "wait"
    room_seq = observed_room_seq if isinstance(observed_room_seq, int) and not isinstance(observed_room_seq, bool) and observed_room_seq >= 0 else None
    safe_selected: dict[str, object] | None = {"label": "wait"} if disposition == "wait" else None
    if isinstance(selected, dict):
        label = selected.get("label")
        action_type = selected.get("action_type")
        if isinstance(label, str) and len(label) <= 128 and action_type in ACTIONS:
            safe_selected = {"label": label, "action_type": action_type}
    result: dict[str, object] = {
        "schema": "worldstream/tower-of-hanoi-decision/v1",
        "kind": "participant_decision",
        "actor": seat[:63],
        "engine": engine,
        "model": model[:128],
        "observed_room_seq": room_seq,
        "activation_reason": activation_reason[:96],
        "disposition": disposition,
        "selected": safe_selected,
        "latency_ms": max(0, min(600_000, latency_ms)),
        "recent_event_count": 0,
    }
    if status == "error":
        result["status"] = "error"
    if isinstance(context, dict):
        events = context.get("events_since_turn")
        result["recent_event_count"] = min(MAX_TRACE_EVENTS, len(events)) if isinstance(events, list) else 0
        cursor_range = context.get("cursor_range")
        if isinstance(cursor_range, dict):
            start, end = cursor_range.get("from"), cursor_range.get("to")
            if all(isinstance(item, int) and not isinstance(item, bool) and item >= 0 for item in (start, end)):
                result["cursor"] = {"from": start, "to": end}
    if isinstance(metadata, dict):
        confidence = _trace_number(metadata.get("confidence"))
        if confidence is not None:
            result["confidence"] = confidence
        probabilities = metadata.get("probabilities")
        if isinstance(probabilities, dict):
            ranked: list[dict[str, object]] = []
            for label, score in probabilities.items():
                probability = _trace_number(score)
                if isinstance(label, str) and len(label) <= 128 and probability is not None:
                    ranked.append({"label": label, "score": probability})
            ranked.sort(key=lambda item: (-float(item["score"]), str(item["label"])))
            if ranked and abs(sum(float(item["score"]) for item in ranked) - 1.0) <= 0.02:
                result["ranked_alternatives"] = ranked[:8]
        alternatives = metadata.get("alternatives")
        if isinstance(alternatives, list):
            ranked = []
            for item in alternatives[:8]:
                if not isinstance(item, dict):
                    continue
                label = item.get("label")
                score = _trace_number(item.get("score"))
                if isinstance(label, str) and len(label) <= 128 and score is not None:
                    ranked.append({"label": label, "score": score})
            if ranked:
                result["ranked_alternatives"] = ranked
        rationale = metadata.get("rationale")
        if isinstance(rationale, str) and rationale and len(rationale) <= 256 and not any(
            token in rationale.lower() for token in ("bearer", "wsb1:", "prompt", "response")
        ):
            result["rationale"] = rationale
        decision = metadata.get("decision")
        decision_reason = decision.get("reason") if isinstance(decision, dict) else None
        if decision_reason in DECISION_REASONS:
            result["reason"] = decision_reason
        judge = metadata.get("judge")
        if isinstance(judge, dict):
            probability = _trace_number(judge.get("probability"))
            approved = judge.get("approved")
            engine_name = judge.get("engine")
            model_name = judge.get("model")
            if (
                probability is not None
                and isinstance(approved, bool)
                and isinstance(engine_name, str)
                and isinstance(model_name, str)
            ):
                result["judge"] = {
                    "engine": engine_name[:64],
                    "model": model_name[:128],
                    "probability": probability,
                    "approved": approved,
                    "repaired": bool(judge.get("accepted_repair", False)),
                }
    return result


def _safe_action_label(action_type: object, payload: object) -> str | None:
    """Derive a small action label from a Pack-shaped result, never from prose."""
    if action_type == "post_completion_claim":
        return "post_completion_claim"
    if action_type == "assess_claim" and isinstance(payload, dict):
        assessment = payload.get("assessment")
        return f"assess:{assessment}" if assessment in {"endorse", "challenge", "defer"} else None
    if action_type == "move_disk" and isinstance(payload, dict):
        source, target, disk = payload.get("from"), payload.get("to"), payload.get("disk")
        if source in {"A", "B", "C"} and target in {"A", "B", "C"} and isinstance(disk, int) and not isinstance(disk, bool) and 1 <= disk <= 128:
            return f"move:{source}>{target}:{disk}"
    return None


MAX_TRACE_EVENTS = 16


def _invoke_provider(
    *,
    engine: str,
    model: str,
    repo: Path,
    membership_file: Path,
    timeout_seconds: float,
    reason: str,
    evidence: Path,
    invocation_name: str,
    seat: str,
    event_file: Path | None,
    cancelled: threading.Event | None,
    invocation_context: dict[str, object] | None = None,
    participant_context: ParticipantContext | None = None,
    judge_engine: str = "",
    judge_model: str = "jev-latest",
    judge_threshold: float = 0.5,
) -> bool:
    """Run one direct JEV/OpenRouter Participant turn through the same WorldStream API."""
    if engine not in PROVIDER_ENGINES - {"codex"} or not model or any(c.isspace() for c in model):
        raise ParticipantRunnerError("provider_configuration_invalid")
    if cancelled is not None and cancelled.is_set():
        return False
    key = _provider_key(engine, repo)
    if not key:
        raise ParticipantRunnerError("provider_key_missing")
    started = time.monotonic()
    result: dict[str, object] = {
        "status": "error",
        "engine": engine,
        "model": model,
        "reason": reason,
    }
    trace_disposition = "wait"
    trace_selected: dict[str, object] | None = None
    trace_metadata: dict[str, object] = {}
    trace_decision: dict[str, object] | None = None
    trace_context = invocation_context
    observed_room_seq: object = None
    try:
        if invocation_context is not None:
            activity = invocation_context.get("activity")
            offers = invocation_context.get("action_offers")
            if not isinstance(activity, dict) or not isinstance(offers, list):
                raise ParticipantRunnerError("activation_context_invalid")
            view = dict(activity)
            view["action_offers"] = offers
            observed_room_seq = invocation_context.get("room_seq")
        else:
            view = asyncio.run(turn.snapshot(membership_file, "solver"))
            observed_room_seq = view.get("room_seq")
        candidates = _legal_candidates(view)
        candidates = _actionable_candidates(candidates)
        expected_room_seq = int(
            observed_room_seq
            if isinstance(observed_room_seq, int)
            else view["room_seq"]
        )
        # Every legal move stays offered. The solver sees the repetition facts
        # in the state and decides for itself whether to break a cycle.
        state = _provider_state(
            view,
            candidates,
            reason,
            room_seq=expected_room_seq,
            context=invocation_context,
        )
        if not candidates:
            trace_disposition = "wait"
            raise ParticipantRunnerError("no_legal_action_candidate")
        if engine == "jev":
            choice, disposition, metadata = _jev_staged_choice(
                state, candidates, model, key, timeout_seconds
            )
        else:
            choice, disposition, metadata = _openrouter_staged_choice(
                state, candidates, model, key, timeout_seconds, room_seq=expected_room_seq
            )
        if engine == "openrouter" and judge_engine == "jev" and disposition != "wait" and isinstance(choice, str):
            choice, disposition, metadata = _judged_choice(
                state,
                candidates,
                choice,
                disposition,
                metadata,
                model,
                key,
                expected_room_seq,
                timeout_seconds,
                judge_model,
                judge_threshold,
                _provider_key("jev", repo),
            )
        trace_disposition = disposition
        trace_metadata = metadata
        decided = metadata.get("decision")
        decided_reason = decided.get("reason") if isinstance(decided, dict) else None
        if disposition == "wait":
            result.update(
                {
                    "status": "wait",
                    "engine": engine,
                    "model": model,
                    "reason": reason,
                    **metadata,
                }
            )
            trace_selected = None
            candidate = None
            trace_decision = decision_from_candidate(
                "wait",
                {**candidates, "wait": {"action_type": "wait", "payload": None}},
                expected_room_seq,
                rationale=metadata.get("rationale"),
                reason=decided_reason,
            )
        else:
            candidate = candidates.get(choice) if isinstance(choice, str) else None
            if candidate is None:
                trace_disposition = "wait"
                raise ParticipantRunnerError("provider_choice_not_offered")
            trace_decision = decision_from_candidate(
                str(choice),
                candidates,
                expected_room_seq,
                rationale=metadata.get("rationale"),
                reason=decided_reason,
            )
            trace_selected = {
                "label": choice,
                "action_type": candidate.get("action_type"),
            }
            if cancelled is not None and cancelled.is_set():
                return False
            if participant_context is not None:
                # Provider inference runs in a worker thread.  The context
                # bridges the Action back to the event loop and submits it on
                # the same Membership Room that is consuming observations.
                result = participant_context.submit_action_sync(
                    str(candidate["action_type"]),
                    candidate["payload"],
                    expected_room_seq,
                    min(60.0, max(1.0, timeout_seconds)),
                )
            else:
                result = asyncio.run(
                    turn.act(
                        membership_file,
                        str(candidate["action_type"]),
                        candidate["payload"],
                        expected_room_seq,
                        min(60.0, max(1.0, timeout_seconds)),
                    )
                )
            result.update({"engine": engine, "model": model, "reason": reason, **metadata})
    except (
        CredentialError,
        HanoiProtocolError,
        OSError,
        ParticipantRunnerError,
        ProtocolError,
        TimeoutError,
        ValueError,
    ) as error:
        result.update({"status": "error", "code": str(error)})
    result["latency_ms"] = int((time.monotonic() - started) * 1000)
    encoded = json.dumps(result, separators=(",", ":"), sort_keys=True)
    transcript_path = evidence.parent / f"{invocation_name}.jsonl"
    descriptor = os.open(transcript_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as output:
        output.write(encoded + "\n")
    _append(
        evidence,
        {
            "kind": "provider_invocation",
            "name": invocation_name,
            "engine": engine,
            "model": result.get("provider_model", model),
            "reason": reason,
            "status": result.get("status"),
            "action_type": result.get("action_type"),
            **(
                {
                    "decision_action": trace_decision.get("action"),
                    "based_on_room_seq": trace_decision.get("based_on_room_seq"),
                }
                if isinstance(trace_decision, dict)
                else {}
            ),
            "latency_ms": result["latency_ms"],
        },
    )
    trace = _decision_trace(
        seat=seat,
        engine=engine,
        model=str(result.get("provider_model", model)),
        observed_room_seq=observed_room_seq,
        activation_reason=reason,
        disposition=trace_disposition,
        selected=trace_selected,
        metadata=trace_metadata,
        latency_ms=int(result["latency_ms"]),
        context=trace_context,
        status=result.get("status"),
    )
    _append(evidence, trace)
    _append_canvas_decision(event_file, trace)
    if participant_context is not None:
        participant_context.record_decision(trace)
    if event_file is not None:
        _append_canvas_receipts(event_file, seat, encoded + "\n")
    return result.get("status") in {"accepted", "stale", "rejected", "wait"}


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


def _append_canvas_decision(event_file: Path | None, trace: dict[str, object]) -> None:
    """Forward the same sanitized decision trace to the comparison event sink."""
    if event_file is None:
        return
    if (
        isinstance(trace.get("observed_room_seq"), bool)
        or not isinstance(trace.get("observed_room_seq"), int)
        or trace["observed_room_seq"] < 0
    ):
        return
    try:
        metadata = event_file.lstat()
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077:
            raise ParticipantRunnerError("canvas_sink_unprotected")
        encoded = json.dumps(trace, separators=(",", ":"), sort_keys=True).encode("utf-8") + b"\n"
    except (OSError, TypeError, ValueError) as error:
        raise ParticipantRunnerError("canvas_decision_invalid") from error
    if len(encoded) > 4096 or b"wsb1:" in encoded.lower():
        raise ParticipantRunnerError("canvas_decision_invalid")
    descriptor = os.open(event_file, os.O_WRONLY | os.O_APPEND)
    try:
        os.write(descriptor, encoded)
    finally:
        os.close(descriptor)


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
    engine: str = "codex",
    model: str = "gpt-5.6-luna",
    context_mode: str = "snapshot",
    invocation_context: dict[str, object] | None = None,
    participant_context: ParticipantContext | None = None,
    judge_engine: str = "",
    judge_model: str = "jev-latest",
    judge_threshold: float = 0.5,
) -> bool:
    """Run one bounded provider turn and retain a redacted bounded transcript.

    A fresh process group lets lease loss and wall-clock expiry stop Codex plus every
    tool subprocess. The Codex child itself still inherits no ambient shell values.
    """
    if engine != "codex":
        return _invoke_provider(
            engine=engine,
            model=model,
            repo=repo,
            membership_file=membership_file,
            timeout_seconds=timeout_seconds,
            reason=reason,
            evidence=evidence,
            invocation_name=invocation_name,
            seat=seat,
            event_file=event_file,
            cancelled=cancelled,
            invocation_context=invocation_context,
            participant_context=participant_context,
            judge_engine=judge_engine,
            judge_model=judge_model,
            judge_threshold=judge_threshold,
        )
    if context_mode not in {"snapshot", "stream"}:
        raise ParticipantRunnerError("context_mode_invalid")
    started = time.monotonic()
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
        context_file: Path | None = None
        if context_mode == "stream" and invocation_context is not None:
            context_file = workspace / "participant-context.json"
            encoded_context = json.dumps(
                invocation_context, separators=(",", ":"), sort_keys=True
            ).encode("utf-8")
            if len(encoded_context) > 32_768:
                raise ParticipantRunnerError("participant_context_oversize")
            descriptor = os.open(context_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(descriptor, "wb") as output:
                output.write(encoded_context)
                output.write(b"\n")
                output.flush()
                os.fsync(output.fileno())
        command = codex_command(
            codex,
            workspace,
            effort,
            participant_prompt(reason, context_mode=context_mode),
            repo=repo,
            python=python,
            membership_file=membership_file,
            model=model,
            context_file=context_file,
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
                    "model": model,
                    "reasoning_effort": effort,
                    "timeout_seconds": timeout_seconds,
                    "exit_status": None if process is None else process.returncode,
                    "timed_out": timed_out,
                    "cancelled": cancelled_run,
                    "process_group_cleanup": cleanup,
                    "workspace_removed": True,
                },
            )
            codex_disposition = "wait"
            codex_selected: dict[str, object] | None = None
            try:
                codex_results = json_results_in(raw_stdout.splitlines())
            except HanoiProtocolError:
                codex_results = []
            for codex_result in codex_results:
                action_type = codex_result.get("action_type")
                if action_type not in ACTIONS:
                    continue
                codex_disposition = {
                    "assess_claim": "assess",
                    "post_completion_claim": "claim",
                    "move_disk": "act",
                }[action_type]
                label = _safe_action_label(action_type, codex_result.get("payload"))
                if label is None:
                    continue
                codex_selected = {"label": label, "action_type": action_type}
                break
            codex_trace = _decision_trace(
                seat=seat,
                engine="codex",
                model=model,
                observed_room_seq=(
                    invocation_context.get("room_seq")
                    if isinstance(invocation_context, dict)
                    else None
                ),
                activation_reason=reason,
                disposition=codex_disposition,
                selected=codex_selected,
                metadata=None,
                latency_ms=int((time.monotonic() - started) * 1000),
                context=invocation_context,
            )
            _append(evidence, codex_trace)
            _append_canvas_decision(event_file, codex_trace)
            if participant_context is not None:
                participant_context.record_decision(codex_trace)
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


async def _settle_grant(
    grant: ActionGrant,
    claimed_seq: int,
    stream_context: ParticipantContext | None,
) -> None:
    """Close out a held grant after one invocation.

    When the Room head advanced, the elected action is done and the grant is
    marked acted so no later solver wastes a provider call on the same head.
    Otherwise the attempt failed without moving the Room, so the grant is
    cleared and another solver may try.
    """
    if stream_context is not None:
        deadline = time.monotonic() + GRANT_SETTLE_SECONDS
        while time.monotonic() < deadline:
            observed = stream_context.current_observation().get("room_seq")
            if isinstance(observed, int) and observed > claimed_seq:
                grant.mark_acted()
                return
            await asyncio.sleep(0.05)
    grant.clear()


async def _release_activation(
    runner: Any, claimed: dict[str, Any], evidence: Path, skip_reason: str
) -> None:
    """Return a claimed Activation we chose not to invoke."""
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
        return
    try:
        await runner.release(activation_id, claim_id, generation)
    except (ProtocolError, LostRunnerReply, OSError, TimeoutError, ValueError) as error:
        _append(
            evidence,
            {
                "kind": "activation_release_failed",
                "activation_id": activation_id,
                "code": type(error).__name__,
            },
        )
    _append(
        evidence,
        {
            "kind": "activation_skipped",
            "activation_id": activation_id,
            "skip": skip_reason,
        },
    )


def _validate_claimed_context(
    claimed: dict[str, object],
    offer: dict[str, object],
    *,
    room_id: str,
    member_id: str,
) -> dict[str, object]:
    """Validate the identity and captured head before using Invocation Context."""
    context = claimed.get("context")
    activation_id = offer.get("activation_id")
    reason = offer.get("reason_code")
    if (
        offer.get("room_id") != room_id
        or offer.get("member_id") != member_id
        or reason not in ATTENTION_REASONS
        or not isinstance(context, dict)
        or context.get("activation_id") != activation_id
        or context.get("claim_id") != claimed.get("claim_id")
        or context.get("lease_generation") != claimed.get("lease_generation")
        or context.get("reason_code") != reason
        or context.get("cause_room_seq") != offer.get("cause_room_seq")
        or context.get("deadline") != offer.get("deadline")
    ):
        raise ParticipantRunnerError("activation_context_invalid")
    room_head = context.get("room_head")
    if (
        not isinstance(room_head, dict)
        or room_head.get("room_id") != room_id
        or room_head.get("room_seq") != context.get("cause_room_seq")
    ):
        raise ParticipantRunnerError("activation_context_invalid")
    # A Participant only consumes the projection and offers after the SDK's
    # envelope validation.  This extra check binds them to this membership and
    # prevents a stale or cross-Room claimed context from reaching a provider.
    projection = context.get("projection")
    offers = context.get("action_offers")
    if not isinstance(projection, dict) or not isinstance(offers, list):
        raise ParticipantRunnerError("activation_context_invalid")
    try:
        activity_from_projection(projection)
        safe_activity(activity_from_projection(projection))
    except HanoiProtocolError as error:
        raise ParticipantRunnerError("activation_context_invalid") from error
    if not all(isinstance(item, dict) for item in offers):
        raise ParticipantRunnerError("activation_context_invalid")
    return context


def _select_offer(raw_offers: list[object]) -> dict[str, object] | None:
    """Prefer a pending claim review, otherwise the newest attention offer.

    A claim review is a one-shot signal: replaying an older board change first
    could let an open completion claim stall short of quorum. Among the same
    reason, the highest ``cause_room_seq`` wins so a solver never parks on an
    offer whose head has already been superseded.
    """

    def newest(candidates: list[dict[str, object]]) -> dict[str, object] | None:
        if not candidates:
            return None

        def key(candidate: dict[str, object]) -> int:
            seq = candidate.get("cause_room_seq")
            return seq if isinstance(seq, int) and not isinstance(seq, bool) else -1

        return max(candidates, key=key)

    claims = [
        candidate
        for candidate in raw_offers
        if isinstance(candidate, dict) and candidate.get("reason_code") == "claim_review_requested"
    ]
    attention = [
        candidate
        for candidate in raw_offers
        if isinstance(candidate, dict) and candidate.get("reason_code") in ATTENTION_REASONS
    ]
    return newest(claims) or newest(attention)


def _observation_task_failure(
    task: asyncio.Task[None], context: ParticipantContext
) -> str | None:
    """Convert an observation task exit into a bounded supervisor error code."""
    if not task.done():
        return None
    if task.cancelled():
        return "observation_stream_cancelled"
    try:
        error = task.exception()
    except asyncio.CancelledError:
        return "observation_stream_cancelled"
    code = context.failure_code
    if code:
        return code
    if error is not None:
        return "observation_stream_failed"
    return None


async def run(arguments: argparse.Namespace) -> dict[str, object]:
    """Bootstrap once, then handle only Pack-issued Activation offers until deadline."""
    from worldstream_sdk import Client

    context_mode = getattr(arguments, "context_mode", "snapshot")
    if context_mode not in {"snapshot", "stream"}:
        raise ParticipantRunnerError("context_mode_invalid")
    membership = load_membership(arguments.membership_file)
    runner_doc = load_runner(arguments.runner_file)
    validate_pair(membership, runner_doc)
    if membership["pack"]["id"] != PACK_ID or membership["role"] != "solver":
        raise CredentialError("tower_runner_membership_required")
    member_client = Client(sdk_base_url(membership), membership["bearer"])
    runner_client = Client(sdk_base_url(runner_doc), runner_doc["bearer"])
    evidence = arguments.evidence_file
    checkpoint_file = getattr(arguments, "checkpoint_file", None)
    if checkpoint_file is None:
        checkpoint_file = evidence.parent / "participant.checkpoint.json"
    checkpoint = load_checkpoint(checkpoint_file) if context_mode == "stream" else {}
    after_frame_seq = checkpoint.get("cursor") if isinstance(checkpoint.get("cursor"), int) else None
    room = await member_client.open_room(
        membership["room_id"], membership["member_id"], after_frame_seq=after_frame_seq
    )
    runner = await runner_client.open_runner(
        runner_doc["runner_id"], 1, [PACK_ID], [membership["pack"]]
    )
    stream_context: ParticipantContext | None = None
    observation_stop: asyncio.Event | None = None
    observation_task: asyncio.Task[None] | None = None
    observation_failure_reported = False
    grant = ActionGrant(arguments.grant_file) if arguments.grant_file is not None else None

    def check_observation_task() -> None:
        nonlocal observation_failure_reported
        if (
            stream_context is None
            or observation_task is None
            or observation_failure_reported
        ):
            return
        code = _observation_task_failure(observation_task, stream_context)
        if code is None:
            return
        observation_failure_reported = True
        _append(
            evidence,
            {
                "kind": "observation_stream_failed",
                "seat": membership["seat"],
                "code": code,
            },
        )
        raise ParticipantRunnerError(code)
    try:
        if context_mode == "stream":
            stream_context = ParticipantContext(
                room,
                member_client,
                membership["room_id"],
                checkpoint_file=checkpoint_file,
                checkpoint=checkpoint,
            )
            await stream_context.bootstrap()
            observation_stop = asyncio.Event()
            observation_task = asyncio.create_task(
                stream_context.consume_until(observation_stop)
            )
            bootstrap_context = stream_context.baseline_context(
                activation_reason="bootstrap"
            )
            stream_context.mark_turn(
                room_seq=bootstrap_context.get("room_seq")
                if isinstance(bootstrap_context.get("room_seq"), int)
                else None,
                cursor=bootstrap_context.get("cursor")
                if isinstance(bootstrap_context.get("cursor"), int)
                else None,
            )
        else:
            await room.sync()
            bootstrap_context = None
        _append(
            evidence,
            {
            "kind": "supervisor_started",
            "seat": membership["seat"],
            "runner_id_present": True,
            "context_mode": context_mode,
        },
        )
        # Provider turns are synchronous because the direct APIs use urllib, so run
        # bootstrap in the same worker thread as later claimed activations. This also
        # keeps the JEV/OpenRouter path from nesting asyncio.run inside this loop.
        bootstrap_ok = False
        for attempt in range(1, BOOTSTRAP_ATTEMPTS + 1):
            bootstrap_timeout = min(
                arguments.codex_timeout_seconds,
                max(1.0, _remaining(arguments.deadline_at_ms)),
            )
            bootstrap_ok = await asyncio.to_thread(
                _invoke,
                codex=arguments.codex,
                python=arguments.python,
                repo=arguments.repo,
                membership_file=arguments.membership_file,
                effort=arguments.solver_effort,
                timeout_seconds=bootstrap_timeout,
                reason="bootstrap",
                evidence=evidence,
                invocation_name="bootstrap" if attempt == 1 else f"bootstrap-{attempt}",
                seat=membership["seat"],
                event_file=arguments.event_file,
                engine=arguments.solver_engine,
                model=arguments.solver_model,
                context_mode=context_mode,
                invocation_context=bootstrap_context,
                participant_context=stream_context,
                judge_engine=arguments.judge_engine,
                judge_model=arguments.judge_model,
                judge_threshold=arguments.judge_threshold,
            )
            if bootstrap_ok or _remaining(arguments.deadline_at_ms) <= 0:
                break
            _append(
                evidence,
                {
                    "kind": "bootstrap_retry",
                    "seat": membership["seat"],
                    "attempt": attempt + 1,
                },
            )
            await asyncio.sleep(BOOTSTRAP_RETRY_SECONDS)
        check_observation_task()
        invocation_count = 1
        while _remaining(arguments.deadline_at_ms) > 0:
            check_observation_task()
            if stream_context is not None:
                current = stream_context.current_observation()
                activity = current.get("activity")
                if not isinstance(activity, dict):
                    await asyncio.sleep(POLL_SECONDS)
                    continue
            else:
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
            offer = _select_offer(raw_offers)
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
            try:
                claimed_context = _validate_claimed_context(
                    claimed,
                    offer,
                    room_id=membership["room_id"],
                    member_id=membership["member_id"],
                )
                invocation_context = (
                    stream_context.context_for_claim(
                        claimed_context, activation_reason=reason
                    )
                    if stream_context is not None
                    else None
                )
                if stream_context is not None and invocation_context is not None:
                    stream_context.mark_turn(
                        room_seq=invocation_context.get("room_seq")
                        if isinstance(invocation_context.get("room_seq"), int)
                        else None,
                        cursor=invocation_context.get("cursor")
                        if isinstance(invocation_context.get("cursor"), int)
                        else None,
                    )
            except (ParticipantRunnerError, HanoiProtocolError):
                _append(
                    evidence,
                    {
                        "kind": "activation_context_invalid",
                        "activation_id": activation_id,
                        "reason": reason,
                    },
                )
                claim_id = claimed.get("claim_id")
                generation = claimed.get("lease_generation")
                if (
                    isinstance(claim_id, str)
                    and isinstance(generation, int)
                    and not isinstance(generation, bool)
                ):
                    try:
                        await runner.complete(
                            activation_id, claim_id, generation, "failed"
                        )
                    except (ProtocolError, LostRunnerReply, OSError, TimeoutError, ValueError):
                        _append(
                            evidence,
                            {
                                "kind": "activation_invalid_completion_failed",
                                "activation_id": activation_id,
                            },
                        )
                continue
            claimed_seq = claimed_context.get("room_seq")
            if grant is not None and isinstance(claimed_seq, int):
                observed_seq: int | None = None
                if stream_context is not None:
                    observed = stream_context.current_observation().get("room_seq")
                    observed_seq = observed if isinstance(observed, int) else None
                if observed_seq is not None and observed_seq > claimed_seq:
                    await _release_activation(runner, claimed, evidence, "stale_head")
                    await asyncio.sleep(POLL_SECONDS)
                    continue
                if not grant.acquire(claimed_seq, membership["member_id"]):
                    await _release_activation(runner, claimed, evidence, "grant_held")
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
                activation_context: dict[str, object] | None = invocation_context,
            ) -> bool:
                attempt = 0
                models = [arguments.solver_model, *arguments.fallback_model]
                ladder = (
                    ModelLadder(arguments.model_file, models)
                    if arguments.model_file is not None
                    else None
                )
                if ladder is not None:
                    model, index = ladder.current()
                else:
                    model, index = models[0], 0
                attempts_on_model = 0
                while not cancelled.is_set() and _remaining(arguments.deadline_at_ms) > 0:
                    attempt += 1
                    ok = _invoke(
                        codex=arguments.codex,
                        python=arguments.python,
                        repo=arguments.repo,
                        membership_file=arguments.membership_file,
                        effort=arguments.solver_effort,
                        timeout_seconds=timeout_seconds,
                        reason=activation_reason,
                        evidence=evidence,
                        invocation_name=name if attempt == 1 else f"{name}-retry{attempt}",
                        seat=membership["seat"],
                        event_file=arguments.event_file,
                        engine=arguments.solver_engine,
                        model=model,
                        cancelled=cancelled,
                        context_mode=context_mode,
                        invocation_context=activation_context,
                        participant_context=stream_context,
                        judge_engine=arguments.judge_engine,
                        judge_model=arguments.judge_model,
                        judge_threshold=arguments.judge_threshold,
                    )
                    if ok:
                        return True
                    if cancelled.is_set() or _remaining(arguments.deadline_at_ms) <= 0:
                        return False
                    attempts_on_model += 1
                    if attempts_on_model >= ACTIVATION_PRIMARY_ATTEMPTS and index < len(models) - 1:
                        if ladder is not None:
                            model, index = ladder.advance(index)
                        else:
                            index += 1
                            model = models[index]
                        attempts_on_model = 0
                    delay = _activation_retry_delay(attempt)
                    _append(
                        evidence,
                        {
                            "kind": "activation_retry",
                            "seat": membership["seat"],
                            "name": name,
                            "attempt": attempt + 1,
                            "model": model,
                            "model_index": index,
                            "delay_ms": int(delay * 1000),
                        },
                    )
                    time.sleep(delay)
                return False

            try:
                await _invoke_claimed(runner, claimed, invoke_activation, evidence)
            finally:
                if grant is not None and grant.held_seq is not None and isinstance(claimed_seq, int):
                    await _settle_grant(grant, claimed_seq, stream_context)
        return {"status": "deadline", "invocations": invocation_count}
    finally:
        if observation_stop is not None:
            observation_stop.set()
        if observation_task is not None:
            observation_task.cancel()
            try:
                await observation_task
            except asyncio.CancelledError:
                pass
        await runner.close()
        if stream_context is not None:
            await stream_context.close()
        else:
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
    parser.add_argument("--solver-engine", choices=sorted(PROVIDER_ENGINES), default="codex")
    parser.add_argument("--solver-model", default="gpt-5.6-luna")
    parser.add_argument(
        "--fallback-model",
        action="append",
        default=[],
        help="model tried after repeated failures on the primary solver model",
    )
    parser.add_argument(
        "--model-file",
        type=Path,
        help="shared model-ladder file so peer Rooms switch models together",
    )
    parser.add_argument(
        "--judge-engine",
        choices=("none", "jev"),
        default="none",
        help="optional verifier that scores the proposer's decision before submission",
    )
    parser.add_argument("--judge-model", default="jev-latest")
    parser.add_argument("--judge-threshold", type=float, default=0.5)
    parser.add_argument("--context-mode", choices=("snapshot", "stream"), default="snapshot")
    parser.add_argument("--checkpoint-file", type=Path)
    parser.add_argument("--deadline-at-ms", type=int, required=True)
    parser.add_argument("--evidence-file", type=Path, required=True)
    parser.add_argument("--event-file", type=Path)
    parser.add_argument(
        "--grant-file",
        type=Path,
        help="shared action-grant file that serializes one acting member per Room head",
    )
    arguments = parser.parse_args()
    arguments.judge_engine = "" if arguments.judge_engine == "none" else arguments.judge_engine
    if not 1 <= arguments.codex_timeout_seconds <= 600 or arguments.deadline_at_ms <= 0:
        parser.error("timeout or deadline is outside the bounded range")
    if arguments.judge_engine == "jev":
        if not arguments.judge_model or any(c.isspace() for c in arguments.judge_model):
            parser.error("judge-model is invalid")
        if not 0.0 < arguments.judge_threshold <= 1.0:
            parser.error("judge-threshold must be in (0, 1]")
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

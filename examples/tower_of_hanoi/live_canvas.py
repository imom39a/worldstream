"""Serve a loopback-only, public Projection view of one local Hanoi Room.

The adapter keeps an actionless observer capability in its own process.  The
browser receives only a bounded rendering DTO: the public board, aggregate
contribution counts under configured display labels, and the Pack's public
``last_move`` fact.  It never receives a Room/member identifier, a bearer, a
Runtime URL, or a raw WorldStream frame.

This is a local demonstration bridge, not an Activity Client deployment or a
remote public relay.  Production public delivery uses the separately reviewed
Hosted Gateway contract.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import math
import os
import queue
import re
import stat
import threading
from collections import deque
from collections.abc import AsyncIterator
from dataclasses import dataclass
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from itertools import pairwise
from pathlib import Path
from typing import Any

from worldstream_sdk import Client, ProtocolError

from examples.cli_activity.credentials import (
    CredentialError,
    load_membership,
    sdk_base_url,
)

from .protocol import MAX_DISKS, PACK_ID

MAX_BROWSER_EVENT_BYTES = 32_768
MAX_RECENT_EVENTS = 100
MAX_DECISION_ALTERNATIVES = 8
MAX_DECISION_RATIONALE = 280
MAX_DECISION_RECENT_EVENTS = 32
MAX_BROWSER_CLIENTS = 16
MAX_REPLAYED_BATCHES = 32
MAX_MOVE_LIMIT = 10_000
MAX_ROUND = 30_000
MEMBER_LABEL_PATTERN = re.compile(r"[A-Za-z0-9][A-Za-z0-9 ._-]{0,63}\Z")
DECISION_SCHEMA = "worldstream/tower-of-hanoi-decision/v1"
DECISION_ACTIONS = {"move_disk", "post_completion_claim", "assess_claim"}
DECISION_DISPOSITIONS = {"act", "claim", "assess", "wait"}
DECISION_REASONS = {"progress", "break_cycle", "uncertain", "abandon"}
DECISION_STATUSES = {"accepted", "stale", "rejected", "error"}


class LiveCanvasError(RuntimeError):
    """A closed error safe to report without upstream detail."""


async def _next_event(stream: AsyncIterator[dict[str, Any]]) -> dict[str, Any]:
    return await stream.__anext__()


def _integer(value: object, label: str, *, minimum: int = 0, maximum: int = 512) -> int:
    if (
        isinstance(value, bool)
        or not isinstance(value, int)
        or not minimum <= value <= maximum
    ):
        raise LiveCanvasError(f"{label}_invalid")
    return value


def _record(value: object, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise LiveCanvasError(f"{label}_invalid")
    return value


def _move(value: object, label: str) -> dict[str, object]:
    move = _record(value, label)
    if set(move) != {"from", "to", "disk"}:
        raise LiveCanvasError(f"{label}_invalid")
    source, destination = move["from"], move["to"]
    if (
        source not in {"A", "B", "C"}
        or destination not in {"A", "B", "C"}
        or source == destination
    ):
        raise LiveCanvasError(f"{label}_invalid")
    return {
        "from": source,
        "to": destination,
        "disk": _integer(move["disk"], label, minimum=1, maximum=MAX_DISKS),
    }


def _decision_text(value: object, label: str, maximum: int) -> str:
    if not isinstance(value, str) or not value or len(value) > maximum:
        raise LiveCanvasError(f"{label}_invalid")
    if any(ord(character) < 0x20 or ord(character) == 0x7F for character in value):
        raise LiveCanvasError(f"{label}_invalid")
    return value


def _decision_ratio(value: object, label: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise LiveCanvasError(f"{label}_invalid")
    ratio = float(value)
    if not math.isfinite(ratio) or not 0.0 <= ratio <= 1.0:
        raise LiveCanvasError(f"{label}_invalid")
    return ratio


def _safe_decision_event(value: object) -> dict[str, object]:
    """Sanitize one provider decision before it reaches the browser.

    The sink is owner-only. Even so, this boundary is deliberately strict: raw
    prompts, provider responses, usage records, action payloads, and WorldStream
    identifiers are not part of the decision trace DTO.
    """
    document = _record(value, "decision")
    allowed = {
        "schema",
        "kind",
        "actor",
        "engine",
        "model",
        "observed_room_seq",
        "activation_reason",
        "disposition",
        "selected",
        "status",
        "confidence",
        "ranked_alternatives",
        "rationale",
        "reason",
        "judge",
        "latency_ms",
        "recent_event_count",
        "cursor",
    }
    if document.get("schema") != DECISION_SCHEMA or set(document) - allowed:
        raise LiveCanvasError("decision_invalid")
    if document.get("kind") != "participant_decision":
        raise LiveCanvasError("decision_invalid")
    actor = document.get("actor")
    if not isinstance(actor, str) or MEMBER_LABEL_PATTERN.fullmatch(actor) is None:
        raise LiveCanvasError("decision_invalid")
    engine = _decision_text(document.get("engine"), "decision_engine", 64)
    model = _decision_text(document.get("model"), "decision_model", 128)
    activation_reason = _decision_text(
        document.get("activation_reason"), "activation_reason", 128
    )
    disposition = document.get("disposition")
    if disposition not in DECISION_DISPOSITIONS:
        raise LiveCanvasError("decision_invalid")
    selected = _record(document.get("selected"), "decision_selected")
    if set(selected) not in ({"label"}, {"label", "action_type"}):
        raise LiveCanvasError("decision_invalid")
    selected_label = _decision_text(selected.get("label"), "selected_label", 128)
    action_type = selected.get("action_type")
    if action_type is not None and action_type not in DECISION_ACTIONS:
        raise LiveCanvasError("decision_invalid")
    selected_public: dict[str, object] = {"label": selected_label}
    result: dict[str, object] = {
        "kind": "participant_decision",
        "actor": actor,
        "engine": engine,
        "model": model,
        "observed_room_seq": _integer(
            document.get("observed_room_seq"),
            "observed_room_seq",
            maximum=9_007_199_254_740_991,
        ),
        "activation_reason": activation_reason,
        "disposition": disposition,
        "selected": selected_public,
        "latency_ms": _integer(
            document.get("latency_ms"), "decision_latency", maximum=600_000
        ),
        "recent_event_count": _integer(
            document.get("recent_event_count"),
            "recent_event_count",
            maximum=MAX_DECISION_RECENT_EVENTS,
        ),
    }
    if action_type is not None:
        selected_public["action_type"] = action_type
    if disposition == "wait" and action_type is not None:
        raise LiveCanvasError("decision_invalid")
    if disposition != "wait" and action_type is None:
        raise LiveCanvasError("decision_invalid")
    status = document.get("status")
    if status is not None:
        if status not in DECISION_STATUSES:
            raise LiveCanvasError("decision_status_invalid")
        result["status"] = status
    confidence = document.get("confidence")
    if confidence is not None:
        result["confidence"] = _decision_ratio(confidence, "confidence")
    alternatives = document.get("ranked_alternatives")
    if alternatives is not None:
        if not isinstance(alternatives, list) or len(alternatives) > MAX_DECISION_ALTERNATIVES:
            raise LiveCanvasError("ranked_alternatives_invalid")
        public_alternatives: list[dict[str, object]] = []
        for alternative in alternatives:
            item = _record(alternative, "ranked_alternative")
            if set(item) != {"label", "score"}:
                raise LiveCanvasError("ranked_alternatives_invalid")
            public_alternatives.append(
                {
                    "label": _decision_text(item.get("label"), "alternative_label", 128),
                    "score": _decision_ratio(item.get("score"), "alternative_score"),
                }
            )
        result["ranked_alternatives"] = public_alternatives
    rationale = document.get("rationale")
    if rationale is not None:
        rationale_value = _decision_text(
            rationale, "decision_rationale", MAX_DECISION_RATIONALE
        ).strip()
        if rationale_value:
            result["rationale"] = rationale_value
    reason = document.get("reason")
    if reason is not None:
        if reason not in DECISION_REASONS:
            raise LiveCanvasError("decision_reason_invalid")
        result["reason"] = reason
    judge = document.get("judge")
    if judge is not None:
        judge_value = _record(judge, "decision_judge")
        if set(judge_value) != {"engine", "model", "probability", "approved", "repaired"}:
            raise LiveCanvasError("decision_judge_invalid")
        approved = judge_value.get("approved")
        repaired = judge_value.get("repaired")
        if not isinstance(approved, bool) or not isinstance(repaired, bool):
            raise LiveCanvasError("decision_judge_invalid")
        result["judge"] = {
            "engine": _decision_text(judge_value.get("engine"), "judge_engine", 64),
            "model": _decision_text(judge_value.get("model"), "judge_model", 128),
            "probability": _decision_ratio(judge_value.get("probability"), "judge_probability"),
            "approved": approved,
            "repaired": repaired,
        }
    cursor = document.get("cursor")
    if cursor is not None:
        cursor_value = _record(cursor, "decision_cursor")
        if set(cursor_value) != {"from", "to"}:
            raise LiveCanvasError("decision_cursor_invalid")
        cursor_from = _integer(
            cursor_value.get("from"),
            "decision_cursor_from",
            maximum=9_007_199_254_740_991,
        )
        cursor_to = _integer(
            cursor_value.get("to"),
            "decision_cursor_to",
            maximum=9_007_199_254_740_991,
        )
        if cursor_to < cursor_from:
            raise LiveCanvasError("decision_cursor_invalid")
        result["cursor"] = {"from": cursor_from, "to": cursor_to}
    return result


def _board(value: object, disks: int) -> dict[str, list[int]]:
    board = _record(value, "board")
    if set(board) != {"A", "B", "C"}:
        raise LiveCanvasError("board_invalid")
    public_board: dict[str, list[int]] = {}
    seen: list[int] = []
    for rod in ("A", "B", "C"):
        stack = board[rod]
        if not isinstance(stack, list):
            raise LiveCanvasError("board_invalid")
        safe_stack = [
            _integer(disk, "disk", minimum=1, maximum=disks) for disk in stack
        ]
        if any(left <= right for left, right in pairwise(safe_stack)):
            raise LiveCanvasError("board_invalid")
        seen.extend(safe_stack)
        public_board[rod] = safe_stack
    if sorted(seen) != list(range(1, disks + 1)):
        raise LiveCanvasError("board_invalid")
    return public_board


def _safe_projection(
    projection_value: object,
    room_seq: object,
    labels: dict[str, str],
) -> tuple[dict[str, object], dict[str, object] | None]:
    """Select a bounded, identifier-free canvas DTO from a public Projection."""
    projection = _record(projection_value, "projection")
    if "activity" in projection:
        projection = _record(projection["activity"], "projection_activity")
    disks = _integer(projection.get("disks"), "disks", minimum=1, maximum=MAX_DISKS)
    objective = _record(projection.get("objective"), "objective")
    if (
        set(objective) != {"source_rod", "target_rod", "description"}
        or objective.get("source_rod") != "A"
        or objective.get("target_rod") != "C"
        or not isinstance(objective.get("description"), str)
        or not objective["description"]
    ):
        raise LiveCanvasError("objective_invalid")
    phase = projection.get("phase")
    if phase not in {"solving", "complete"}:
        raise LiveCanvasError("phase_invalid")
    outcome = _record(projection.get("outcome"), "outcome")
    if set(outcome) != {"moves", "status"} or outcome["status"] not in {
        "in_progress",
        "participant_accepted_completion",
    }:
        raise LiveCanvasError("outcome_invalid")
    room_sequence = _integer(room_seq, "room_seq", maximum=9_007_199_254_740_991)
    move_limit = _integer(
        _record(projection.get("rules"), "rules").get("move_limit"),
        "move_limit",
        minimum=1,
        maximum=MAX_MOVE_LIMIT,
    )
    work_revision = _integer(
        projection.get("work_revision"), "work_revision", maximum=MAX_MOVE_LIMIT
    )
    contributions = _record(projection.get("contributions_by_member"), "contributions")
    if len(contributions) > 16:
        raise LiveCanvasError("contributions_invalid")
    public_contributions: list[dict[str, object]] = []
    for member_id, count in sorted(contributions.items()):
        if not isinstance(member_id, str) or not member_id:
            raise LiveCanvasError("contributions_invalid")
        public_contributions.append(
            {
                "actor": labels.get(member_id, "solver"),
                "moves": _integer(count, "contribution", maximum=move_limit),
            }
        )

    completion = _record(projection.get("completion"), "completion")
    required = {
        "claim_open",
        "assessments_by_member",
        "endorsement_count",
        "approval_count",
        "quorum",
    }
    if not required.issubset(completion) or set(completion) - required - {"claim"}:
        raise LiveCanvasError("completion_invalid")
    claim_open = completion["claim_open"]
    assessments = _record(completion["assessments_by_member"], "claim_assessments")
    quorum = _integer(completion["quorum"], "completion_quorum", minimum=1, maximum=16)
    endorsement_count = _integer(
        completion["endorsement_count"], "endorsement_count", maximum=16
    )
    approval_count = _integer(
        completion["approval_count"], "approval_count", maximum=quorum
    )
    if not isinstance(claim_open, bool) or len(assessments) > 16:
        raise LiveCanvasError("completion_invalid")
    public_assessments: list[dict[str, str]] = []
    for member_id, assessment in sorted(assessments.items()):
        if not isinstance(member_id, str) or assessment not in {
            "endorse",
            "challenge",
            "defer",
        }:
            raise LiveCanvasError("completion_invalid")
        public_assessments.append(
            {"actor": labels.get(member_id, "solver"), "assessment": assessment}
        )
    if (
        sum(item["assessment"] == "endorse" for item in public_assessments)
        != endorsement_count
    ):
        raise LiveCanvasError("completion_invalid")
    claim_value = completion.get("claim")
    public_claim: dict[str, object] | None = None
    if claim_open:
        claim = _record(claim_value, "completion_claim")
        if set(claim) != {
            "claimant_member_id",
            "claim_round",
            "work_revision",
            "electorate_size",
            "quorum",
        }:
            raise LiveCanvasError("completion_invalid")
        claimant = claim["claimant_member_id"]
        if not isinstance(claimant, str) or not claimant:
            raise LiveCanvasError("completion_invalid")
        public_claim = {
            "actor": labels.get(claimant, "solver"),
            "claim_round": _integer(
                claim["claim_round"], "claim_round", minimum=1, maximum=MAX_ROUND
            ),
            "work_revision": _integer(
                claim["work_revision"], "claim_work_revision", maximum=MAX_MOVE_LIMIT
            ),
            "electorate_size": _integer(
                claim["electorate_size"], "electorate_size", minimum=1, maximum=16
            ),
            "quorum": _integer(claim["quorum"], "claim_quorum", minimum=1, maximum=16),
        }
        if (
            public_claim["work_revision"] != work_revision
            or public_claim["quorum"] != quorum
        ):
            raise LiveCanvasError("completion_invalid")
    elif (
        claim_value is not None
        or public_assessments
        or endorsement_count
        or approval_count
    ):
        raise LiveCanvasError("completion_invalid")

    latest_event: dict[str, object] | None = None
    last_move = projection.get("last_move")
    if last_move is not None:
        event = _record(last_move, "last_move")
        if set(event) != {"member_id", "move", "round"} or not isinstance(
            event["member_id"], str
        ):
            raise LiveCanvasError("last_move_invalid")
        latest_event = {
            "kind": "disk_moved",
            "actor": labels.get(event["member_id"], "solver"),
            "action_type": "move_disk",
            "status": "observed",
            "move": _move(event["move"], "last_move_move"),
            "round": _integer(
                event["round"], "last_move_round", minimum=1, maximum=MAX_ROUND
            ),
            "room_seq": room_sequence,
            "work_revision": work_revision,
        }
    return (
        {
            "room_seq": room_sequence,
            "board": _board(projection.get("board"), disks),
            "disks": disks,
            "phase": phase,
            "round": _integer(
                projection.get("round"), "round", minimum=1, maximum=MAX_ROUND
            ),
            "work_revision": work_revision,
            "move_limit": move_limit,
            "objective": {
                "source_rod": "A",
                "target_rod": "C",
                "description": objective["description"],
            },
            "outcome": {
                "moves": _integer(
                    outcome["moves"], "outcome_moves", maximum=move_limit
                ),
                "status": outcome["status"],
            },
            "contributions": public_contributions,
            "completion": {
                "claim_open": claim_open,
                "claim": public_claim,
                "assessments": public_assessments,
                "endorsement_count": endorsement_count,
                "approval_count": approval_count,
                "quorum": quorum,
                "review_attention_pending": max(
                    0,
                    (public_claim["electorate_size"] - 1 - len(public_assessments))
                    if public_claim
                    else 0,
                ),
            },
        },
        latest_event,
    )


def _latest_event_key(event: dict[str, object] | None) -> tuple[object, ...] | None:
    if event is None:
        return None
    kind = event.get("kind")
    if kind == "disk_moved":
        move = _record(event.get("move"), "browser_move")
        return (
            kind,
            event.get("round"),
            event.get("actor"),
            move.get("from"),
            move.get("to"),
            move.get("disk"),
        )
    return (
        kind,
        event.get("room_seq"),
        event.get("actor"),
        event.get("claim_round"),
        event.get("assessment"),
    )


def _safe_envelope(value: dict[str, object]) -> str:
    encoded = json.dumps(value, allow_nan=False, separators=(",", ":"), sort_keys=True)
    if len(encoded.encode("utf-8")) > MAX_BROWSER_EVENT_BYTES:
        raise LiveCanvasError("browser_event_oversize")
    return encoded


def _owner_only(path: Path) -> None:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise LiveCanvasError("observer_credential_unavailable") from error
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077:
        raise LiveCanvasError("observer_credential_unprotected")


def _labels(path: Path) -> dict[str, str]:
    _owner_only(path)
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise LiveCanvasError("labels_invalid") from error
    document = _record(value, "labels")
    members = _record(document.get("members"), "labels_members")
    if len(members) > 16:
        raise LiveCanvasError("labels_invalid")
    parsed: dict[str, str] = {}
    for member_id, label in members.items():
        if (
            not isinstance(member_id, str)
            or not isinstance(label, str)
            or MEMBER_LABEL_PATTERN.fullmatch(label) is None
        ):
            raise LiveCanvasError("labels_invalid")
        parsed[member_id] = label
    return parsed


@dataclass(frozen=True)
class ObserverConfig:
    membership_file: Path
    labels: dict[str, str]
    batch_ms: int


class BroadcastState:
    """A bounded fan-out of already-sanitized local browser events."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._subscribers: set[queue.Queue[tuple[int, str]]] = set()
        self._cursor = 0
        self._history: deque[tuple[int, str]] = deque(maxlen=MAX_REPLAYED_BATCHES)
        self.latest = self._encode({"type": "status", "state": "connecting"})

    def _encode(self, value: dict[str, object]) -> tuple[int, str]:
        self._cursor += 1
        envelope = {**value, "cursor": self._cursor}
        return self._cursor, _safe_envelope(envelope)

    def subscribe(
        self, after_cursor: str | None
    ) -> queue.Queue[tuple[int, str]] | None:
        with self._lock:
            if len(self._subscribers) >= MAX_BROWSER_CLIENTS:
                return None
            subscriber: queue.Queue[tuple[int, str]] = queue.Queue(
                maxsize=MAX_REPLAYED_BATCHES
            )
            try:
                cursor = int(after_cursor) if after_cursor is not None else None
            except ValueError:
                cursor = None
            if cursor is None:
                replay = [self.latest]
            else:
                replay = [item for item in self._history if item[0] > cursor]
            for item in replay:
                subscriber.put_nowait(item)
            self._subscribers.add(subscriber)
            return subscriber

    def unsubscribe(self, subscriber: queue.Queue[tuple[int, str]]) -> None:
        with self._lock:
            self._subscribers.discard(subscriber)

    def publish(self, value: dict[str, object]) -> None:
        with self._lock:
            encoded = self._encode(value)
            self.latest = encoded
            self._history.append(encoded)
            for subscriber in tuple(self._subscribers):
                try:
                    subscriber.put_nowait(encoded)
                except queue.Full:
                    try:
                        subscriber.get_nowait()
                        subscriber.put_nowait(encoded)
                    except queue.Empty:
                        pass


class HanoiObserver:
    """Consume one observer Observation Stream and publish only its public DTO."""

    def __init__(
        self,
        config: ObserverConfig,
        state: BroadcastState,
        stop: threading.Event,
        live_file: Path | None = None,
    ):
        self.config = config
        self.state = state
        self.stop = stop
        self.live_file = live_file
        self._live_notified = False
        self._last_event_key: tuple[object, ...] | None = None
        self._last_claim_key: tuple[object, ...] | None = None
        self._last_assessments: dict[str, str] = {}

    def run(self) -> None:
        try:
            asyncio.run(self._run())
        except Exception:  # noqa: BLE001 - browser state deliberately hides unknown upstream failures
            self.state.publish({"type": "status", "state": "unavailable"})

    async def _run(self) -> None:
        while not self.stop.is_set():
            try:
                await self._stream_once()
            except (
                CredentialError,
                LiveCanvasError,
                OSError,
                ProtocolError,
                TimeoutError,
            ):
                if not self.stop.is_set():
                    self.state.publish({"type": "status", "state": "reconnecting"})
                    await asyncio.sleep(0.5)

    async def _stream_once(self) -> None:
        _owner_only(self.config.membership_file)
        membership = load_membership(self.config.membership_file)
        if (
            membership.get("role") != "observer"
            or membership.get("pack", {}).get("id") != PACK_ID
        ):
            raise LiveCanvasError("observer_membership_required")
        client = Client(sdk_base_url(membership), membership["bearer"])
        room = await client.open_room(membership["room_id"], membership["member_id"])
        try:
            await room.sync()
            reset = room.last_projection_reset
            if isinstance(reset, dict):
                head = _record(reset.get("room_head"), "reset_head")
                projection, event = _safe_projection(
                    reset.get("projection"), head.get("room_seq"), self.config.labels
                )
            else:
                current = await client.projection(membership["room_id"])
                head = _record(current.get("room_head"), "current_head")
                projection, event = _safe_projection(
                    current.get("projection"), head.get("room_seq"), self.config.labels
                )
            self._publish("snapshot", projection, [event] if event is not None else [])
            if self.live_file is not None and not self._live_notified:
                _write_signal(self.live_file, {"status": "live"})
                self._live_notified = True

            stream: AsyncIterator[dict[str, Any]] = room.events()
            while not self.stop.is_set():
                frame = await _next_event(stream)
                projection, events = self._batch([frame])
                # Room.events() and Room.ack() both receive on the same websocket.
                # A prefetch task would race the acknowledgement's receive and make
                # the observer reconnect even though direct snapshots still work.
                await room.ack(frame["frame_seq"])
                self._publish("observation_batch", projection, events)
        finally:
            await room.close()

    def _batch(
        self, frames: list[dict[str, Any]]
    ) -> tuple[dict[str, object], list[dict[str, object]]]:
        events: list[dict[str, object]] = []
        projection: dict[str, object] | None = None
        for frame in frames:
            projection, event = _safe_projection(
                frame.get("observation"),
                frame.get("cause_room_seq"),
                self.config.labels,
            )
            key = _latest_event_key(event)
            if event is not None and key != self._last_event_key:
                events.append(event)
                self._last_event_key = key
        if projection is None:
            raise LiveCanvasError("observation_batch_invalid")
        completion = _record(projection.get("completion"), "browser_completion")
        claim = completion.get("claim")
        claim_key = (
            None
            if claim is None
            else (
                claim.get("actor"),
                claim.get("claim_round"),
                claim.get("work_revision"),
            )
        )
        if claim_key != self._last_claim_key:
            if isinstance(claim, dict):
                events.append(
                    {
                        "kind": "completion_claim_posted",
                        "actor": claim.get("actor", "solver"),
                        "action_type": "post_completion_claim",
                        "status": "observed",
                        "claim_open": True,
                        "claim_round": claim.get("claim_round"),
                        "work_revision": claim.get("work_revision"),
                        "quorum": claim.get("quorum"),
                        "approval_count": completion.get("approval_count"),
                        "endorsement_count": completion.get("endorsement_count"),
                        "room_seq": projection.get("room_seq"),
                    }
                )
            elif self._last_claim_key is not None:
                events.append(
                    {
                        "kind": "completion_claim_superseded",
                        "action_type": "post_completion_claim",
                        "status": "observed",
                        "claim_open": False,
                        "room_seq": projection.get("room_seq"),
                    }
                )
            self._last_claim_key = claim_key
            self._last_assessments = {}
        assessments = completion.get("assessments")
        if isinstance(assessments, list):
            current_assessments = {
                item["actor"]: item["assessment"]
                for item in assessments
                if isinstance(item, dict)
                and isinstance(item.get("actor"), str)
                and isinstance(item.get("assessment"), str)
            }
            for actor, assessment in current_assessments.items():
                if self._last_assessments.get(actor) != assessment:
                    events.append(
                        {
                            "kind": "completion_claim_assessed",
                            "actor": actor,
                            "action_type": "assess_claim",
                            "status": "observed",
                            "claim_open": bool(completion.get("claim_open")),
                            "assessment": assessment,
                            "claim_round": claim.get("claim_round")
                            if isinstance(claim, dict)
                            else None,
                            "quorum": completion.get("quorum"),
                            "approval_count": completion.get("approval_count"),
                            "endorsement_count": completion.get("endorsement_count"),
                            "room_seq": projection.get("room_seq"),
                        }
                    )
            self._last_assessments = current_assessments
        return projection, events[-MAX_RECENT_EVENTS:]

    def _publish(
        self, kind: str, projection: dict[str, object], events: list[dict[str, object]]
    ) -> None:
        recent = events[-MAX_RECENT_EVENTS:]
        if recent:
            self._last_event_key = _latest_event_key(recent[-1])
        self.state.publish(
            {
                "type": "hanoi",
                "kind": kind,
                "projection": projection,
                "events": recent,
            }
        )


class ReceiptTailer:
    """Tail the harness's owner-only, already-sanitized receipt sink."""

    def __init__(self, event_file: Path, state: BroadcastState, stop: threading.Event):
        self.event_file = event_file
        self.state = state
        self.stop = stop

    def run(self) -> None:
        offset = 0
        buffered = b""
        while not self.stop.wait(0.05):
            try:
                _owner_only(self.event_file)
                payload = self.event_file.read_bytes()
            except (LiveCanvasError, OSError):
                continue
            if len(payload) < offset:
                offset, buffered = 0, b""
            buffered += payload[offset:]
            offset = len(payload)
            lines = buffered.split(b"\n")
            buffered = lines.pop()
            for line in lines:
                if not line or len(line) > MAX_BROWSER_EVENT_BYTES:
                    continue
                try:
                    value = json.loads(line)
                    event, deadline = _safe_sink_event(value)
                except (LiveCanvasError, UnicodeError, json.JSONDecodeError):
                    continue
                if deadline is not None:
                    self.state.publish(
                        {
                            "type": "hanoi",
                            "kind": "gameplay_window",
                            "deadline": deadline,
                        }
                    )
                elif event is not None:
                    self.state.publish(
                        {"type": "hanoi", "kind": "receipt", "events": [event]}
                    )


def _safe_receipt_event(value: object) -> dict[str, object]:
    """Accept a supervisor receipt that contains no action payload or identity."""
    receipt = _record(value, "receipt")
    if receipt.get("schema") != "worldstream/tower-of-hanoi-live-receipt/v2":
        raise LiveCanvasError("receipt_invalid")
    allowed = {"schema", "status", "actor", "action_type", "room_seq", "work_revision"}
    if set(receipt) - allowed or not {
        "schema",
        "status",
        "actor",
        "action_type",
        "room_seq",
    }.issubset(receipt):
        raise LiveCanvasError("receipt_invalid")
    status, actor, action_type = (
        receipt["status"],
        receipt["actor"],
        receipt["action_type"],
    )
    if (
        status not in {"accepted", "stale", "rejected"}
        or action_type not in {"move_disk", "post_completion_claim", "assess_claim"}
        or not isinstance(actor, str)
        or MEMBER_LABEL_PATTERN.fullmatch(actor) is None
    ):
        raise LiveCanvasError("receipt_invalid")
    event: dict[str, object] = {
        "kind": f"{action_type}_{status}",
        "actor": actor,
        "action_type": action_type,
        "status": status,
        "room_seq": _integer(
            receipt["room_seq"], "receipt_room_seq", maximum=9_007_199_254_740_991
        ),
    }
    if "work_revision" in receipt:
        event["work_revision"] = _integer(
            receipt["work_revision"], "receipt_work_revision", maximum=MAX_MOVE_LIMIT
        )
    return event


def _safe_sink_event(
    value: object,
) -> tuple[dict[str, object] | None, dict[str, object] | None]:
    document = _record(value, "sink")
    schema = document.get("schema")
    if schema == DECISION_SCHEMA:
        return _safe_decision_event(document), None
    if schema in {
        "worldstream/tower-of-hanoi-live-receipt/v1",
        "worldstream/tower-of-hanoi-live-receipt/v2",
    }:
        return _safe_receipt_event(document), None
    if schema != "worldstream/tower-of-hanoi-live-window/v1" or set(document) != {
        "schema",
        "status",
        "deadline_at_ms",
    }:
        raise LiveCanvasError("sink_invalid")
    status = document["status"]
    if status not in {"running", "complete", "time_limit"}:
        raise LiveCanvasError("sink_invalid")
    return None, {
        "status": status,
        "deadline_at_ms": _integer(
            document["deadline_at_ms"], "deadline", maximum=9_007_199_254_740_991
        ),
    }


class CanvasServer(ThreadingHTTPServer):
    def __init__(
        self,
        address: tuple[str, int],
        document: Path,
        broadcasts: BroadcastState,
        stop: threading.Event,
    ):
        super().__init__(address, CanvasHandler)
        self.document = document
        self.broadcasts = broadcasts
        self.stop = stop


class CanvasHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:
        if self.path == "/":
            self._document()
        elif self.path == "/events":
            self._events()
        elif self.path == "/healthz":
            self._response(HTTPStatus.OK, b"ok\n", "text/plain; charset=utf-8")
        else:
            self._response(
                HTTPStatus.NOT_FOUND, b"not found\n", "text/plain; charset=utf-8"
            )

    def _headers(self, content_type: str, length: int | None = None) -> None:
        self.send_header("Content-Type", content_type)
        self.send_header("Cache-Control", "no-store, max-age=0")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header(
            "Content-Security-Policy",
            "default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'",
        )
        if length is not None:
            self.send_header("Content-Length", str(length))

    def _response(self, status: HTTPStatus, body: bytes, content_type: str) -> None:
        self.send_response(status)
        self._headers(content_type, len(body))
        self.end_headers()
        self.wfile.write(body)

    def _document(self) -> None:
        try:
            body = self.server.document.read_bytes()  # type: ignore[attr-defined]
        except OSError:
            self._response(
                HTTPStatus.SERVICE_UNAVAILABLE,
                b"demo unavailable\n",
                "text/plain; charset=utf-8",
            )
            return
        self._response(HTTPStatus.OK, body, "text/html; charset=utf-8")

    def _events(self) -> None:
        subscriber = self.server.broadcasts.subscribe(self.headers.get("Last-Event-ID"))  # type: ignore[attr-defined]
        if subscriber is None:
            self._response(
                HTTPStatus.SERVICE_UNAVAILABLE,
                b"viewer capacity reached\n",
                "text/plain; charset=utf-8",
            )
            return
        try:
            self.send_response(HTTPStatus.OK)
            self._headers("text/event-stream; charset=utf-8")
            self.send_header("Connection", "keep-alive")
            self.end_headers()
            self.wfile.flush()
            while not self.server.stop.is_set():  # type: ignore[attr-defined]
                try:
                    cursor, payload = subscriber.get(timeout=10)
                    self.wfile.write(f"id: {cursor}\n".encode("ascii"))
                    self.wfile.write(b"event: hanoi\n")
                    self.wfile.write(b"data: " + payload.encode("utf-8") + b"\n\n")
                except queue.Empty:
                    self.wfile.write(b": keepalive\n\n")
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError, OSError):
            return
        finally:
            self.server.broadcasts.unsubscribe(subscriber)  # type: ignore[attr-defined]

    def log_message(self, _format: str, *_arguments: object) -> None:
        """Do not place browser paths or upstream details in terminal output."""


def _write_ready(path: Path, url: str) -> None:
    _write_signal(path, {"status": "ready", "url": url})


def _write_signal(path: Path, value: dict[str, object]) -> None:
    encoded = _safe_envelope(value) + "\n"
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as output:
        output.write(encoded)
        output.flush()
        os.fsync(output.fileno())


def _arguments() -> argparse.Namespace:
    repository = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--membership-file", type=Path, required=True)
    parser.add_argument("--labels-file", type=Path, required=True)
    parser.add_argument(
        "--document",
        type=Path,
        default=repository / "web/demos/community-hanoi-live.html",
    )
    parser.add_argument("--ready-file", type=Path)
    parser.add_argument("--live-file", type=Path)
    parser.add_argument("--event-file", type=Path)
    parser.add_argument("--port", type=int, default=5190)
    parser.add_argument("--batch-ms", type=int, default=120)
    arguments = parser.parse_args()
    if not 1024 <= arguments.port <= 65535:
        parser.error("port must be between 1024 and 65535")
    if not 10 <= arguments.batch_ms <= 1_000:
        parser.error("batch-ms must be between 10 and 1000")
    if not arguments.document.is_file():
        parser.error("document must name the checked-in Community Hanoi canvas")
    for signal_file, option in (
        (arguments.ready_file, "ready-file"),
        (arguments.live_file, "live-file"),
    ):
        if signal_file is not None and signal_file.exists():
            parser.error(f"{option} must not already exist")
    return arguments


def main() -> int:
    arguments = _arguments()
    try:
        config = ObserverConfig(
            membership_file=arguments.membership_file.resolve(),
            labels=_labels(arguments.labels_file.resolve()),
            batch_ms=arguments.batch_ms,
        )
        _owner_only(config.membership_file)
        state = BroadcastState()
        stop = threading.Event()
        server = CanvasServer(
            ("127.0.0.1", arguments.port), arguments.document.resolve(), state, stop
        )
        url = f"http://127.0.0.1:{server.server_port}/"
        if arguments.ready_file is not None:
            _write_ready(arguments.ready_file.resolve(), url)
        print(_safe_envelope({"status": "ready", "url": url}), flush=True)
        observer = threading.Thread(
            target=HanoiObserver(
                config,
                state,
                stop,
                arguments.live_file.resolve()
                if arguments.live_file is not None
                else None,
            ).run,
            name="hanoi-live-observer",
            daemon=True,
        )
        observer.start()
        receipt_tailer = (
            threading.Thread(
                target=ReceiptTailer(arguments.event_file.resolve(), state, stop).run,
                name="hanoi-live-receipts",
                daemon=True,
            )
            if arguments.event_file is not None
            else None
        )
        if receipt_tailer is not None:
            receipt_tailer.start()
        try:
            server.serve_forever(poll_interval=0.2)
        except KeyboardInterrupt:
            pass
        finally:
            stop.set()
            server.shutdown()
            server.server_close()
            observer.join(timeout=2)
            if receipt_tailer is not None:
                receipt_tailer.join(timeout=2)
        return 0
    except (CredentialError, LiveCanvasError, OSError, ValueError):
        print(
            _safe_envelope({"status": "error", "code": "local_canvas_unavailable"}),
            flush=True,
        )
        return 3


if __name__ == "__main__":
    raise SystemExit(main())

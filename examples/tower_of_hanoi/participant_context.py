"""Bounded, reconnectable context for one Hanoi Participant Membership.

The Runner's claimed Invocation Context is the authority for a turn.  This
module owns the separate Membership Room connection used to acknowledge public
observations and recover a participant's cursor.  It deliberately never
replaces a claimed projection with a later sample: doing so would let a
provider reason about one Room Head and submit against another.
"""

from __future__ import annotations

import asyncio
import concurrent.futures
import copy
import hashlib
import json
import os
import stat
import tempfile
import threading
from collections import Counter, deque
from pathlib import Path
from typing import Any

from worldstream_sdk import ProtocolError

from examples.tower_of_hanoi.protocol import (
    HanoiProtocolError,
    activity_from_projection,
    board_fingerprint,
    offered_actions,
    room_seq_from_projection,
    safe_activity,
)

SCHEMA = "worldstream/tower-of-hanoi-participant-context/v1"
CHECKPOINT_SCHEMA = "worldstream/tower-of-hanoi-participant-checkpoint/v1"
MAX_CONTEXT_BYTES = 32_768
MAX_EVENTS = 32
MAX_DECISIONS = 8
MAX_VISITS = 64
MAX_EVENT_TEXT = 256


def _bounded_int(value: object, maximum: int = 2**63 - 1) -> int | None:
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= maximum:
        return None
    return value


def _bounded_text(value: object, maximum: int = MAX_EVENT_TEXT) -> str | None:
    if not isinstance(value, str) or not value:
        return None
    return value[:maximum]


def _safe_cursor(value: object) -> int | None:
    return _bounded_int(value)


def _fingerprint(activity: dict[str, object]) -> str:
    """Hash only public state facts; the digest is a cycle hint, not a solution."""
    value = {
        "board": activity.get("board"),
        "phase": activity.get("phase"),
        "claim_open": (
            activity.get("completion", {}).get("claim_open")
            if isinstance(activity.get("completion"), dict)
            else None
        ),
    }
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.blake2s(encoded, digest_size=8).hexdigest()


def _safe_event(frame: object, activity: dict[str, object] | None) -> dict[str, object]:
    """Reduce one observation to facts useful for a later bounded turn."""
    if not isinstance(frame, dict):
        return {"kind": "observation"}
    event: dict[str, object] = {"kind": "observation"}
    frame_seq = _safe_cursor(frame.get("frame_seq"))
    room_seq = _safe_cursor(frame.get("cause_room_seq"))
    if frame_seq is not None:
        event["frame_seq"] = frame_seq
    if room_seq is not None:
        event["room_seq"] = room_seq
    observation = frame.get("observation", frame.get("payload"))
    if isinstance(observation, dict) and isinstance(observation.get("observation"), dict):
        observation = observation["observation"]
    if activity is not None:
        event.update(
            {
                "phase": activity.get("phase"),
                "work_revision": activity.get("work_revision"),
                "outcome": activity.get("outcome", {}).get("status")
                if isinstance(activity.get("outcome"), dict)
                else None,
                "claim_open": activity.get("completion", {}).get("claim_open")
                if isinstance(activity.get("completion"), dict)
                else None,
            }
        )
    raw_projection = observation.get("projection", observation) if isinstance(observation, dict) else None
    raw_activity = (
        raw_projection.get("activity", raw_projection)
        if isinstance(raw_projection, dict)
        else None
    )
    last_move = raw_activity.get("last_move") if isinstance(raw_activity, dict) else None
    if isinstance(last_move, dict):
        move = last_move.get("move")
        if (
            isinstance(move, dict)
            and move.get("from") in {"A", "B", "C"}
            and move.get("to") in {"A", "B", "C"}
            and isinstance(move.get("disk"), int)
            and not isinstance(move.get("disk"), bool)
            and 1 <= move["disk"] <= 128
        ):
            event["move"] = {"from": move["from"], "to": move["to"], "disk": move["disk"]}
        actor = _bounded_text(last_move.get("member_id"), 64)
        if actor is not None and "wsb1:" not in actor.lower():
            event["actor"] = actor
    frame_kind = _bounded_text(frame.get("frame_kind"), 64)
    if frame_kind is not None:
        event["frame_kind"] = frame_kind
    return event


def _atomic_private_json(path: Path, value: dict[str, object]) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    if os.name != "nt" and stat.S_IMODE(path.parent.stat().st_mode) & 0o077:
        path.parent.chmod(0o700)
    value = _fit_checkpoint(value)
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    if len(encoded) > 4096:
        raise HanoiProtocolError("participant_checkpoint_oversize")
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary_path = Path(temporary)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "wb") as output:
            output.write(encoded)
            output.write(b"\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary_path, path)
        if os.name != "nt" and stat.S_IMODE(path.stat().st_mode) != 0o600:
            path.chmod(0o600)
    finally:
        temporary_path.unlink(missing_ok=True)


def load_checkpoint(path: Path | None) -> dict[str, object]:
    """Load only safe cursor metadata; malformed or absent state starts fresh."""
    if path is None:
        return {}
    try:
        metadata = path.lstat()
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077:
            return {}
        value = json.loads(path.read_bytes()[:8193].decode("utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return {}
    if not isinstance(value, dict) or value.get("schema") != CHECKPOINT_SCHEMA:
        return {}
    cursor = _safe_cursor(value.get("cursor"))
    room_seq = _safe_cursor(value.get("room_seq"))
    events = value.get("events")
    decisions = value.get("decisions")
    visits = value.get("visits")
    safe_events = (
        events[-MAX_EVENTS:]
        if isinstance(events, list) and all(isinstance(item, dict) for item in events)
        else []
    )
    safe_decisions = (
        decisions[-MAX_DECISIONS:]
        if isinstance(decisions, list) and all(isinstance(item, dict) for item in decisions)
        else []
    )
    safe_visits = (
        visits[-MAX_VISITS:]
        if isinstance(visits, list)
        and all(isinstance(item, str) and len(item) <= 32 for item in visits)
        else []
    )
    return {
        "schema": CHECKPOINT_SCHEMA,
        "cursor": cursor,
        "room_seq": room_seq,
        "projection_reset": bool(value.get("projection_reset", False)),
        "events": safe_events,
        "decisions": safe_decisions,
        "visits": safe_visits,
    }


class ParticipantContext:
    """Keep public observation state bounded while a Membership stays attached."""

    def __init__(
        self,
        room: Any,
        client: Any,
        room_id: str,
        *,
        checkpoint_file: Path | None = None,
        checkpoint: dict[str, object] | None = None,
    ) -> None:
        self.room = room
        self.client = client
        self.room_id = room_id
        self.checkpoint_file = checkpoint_file
        self._lock = threading.RLock()
        self._events: deque[dict[str, object]] = deque(maxlen=MAX_EVENTS)
        self._decisions: deque[dict[str, object]] = deque(maxlen=MAX_DECISIONS)
        self._visits: deque[str] = deque(maxlen=MAX_VISITS)
        self._visit_counts: Counter[str] = Counter()
        self._board_visits: deque[str] = deque(maxlen=MAX_VISITS)
        self._board_visit_counts: Counter[str] = Counter()
        self._stream: Any = None
        self._stopped = False
        self._failure: str | None = None
        self._loop: asyncio.AbstractEventLoop | None = None
        self._io_lock: asyncio.Lock | None = None
        self._action_lock: asyncio.Lock | None = None
        self._action_active = False
        self._action_done: asyncio.Event | None = None
        self._reader_done: asyncio.Event | None = None
        self._connection_generation = 0
        self._projection: dict[str, object] | None = None
        self._activity: dict[str, object] | None = None
        self._action_offers: list[str] = []
        self._room_seq: int | None = None
        self._cursor: int | None = _safe_cursor(
            (checkpoint or {}).get("cursor") if checkpoint else None
        )
        self._projection_reset = bool((checkpoint or {}).get("projection_reset", False))
        self._cursor_from_turn: int | None = self._cursor
        if checkpoint:
            for event in checkpoint.get("events", []):
                self._events.append(copy.deepcopy(event))
            for decision in checkpoint.get("decisions", []):
                self._decisions.append(copy.deepcopy(decision))
            for visit in checkpoint.get("visits", []):
                self._visits.append(visit)
                self._visit_counts[visit] += 1

    @property
    def cursor(self) -> int | None:
        with self._lock:
            return self._cursor

    @property
    def room_seq(self) -> int | None:
        with self._lock:
            return self._room_seq

    @property
    def failure_code(self) -> str | None:
        """Return a safe code when the observation task stopped unexpectedly."""
        with self._lock:
            return self._failure

    def _set_failure(self, code: str) -> None:
        with self._lock:
            if self._failure is None:
                self._failure = code[:96]
        self._checkpoint()

    def _ensure_async_state(self) -> None:
        if self._io_lock is None:
            self._io_lock = asyncio.Lock()
        if self._action_lock is None:
            self._action_lock = asyncio.Lock()
        if self._action_done is None:
            self._action_done = asyncio.Event()
            self._action_done.set()
        if self._reader_done is None:
            self._reader_done = asyncio.Event()
            self._reader_done.set()

    async def bootstrap(self) -> dict[str, object]:
        """Synchronize the participant connection and install one safe baseline."""
        self._loop = asyncio.get_running_loop()
        self._ensure_async_state()
        synchronized = await self.room.sync()
        projection = await self.client.projection(self.room_id)
        self._install_projection(projection, reset=self.room.last_projection_reset is not None)
        for frame in synchronized:
            if isinstance(frame, dict) and frame.get("frame_seq") is not None:
                self._record_frame(frame)
        await self._ack_sync_barrier(synchronized)
        self._checkpoint()
        return self.current_observation()

    async def _ack_sync_barrier(self, synchronized: object) -> None:
        """Advance the durable Observation Cursor after processing the sync batch."""
        barrier: int | None = None
        reset = self.room.last_projection_reset
        if isinstance(reset, dict):
            barrier = _safe_cursor(reset.get("baseline_frame_head"))
        elif isinstance(getattr(self.room, "attached", None), dict):
            branch = self.room.attached.get("sync")
            if isinstance(branch, dict):
                barrier = _safe_cursor(
                    branch.get("through_frame_head", branch.get("baseline_frame_head"))
                )
        if barrier is None and isinstance(synchronized, list):
            sequences = [
                _safe_cursor(frame.get("frame_seq"))
                for frame in synchronized
                if isinstance(frame, dict)
            ]
            valid = [sequence for sequence in sequences if sequence is not None]
            barrier = max(valid) if valid else None
        current = _safe_cursor(getattr(self.room, "cursor", None))
        if barrier is not None and (current is None or barrier > current):
            await self.room.ack(barrier)
            with self._lock:
                self._cursor = _safe_cursor(getattr(self.room, "cursor", None))

    def _install_projection(self, projection: object, *, reset: bool) -> None:
        activity = safe_activity(activity_from_projection(projection))
        sequence = room_seq_from_projection(projection)
        offers = offered_actions(projection)
        with self._lock:
            self._projection = copy.deepcopy(projection) if isinstance(projection, dict) else None
            self._activity = activity
            self._action_offers = list(offers)
            self._room_seq = sequence
            self._projection_reset = reset
            if reset:
                self._events.clear()
                self._visits.clear()
                self._visit_counts.clear()
                self._board_visits.clear()
                self._board_visit_counts.clear()
            self._record_visit_locked(activity)

    def _record_visit_locked(self, activity: dict[str, object]) -> None:
        key = _fingerprint(activity)
        if len(self._visits) == self._visits.maxlen:
            removed = self._visits.popleft()
            self._visit_counts[removed] -= 1
            if self._visit_counts[removed] <= 0:
                del self._visit_counts[removed]
        self._visits.append(key)
        self._visit_counts[key] += 1
        board = activity.get("board")
        if isinstance(board, dict):
            board_key = board_fingerprint(board)
            if len(self._board_visits) == self._board_visits.maxlen:
                removed_board = self._board_visits.popleft()
                self._board_visit_counts[removed_board] -= 1
                if self._board_visit_counts[removed_board] <= 0:
                    del self._board_visit_counts[removed_board]
            self._board_visits.append(board_key)
            self._board_visit_counts[board_key] += 1

    def _board_visit_counts_locked(self) -> dict[str, int]:
        return dict(self._board_visit_counts)

    def _record_frame(self, frame: object) -> bool:
        if not isinstance(frame, dict):
            return False
        frame_seq = _safe_cursor(frame.get("frame_seq"))
        cause_seq = _safe_cursor(frame.get("cause_room_seq"))
        observation = frame.get("observation", frame.get("payload"))
        if isinstance(observation, dict) and isinstance(observation.get("observation"), dict):
            observation = observation["observation"]
        if frame_seq is None or cause_seq is None or not isinstance(observation, dict):
            return False
        try:
            activity = safe_activity(activity_from_projection(observation))
        except HanoiProtocolError:
            return False
        with self._lock:
            if self._room_seq is not None and cause_seq < self._room_seq:
                # Keep the bounded event history for a retained sync suffix, but
                # never roll the current projection backwards.
                self._events.append(_safe_event(frame, activity))
                return False
            self._activity = activity
            # ObservationDeliver frames intentionally carry the activity delta;
            # action_offers may be null/omitted.  Offers are authoritative only
            # in the claimed Invocation Context, so retain the last safe list
            # for diagnostics until a claim supplies the exact offers.
            frame_body = (
                observation.get("projection", observation)
                if isinstance(observation, dict)
                else None
            )
            frame_offer_value = (
                frame_body.get("action_offers")
                if isinstance(frame_body, dict)
                else None
            )
            frame_offers = (
                list(self._action_offers)
                if frame_offer_value is None
                else offered_actions(observation)
            )
            if frame_offer_value is not None:
                self._action_offers = list(frame_offers)
            self._room_seq = cause_seq
            self._events.append(_safe_event(frame, activity))
            self._record_visit_locked(activity)
        return True

    async def consume_one(self) -> dict[str, object] | None:
        """Read exactly one frame, apply it, then ACK it before reading another."""
        self._ensure_async_state()
        if self._action_active and self._action_done is not None:
            await self._action_done.wait()
        if self._stream is None:
            self._stream = self.room.events()
        generation = self._connection_generation
        if self._reader_done is None:
            raise RuntimeError("participant_reader_state_missing")
        self._reader_done.clear()
        try:
            frame = await self._stream.__anext__()
        finally:
            # An Action gateway closes this websocket before reconnecting.  It
            # waits for this event so the old SDK reader cannot clear the newly
            # connected Room.websocket after a transport exception.
            self._reader_done.set()
        # An Action gateway may have reconnected the same Room while this read
        # was pending.  The old frame belongs to the old websocket and must not
        # be acknowledged or applied to the new cursor.
        if generation != self._connection_generation:
            return None
        frame_seq = _safe_cursor(frame.get("frame_seq")) if isinstance(frame, dict) else None
        if frame_seq is None:
            raise ProtocolError("invalid_envelope", "observation frame is invalid", False)
        if self._io_lock is None:
            raise RuntimeError("participant_io_state_missing")
        async with self._io_lock:
            if generation != self._connection_generation or self._action_active:
                return None
            self._record_frame(frame)
            await self.room.ack(frame_seq)
        with self._lock:
            self._cursor = frame_seq
        self._checkpoint()
        return frame

    async def resume(self) -> None:
        """Reconnect from the durable cursor; a reset replaces local observations."""
        self._ensure_async_state()
        if self._io_lock is None:
            raise RuntimeError("participant_io_state_missing")
        async with self._io_lock:
            await self._resume_locked()

    async def _resume_locked(self) -> None:
        self._connection_generation += 1
        await self.room.reconnect()
        self._stream = None
        synchronized = await self.room.sync()
        if self.room.last_projection_reset is not None:
            projection = await self.client.projection(self.room_id)
            self._install_projection(projection, reset=True)
        for frame in synchronized:
            if isinstance(frame, dict) and frame.get("observation") is not None:
                self._record_frame(frame)
        await self._ack_sync_barrier(synchronized)
        self._checkpoint()

    async def submit_action(
        self,
        action_type: str,
        payload: object,
        expected_room_seq: int,
        timeout: float,
    ) -> dict[str, object]:
        """Submit through this Participant's Room, fencing no sibling session.

        The observation reader is paused by closing and reconnecting this same
        SDK Room object.  This avoids two simultaneous SDK readers while the
        Action reply is correlated, and lets retained observations resume from
        the durable cursor immediately after the Action.
        """
        from examples.tower_of_hanoi import turn

        self._ensure_async_state()
        if self._action_lock is None:
            raise RuntimeError("participant_action_state_missing")
        async with self._action_lock:
            if self._io_lock is None or self._action_done is None:
                raise RuntimeError("participant_io_state_missing")
            async with self._io_lock:
                self._action_active = True
                self._action_done.clear()
                try:
                    self._stream = None
                    await self.room.close()
                    if self._reader_done is not None:
                        await self._reader_done.wait()
                    await self._resume_locked()
                    return await turn.act_on_room(
                        self.room,
                        self.client,
                        action_type,
                        payload,
                        expected_room_seq,
                        timeout,
                    )
                finally:
                    self._action_active = False
                    self._action_done.set()

    def submit_action_sync(
        self,
        action_type: str,
        payload: object,
        expected_room_seq: int,
        timeout: float,
    ) -> dict[str, object]:
        """Bridge a synchronous provider worker to the owning event loop."""
        if self._loop is not None and self._loop.is_running():
            try:
                running = asyncio.get_running_loop()
            except RuntimeError:
                running = None
            if running is self._loop:
                raise RuntimeError("participant_action_called_from_event_loop")
            future = asyncio.run_coroutine_threadsafe(
                self.submit_action(action_type, payload, expected_room_seq, timeout),
                self._loop,
            )
            try:
                return future.result(timeout=max(5.0, timeout + 10.0))
            except concurrent.futures.TimeoutError as error:
                future.cancel()
                raise TimeoutError("participant_action_timeout") from error
        from examples.tower_of_hanoi import turn

        return asyncio.run(
            turn.act_on_room(
                self.room,
                self.client,
                action_type,
                payload,
                expected_room_seq,
                timeout,
            )
        )

    async def consume_until(self, stop: asyncio.Event) -> None:
        """Keep the observation connection alive with bounded reconnects."""
        self._ensure_async_state()
        reconnect_failures = 0
        while not stop.is_set() and not self._stopped:
            try:
                frame = await self.consume_one()
                if frame is not None:
                    reconnect_failures = 0
            except asyncio.CancelledError:
                raise
            except StopAsyncIteration:
                if not stop.is_set() and not self._stopped:
                    self._set_failure("observation_stream_closed")
                return
            except (ProtocolError, OSError, TimeoutError):
                self._stream = None
                if stop.is_set() or self._stopped:
                    return
                if self._action_active and self._action_done is not None:
                    await self._action_done.wait()
                    continue
                await asyncio.sleep(0.2)
                try:
                    await self.resume()
                    reconnect_failures = 0
                except (ProtocolError, OSError, TimeoutError):
                    reconnect_failures += 1
                    if reconnect_failures >= 3:
                        self._set_failure("observation_reconnect_failed")
                        return
                    await asyncio.sleep(0.5)
            except (KeyError, RuntimeError, TypeError, ValueError):
                self._stream = None
                if not stop.is_set() and not self._stopped:
                    self._set_failure("observation_stream_failed")
                return

    def mark_turn(self, *, room_seq: int | None = None, cursor: int | None = None) -> None:
        with self._lock:
            if room_seq is None and cursor is None:
                self._events.clear()
                self._cursor_from_turn = self._cursor
            else:
                retained = [
                    event
                    for event in self._events
                    if (
                        isinstance(event.get("room_seq"), int)
                        and event["room_seq"] > (room_seq if room_seq is not None else -1)
                    )
                    or (
                        isinstance(event.get("frame_seq"), int)
                        and cursor is not None
                        and event["frame_seq"] > cursor
                    )
                ]
                self._events = deque(retained[-MAX_EVENTS:], maxlen=MAX_EVENTS)
                self._cursor_from_turn = cursor if cursor is not None else self._cursor
            self._checkpoint()

    def record_decision(self, decision: dict[str, object]) -> None:
        with self._lock:
            self._decisions.append(copy.deepcopy(decision))
            self._checkpoint()

    def current_observation(self) -> dict[str, object]:
        with self._lock:
            return {
                "room_seq": self._room_seq,
                "cursor": self._cursor,
                "activity": copy.deepcopy(self._activity),
                "action_offers": list(self._action_offers),
                "projection_reset": self._projection_reset,
            }

    def baseline_context(self, *, activation_reason: str) -> dict[str, object]:
        """Expose the synchronized baseline for the unclaimed bootstrap turn."""
        with self._lock:
            if self._activity is None:
                raise HanoiProtocolError("participant_projection_missing")
            value = {
                "schema": SCHEMA,
                "activation_reason": activation_reason,
                "room_seq": self._room_seq,
                "cursor": self._cursor,
                "cursor_range": {"from": self._cursor_from_turn, "to": self._cursor},
                "activity": copy.deepcopy(self._activity),
                "action_offers": list(self._action_offers),
                "events_since_turn": copy.deepcopy(list(self._events)),
                "recent_own_decisions": copy.deepcopy(list(self._decisions)),
                "state_visit": {
                    "count": self._visit_counts.get(_fingerprint(self._activity), 0),
                    "cycle_hint": self._visit_counts.get(_fingerprint(self._activity), 0) > 1,
                },
                "visited_boards": self._board_visit_counts_locked(),
                "delivery": {"kind": "baseline"},
            }
        return _fit_context(value)

    def context_for_claim(
        self,
        claimed: dict[str, object],
        *,
        activation_reason: str,
    ) -> dict[str, object]:
        """Make an atomic bounded provider context from one claimed Invocation Context."""
        if not isinstance(claimed, dict):
            raise HanoiProtocolError("activation_context_invalid")
        projection = claimed.get("projection")
        if not isinstance(projection, dict):
            raise HanoiProtocolError("activation_context_invalid")
        activity = safe_activity(activity_from_projection(projection))
        room_head = claimed.get("room_head")
        room_seq = _safe_cursor(claimed.get("cause_room_seq"))
        if not isinstance(room_head, dict) or room_seq is None:
            raise HanoiProtocolError("activation_context_invalid")
        if room_head.get("room_seq") != room_seq or room_head.get("room_id") != self.room_id:
            raise HanoiProtocolError("activation_context_invalid")
        offers = claimed.get("action_offers")
        if not isinstance(offers, list):
            raise HanoiProtocolError("activation_context_invalid")
        try:
            action_types = offered_actions({"action_offers": offers})
        except HanoiProtocolError as error:
            raise HanoiProtocolError("activation_context_invalid") from error
        # The claimed context's projection and offers are copied together.  The
        # attached Room may have advanced; its state is used only for diagnostics.
        view = dict(activity)
        view["action_offers"] = action_types
        with self._lock:
            events = [
                copy.deepcopy(event)
                for event in self._events
                if event.get("room_seq") is None or event.get("room_seq") <= room_seq
            ][-MAX_EVENTS:]
            decisions = copy.deepcopy(list(self._decisions))[-MAX_DECISIONS:]
            visits = self._visit_counts.get(_fingerprint(activity), 0)
            visited_boards = self._board_visit_counts_locked()
            cursor = _safe_cursor(claimed.get("cursor"))
        delivery = claimed.get("delivery")
        delivery_kind = delivery.get("kind") if isinstance(delivery, dict) else None
        bounded_delivery = {
            "kind": delivery_kind if delivery_kind in {"retained_frames", "projection_reset"} else None,
            "cursor_exclusive": _safe_cursor(delivery.get("cursor_exclusive"))
            if isinstance(delivery, dict)
            else None,
            "through_frame_head": _safe_cursor(delivery.get("through_frame_head"))
            if isinstance(delivery, dict)
            else None,
            "baseline_frame_head": _safe_cursor(delivery.get("baseline_frame_head"))
            if isinstance(delivery, dict)
            else None,
        }
        cursor_from = (
            bounded_delivery.get("cursor_exclusive")
            if delivery_kind == "retained_frames"
            else bounded_delivery.get("baseline_frame_head")
        )
        delivery_events: list[dict[str, object]] = []
        if isinstance(delivery, dict) and delivery_kind == "retained_frames":
            frames = delivery.get("frames")
            if isinstance(frames, list):
                for frame in frames[-MAX_EVENTS:]:
                    if not isinstance(frame, dict):
                        continue
                    frame_seq = _safe_cursor(frame.get("frame_seq"))
                    cause_seq = _safe_cursor(frame.get("cause_room_seq"))
                    if frame_seq is None or cause_seq is None or cause_seq > room_seq:
                        continue
                    delivery_events.append(_safe_event(frame, None))
        merged: dict[int, dict[str, object]] = {}
        for event in [*events, *delivery_events]:
            frame_seq = event.get("frame_seq")
            if isinstance(frame_seq, int) and not isinstance(frame_seq, bool):
                merged[frame_seq] = event
        merged_events = sorted(
            merged.values(), key=lambda item: int(item.get("frame_seq", 0))
        )[-MAX_EVENTS:]
        bounded = {
            "schema": SCHEMA,
            "activation_reason": activation_reason,
            "room_seq": room_seq,
            "cursor": cursor,
            "cursor_range": {"from": cursor_from, "to": cursor},
            "activity": activity,
            "action_offers": view["action_offers"],
            "events_since_turn": merged_events,
            "recent_own_decisions": decisions,
            "state_visit": {"count": visits, "cycle_hint": visits > 1},
            "visited_boards": visited_boards,
            "delivery": bounded_delivery,
        }
        return _fit_context(bounded)

    def _checkpoint(self) -> None:
        if self.checkpoint_file is None:
            return
        with self._lock:
            value = {
                "schema": CHECKPOINT_SCHEMA,
                "cursor": self._cursor,
                "room_seq": self._room_seq,
                "projection_reset": self._projection_reset,
                "events": list(self._events),
                "decisions": list(self._decisions),
                "visits": list(self._visits),
            }
        _atomic_private_json(self.checkpoint_file, value)

    async def close(self) -> None:
        self._stopped = True
        self._stream = None
        if self._action_done is not None:
            self._action_done.set()
        await self.room.close()


def _fit_context(value: dict[str, object]) -> dict[str, object]:
    """Keep the provider input below a stable bound by dropping oldest history."""
    result = copy.deepcopy(value)
    events = result.get("events_since_turn")
    decisions = result.get("recent_own_decisions")
    while len(json.dumps(result, separators=(",", ":")).encode()) > MAX_CONTEXT_BYTES:
        if isinstance(events, list) and events:
            events.pop(0)
            continue
        if isinstance(decisions, list) and decisions:
            decisions.pop(0)
            continue
        break
    if len(json.dumps(result, separators=(",", ":")).encode()) > MAX_CONTEXT_BYTES:
        raise HanoiProtocolError("participant_context_oversize")
    return result


def _fit_checkpoint(value: dict[str, object]) -> dict[str, object]:
    """Fit restart metadata below 4 KiB by dropping oldest continuity first."""
    result = copy.deepcopy(value)
    events = result.get("events")
    decisions = result.get("decisions")
    visits = result.get("visits")
    while len(json.dumps(result, separators=(",", ":")).encode()) > 4096:
        if isinstance(events, list) and events:
            events.pop(0)
            continue
        if isinstance(decisions, list) and decisions:
            decisions.pop(0)
            continue
        if isinstance(visits, list) and visits:
            visits.pop(0)
            continue
        break
    if len(json.dumps(result, separators=(",", ":")).encode()) > 4096:
        raise HanoiProtocolError("participant_checkpoint_oversize")
    return result

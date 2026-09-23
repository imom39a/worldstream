"""Bounded stream-context tests for the Hanoi Participant supervisor."""

from __future__ import annotations

import asyncio
import json
import stat
import tempfile
import unittest
from pathlib import Path
from unittest.mock import AsyncMock

from examples.tower_of_hanoi.participant_context import (
    CHECKPOINT_SCHEMA,
    ParticipantContext,
    load_checkpoint,
)


def _activity(room_seq: int = 0, *, last_move: object = None) -> dict[str, object]:
    return {
        "board": {"A": [3, 2], "B": [], "C": [1]},
        "completion": {
            "claim_open": False,
            "assessments_by_member": {},
            "endorsement_count": 0,
            "approval_count": 0,
            "quorum": 1,
        },
        "disks": 3,
        "last_move": last_move,
        "objective": {
            "source_rod": "A",
            "target_rod": "C",
            "description": "Participants may aim to move the full tower from A to C.",
        },
        "outcome": {"moves": room_seq, "status": "in_progress"},
        "phase": "solving",
        "round": max(1, room_seq + 1),
        "work_revision": room_seq,
    }


def _projection(room_seq: int = 0, *, last_move: object = None) -> dict[str, object]:
    return {
        "room_head": {"room_id": "room", "room_seq": room_seq},
        "projection": {
            **_activity(room_seq, last_move=last_move),
            "action_offers": ["move_disk", "post_completion_claim"],
        },
    }


class _Stream:
    def __init__(self, room: _Room, frames: list[dict[str, object]]) -> None:
        self.room = room
        self.frames = iter(frames)

    def __aiter__(self) -> _Stream:
        return self

    async def __anext__(self) -> dict[str, object]:
        if self.room.reading:
            raise AssertionError("concurrent observation read")
        self.room.reading = True
        try:
            return next(self.frames)
        except StopIteration as error:
            raise StopAsyncIteration from error
        finally:
            self.room.reading = False


class _Room:
    def __init__(self, frames: list[dict[str, object]] | None = None) -> None:
        self.room_id = "room"
        self.cursor = None
        self.attached = {"sync": {"kind": "retained_frames", "through_frame_head": 0}}
        self.last_projection_reset = None
        self.frames = frames or []
        self.reading = False
        self.acks: list[int] = []
        self.reconnects = 0

    async def sync(self) -> list[dict[str, object]]:
        return []

    def events(self) -> _Stream:
        return _Stream(self, self.frames)

    async def ack(self, frame_seq: int) -> None:
        if self.reading:
            raise AssertionError("ACK while read is pending")
        self.acks.append(frame_seq)
        self.cursor = frame_seq

    async def reconnect(self) -> None:
        self.reconnects += 1

    async def close(self) -> None:
        return None


class _ActionRoom(_Room):
    def __init__(self) -> None:
        super().__init__()
        self.actions: list[tuple[str, object, int]] = []

    async def act(
        self,
        action_type: str,
        payload: object,
        *,
        expected_room_seq: int,
        timeout: float,
    ) -> dict[str, object]:
        self.actions.append((action_type, payload, expected_room_seq))
        return {
            "action_id": "action",
            "code": "accepted",
            "room_head": {"room_id": "room", "room_seq": 1},
        }


class ParticipantContextTests(unittest.TestCase):
    def test_stream_reads_one_frame_then_acks_before_the_next_read(self) -> None:
        room = _Room(
            [
                {
                    "frame_seq": 1,
                    "cause_room_seq": 1,
                    "observation": _projection(1),
                },
                {
                    "frame_seq": 2,
                    "cause_room_seq": 2,
                    "observation": _projection(2),
                },
            ]
        )
        client = type("Client", (), {"projection": AsyncMock(return_value=_projection())})()
        context = ParticipantContext(room, client, "room")

        asyncio.run(context.bootstrap())
        asyncio.run(context.consume_one())
        asyncio.run(context.consume_one())

        self.assertEqual(room.acks, [0, 1, 2])
        self.assertEqual(context.room_seq, 2)

    def test_claim_context_uses_real_action_offer_objects_and_retained_delivery(self) -> None:
        room = _Room()
        client = type("Client", (), {"projection": AsyncMock(return_value=_projection())})()
        context = ParticipantContext(room, client, "room")
        claim = {
            "activation_id": "activation",
            "claim_id": "claim",
            "lease_generation": 1,
            "reason_code": "board_changed",
            "cause_room_seq": 0,
            "room_head": {"room_id": "room", "room_seq": 0},
            "cursor": 4,
            "projection": _activity(),
            "action_offers": [
                {"action_type": "move_disk", "payload_schema_digest": "blake3:" + "a" * 64},
                {"action_type": "post_completion_claim", "payload_schema_digest": "blake3:" + "b" * 64},
            ],
            "delivery": {
                "kind": "retained_frames",
                "cursor_exclusive": 2,
                "through_frame_head": 4,
                "frames": [
                    {"frame_seq": 3, "cause_room_seq": 0, "payload": _projection(0)},
                ],
            },
        }
        value = context.context_for_claim(claim, activation_reason="board_changed")
        self.assertEqual(value["action_offers"], ["move_disk", "post_completion_claim"])
        self.assertEqual(value["events_since_turn"][0]["frame_seq"], 3)
        self.assertLessEqual(len(json.dumps(value).encode()), 32_768)

    def test_checkpoint_is_private_and_restores_bounded_continuity(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "checkpoint.json"
            room = _Room()
            client = type("Client", (), {"projection": AsyncMock(return_value=_projection())})()
            context = ParticipantContext(room, client, "room", checkpoint_file=path)
            context.record_decision({"kind": "participant_decision", "selected": {"label": "wait"}})
            context.mark_turn()
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            loaded = load_checkpoint(path)
            self.assertEqual(loaded["schema"], CHECKPOINT_SCHEMA)
            self.assertEqual(len(loaded["decisions"]), 1)

    def test_turn_boundary_preserves_frames_that_arrived_during_inference(self) -> None:
        room = _Room()
        client = type("Client", (), {"projection": AsyncMock(return_value=_projection())})()
        context = ParticipantContext(room, client, "room")
        context._record_frame(
            {"frame_seq": 1, "cause_room_seq": 1, "observation": _projection(1)}
        )
        context._record_frame(
            {"frame_seq": 2, "cause_room_seq": 2, "observation": _projection(2)}
        )
        context.mark_turn(room_seq=1, cursor=1)
        self.assertEqual(
            [event["frame_seq"] for event in context.baseline_context(activation_reason="board_changed")["events_since_turn"]],
            [2],
        )

    def test_nested_activity_last_move_is_retained_as_safe_event_fact(self) -> None:
        room = _Room()
        client = type("Client", (), {"projection": AsyncMock(return_value=_projection())})()
        context = ParticipantContext(room, client, "room")
        nested = {
            "projection": {
                "activity": _activity(
                    1,
                    last_move={
                        "member_id": "solver-a",
                        "move": {"from": "A", "to": "B", "disk": 2},
                        "round": 2,
                    },
                ),
                "action_offers": ["move_disk"],
            }
        }
        context._record_frame({"frame_seq": 1, "cause_room_seq": 1, "observation": nested})
        event = context.baseline_context(activation_reason="board_changed")["events_since_turn"][0]
        self.assertEqual(event["move"], {"from": "A", "to": "B", "disk": 2})

    def test_activity_only_live_frame_keeps_previous_offers_and_acknowledges(self) -> None:
        room = _Room(
            [
                {
                    "frame_seq": 1,
                    "cause_room_seq": 1,
                    "observation": {"projection": {"activity": _activity(1)}},
                }
            ]
        )
        client = type("Client", (), {"projection": AsyncMock(return_value=_projection())})()
        context = ParticipantContext(room, client, "room")

        asyncio.run(context.bootstrap())
        asyncio.run(context.consume_one())

        self.assertEqual(room.acks, [0, 1])
        self.assertEqual(context.current_observation()["action_offers"], [
            "move_disk",
            "post_completion_claim",
        ])
        self.assertEqual(context.failure_code, None)

    def test_observation_stream_failure_is_recorded_for_supervisor(self) -> None:
        room = _Room()
        client = type("Client", (), {"projection": AsyncMock(return_value=_projection())})()
        context = ParticipantContext(room, client, "room")
        asyncio.run(context.bootstrap())
        stop = asyncio.Event()
        asyncio.run(context.consume_until(stop))
        self.assertEqual(context.failure_code, "observation_stream_closed")

    def test_action_gateway_uses_existing_room_object(self) -> None:
        room = _ActionRoom()
        client = type(
            "Client",
            (),
            {
                "projection": AsyncMock(
                    side_effect=[_projection(0), _projection(1)]
                )
            },
        )()
        context = ParticipantContext(room, client, "room")

        result = asyncio.run(
            context.submit_action(
                "move_disk",
                {"from": "A", "to": "B", "disk": 1},
                0,
                1,
            )
        )

        self.assertEqual(result["status"], "accepted")
        self.assertEqual(room.actions[0][0], "move_disk")


if __name__ == "__main__":
    unittest.main()

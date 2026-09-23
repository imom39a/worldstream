"""Focused safety checks for the local Community Hanoi browser bridge."""

from __future__ import annotations

import asyncio
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

from examples.tower_of_hanoi.live_canvas import (
    BroadcastState,
    HanoiObserver,
    LiveCanvasError,
    ObserverConfig,
    _safe_decision_event,
    _safe_projection,
    _safe_receipt_event,
    _safe_sink_event,
)

MEMBER_A = "01ARZ3NDEKTSV4RRFFQ69G5FH1"
MEMBER_B = "01ARZ3NDEKTSV4RRFFQ69G5FJ1"


def projection(*, last_move: object = None) -> dict[str, object]:
    return {
        "board": {"A": [3, 2], "B": [], "C": [1]},
        "completion": {
            "claim_open": False,
            "assessments_by_member": {},
            "endorsement_count": 0,
            "approval_count": 0,
            "quorum": 2,
        },
        "contributions_by_member": {MEMBER_A: 1, MEMBER_B: 0},
        "disks": 3,
        "last_move": last_move,
        "objective": {
            "source_rod": "A",
            "target_rod": "C",
            "description": "Participants may aim to move the full tower from A to C.",
        },
        "outcome": {"moves": 1, "status": "in_progress"},
        "phase": "solving",
        "round": 2,
        "work_revision": 1,
        "rules": {
            "largest_disk_on_bottom": True,
            "move_limit": 10_000,
            "one_disk_per_action": True,
        },
    }


class LiveCanvasTests(unittest.TestCase):
    def test_projection_reset_activity_wrapper_is_sanitized(self) -> None:
        public, event = _safe_projection(
            {"activity": projection()},
            1,
            {MEMBER_A: "solver-a", MEMBER_B: "solver-b"},
        )
        self.assertEqual(public["board"], {"A": [3, 2], "B": [], "C": [1]})
        self.assertEqual(public["objective"]["target_rod"], "C")
        self.assertIsNone(event)

    def test_projection_strips_member_identifiers_and_keeps_claim_assessments_labeled(
        self,
    ) -> None:
        value = projection(
            last_move={
                "member_id": MEMBER_A,
                "move": {"from": "A", "to": "C", "disk": 1},
                "round": 2,
            }
        )
        value["completion"] = {
            "claim_open": True,
            "claim": {
                "claimant_member_id": MEMBER_A,
                "claim_round": 3,
                "work_revision": 1,
                "electorate_size": 2,
                "quorum": 2,
            },
            "assessments_by_member": {MEMBER_B: "endorse"},
            "endorsement_count": 1,
            "approval_count": 2,
            "quorum": 2,
        }
        public, event = _safe_projection(
            value, 7, {MEMBER_A: "solver-a", MEMBER_B: "solver-b"}
        )
        self.assertEqual(
            public["contributions"],
            [{"actor": "solver-a", "moves": 1}, {"actor": "solver-b", "moves": 0}],
        )
        self.assertEqual(
            public["completion"],
            {
                "claim_open": True,
                "claim": {
                    "actor": "solver-a",
                    "claim_round": 3,
                    "work_revision": 1,
                    "electorate_size": 2,
                    "quorum": 2,
                },
                "assessments": [{"actor": "solver-b", "assessment": "endorse"}],
                "endorsement_count": 1,
                "approval_count": 2,
                "quorum": 2,
                "review_attention_pending": 0,
            },
        )
        self.assertNotIn(MEMBER_A, str(public))
        self.assertNotIn("member_id", str(public))
        self.assertEqual(
            event,
            {
                "kind": "disk_moved",
                "actor": "solver-a",
                "action_type": "move_disk",
                "status": "observed",
                "move": {"from": "A", "to": "C", "disk": 1},
                "round": 2,
                "room_seq": 7,
                "work_revision": 1,
            },
        )

    def test_invalid_browser_receipt_is_rejected(self) -> None:
        with self.assertRaisesRegex(LiveCanvasError, "receipt_invalid"):
            _safe_receipt_event({"status": "accepted"})
        with self.assertRaisesRegex(LiveCanvasError, "receipt_invalid"):
            _safe_receipt_event(
                {
                    "schema": "worldstream/tower-of-hanoi-live-receipt/v2",
                    "status": "accepted",
                    "actor": "solver-a",
                    "action_type": "obsolete_action",
                    "room_seq": 7,
                }
            )

    def test_decision_sink_is_bounded_and_strips_unlisted_provider_fields(self) -> None:
        event, deadline = _safe_sink_event(
            {
                "schema": "worldstream/tower-of-hanoi-decision/v1",
                "kind": "participant_decision",
                "actor": "solver-a",
                "engine": "openrouter",
                "model": "openai/gpt-5.6-luna",
                "observed_room_seq": 18,
                "activation_reason": "board_changed",
                "disposition": "act",
                "selected": {"label": "move:A>C:1", "action_type": "move_disk"},
                "status": "accepted",
                "confidence": 0.91,
                "ranked_alternatives": [
                    {"label": "move:A>C:1", "score": 0.91},
                    {"label": "move:A>B:1", "score": 0.09},
                ],
                "rationale": "Advance the tower while preserving the legal move set.",
                "reason": "break_cycle",
                "latency_ms": 312,
                "recent_event_count": 4,
                "cursor": {"from": 14, "to": 18},
            }
        )
        self.assertIsNone(deadline)
        self.assertEqual(event["kind"], "participant_decision")
        self.assertEqual(event["selected"]["action_type"], "move_disk")
        self.assertEqual(event["ranked_alternatives"][0]["score"], 0.91)
        self.assertEqual(event["reason"], "break_cycle")
        self.assertNotIn("schema", event)
        self.assertNotIn("raw_prompt", event)

    def test_decision_sink_supports_wait_without_an_action(self) -> None:
        event = _safe_decision_event(
            {
                "schema": "worldstream/tower-of-hanoi-decision/v1",
                "kind": "participant_decision",
                "actor": "solver-a",
                "engine": "openrouter",
                "model": "openai/gpt-5.6-luna",
                "observed_room_seq": 18,
                "activation_reason": "claim_review_requested",
                "disposition": "wait",
                "selected": {"label": "wait", "action_type": None},
                "status": "error",
                "latency_ms": 120,
                "recent_event_count": 1,
            }
        )
        self.assertEqual(event["disposition"], "wait")
        self.assertEqual(event["selected"], {"label": "wait"})

    def test_decision_sink_rejects_unbounded_or_unknown_fields(self) -> None:
        base = {
            "schema": "worldstream/tower-of-hanoi-decision/v1",
            "kind": "participant_decision",
            "actor": "solver-a",
            "engine": "jev",
            "model": "jev-1.13.0",
            "observed_room_seq": 1,
            "activation_reason": "bootstrap",
            "disposition": "act",
            "selected": {"label": "move:A>C:1", "action_type": "move_disk"},
            "latency_ms": 1,
            "recent_event_count": 0,
        }
        with self.assertRaisesRegex(LiveCanvasError, "decision_invalid"):
            _safe_decision_event({**base, "secret": "wsb1:" + "a" * 64})
        with self.assertRaisesRegex(LiveCanvasError, "ranked_alternatives_invalid"):
            _safe_decision_event(
                {
                    **base,
                    "ranked_alternatives": [
                        {"label": str(index), "score": 0.1} for index in range(9)
                    ],
                }
            )
        with self.assertRaisesRegex(LiveCanvasError, "decision_invalid"):
            _safe_decision_event({**base, "disposition": "accepted"})

    def test_supervisor_receipt_exposes_only_labeled_action_result(self) -> None:
        event = _safe_receipt_event(
            {
                "schema": "worldstream/tower-of-hanoi-live-receipt/v2",
                "status": "accepted",
                "actor": "solver-a",
                "action_type": "assess_claim",
                "room_seq": 7,
                "work_revision": 1,
            }
        )
        self.assertEqual(
            event,
            {
                "kind": "assess_claim_accepted",
                "actor": "solver-a",
                "action_type": "assess_claim",
                "status": "accepted",
                "room_seq": 7,
                "work_revision": 1,
            },
        )
        self.assertNotIn(MEMBER_A, str(event))

    def test_v2_supervisor_receipt_exposes_action_status(self) -> None:
        event, deadline = _safe_sink_event(
            {
                "schema": "worldstream/tower-of-hanoi-live-receipt/v2",
                "status": "stale",
                "actor": "solver-a",
                "action_type": "move_disk",
                "room_seq": 8,
                "work_revision": 7,
            }
        )
        self.assertIsNone(deadline)
        self.assertEqual(
            event,
            {
                "kind": "move_disk_stale",
                "actor": "solver-a",
                "action_type": "move_disk",
                "status": "stale",
                "room_seq": 8,
                "work_revision": 7,
            },
        )

    def test_projection_accepts_a_ten_disk_board(self) -> None:
        ten_disks = projection()
        ten_disks["board"] = {"A": list(range(10, 0, -1)), "B": [], "C": []}
        ten_disks["disks"] = 10
        ten_disks["outcome"] = {"moves": 0, "status": "in_progress"}
        ten_disks["round"] = 1
        ten_disks["work_revision"] = 0
        public, event = _safe_projection(ten_disks, 0, {MEMBER_A: "solver-a"})
        self.assertEqual(public["disks"], 10)
        self.assertEqual(public["board"]["A"][0], 10)
        self.assertIsNone(event)

    def test_two_subscribers_receive_one_copy_of_a_new_batch(self) -> None:
        broadcasts = BroadcastState()
        first = broadcasts.subscribe(None)
        second = broadcasts.subscribe(None)
        assert first is not None and second is not None
        first.get_nowait()
        second.get_nowait()
        broadcasts.publish({"type": "hanoi", "kind": "receipt", "events": []})
        self.assertEqual(first.get_nowait(), second.get_nowait())
        self.assertTrue(first.empty())
        self.assertTrue(second.empty())

    def test_reconnect_replays_only_batches_after_the_last_cursor(self) -> None:
        broadcasts = BroadcastState()
        first = broadcasts.subscribe(None)
        assert first is not None
        initial_cursor, _ = first.get_nowait()
        broadcasts.publish({"type": "hanoi", "kind": "snapshot", "events": []})
        cursor, expected = first.get_nowait()
        reconnect = broadcasts.subscribe(str(initial_cursor))
        assert reconnect is not None
        self.assertEqual(reconnect.get_nowait(), (cursor, expected))
        self.assertTrue(reconnect.empty())
        caught_up = broadcasts.subscribe(str(cursor))
        assert caught_up is not None
        self.assertTrue(caught_up.empty())

    def test_observer_ack_has_no_prefetched_receive_and_repeats_serially(self) -> None:
        stop = threading.Event()

        class FakeStream:
            def __init__(self) -> None:
                self.frames = [
                    {
                        "frame_seq": 1,
                        "cause_room_seq": 1,
                        "observation": projection(
                            last_move={
                                "member_id": MEMBER_A,
                                "move": {"from": "A", "to": "C", "disk": 1},
                                "round": 2,
                            }
                        ),
                    },
                    {
                        "frame_seq": 2,
                        "cause_room_seq": 2,
                        "observation": projection(),
                    },
                ]
                self.receivers = 0

            def __aiter__(self):
                return self

            async def __anext__(self):
                self.receivers += 1
                try:
                    if self.frames:
                        return self.frames.pop(0)
                    raise StopAsyncIteration
                finally:
                    self.receivers -= 1

        class FakeRoom:
            def __init__(self, stream: FakeStream) -> None:
                self.last_projection_reset = {
                    "projection": {"activity": projection()},
                    "room_head": {"room_seq": 0},
                }
                self.stream = stream
                self.acknowledged: list[int] = []

            async def sync(self) -> list[dict[str, object]]:
                return []

            def events(self) -> FakeStream:
                return self.stream

            async def ack(self, frame_seq: int) -> None:
                self.assert_no_pending_receive()
                self.acknowledged.append(frame_seq)
                if len(self.acknowledged) == 2:
                    stop.set()

            def assert_no_pending_receive(self) -> None:
                if self.stream.receivers:
                    raise AssertionError("observation receive remained pending during ack")

            async def close(self) -> None:
                return None

        class FakeClient:
            def __init__(self, room: FakeRoom) -> None:
                self.room = room

            async def open_room(self, _room_id: str, _member_id: str) -> FakeRoom:
                return self.room

        stream = FakeStream()
        room = FakeRoom(stream)
        client = FakeClient(room)
        membership = {
            "schema": "worldstream/membership-credentials/v1",
            "role": "observer",
            "pack": {"id": "worldstream.tower-of-hanoi"},
            "room_id": "room",
            "member_id": "member",
            "bearer": "wsb1:" + "a" * 64,
            "runtime_url": "ws://127.0.0.1:9999/v1/stream",
        }
        observer = HanoiObserver(
            ObserverConfig(Path("/private/observer.json"), {}, 120),
            BroadcastState(),
            stop,
        )

        with (
            patch("examples.tower_of_hanoi.live_canvas._owner_only"),
            patch("examples.tower_of_hanoi.live_canvas.load_membership", return_value=membership),
            patch("examples.tower_of_hanoi.live_canvas.Client", return_value=client),
        ):
            asyncio.run(observer._stream_once())

        self.assertEqual(room.acknowledged, [1, 2])
        self.assertEqual(stream.receivers, 0)


if __name__ == "__main__":
    unittest.main()

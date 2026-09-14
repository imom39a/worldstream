"""Focused safety checks for the local Community Hanoi browser bridge."""

from __future__ import annotations

import unittest

from examples.tower_of_hanoi.live_canvas import (
    BroadcastState,
    LiveCanvasError,
    _safe_projection,
    _safe_receipt_event,
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
                "room_seq": 7,
                "work_revision": 1,
            },
        )
        self.assertNotIn(MEMBER_A, str(event))

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


if __name__ == "__main__":
    unittest.main()

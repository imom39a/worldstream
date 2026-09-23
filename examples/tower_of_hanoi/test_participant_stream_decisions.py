"""Decision staging and trace safety tests for direct Hanoi providers."""

from __future__ import annotations

import json
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock, patch

from examples.tower_of_hanoi.participant_runner import (
    ActionGrant,
    ModelLadder,
    ParticipantRunnerError,
    _actionable_candidates,
    _activation_retry_delay,
    _choice_history,
    _http_json,
    _immediate_undo_label,
    _invoke_provider,
    _jev_judge,
    _jev_staged_choice,
    _judged_choice,
    _legal_candidates,
    _openrouter_staged_choice,
    _provider_state,
    _recent_move_events,
    _select_offer,
)
from examples.tower_of_hanoi.protocol import (
    HanoiProtocolError,
    board_fingerprint,
    resolve_solver_decision,
)


def _activity(*, claim: bool = False) -> dict[str, object]:
    completion: dict[str, object] = {
        "claim_open": claim,
        "assessments_by_member": {},
        "endorsement_count": 0,
        "approval_count": 0,
        "quorum": 1,
    }
    if claim:
        completion["claim"] = {
            "claimant_member_id": "member",
            "claim_round": 1,
            "work_revision": 0,
            "electorate_size": 1,
            "quorum": 1,
        }
    return {
        "board": {"A": [3, 2], "B": [], "C": [1]},
        "completion": completion,
        "disks": 3,
        "objective": {
            "source_rod": "A",
            "target_rod": "C",
            "description": "Move the full tower from A to C.",
        },
        "outcome": {"moves": 0, "status": "in_progress"},
        "phase": "solving",
        "round": 1,
        "work_revision": 0,
    }


class ParticipantStreamDecisionTests(unittest.TestCase):
    def test_immediate_undo_is_detected_from_recent_moves(self) -> None:
        context = {
            "events_since_turn": [
                {"kind": "disk_moved", "actor": "ada", "room_seq": 5, "move": {"from": "A", "to": "B", "disk": 1}},
                {"kind": "disk_moved", "actor": "ada", "room_seq": 6, "move": {"from": "B", "to": "A", "disk": 1}},
            ],
            "state_visit": {"count": 3, "cycle_hint": True},
        }
        candidates = _legal_candidates(
            {
                "board": {"A": [3, 2, 1], "B": [], "C": []},
                "disks": 3,
                "action_offers": ["move_disk"],
                "work_revision": 6,
            }
        )
        # The most recent move is B->A:1, so its reverse A->B:1 is the undo.
        self.assertEqual(_immediate_undo_label(candidates, context), "move:A>B:1")
        self.assertEqual(_recent_move_events(context)[-1]["to"], "A")
        self.assertIsNone(_immediate_undo_label(candidates, None))
        state = _provider_state(
            {
                "board": {"A": [3, 2, 1], "B": [], "C": []},
                "disks": 3,
                "work_revision": 6,
                "objective": {"source_rod": "A", "target_rod": "C", "description": "Move to C."},
                "outcome": {"moves": 6, "status": "in_progress"},
                "phase": "solving",
                "round": 7,
                "completion": {"claim_open": False, "assessments_by_member": {}, "endorsement_count": 0, "approval_count": 0, "quorum": 1},
            },
            candidates,
            "board_changed",
            room_seq=6,
            context=context,
        )
        self.assertTrue(state["cycle_hint"])
        self.assertEqual(state["recent_moves"][-1]["from"], "B")

    def test_invoke_provider_keeps_the_immediate_undo_and_annotates_it(self) -> None:
        context = {
            "schema": "worldstream/tower-of-hanoi-participant-context/v1",
            "room_seq": 6,
            "cursor": 6,
            "cursor_range": {"from": 5, "to": 6},
            "activity": {
                "board": {"A": [3, 2], "B": [1], "C": []},
                "disks": 3,
                "objective": {"source_rod": "A", "target_rod": "C", "description": "Move to C."},
                "outcome": {"moves": 1, "status": "in_progress"},
                "phase": "solving",
                "round": 2,
                "work_revision": 1,
                "completion": {"claim_open": False, "assessments_by_member": {}, "endorsement_count": 0, "approval_count": 0, "quorum": 1},
            },
            "action_offers": ["move_disk"],
            "events_since_turn": [
                {"kind": "disk_moved", "actor": "ada", "room_seq": 6, "move": {"from": "A", "to": "B", "disk": 1}},
            ],
            "recent_own_decisions": [],
            "state_visit": {"count": 2, "cycle_hint": True},
            "delivery": {"kind": "retained_frames"},
        }
        evidence = Path(tempfile.mkdtemp()) / "events.jsonl"
        captured: dict[str, object] = {}

        def fake_stage(state, candidates, model, key, timeout):
            captured["state"] = state
            captured["candidates"] = dict(candidates)
            return "move:B>A:1", "act", {"confidence": 0.8}

        with (
            patch("examples.tower_of_hanoi.participant_runner._provider_key", return_value="key"),
            patch("examples.tower_of_hanoi.participant_runner._jev_staged_choice", side_effect=fake_stage),
            patch(
                "examples.tower_of_hanoi.participant_runner.turn.act",
                new_callable=AsyncMock,
                return_value={"status": "accepted", "room_seq": 7, "action_type": "move_disk"},
            ),
        ):
            result = _invoke_provider(
                engine="jev",
                model="jev-1.13.0",
                repo=Path("/repo"),
                membership_file=Path("/membership"),
                timeout_seconds=1,
                reason="board_changed",
                evidence=evidence,
                invocation_name="anti-cycle",
                seat="ada",
                event_file=None,
                cancelled=None,
                invocation_context=context,
            )
        self.assertTrue(result)
        candidates = captured["candidates"]
        self.assertIn("move:B>A:1", candidates)
        history = captured["state"]["choice_history"]
        self.assertTrue(history["move:B>A:1"]["undoes_last"])
        self.assertFalse(history["move:B>A:1"]["repeats_recent"])

    def test_choice_history_annotates_revisited_moves_and_exempts_the_target(self) -> None:
        view = {
            "board": {"A": [3], "B": [2, 1], "C": []},
            "disks": 3,
            "action_offers": ["move_disk"],
        }
        candidates = _legal_candidates(view)
        visited = {board_fingerprint({"A": [3], "B": [2], "C": [1]}): 2}
        history = _choice_history(
            view, candidates, {"visited_boards": visited}, set(), None
        )
        self.assertEqual(history["move:B>C:1"]["visits"], 2)
        self.assertEqual(history["move:B>A:1"]["visits"], 0)
        # A move that reaches the objective board is never treated as a repeat.
        near = {
            "board": {"A": [1], "B": [], "C": [3, 2]},
            "disks": 3,
            "action_offers": ["move_disk"],
        }
        solved = {board_fingerprint({"A": [], "B": [], "C": [3, 2, 1]}): 1}
        near_history = _choice_history(
            near, _legal_candidates(near), {"visited_boards": solved}, set(), None
        )
        self.assertEqual(near_history["move:A>C:1"]["visits"], 0)

    def test_invoke_provider_retains_and_annotates_revisited_moves(self) -> None:
        context = {
            "schema": "worldstream/tower-of-hanoi-participant-context/v1",
            "room_seq": 8,
            "cursor": 8,
            "cursor_range": {"from": 7, "to": 8},
            "activity": {
                "board": {"A": [3], "B": [2, 1], "C": []},
                "disks": 3,
                "objective": {"source_rod": "A", "target_rod": "C", "description": "Move to C."},
                "outcome": {"moves": 8, "status": "in_progress"},
                "phase": "solving",
                "round": 9,
                "work_revision": 8,
                "completion": {"claim_open": False, "assessments_by_member": {}, "endorsement_count": 0, "approval_count": 0, "quorum": 1},
            },
            "action_offers": ["move_disk"],
            "events_since_turn": [],
            "recent_own_decisions": [],
            "state_visit": {"count": 2, "cycle_hint": True},
            "visited_boards": {board_fingerprint({"A": [3], "B": [2], "C": [1]}): 2},
            "delivery": {"kind": "retained_frames"},
        }
        evidence = Path(tempfile.mkdtemp()) / "events.jsonl"
        captured: dict[str, object] = {}

        def fake_stage(state, candidates, model, key, timeout):
            captured["state"] = state
            captured["candidates"] = dict(candidates)
            return "move:B>A:1", "act", {"confidence": 0.5}

        with (
            patch("examples.tower_of_hanoi.participant_runner._provider_key", return_value="key"),
            patch("examples.tower_of_hanoi.participant_runner._jev_staged_choice", side_effect=fake_stage),
            patch(
                "examples.tower_of_hanoi.participant_runner.turn.act",
                new_callable=AsyncMock,
                return_value={"status": "accepted", "room_seq": 9, "action_type": "move_disk"},
            ),
        ):
            result = _invoke_provider(
                engine="jev",
                model="jev-1.13.0",
                repo=Path("/repo"),
                membership_file=Path("/membership"),
                timeout_seconds=1,
                reason="board_changed",
                evidence=evidence,
                invocation_name="repeat",
                seat="ada",
                event_file=None,
                cancelled=None,
                invocation_context=context,
            )
        self.assertTrue(result)
        candidates = captured["candidates"]
        self.assertIn("move:B>C:1", candidates)
        history = captured["state"]["choice_history"]
        self.assertEqual(history["move:B>C:1"]["visits"], 2)

    def test_target_reached_is_surfaced_and_claims(self) -> None:
        view = {
            "board": {"A": [], "B": [], "C": [3, 2, 1]},
            "disks": 3,
            "work_revision": 20,
            "round": 21,
            "phase": "solving",
            "objective": {"source_rod": "A", "target_rod": "C", "description": "Move to C."},
            "outcome": {"moves": 20, "status": "in_progress"},
            "completion": {"claim_open": False, "assessments_by_member": {}, "endorsement_count": 0, "approval_count": 0, "quorum": 1},
            "action_offers": ["move_disk", "post_completion_claim"],
        }
        candidates = _legal_candidates(view)
        state = _provider_state(view, candidates, "board_changed", room_seq=20)
        self.assertTrue(state["target_reached"])
        response = {
            "model": "jev-1.13.0",
            "answers": {"action": {"type": "choice", "choice": "post_completion_claim"}},
        }
        with patch(
            "examples.tower_of_hanoi.participant_runner._http_json", return_value=response
        ) as request:
            choice, disposition, _metadata = _jev_staged_choice(
                state, candidates, "jev-1.13.0", "key", 1
            )
        self.assertEqual((choice, disposition), ("post_completion_claim", "claim"))
        criteria = request.call_args.args[1]["questions"]["action"]["criteria"]
        self.assertIn("already complete", criteria["post_completion_claim"])
        self.assertIn("off the completed tower", criteria["move:C>A:1"])

    def test_open_claim_steers_to_endorsement_not_another_claim(self) -> None:
        view = {
            "board": {"A": [], "B": [], "C": [3, 2, 1]},
            "disks": 3,
            "work_revision": 5,
            "round": 6,
            "phase": "solving",
            "objective": {"source_rod": "A", "target_rod": "C", "description": "Move to C."},
            "outcome": {"moves": 5, "status": "in_progress"},
            "completion": {
                "claim_open": True,
                "claim": {
                    "claimant_member_id": "nova",
                    "claim_round": 6,
                    "work_revision": 5,
                    "electorate_size": 3,
                    "quorum": 2,
                },
                "assessments_by_member": {},
                "endorsement_count": 0,
                "approval_count": 1,
                "quorum": 2,
            },
            "action_offers": ["assess_claim", "move_disk", "post_completion_claim"],
        }
        candidates = _legal_candidates(view)
        state = _provider_state(view, candidates, "claim_review_requested", room_seq=5)
        response = {
            "model": "jev-1.13.0",
            "answers": {"action": {"type": "choice", "choice": "assess:endorse"}},
        }
        with patch(
            "examples.tower_of_hanoi.participant_runner._http_json", return_value=response
        ) as request:
            choice, disposition, _metadata = _jev_staged_choice(
                state, candidates, "jev-1.13.0", "key", 1
            )
        self.assertEqual((choice, disposition), ("assess:endorse", "assess"))
        criteria = request.call_args.args[1]["questions"]["action"]["criteria"]
        self.assertIn("already open", criteria["post_completion_claim"])
        self.assertIn("endorsing records", criteria["assess:endorse"])

    def test_canonical_decision_accepts_action_payload_or_label(self) -> None:
        candidates = _legal_candidates(
            {**_activity(), "action_offers": ["move_disk", "post_completion_claim"]}
        )
        label, decision = resolve_solver_decision(
            {
                "based_on_room_seq": 5,
                "action": "move_disk",
                "payload": {"from": "A", "to": "B", "disk": 2},
                "rationale": "Free rod B.",
            },
            candidates,
            5,
        )
        self.assertEqual(label, "move:A>B:2")
        self.assertEqual(decision["action"], "move_disk")
        self.assertEqual(decision["payload"], {"from": "A", "to": "B", "disk": 2})
        self.assertEqual(decision["based_on_room_seq"], 5)
        self.assertEqual(decision["rationale"], "Free rod B.")

        label, decision = resolve_solver_decision(
            {"action": "wait", "reason": "break_cycle"}, candidates, 5
        )
        self.assertEqual((label, decision["action"], decision["payload"]), ("wait", "wait", {}))
        self.assertEqual(decision["reason"], "break_cycle")
        label, decision = resolve_solver_decision(
            {"action": "wait", "reason": "not_a_reason"}, candidates, 5
        )
        self.assertNotIn("reason", decision)

    def test_canonical_decision_rejects_stale_or_unoffered_choices(self) -> None:
        candidates = _legal_candidates(
            {**_activity(), "action_offers": ["move_disk"]}
        )
        with self.assertRaises(HanoiProtocolError):
            resolve_solver_decision(
                {"based_on_room_seq": 4, "action": "wait"}, candidates, 5
            )
        with self.assertRaises(HanoiProtocolError):
            resolve_solver_decision(
                {"action": "move_disk", "payload": {"from": "A", "to": "C", "disk": 3}},
                candidates,
                5,
            )

    def test_provider_state_carries_the_shared_context(self) -> None:
        view = {**_activity(claim=True), "action_offers": ["assess_claim", "move_disk"]}
        candidates = _legal_candidates(view)
        context = {
            "events_since_turn": [{"kind": "disk_moved", "room_seq": 4, "move": {"from": "A", "to": "B", "disk": 1}}],
            "recent_own_decisions": [{"label": "move:A>B:1"}],
            "state_visit": {"count": 2, "cycle_hint": True},
            "delivery": {"kind": "retained_frames"},
            "cursor": 9,
            "cursor_range": {"from": 4, "to": 9},
        }
        state = _provider_state(view, candidates, "claim_review_requested", room_seq=9, context=context)
        self.assertEqual(state["based_on_room_seq"], 9)
        self.assertEqual(state["objective"]["target_rod"], "C")
        self.assertEqual(state["open_completion_claim"]["quorum"], 1)
        self.assertFalse(state["open_completion_claim"]["accepted"])
        self.assertIn("move:A>B:2", state["available_choices"])
        self.assertIn("assess:defer", state["available_choices"])
        self.assertEqual(state["recent_events"][0]["move"]["to"], "B")
        self.assertEqual(state["recent_own_decisions"][0]["label"], "move:A>B:1")
        self.assertTrue(state["state_visit"]["cycle_hint"])

    def test_openrouter_canonical_decision_is_resolved(self) -> None:
        view = {**_activity(), "action_offers": ["move_disk"]}
        candidates = _legal_candidates(view)
        response = {
            "model": "openai/gpt-5.6-luna",
            "choices": [
                {
                    "message": {
                        "content": json.dumps(
                            {
                                "based_on_room_seq": 2,
                                "action": "move_disk",
                                "payload": {"from": "A", "to": "B", "disk": 2},
                            }
                        )
                    }
                }
            ],
        }
        with patch(
            "examples.tower_of_hanoi.participant_runner._http_json", return_value=response
        ):
            choice, disposition, metadata = _openrouter_staged_choice(
                _provider_state(view, candidates, "board_changed", room_seq=2),
                candidates,
                "openai/gpt-5.6-luna",
                "key",
                1,
                room_seq=2,
            )
        self.assertEqual(choice, "move:A>B:2")
        self.assertEqual(disposition, "act")
        self.assertEqual(metadata["decision"]["action"], "move_disk")

    def test_claim_review_is_prioritized_as_a_single_assessment(self) -> None:
        candidates = _legal_candidates(
            {**_activity(claim=True), "action_offers": ["assess_claim", "move_disk"]}
        )
        response = {
            "model": "jev-1.13.0",
            "answers": {
                "action": {
                    "type": "choice",
                    "choice": "assess:defer",
                    "probabilities": {
                        "assess:endorse": 0.1,
                        "assess:challenge": 0.2,
                        "assess:defer": 0.7,
                    },
                    "confidence": 0.7,
                }
            },
            "usage": {"input_tokens": 1, "output_tokens": 1},
        }
        with patch("examples.tower_of_hanoi.participant_runner._http_json", return_value=response) as request:
            choice, disposition, metadata = _jev_staged_choice(
                {"activity": _activity(claim=True)}, candidates, "jev-1.13.0", "key", 1
            )
        self.assertEqual((choice, disposition), ("assess:defer", "assess"))
        self.assertEqual(metadata["probabilities"]["assess:defer"], 0.7)
        request.assert_called_once()
        self.assertEqual(set(request.call_args.args[1]["questions"]), {"action"})

    def test_uncertain_is_a_wait_and_does_not_call_pack_action(self) -> None:
        context = {
            "schema": "worldstream/tower-of-hanoi-participant-context/v1",
            "room_seq": 3,
            "cursor": 4,
            "cursor_range": {"from": 4, "to": 4},
            "activity": _activity(),
            "action_offers": ["move_disk"],
            "events_since_turn": [],
            "recent_own_decisions": [],
            "state_visit": {"count": 1, "cycle_hint": False},
            "delivery": {"kind": "retained_frames"},
        }
        evidence = Path(tempfile.mkdtemp()) / "events.jsonl"
        with (
            patch("examples.tower_of_hanoi.participant_runner._provider_key", return_value="key"),
            patch(
                "examples.tower_of_hanoi.participant_runner._jev_staged_choice",
                return_value=(None, "wait", {"confidence": 0.2}),
            ),
            patch("examples.tower_of_hanoi.participant_runner.turn.act") as act,
        ):
            result = _invoke_provider(
                engine="jev",
                model="jev-1.13.0",
                repo=Path("/repo"),
                membership_file=Path("/membership"),
                timeout_seconds=1,
                reason="board_changed",
                evidence=evidence,
                invocation_name="uncertain",
                seat="solver-a",
                event_file=None,
                cancelled=None,
                invocation_context=context,
            )
        self.assertTrue(result)
        act.assert_not_called()
        trace = next(
            json.loads(line)
            for line in evidence.read_text().splitlines()
            if "participant_decision" in line
        )
        self.assertEqual(trace["disposition"], "wait")
        self.assertEqual(trace["selected"], {"label": "wait"})
        self.assertNotIn("raw", trace)

    def test_stale_claimed_head_is_submitted_once_and_traced_as_action_intent(self) -> None:
        context = {
            "schema": "worldstream/tower-of-hanoi-participant-context/v1",
            "room_seq": 3,
            "cursor": 4,
            "cursor_range": {"from": 4, "to": 4},
            "activity": _activity(),
            "action_offers": ["move_disk"],
            "events_since_turn": [],
            "recent_own_decisions": [],
            "state_visit": {"count": 1, "cycle_hint": False},
            "delivery": {"kind": "retained_frames"},
        }
        evidence = Path(tempfile.mkdtemp()) / "events.jsonl"
        with (
            patch("examples.tower_of_hanoi.participant_runner._provider_key", return_value="key"),
            patch(
                "examples.tower_of_hanoi.participant_runner._jev_staged_choice",
                return_value=("move:A>B:2", "act", {"confidence": 0.8, "probabilities": {"move:A>B:2": 1.0}}),
            ),
            patch(
                "examples.tower_of_hanoi.participant_runner.turn.act",
                new_callable=AsyncMock,
                return_value={"status": "stale", "current_room_seq": 4},
            ) as act,
        ):
            result = _invoke_provider(
                engine="jev",
                model="jev-1.13.0",
                repo=Path("/repo"),
                membership_file=Path("/membership"),
                timeout_seconds=1,
                reason="board_changed",
                evidence=evidence,
                invocation_name="stale",
                seat="solver-a",
                event_file=None,
                cancelled=None,
                invocation_context=context,
            )
        self.assertTrue(result)
        act.assert_called_once()
        trace = next(
            json.loads(line)
            for line in evidence.read_text().splitlines()
            if "participant_decision" in line
        )
        self.assertEqual(trace["disposition"], "act")
        self.assertEqual(trace["selected"]["action_type"], "move_disk")

    def test_stream_provider_uses_participant_action_gateway(self) -> None:
        context = {
            "schema": "worldstream/tower-of-hanoi-participant-context/v1",
            "room_seq": 0,
            "cursor": 0,
            "cursor_range": {"from": 0, "to": 0},
            "activity": _activity(),
            "action_offers": ["move_disk"],
            "events_since_turn": [],
            "recent_own_decisions": [],
            "state_visit": {"count": 1, "cycle_hint": False},
            "delivery": {"kind": "retained_frames"},
        }
        evidence = Path(tempfile.mkdtemp()) / "events.jsonl"
        participant = MagicMock()
        participant.submit_action_sync.return_value = {
            "status": "accepted",
            "room_seq": 1,
            "action_type": "move_disk",
        }
        with (
            patch("examples.tower_of_hanoi.participant_runner._provider_key", return_value="key"),
            patch(
                "examples.tower_of_hanoi.participant_runner._jev_staged_choice",
                return_value=("move:A>B:2", "act", {"confidence": 0.8}),
            ),
            patch("examples.tower_of_hanoi.participant_runner.turn.act") as fresh_act,
        ):
            result = _invoke_provider(
                engine="jev",
                model="jev-1.13.0",
                repo=Path("/repo"),
                membership_file=Path("/membership"),
                timeout_seconds=1,
                reason="board_changed",
                evidence=evidence,
                invocation_name="gateway",
                seat="solver-a",
                event_file=None,
                cancelled=None,
                invocation_context=context,
                participant_context=participant,
            )
        self.assertTrue(result)
        participant.submit_action_sync.assert_called_once_with(
            "move_disk", {"from": "A", "to": "B", "disk": 2}, 0, 1
        )
        fresh_act.assert_not_called()


class ActionGrantTests(unittest.TestCase):
    def _grant(self) -> ActionGrant:
        return ActionGrant(Path(tempfile.mkdtemp()) / "grant.json")

    def test_only_one_member_may_hold_a_head(self) -> None:
        grant = self._grant()
        self.assertTrue(grant.acquire(3, "member-a"))
        self.assertFalse(grant.acquire(3, "member-b"))

    def test_acted_head_is_not_reacquirable(self) -> None:
        grant = self._grant()
        self.assertTrue(grant.acquire(3, "member-a"))
        grant.mark_acted()
        self.assertFalse(grant.acquire(3, "member-b"))
        self.assertTrue(grant.acquire(4, "member-b"))

    def test_cleared_head_can_be_retried(self) -> None:
        grant = self._grant()
        self.assertTrue(grant.acquire(3, "member-a"))
        grant.clear()
        self.assertTrue(grant.acquire(3, "member-b"))

    def test_member_cannot_hold_an_older_head(self) -> None:
        grant = self._grant()
        self.assertTrue(grant.acquire(5, "member-a"))
        self.assertFalse(grant.acquire(4, "member-b"))


class OfferSelectionTests(unittest.TestCase):
    def test_newest_attention_offer_wins(self) -> None:
        offers = [
            {"reason_code": "board_changed", "cause_room_seq": 4, "activation_id": "old"},
            {"reason_code": "board_changed", "cause_room_seq": 9, "activation_id": "new"},
            {"reason_code": "board_changed", "cause_room_seq": 6, "activation_id": "mid"},
        ]
        self.assertEqual(_select_offer(offers)["activation_id"], "new")

    def test_claim_review_outranks_a_newer_board_change(self) -> None:
        offers = [
            {"reason_code": "board_changed", "cause_room_seq": 9, "activation_id": "board"},
            {"reason_code": "claim_review_requested", "cause_room_seq": 2, "activation_id": "claim"},
        ]
        self.assertEqual(_select_offer(offers)["activation_id"], "claim")

    def test_no_attention_offer_returns_none(self) -> None:
        self.assertIsNone(_select_offer([{"reason_code": "unrelated", "cause_room_seq": 1}]))


class ModelLadderTests(unittest.TestCase):
    def _ladder(self) -> ModelLadder:
        return ModelLadder(Path(tempfile.mkdtemp()) / "ladder.json", ["primary", "second", "third"])

    def test_starts_on_the_primary(self) -> None:
        self.assertEqual(self._ladder().current(), ("primary", 0))

    def test_advance_promotes_both_readers_once(self) -> None:
        ladder = self._ladder()
        self.assertEqual(ladder.advance(0), ("second", 1))
        # A peer that also thought it was on index 0 must not double-promote.
        self.assertEqual(ladder.advance(0), ("second", 1))
        self.assertEqual(ladder.current(), ("second", 1))

    def test_advance_caps_at_the_last_model(self) -> None:
        ladder = self._ladder()
        ladder.advance(0)
        ladder.advance(1)
        self.assertEqual(ladder.advance(2), ("third", 2))
        self.assertEqual(ladder.current(), ("third", 2))


class RetryDelayTests(unittest.TestCase):
    def test_backoff_grows_then_caps_with_jitter(self) -> None:
        first = _activation_retry_delay(1)
        second = _activation_retry_delay(2)
        late = _activation_retry_delay(20)
        self.assertTrue(0.75 <= first <= 1.5)
        self.assertTrue(1.5 <= second <= 3.0)
        self.assertLessEqual(late, 30.0 * 1.25)
        self.assertGreaterEqual(late, 30.0 * 0.75)


class HttpDeadlineTests(unittest.TestCase):
    def test_http_json_enforces_a_hard_wall_clock_deadline(self) -> None:
        def slow(*_arguments: object, **_keywords: object) -> object:
            time.sleep(5)
            raise OSError("late response")

        with patch(
            "examples.tower_of_hanoi.participant_runner.urllib.request.urlopen",
            side_effect=slow,
        ), self.assertRaises(ParticipantRunnerError) as context:
            _http_json("https://example.test/v1", {"a": 1}, {}, 1.0)
        self.assertIn("provider_request_timeout", str(context.exception))


class ActionableCandidatesTests(unittest.TestCase):
    def test_wait_is_declined_while_a_concrete_action_exists(self) -> None:
        candidates = {
            "move:A>B:2": {"action_type": "move_disk", "payload": {}},
            "wait": {"action_type": "wait", "payload": None},
        }
        self.assertEqual(list(_actionable_candidates(candidates)), ["move:A>B:2"])

    def test_wait_remains_when_it_is_the_only_option(self) -> None:
        candidates = {"wait": {"action_type": "wait", "payload": None}}
        self.assertIn("wait", _actionable_candidates(candidates))


class JevJudgeTests(unittest.TestCase):
    def _state(self) -> dict[str, object]:
        return {
            "schema": "worldstream/tower-of-hanoi-participant-context/v1",
            "board": {"A": [3, 2], "B": [], "C": [1]},
            "disks": 3,
            "available_choices": {
                "move:A>B:2": "Move disk 2 from rod A to rod B.",
                "wait": "Take no action.",
            },
        }

    def test_jev_judge_reads_probability_and_applies_threshold(self) -> None:
        state = self._state()
        response = {"model": "jev-latest", "answers": {"judge": {"type": "noul", "noul": 0.9}}}
        with patch(
            "examples.tower_of_hanoi.participant_runner._http_json",
            return_value=response,
        ) as request:
            approved, probability, metadata = _jev_judge(
                state,
                state["available_choices"],
                "move:A>B:2",
                "jev-latest",
                "key",
                1,
                0.5,
            )
        self.assertTrue(approved)
        self.assertAlmostEqual(probability, 0.9)
        self.assertTrue(metadata["approved"])
        self.assertEqual(metadata["proposed"], "move:A>B:2")
        self.assertEqual(request.call_args.args[0], "https://api.typesafe.ai/v1/systemone")

    def test_jev_judge_rejects_missing_answer(self) -> None:
        state = self._state()
        with patch(
            "examples.tower_of_hanoi.participant_runner._http_json",
            return_value={"answers": {}},
        ), self.assertRaises(ParticipantRunnerError):
            _jev_judge(state, state["available_choices"], "wait", "jev-latest", "key", 1, 0.5)

    def test_judged_choice_accepts_approved_proposal_without_repair(self) -> None:
        state = self._state()
        candidates = {"move:A>B:2": {"action_type": "move_disk", "payload": {}}, "wait": {"action_type": "wait"}}
        with patch(
            "examples.tower_of_hanoi.participant_runner._jev_judge",
            return_value=(True, 0.8, {"approved": True}),
        ), patch(
            "examples.tower_of_hanoi.participant_runner._openrouter_staged_choice"
        ) as repair:
            choice, disposition, metadata = _judged_choice(
                state,
                candidates,
                "move:A>B:2",
                "act",
                {"provider_model": "openai/gpt-5.6-luna"},
                "openai/gpt-5.6-luna",
                "key",
                0,
                1,
                "jev-latest",
                0.5,
                "jev-key",
            )
        self.assertEqual(choice, "move:A>B:2")
        self.assertEqual(disposition, "act")
        self.assertEqual(metadata["judge"]["approved"], True)
        repair.assert_not_called()

    def test_judged_choice_keeps_judge_preferred_repair(self) -> None:
        state = self._state()
        candidates = {"move:A>B:2": {"action_type": "move_disk", "payload": {}}, "move:A>C:2": {"action_type": "move_disk", "payload": {}}, "wait": {"action_type": "wait"}}
        with patch(
            "examples.tower_of_hanoi.participant_runner._jev_judge",
            side_effect=[(False, 0.1, {"approved": False}), (True, 0.9, {"approved": True})],
        ), patch(
            "examples.tower_of_hanoi.participant_runner._openrouter_staged_choice",
            return_value=("move:A>C:2", "act", {"provider_model": "openai/gpt-5.6-luna"}),
        ):
            choice, disposition, metadata = _judged_choice(
                state,
                candidates,
                "move:A>B:2",
                "act",
                {"provider_model": "openai/gpt-5.6-luna"},
                "openai/gpt-5.6-luna",
                "key",
                0,
                1,
                "jev-latest",
                0.5,
                "jev-key",
            )
        self.assertEqual(choice, "move:A>C:2")
        self.assertEqual(disposition, "act")
        self.assertTrue(metadata["judge"]["accepted_repair"])

    def test_judged_choice_keeps_original_when_repair_scores_lower(self) -> None:
        state = self._state()
        candidates = {"move:A>B:2": {"action_type": "move_disk", "payload": {}}, "move:A>C:2": {"action_type": "move_disk", "payload": {}}, "wait": {"action_type": "wait"}}
        with patch(
            "examples.tower_of_hanoi.participant_runner._jev_judge",
            side_effect=[(False, 0.4, {"approved": False}), (False, 0.2, {"approved": False})],
        ), patch(
            "examples.tower_of_hanoi.participant_runner._openrouter_staged_choice",
            return_value=("move:A>C:2", "act", {"provider_model": "openai/gpt-5.6-luna"}),
        ):
            choice, _disposition, metadata = _judged_choice(
                state,
                candidates,
                "move:A>B:2",
                "act",
                {"provider_model": "openai/gpt-5.6-luna"},
                "openai/gpt-5.6-luna",
                "key",
                0,
                1,
                "jev-latest",
                0.5,
                "jev-key",
            )
        self.assertEqual(choice, "move:A>B:2")
        self.assertEqual(metadata["judge"]["approved"], False)


if __name__ == "__main__":
    unittest.main()

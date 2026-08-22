from __future__ import annotations

import json
import unittest
from pathlib import Path

from story import (
    ACTION_ORDER,
    FIXTURE,
    MAX_OPEN_OFFERS_PER_ROLE,
    ROLES,
    Story,
    _core_state,
    _initial_state,
    action_offers,
    build_story,
    reduce_state,
)

FIXTURE_PATH = Path(__file__).with_name("imo53_contract_fixture.json")


def _resolve_row(row: dict[str, object]) -> dict[str, object]:
    state = _initial_state()
    state["phase"] = "Resolution"
    state["phase_generation"] = 4
    state["plans"] = [
        {
            "plan_id": plan_id,
            "proposer_role": "navigator",
            "created_room_seq": 1,
            "route": (
                row.get("plan_route", "service")
                if plan_id == "plan-1"
                else "wrong-route"
                if plan_id == "wrong"
                else "service"
            ),
            "entry_window": "early",
            "required_tool": "thermal_key",
            "extraction": "boat",
        }
        for plan_id in ("plan-1", "plan-2", "plan-3", "correct", "wrong")
    ]
    state["commitments"] = {
        role: {
            "selected_plan_id": values[0],
            "contribute_required_resource": values[1],
        }
        for role, values in row["commitments"].items()
    }
    return reduce_state(
        state,
        {
            "kind": "timer",
            "next_room_seq": 1,
            "timer": {
                "timer_id": "resolve_now",
                "generation": 1,
                "scheduled_for": "2026-08-15T12:02:30.000001Z",
                "kind": "resolve_now",
                "payload": {},
            },
        },
    )["outcome"]


class Imo53ContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.fixture = json.loads(FIXTURE_PATH.read_text(encoding="utf-8"))
        cls.transcript = build_story()

    def test_fixed_seed_three_immutable_seats_and_missing_denominator(self) -> None:
        self.assertEqual(self.fixture["roles"], list(ROLES))
        self.assertEqual(self.fixture["missing_seat_denominator"], len(ROLES))
        memberships = _core_state()["memberships"]
        self.assertEqual(list(memberships), list(ROLES))
        self.assertEqual(
            [membership["role"] for membership in memberships.values()], list(ROLES)
        )
        self.assertTrue(
            all(
                membership["standing"] == "enabled"
                and membership["access_mode"] == "participant"
                for membership in memberships.values()
            )
        )
        self.assertEqual(self.transcript["genesis"]["fixture"], FIXTURE)
        self.assertEqual(
            self.transcript["expected_fixture_outcome"]["missing_roles"], ["broker"]
        )
        self.assertEqual(
            self.fixture["room_seed"],
            "hex:" + "".join(f"{i:02x}" for i in range(32)),
        )
        registry = (
            Path(__file__).parents[2]
            / "crates/worldstream-core/src/agent_heist_registry.rs"
        ).read_text(encoding="utf-8")
        self.assertIn(
            f'room_seed: parsed("{self.fixture["room_seed"]}")',
            registry,
        )

    def test_action_surface_is_bounded_and_rejections_are_stable(self) -> None:
        self.assertEqual(self.fixture["maximum_open_offers_per_role"], 4)
        self.assertEqual(MAX_OPEN_OFFERS_PER_ROLE, 4)
        for transition in self.transcript["transitions"]:
            for role in ROLES:
                offers = action_offers(transition["state_after"], role)
                self.assertLessEqual(len(offers), 4)
                self.assertTrue(set(offers).issubset(set(ACTION_ORDER)))

        cases = (
            (
                "unknown_role",
                lambda story: story.action("ghost", "inspect_clue", "id", "t", {}),
            ),
            (
                "unknown_action",
                lambda story: story.action("navigator", "teleport", "id", "t", {}),
            ),
            (
                "empty_action_id",
                lambda story: story.action("navigator", "inspect_clue", "", "t", {}),
            ),
            (
                "non_object_payload",
                lambda story: story.action("navigator", "inspect_clue", "id", "t", []),
            ),
        )
        for label, call in cases:
            exception_name, message = self.fixture["action_rejections"][label]
            exception_type = ValueError if exception_name == "ValueError" else TypeError
            with self.subTest(label=label), self.assertRaises(exception_type) as raised:
                call(Story())
            self.assertEqual(str(raised.exception), message)

    def test_golden_score_matrix_and_thresholds(self) -> None:
        for row in self.fixture["score_matrix"]:
            with self.subTest(commitments=row["commitments"]):
                outcome = _resolve_row(row)
                self.assertEqual(outcome["outcome"], row["outcome"])
                self.assertEqual(outcome["score"], row["score"])

    def test_commitment_reminder_generation_and_resolve_order_are_exact(self) -> None:
        observed = []
        for transition in self.transcript["transitions"]:
            timer = transition["stimulus"].get("timer")
            if timer is not None:
                observed.append(
                    [
                        timer["kind"],
                        timer["generation"],
                        transition["state_after"]["phase"],
                    ]
                )
        self.assertEqual(observed, self.fixture["timer_path"])
        reminder = self.transcript["transitions"][9]
        prior = self.transcript["transitions"][8]
        self.assertEqual(reminder["state_hash"], prior["state_hash"])
        self.assertEqual(reminder["events"], [{"event_type": "commitment_reminder"}])
        self.assertEqual(
            self.transcript["transitions"][10]["state_after"]["phase"], "Resolution"
        )
        self.assertEqual(
            self.transcript["transitions"][11]["state_after"]["phase"], "Result"
        )

    def test_result_complete_duplicates_stale_crash_and_genesis_fold_are_exact(
        self,
    ) -> None:
        self.assertEqual(
            self.transcript["final_head"]["room_seq"], self.fixture["transition_count"]
        )
        self.assertEqual(self.transcript["phase_path"][-1], self.fixture["final_phase"])
        self.assertEqual(
            self.transcript["final_read_only_replay"]["verified_transition_count"],
            self.fixture["replayed_transition_count"],
        )
        evidence = self.transcript["operational_evidence"]
        self.assertTrue(
            any(item.get("kind") == "duplicate_action" for item in evidence)
        )
        self.assertTrue(
            any(item.get("kind") == "stale_generation" for item in evidence)
        )
        self.assertEqual(
            [
                item["marker"]
                for item in evidence
                if item.get("kind") == "recovery_marker"
            ],
            ["kill_before_commit", "kill_after_commit_before_publication"],
        )
        self.assertEqual(
            self.transcript["final_read_only_replay"]["source"],
            "genesis_plus_ordered_transitions",
        )
        self.assertTrue(self.transcript["final_read_only_replay"]["verified"])
        self.assertTrue(
            all(
                item["transition_hash"].startswith("sha256:")
                for item in self.transcript["transitions"]
            )
        )
        self.assertEqual(
            self.transcript["final_head"]["lineage_hash"],
            self.transcript["transitions"][-1]["transition_hash"],
        )


if __name__ == "__main__":
    unittest.main()

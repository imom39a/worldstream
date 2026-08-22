from __future__ import annotations

import json
import unittest
from copy import deepcopy
from pathlib import Path

import tomllib
from story import (
    EXPECTED_OUTCOME,
    PACK_DIGEST,
    RetainedHeistExecutor,
    audience_projection,
    build_story,
    canonical_bytes,
    canonical_text,
    digest,
    retained_executor_self_test,
    self_test,
    validate_transcript,
)


class AbsentBrokerStoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.transcript = build_story()

    def test_six_phase_path_and_public_outcome(self) -> None:
        self.assertEqual(
            self.transcript["phase_path"],
            [
                "Briefing",
                "Negotiation",
                "Negotiation",
                "Negotiation",
                "Negotiation",
                "Negotiation",
                "Negotiation",
                "Commitment",
                "Commitment",
                "Commitment",
                "Commitment",
                "Resolution",
                "Result",
                "Result",
                "Result",
                "Complete",
            ],
        )
        self.assertEqual(self.transcript["expected_fixture_outcome"], EXPECTED_OUTCOME)
        self.assertEqual(
            self.transcript["expected_fixture_outcome"]["missing_roles"], ["broker"]
        )

    def test_activation_claim_context_is_exact_and_separate(self) -> None:
        contexts = [
            item
            for item in self.transcript["operational_evidence"]
            if item.get("kind") == "fresh_activation_context"
        ]
        self.assertEqual(len(contexts), 1)
        context = contexts[0]
        self.assertEqual(context["context"]["head"]["room_seq"], 7)
        self.assertEqual(context["context"]["projection"]["phase"], "Commitment")
        self.assertEqual(
            context["context"]["action_offers"], ["inspect_clue", "commit_move"]
        )
        self.assertEqual(
            context["participant_action_authority"], "not_granted_by_claim"
        )
        participant = [
            item
            for item in self.transcript["operational_evidence"]
            if item.get("kind") == "participant_authority"
        ]
        self.assertEqual(len(participant), 1)
        self.assertEqual(participant[0]["authority"], "participant-action-capability")
        self.assertEqual(
            context["context"]["lease_witness"],
            {
                "activation_id": context["activation_id"],
                "lease_generation": 0,
                "fencing": "compare_and_swap_generation",
            },
        )

    def test_audience_boundaries_are_private_and_noninterfering(self) -> None:
        state = {
            "phase": "Commitment",
            "phase_generation": 3,
            "phase_start": "2026-08-15T12:02:00Z",
            "phase_deadline": "2026-08-15T12:02:30Z",
            "fixture_id": "service_window",
            "fixture": {
                "fixture_id": "service_window",
                "route": "service",
                "entry_window": "early",
                "required_tool": "thermal_key",
                "extraction": "boat",
            },
            "known_clues": {
                "navigator": ["route"],
                "insider": ["entry_window"],
                "broker": [],
            },
            "published_claims": ["route"],
            "plans": [],
            "endorsements": {},
            "commitments": {
                "navigator": {
                    "selected_plan_id": "public-plan",
                    "contribute_required_resource": False,
                }
            },
            "result_acknowledgements": [],
            "outcome": None,
        }
        public = audience_projection(state, "public")
        operator = audience_projection(state, "operator")
        historical = audience_projection(state, "historical")
        participant = audience_projection(state, "participant", "navigator")

        for view in (public, operator, historical):
            self.assertNotIn("private_clues", view)
            self.assertNotIn("own_commitment", view)
            self.assertNotIn("action_offers", view)
        self.assertTrue(operator["operator"]["redacted"])
        self.assertTrue(historical["historical"])
        self.assertIn("private_clues", participant)

        changed = {
            **state,
            "known_clues": {**state["known_clues"], "broker": ["required_tool"]},
        }
        changed["commitments"] = {
            "broker": {
                "selected_plan_id": "private-plan",
                "contribute_required_resource": True,
            }
        }
        self.assertEqual(public, audience_projection(changed, "public"))
        self.assertEqual(operator, audience_projection(changed, "operator"))
        self.assertEqual(historical, audience_projection(changed, "historical"))
        self.assertNotEqual(
            participant,
            audience_projection(changed, "participant", "broker"),
        )

    def test_hidden_state_does_not_change_live_catchup_reset_error_log_ui_or_replay(
        self,
    ) -> None:
        state = {
            "phase": "Result",
            "phase_generation": 5,
            "phase_start": "2026-08-15T12:02:30Z",
            "phase_deadline": "2026-08-15T12:02:50Z",
            "fixture_id": "service_window",
            "fixture": {
                "fixture_id": "service_window",
                "route": "service",
                "entry_window": "early",
                "required_tool": "thermal_key",
                "extraction": "boat",
            },
            "known_clues": {"navigator": ["route"], "insider": [], "broker": []},
            "published_claims": ["route"],
            "plans": [{"plan_id": "plan-1", "route": "service"}],
            "endorsements": {},
            "commitments": {"navigator": {"selected_plan_id": "plan-1"}},
            "result_acknowledgements": [],
            "outcome": {"outcome": "success", "score": 5},
        }
        changed = deepcopy(state)
        changed["known_clues"]["broker"] = ["required_tool", "extraction"]
        changed["commitments"]["navigator"] = {
            "selected_plan_id": "secret-plan",
            "contribute_required_resource": True,
        }

        def surfaces(value: dict) -> dict[str, object]:
            public = audience_projection(value, "public")
            return {
                "live": public,
                "catch_up": {"projection": public, "cursor": 10},
                "reset": {"projection": public, "baseline_frame_head": 10},
                "error": {"code": "slow_consumer", "retryable": True},
                "log": canonical_text(public),
                "ui": canonical_text(public),
                "replay": public,
            }

        self.assertEqual(surfaces(state), surfaces(changed))

    def test_attention_context_hash_and_replay_are_deterministic(self) -> None:
        first = build_story()
        second = build_story()
        self.assertEqual(first["transcript_digest"], second["transcript_digest"])
        context = next(
            item
            for item in first["operational_evidence"]
            if item.get("kind") == "fresh_activation_context"
        )
        self.assertEqual(context["context_hash"], digest(context["context"]))
        self.assertEqual(
            context["context"]["delivery_witness"],
            {"kind": "projection_reset", "cursor": None},
        )

    def test_failure_and_recovery_outcomes_are_explicit(self) -> None:
        outcomes = next(
            item
            for item in self.transcript["operational_evidence"]
            if item.get("kind") == "activation_outcomes"
        )
        self.assertEqual(outcomes["duplicate_claim"], "duplicate_existing")
        self.assertEqual(
            outcomes["lost_claim_reply"], "retry_resolved_original_receipt"
        )
        self.assertEqual(outcomes["lease_reclaim"], "reclaimed")
        self.assertEqual(outcomes["stale_generation"], "stale_generation")
        self.assertEqual(outcomes["idempotent_completion"], "idempotent_existing")
        markers = [
            item["marker"]
            for item in self.transcript["operational_evidence"]
            if item.get("kind") == "recovery_marker"
        ]
        self.assertEqual(
            markers, ["kill_before_commit", "kill_after_commit_before_publication"]
        )
        duplicate_offer = [
            item
            for item in self.transcript["operational_evidence"]
            if item.get("kind") == "duplicate_offer"
        ]
        self.assertEqual(len(duplicate_offer), 1)
        self.assertEqual(duplicate_offer[0]["outcome"], "duplicate_existing")
        self.assertEqual(duplicate_offer[0]["cursor_before"], 5)
        self.assertEqual(duplicate_offer[0]["cursor_after"], 5)
        self.assertTrue(duplicate_offer[0]["no_new_transition"])

    def test_replay_is_read_only_and_does_not_create_activation(self) -> None:
        replay = self.transcript["final_read_only_replay"]
        self.assertTrue(replay["read_only"])
        self.assertTrue(replay["verified"])
        self.assertEqual(replay["snapshots_used"], 0)
        self.assertEqual(replay["activation_created"], 4)

    def test_retained_executor_load_project_advance_and_replay_compare_exact_hashes(
        self,
    ) -> None:
        evidence = retained_executor_self_test(self.transcript)
        self.assertEqual(evidence["pack_digest"], PACK_DIGEST)
        self.assertTrue(evidence["runnable_for_retained_rooms"])
        self.assertEqual(evidence["initial_phase"], "Briefing")
        self.assertEqual(evidence["final_phase"], "Complete")
        self.assertEqual(evidence["advanced_transition_count"], 15)
        self.assertEqual(evidence["phase_path"], self.transcript["phase_path"])
        comparison = evidence["replay"]["hash_comparison"]
        self.assertEqual(evidence["replay"]["activation_created"], 0)
        self.assertEqual(evidence["replay"]["attention_replayed"], 4)
        self.assertTrue(all(item["match"] for item in comparison.values()))
        self.assertEqual(
            comparison["core_state_hash"]["expected"],
            self.transcript["final_head"]["core_state_hash"],
        )
        self.assertEqual(
            comparison["activity_state_hash"]["expected"],
            self.transcript["final_head"]["activity_state_hash"],
        )
        self.assertEqual(
            comparison["authoritative_state_hash"]["expected"],
            self.transcript["final_head"]["authoritative_state_hash"],
        )
        self.assertEqual(
            comparison["transition_hashes"]["expected"],
            [item["transition_hash"] for item in self.transcript["transitions"]],
        )

    def test_retained_executor_digest_and_transition_are_fail_closed(self) -> None:
        with self.assertRaises(ValueError):
            RetainedHeistExecutor(pack_digest="blake3:not-the-retained-executor").load(
                self.transcript
            )

        tampered = deepcopy(self.transcript)
        tampered["transitions"][4]["transition_hash"] = "sha256:tampered"
        with self.assertRaises(ValueError):
            RetainedHeistExecutor().load(tampered)

    def test_parity_fixture_identity_matches_selectable_manifest_row(self) -> None:
        manifest_path = Path(__file__).parents[2] / "compatibility.toml"
        manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
        rows = [
            row
            for row in manifest["pack_executors"]
            if row["pack_id"] == "worldstream.agent-heist"
            and row["explanatory_version"] == "0.1.0"
        ]
        self.assertEqual(len(rows), 1)
        row = rows[0]
        parity = json.loads(
            (Path(__file__).with_name("parity_fixture.json")).read_text(
                encoding="utf-8"
            )
        )
        fixture = parity["retained_executor"]
        self.assertEqual(fixture["pack_id"], row["pack_id"])
        self.assertEqual(fixture["pack_version"], row["explanatory_version"])
        self.assertEqual(fixture["pack_digest"], row["revision_digest"])
        self.assertTrue(row["selectable_for_new_rooms"])
        self.assertTrue(row["runnable_for_retained_rooms"])

    def test_canonical_json_and_hashes_are_repeatable(self) -> None:
        first = self_test()
        second = self_test()
        self.assertEqual(first["transcript_digest"], second["transcript_digest"])
        self.assertEqual(canonical_text({"b": 1, "a": 2}), '{"a":2,"b":1}')
        self.assertTrue(
            all(
                item["transition_hash"].startswith("sha256:")
                for item in first["transitions"]
            )
        )

    def test_transcript_validation_rejects_tampered_evidence(self) -> None:
        tampered = deepcopy(self.transcript)
        tampered["final_read_only_replay"]["read_only"] = False
        with self.assertRaises(ValueError):
            validate_transcript(tampered)

        extra = {**self.transcript, "unexpected": True}
        with self.assertRaises(ValueError):
            validate_transcript(extra)

        tampered_transition = deepcopy(self.transcript)
        tampered_transition["transitions"][0]["state_after"]["phase"] = "Complete"
        with self.assertRaises(ValueError):
            validate_transcript(tampered_transition)

        tampered_operational = deepcopy(self.transcript)
        tampered_operational["operational_evidence"][0]["canonical"] = True
        with self.assertRaises(ValueError):
            validate_transcript(tampered_operational)

    def test_canonical_json_rejects_float_and_unsafe_integer_values(self) -> None:
        with self.assertRaises(TypeError):
            canonical_bytes({"value": 1.5})
        with self.assertRaises(ValueError):
            canonical_bytes({"value": 9_007_199_254_740_992})


if __name__ == "__main__":
    unittest.main()

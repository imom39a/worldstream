from __future__ import annotations

import copy
import unittest

from story import audience_projection, canonical_text, digest

GOLDEN_MATRIX_DIGEST = (
    "sha256:2946c4b2f1190d6bc4af7a253a0ccb2098031939926a01910e12cde482dc6ab7"
)


def _state() -> dict:
    return {
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
            "broker": ["required_tool"],
        },
        "published_claims": ["route"],
        "plans": [{"plan_id": "public-plan", "route": "service"}],
        "endorsements": {"insider": "public-plan"},
        "commitments": {
            "navigator": {
                "selected_plan_id": "public-plan",
                "contribute_required_resource": False,
            },
            "insider": {
                "selected_plan_id": "public-plan",
                "contribute_required_resource": False,
            },
            "broker": {
                "selected_plan_id": "broker-plan",
                "contribute_required_resource": False,
            },
        },
        "result_acknowledgements": [],
        "outcome": None,
    }


def _paired_state(owner: str) -> dict:
    changed = copy.deepcopy(_state())
    changed["known_clues"][owner] = ["extraction"]
    changed["commitments"][owner] = {
        "selected_plan_id": "private-plan",
        "contribute_required_resource": True,
    }
    return changed


def _wire_surfaces(state: dict) -> dict:
    public = audience_projection(state, "public")
    historical = audience_projection(state, "historical")
    return {
        "frame": {"type": "observation.deliver", "frame_seq": 22, "projection": public},
        "reset": {
            "type": "projection.reset",
            "baseline_frame_head": 22,
            "projection": public,
        },
        "catch_up": {
            "type": "room.attached",
            "sync": {
                "kind": "retained_frames",
                "cursor_exclusive": 21,
                "through_frame_head": 22,
            },
            "projection": public,
        },
        "error": {"type": "error", "code": "slow_consumer", "retryable": True},
        "log": canonical_text(public),
        "ui": {"surface": "public", "projection": public},
        "replay": {
            "read_only": True,
            "historical_authorization": "public_projection",
            "projection": historical,
        },
    }


class PrivacyMatrixTests(unittest.TestCase):
    def test_each_hidden_owner_is_noninterfering_across_unauthorized_surfaces(
        self,
    ) -> None:
        for owner in ("navigator", "insider", "broker"):
            baseline = _state()
            paired = _paired_state(owner)
            for audience in ("public", "operator", "historical"):
                self.assertEqual(
                    audience_projection(baseline, audience),
                    audience_projection(paired, audience),
                    f"{owner} hidden state changed {audience} projection",
                )
            for audience in ("navigator", "insider", "broker"):
                before = audience_projection(baseline, "participant", audience)
                after = audience_projection(paired, "participant", audience)
                if audience == owner:
                    self.assertNotEqual(
                        before, after, f"{owner} owner view did not change"
                    )
                else:
                    self.assertEqual(
                        before, after, f"{owner} hidden state leaked to {audience}"
                    )

    def test_protocol_shaped_surfaces_and_historical_replay_are_pairwise_equal(
        self,
    ) -> None:
        baseline = _wire_surfaces(_state())
        for owner in ("navigator", "insider", "broker"):
            self.assertEqual(
                baseline,
                _wire_surfaces(_paired_state(owner)),
                f"{owner} hidden state changed a public wire/UI surface",
            )
        self.assertTrue(baseline["replay"]["read_only"])
        self.assertEqual(
            baseline["replay"]["historical_authorization"], "public_projection"
        )
        for surface in baseline.values():
            self.assertNotIn("private-plan", canonical_text(surface))
        self.assertEqual(digest(baseline), GOLDEN_MATRIX_DIGEST)


if __name__ == "__main__":
    unittest.main()

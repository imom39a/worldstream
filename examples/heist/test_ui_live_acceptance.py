from __future__ import annotations

import unittest

from wave9_live.run_ui_live_acceptance import (
    TRANSITION_PREREQUISITE,
    classify_offer_probe,
)


class UiLiveAcceptanceProbeTests(unittest.TestCase):
    def test_zero_offers_fail_closed_with_the_transition_prerequisite(self) -> None:
        result = classify_offer_probe(readyz=200, offers=[])

        self.assertEqual(result["status"], "blocked")
        self.assertEqual(
            result["reason_code"], "activation_transition_producer_missing"
        )
        self.assertEqual(result["prerequisite"], TRANSITION_PREREQUISITE)
        self.assertEqual(result["offer_count"], 0)
        self.assertFalse(result["fabricated_offer"])

    def test_real_offer_is_reported_without_claiming_a_heist_completion(self) -> None:
        result = classify_offer_probe(
            readyz=200, offers=[{"activation_id": "activation-real"}]
        )

        self.assertEqual(result["status"], "activation_available")
        self.assertEqual(result["offer_count"], 1)
        self.assertFalse(result["fabricated_offer"])

    def test_unready_daemon_and_malformed_response_are_blocked(self) -> None:
        self.assertEqual(
            classify_offer_probe(readyz=503, offers=[])["reason_code"],
            "daemon_not_ready",
        )
        self.assertEqual(
            classify_offer_probe(readyz=200, offers=None)["reason_code"],
            "invalid_offer_response",
        )


if __name__ == "__main__":
    unittest.main()

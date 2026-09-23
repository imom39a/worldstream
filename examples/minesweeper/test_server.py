"""Unit tests for the comparison server's configuration and run loop."""

from __future__ import annotations

import time
import unittest
from unittest.mock import patch

from examples.minesweeper import server
from examples.minesweeper.server import _validate_start


class ValidateStartTests(unittest.TestCase):
    def test_defaults_and_explicit_values(self) -> None:
        config = _validate_start({"model": "some/model", "jev_model": "jev-latest"})
        self.assertEqual((config["width"], config["height"], config["mines"]), (8, 8, 10))
        self.assertEqual(config["model"], "some/model")
        self.assertEqual(config["jev_model"], "jev-latest")
        self.assertIsInstance(config["seed"], int)

    def test_seed_is_honoured(self) -> None:
        config = _validate_start({"model": "m", "seed": 42})
        self.assertEqual(config["seed"], 42)

    def test_rejects_bad_values(self) -> None:
        for body in (
            {"model": "m", "width": 1},
            {"model": "m", "mines": 99, "width": 4, "height": 4},
            {"model": "m", "jev_model": "not-a-model"},
            {"model": ""},
            {"model": "m", "duration_seconds": 1},
        ):
            with self.assertRaises(ValueError):
                _validate_start(body)

    def test_accepts_large_board(self) -> None:
        config = _validate_start({"model": "m", "width": 500, "height": 500, "mines": 99})
        self.assertEqual((config["width"], config["height"]), (500, 500))

    def test_accepts_many_mines_up_to_capacity(self) -> None:
        config = _validate_start({"model": "m", "width": 200, "height": 200, "mines": 9000})
        self.assertEqual(config["mines"], 9000)
        with self.assertRaises(ValueError):
            _validate_start({"model": "m", "width": 200, "height": 200, "mines": 40000})

    def test_engines_and_candidate_limit(self) -> None:
        config = _validate_start(
            {"model": "m", "left_engine": "none", "right_engine": "jev", "candidate_limit": 16}
        )
        self.assertEqual(config["left_engine"], "none")
        self.assertEqual(config["candidate_limit"], 16)
        for body in (
            {"model": "m", "left_engine": "bogus"},
            {"model": "m", "left_engine": "none", "right_engine": "none"},
            {"model": "m", "candidate_limit": 100},
        ):
            with self.assertRaises(ValueError):
                _validate_start(body)


class ComparisonRunTests(unittest.TestCase):
    """Drive the real loop with mocked providers so no API calls are made."""

    def _run(self, llm, jev, seed: int = 7) -> dict[str, object]:
        comparison = server.Comparison(server._default_html_path())
        config = _validate_start(
            {
                "model": "mock/model",
                "width": 4,
                "height": 4,
                "mines": 1,
                "duration_seconds": 20,
                "seed": seed,
            }
        )
        with (
            patch.object(server, "llm_choose", llm),
            patch.object(server, "jev_choose", jev),
            patch.object(server, "openrouter_key", lambda: "test-key"),
            patch.object(server, "jev_key", lambda: "test-key"),
        ):
            comparison.start(config)
            snapshot: dict[str, object] = {}
            for _ in range(100):
                snapshot = comparison.snapshot()
                if snapshot["timer"]["status"] == "complete":  # type: ignore[index]
                    break
                time.sleep(0.05)
            comparison.stop()
        return snapshot

    def test_both_rooms_record_decisions_and_complete(self) -> None:
        def llm(state, model, key, timeout=30.0):
            return state["candidates"][0]["cell"], {"confidence": 0.5, "reason": "safe"}

        def jev(state, criteria, model, key, timeout=20.0):
            return next(iter(criteria)), {"confidence": 0.9}

        snapshot = self._run(llm, jev)
        self.assertEqual(snapshot["timer"]["status"], "complete")  # type: ignore[index]
        sides = snapshot["sides"]  # type: ignore[index]
        for key in ("left", "right"):
            self.assertIn(sides[key]["status"], {"won", "lost"})
            self.assertTrue(sides[key]["decisions"])
            self.assertFalse(sides[key]["decisions"][-1]["fallback"])
            event_kinds = [event["kind"] for event in sides[key]["events"]]
            self.assertIn("decision", event_kinds)
            self.assertIn("cell_revealed", event_kinds)

    def test_unusable_choice_falls_back_without_stalling(self) -> None:
        def llm(state, model, key, timeout=30.0):
            return "not-a-cell", {}

        def jev(state, criteria, model, key, timeout=20.0):
            return "nonsense", {}

        snapshot = self._run(llm, jev)
        self.assertEqual(snapshot["timer"]["status"], "complete")  # type: ignore[index]
        sides = snapshot["sides"]  # type: ignore[index]
        for key in ("left", "right"):
            self.assertIn(sides[key]["status"], {"won", "lost"})
            self.assertTrue(sides[key]["decisions"][-1]["fallback"])

    def test_clock_freezes_at_first_finish(self) -> None:
        def llm(state, model, key, timeout=30.0):
            return state["candidates"][0]["cell"], {"confidence": 0.9}

        def jev(state, criteria, model, key, timeout=20.0):
            time.sleep(0.5)
            return next(iter(criteria)), {"confidence": 0.9}

        comparison = server.Comparison(server._default_html_path())
        config = _validate_start(
            {
                "model": "mock/model",
                "width": 4,
                "height": 4,
                "mines": 1,
                "duration_seconds": 30,
                "seed": 7,
            }
        )
        with (
            patch.object(server, "llm_choose", llm),
            patch.object(server, "jev_choose", jev),
            patch.object(server, "openrouter_key", lambda: "test-key"),
            patch.object(server, "jev_key", lambda: "test-key"),
        ):
            comparison.start(config)
            for _ in range(100):
                snapshot = comparison.snapshot()
                if snapshot["sides"]["left"]["status"] in {"won", "lost"}:  # type: ignore[index]
                    break
                time.sleep(0.05)
            snapshot = comparison.snapshot()
            frozen = snapshot["timer"]["frozen_elapsed_ms"]  # type: ignore[index]
            self.assertIsInstance(frozen, int)
            self.assertEqual(snapshot["timer"]["frozen_by"], "left")  # type: ignore[index]
            time.sleep(0.6)
            self.assertEqual(comparison.snapshot()["timer"]["frozen_elapsed_ms"], frozen)  # type: ignore[index]
            comparison.stop()


if __name__ == "__main__":
    unittest.main()

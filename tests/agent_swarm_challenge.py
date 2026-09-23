"""Evidence and behavioral-check tests for the controlled coding challenge."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "worldstream_agent_swarm_challenge",
    Path(__file__).resolve().parents[1] / "scripts/agent_swarm_challenge.py",
)
assert SPEC is not None and SPEC.loader is not None
CHALLENGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHALLENGE)


class NativeOverlapEvidence(unittest.TestCase):
    def trial(self):
        trial = CHALLENGE.Trial.__new__(CHALLENGE.Trial)
        trial.control = Path("/fixture/swarmctl")
        trial.execution_state = Path("/fixture/state")
        trial.root = Path("/fixture")
        return trial

    def test_retained_terminal_result_cannot_count_as_native_overlap(self):
        # Supervisor status may still say Running until acknowledgment.
        terminal = subprocess.CompletedProcess([], 0, json.dumps({
            "kind": "completion", "value": {
                "invocation_id": "turn-a", "resolution": "completed",
            },
        }), "")
        with patch.object(CHALLENGE.subprocess, "run", return_value=terminal):
            self.assertFalse(self.trial().native_is_running("turn-a"))

    def test_only_exact_missing_completion_counts_as_pending(self):
        pending = subprocess.CompletedProcess([], 1, "", "the requested execution object was not found\n")
        with patch.object(CHALLENGE.subprocess, "run", return_value=pending):
            self.assertTrue(self.trial().native_is_running("turn-a"))

    def test_control_failure_does_not_fabricate_overlap(self):
        failed = subprocess.CompletedProcess([], 1, "", "Error: Unavailable\n")
        with (
            patch.object(CHALLENGE.subprocess, "run", return_value=failed),
            self.assertRaisesRegex(RuntimeError, "unexpected reason"),
        ):
            self.trial().native_is_running("turn-a")

    def test_wrong_completion_identity_is_rejected(self):
        wrong = subprocess.CompletedProcess([], 0, json.dumps({
            "kind": "completion", "value": {"invocation_id": "turn-b"},
        }), "")
        with (
            patch.object(CHALLENGE.subprocess, "run", return_value=wrong),
            self.assertRaisesRegex(RuntimeError, "mismatched"),
        ):
            self.trial().native_is_running("turn-a")


class CsvBehavioralCheck(unittest.TestCase):
    def check_source(self, formatter):
        with tempfile.TemporaryDirectory() as root:
            Path(root, "csv_tool.py").write_text(
                CHALLENGE.HUMAN_EDIT + CHALLENGE.PARSER + "\n" + formatter,
                encoding="utf-8",
            )
            result = subprocess.run(
                [sys.executable, "-B", "-c", CHALLENGE.CHECKER],
                cwd=root, capture_output=True, text=True, check=False, timeout=10,
            )
        return result.returncode, json.loads(result.stdout)

    def test_correct_source_passes_all_cases(self):
        code, report = self.check_source(CHALLENGE.FORMATTER)
        self.assertEqual(code, 0)
        self.assertEqual(report, {"cases": 16, "passed": 16, "failed": []})

    def test_deliberate_escaping_bug_fails_a_behavioral_case(self):
        code, report = self.check_source(CHALLENGE.BAD_FORMATTER)
        self.assertNotEqual(code, 0)
        self.assertEqual(report, {"cases": 16, "passed": 15, "failed": ["round_trip"]})


class FailureDiagnostics(unittest.TestCase):
    def trial(self, root):
        trial = CHALLENGE.Trial.__new__(CHALLENGE.Trial)
        trial.root = Path(root)
        trial.app = Path("/fixture/app")
        trial.control = Path("/fixture/swarmctl")
        trial.execution_state = Path("/fixture/state")
        trial.swarm_id = "swarm"
        trial.coordinator = []
        return trial

    def test_failure_captures_running_state_before_cleanup_without_private_payload(self):
        with tempfile.TemporaryDirectory() as root:
            trial = self.trial(root)
            journal = trial.root / "coordinator/coordinator/coordinator.json"
            journal.parent.mkdir(parents=True)
            journal.write_text(json.dumps({"state": {"intents": {"turn-a": {
                "state": "launch_prepared", "prepared": {"stdin": "PRIVATE_PROMPT"},
                "submission": {"status": "not_attempted"},
            }}}}))
            status = subprocess.CompletedProcess([], 0, json.dumps({"value": {"swarms": [{
                "swarm_id": "swarm", "phase": "running", "active": [{
                    "ticket": {"invocation_id": "turn-a"}, "resolution": "running",
                }], "private": "PRIVATE_TOKEN",
            }]}}), "")
            original = RuntimeError("dispatch failed")
            with (
                patch.object(trial, "command", side_effect=original),
                patch.object(CHALLENGE.subprocess, "run", return_value=status) as run,
                self.assertRaises(RuntimeError) as raised,
            ):
                trial.settle(["turn-a"])
            self.assertIs(raised.exception, original)
            run.assert_called_once()
            self.assertEqual(run.call_args.kwargs["timeout"], 2)
            # Simulate the later cleanup that would erase the useful state.
            journal.write_text(json.dumps({"state": {"intents": {}}}))
            serialized = (trial.root / "failure-diagnostics.json").read_text()
            captured = json.loads(serialized)
            self.assertEqual(captured["operation"], "worker-dispatch")
            self.assertEqual(captured["daemon_status"]["swarms"][0]["phase"], "running")
            self.assertEqual(captured["coordinator_intents"]["turn-a"]["state"], "launch_prepared")
            self.assertTrue(captured["coordinator_intents"]["turn-a"]["prepared"])
            self.assertNotIn("PRIVATE", serialized)

    def test_diagnostic_timeout_and_missing_journal_preserve_original_failure(self):
        with tempfile.TemporaryDirectory() as root:
            trial = self.trial(root)
            original = RuntimeError("dispatch failed")
            with (
                patch.object(trial, "command", side_effect=original),
                patch.object(CHALLENGE.subprocess, "run", side_effect=subprocess.TimeoutExpired("status", 2)),
                self.assertRaises(RuntimeError) as raised,
            ):
                trial.settle(["turn-a"])
            self.assertIs(raised.exception, original)
            captured = json.loads((trial.root / "failure-diagnostics.json").read_text())
            self.assertEqual(captured["daemon_status"]["capture_error"], "TimeoutExpired")
            self.assertEqual(captured["coordinator_capture_error"], "FileNotFoundError")


if __name__ == "__main__":
    unittest.main()

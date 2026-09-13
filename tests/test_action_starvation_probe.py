from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "action-starvation-probe.py"


def load_probe():
    spec = importlib.util.spec_from_file_location("action_starvation_probe", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError("probe module could not be loaded")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class ActionStarvationProbeTests(unittest.TestCase):
    def test_report_is_repeatable_and_has_machine_readable_decision(self):
        command = [sys.executable, str(SCRIPT), "--compact"]
        first = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=False)
        second = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=False)
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(first.stdout, second.stdout)
        report = json.loads(first.stdout)
        self.assertEqual(report["schema"], "worldstream/action-starvation-probe/v1")
        self.assertEqual(report["probe_version"], "deterministic-periodic-v1")
        self.assertTrue(report["admission_contract"]["based_on_room_seq"].startswith("exact_"))
        self.assertFalse(report["admission_contract"]["stale_action_reused"])
        self.assertIn("decision", report["threshold_evaluation"])
        self.assertGreaterEqual(len(report["scenarios"]), 90)

    def test_stale_attempts_preserve_exact_basis_and_use_new_action_ids(self):
        probe = load_probe()
        report = probe.run_scenario(
            probe.Scenario(100, 10, 1, "visible", "unrelated", trials=1),
            sample_trace=True,
        )
        attempts = report["sample_trial_attempts"]
        self.assertEqual(report["measurements"]["starved_trials"], 1)
        self.assertEqual(report["measurements"]["attempts_per_trial_max"], probe.MAX_ATTEMPTS_PER_TRIAL)
        self.assertEqual(len({item["action_id"] for item in attempts}), len(attempts))
        for item in attempts:
            if item["outcome"] == "stale_room_state":
                self.assertNotEqual(item["based_on_room_seq"], item["current_room_seq_at_admission"])
                self.assertTrue(item["new_action_after_stale"])
        self.assertEqual(
            report["measurements"]["model_equivalent_wasted_work"],
            report["measurements"]["stale_rejections"],
        )

    def test_hidden_head_lag_is_reported_separately_from_during_reasoning(self):
        probe = load_probe()
        hidden = probe.run_scenario(
            probe.Scenario(0, 0, 1, "hidden", "related", "hidden_head_lag", trials=1),
            sample_trace=True,
        )
        synchronized = probe.run_scenario(
            probe.Scenario(0, 0, 1, "hidden", "related", "during_reasoning", trials=1),
            sample_trace=True,
        )
        self.assertEqual(hidden["measurements"]["stale_rejections"], 1)
        self.assertEqual(hidden["measurements"]["updates"]["hidden"], 1)
        self.assertEqual(synchronized["measurements"]["stale_rejections"], 0)
        self.assertTrue(hidden["scenario"]["mode"] != synchronized["scenario"]["mode"])
        trace = hidden["sample_trial_attempts"][0]
        self.assertEqual(trace["based_on_room_seq"], 0)
        self.assertEqual(trace["current_room_seq_at_admission"], 1)

    def test_related_and_unrelated_updates_both_obey_whole_head(self):
        probe = load_probe()
        for relation in ("related", "unrelated"):
            report = probe.run_scenario(
                probe.Scenario(100, 5, 2, "visible", relation, trials=2),
                sample_trace=True,
            )
            self.assertGreater(report["measurements"]["stale_rejections"], 0)
            self.assertGreater(report["measurements"]["updates"][relation], 0)
            self.assertLessEqual(
                report["measurements"]["pending_attempts_max"],
                report["bounds"]["max_pending_attempts"],
            )


if __name__ == "__main__":
    unittest.main()

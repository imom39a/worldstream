import json
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "action-starvation-production-matrix.py"


class ProductionActionStarvationMatrixTests(unittest.TestCase):
    def test_production_admission_evidence_is_machine_readable_and_exact(self):
        result = subprocess.run([sys.executable, str(SCRIPT), "--compact"], cwd=ROOT, text=True, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report["schema"], "worldstream/action-starvation-production-matrix/v1")
        evidence = report["production_admission"]
        self.assertEqual(evidence["status"], "passed")
        self.assertTrue(evidence["exact_basis"])
        self.assertFalse(evidence["auto_rebase"])
        self.assertEqual(evidence["dimensions"]["update_visibility"], ["visible", "hidden"])
        self.assertEqual(evidence["dimensions"]["update_relation"], ["related", "unrelated"])
        self.assertTrue(report["comparison"]["imo_217_hidden_head_lag_is_separate"])


if __name__ == "__main__":
    unittest.main()

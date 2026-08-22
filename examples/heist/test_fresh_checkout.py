from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


class FreshCheckoutRunnerTests(unittest.TestCase):
    def test_runner_works_from_a_clean_offline_story_checkout(self) -> None:
        source = Path(__file__).parent
        with tempfile.TemporaryDirectory() as directory:
            checkout = Path(directory) / "examples" / "heist"
            checkout.mkdir(parents=True)
            for name in ("story.py", "run_story.py", "parity_fixture.json"):
                shutil.copy2(source / name, checkout / name)

            environment = {
                "PATH": os.environ.get("PATH", ""),
                "PYTHONIOENCODING": "utf-8",
            }
            result = subprocess.run(
                [sys.executable, str(checkout / "run_story.py"), "--self-test"],
                cwd=checkout.parent.parent.parent,
                env=environment,
                check=True,
                capture_output=True,
                text=True,
            )

        payload = json.loads(result.stdout)
        self.assertEqual(payload["status"], "ok")
        self.assertEqual(payload["room_seq"], 15)
        self.assertEqual(payload["expected_fixture_outcome"]["outcome"], "success")
        self.assertTrue(payload["replay"]["verified"])


if __name__ == "__main__":
    unittest.main()

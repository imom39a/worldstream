from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).resolve().with_name("run_live_acceptance.py")
SPEC = importlib.util.spec_from_file_location(
    "worldstream_heist_wave11_acceptance", MODULE_PATH
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load the sibling Heist acceptance runner")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
parse_result = MODULE.parse_result
redact = MODULE.redact


class LiveAcceptanceBoundaryTests(unittest.TestCase):
    def test_redacts_bearers_and_ulids(self) -> None:
        value = "wsb1:" + "a" * 64 + " 01ARZ3NDEKTSV4RRFFQ69G5FB0"
        result = redact(value)
        self.assertNotIn("a" * 64, result)
        self.assertNotIn("01ARZ3NDEKTSV4RRFFQ69G5FB0", result)
        self.assertIn("<redacted>", result)

    def test_parse_result_does_not_accept_non_json_logs(self) -> None:
        result = parse_result(
            {
                "timed_out": False,
                "stdout": "warning\nnot json\n",
                "stderr": "",
                "exit_code": 2,
            }
        )
        self.assertEqual(result["status"], "blocked")
        self.assertEqual(result["reason_code"], "runner_returned_no_json")

    def test_parse_result_removes_room_identity(self) -> None:
        result = parse_result(
            {
                "timed_out": False,
                "stdout": '{"status":"completed","room_id":"01ARZ3NDEKTSV4RRFFQ69G5FB0"}\n',
                "stderr": "",
                "exit_code": 0,
            }
        )
        self.assertEqual(result, {"status": "completed"})


if __name__ == "__main__":
    unittest.main()

"""Verify that adaptive-recovery evidence cannot be manufactured by a retry."""

from __future__ import annotations

import importlib.util
import json
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
CHALLENGE_SPEC = importlib.util.spec_from_file_location(
    "worldstream_autonomy_challenge_helper", SCRIPTS / "agent_swarm_challenge.py"
)
assert CHALLENGE_SPEC is not None and CHALLENGE_SPEC.loader is not None
CHALLENGE = importlib.util.module_from_spec(CHALLENGE_SPEC)
CHALLENGE_SPEC.loader.exec_module(CHALLENGE)
AUTONOMY_SPEC = importlib.util.spec_from_file_location(
    "worldstream_autonomy_protocol_helper", SCRIPTS / "agent_swarm_autonomy.py"
)
assert AUTONOMY_SPEC is not None and AUTONOMY_SPEC.loader is not None
AUTONOMY = importlib.util.module_from_spec(AUTONOMY_SPEC)
with patch.dict(sys.modules, {"agent_swarm_challenge": CHALLENGE}):
    AUTONOMY_SPEC.loader.exec_module(AUTONOMY)


def intent(*, revised: bool, accepted: bool, kind: str = "work_attempt"):
    return {
        "plan": {"semantic_target": {"kind": kind, "work_id": "work-1"},
                 "instruction": json.dumps({"instructions": "Retained process failure: revised task." if revised else "Original task."})},
        "submission": {"status": "received", "value": {"status": "accepted"}} if accepted
        else {"status": "not_submitted", "value": "process_failed"},
    }


class RecoveryEvidence(unittest.TestCase):
    def test_unchanged_successful_retry_is_not_adaptive_recovery(self):
        evidence = AUTONOMY.summarize_recovery({
            "first": intent(revised=False, accepted=False),
            "retry": intent(revised=False, accepted=True),
        })
        self.assertEqual(evidence["recovered_work_count"], 0)
        self.assertEqual(evidence["failed_invocation_count"], 1)

    def test_changed_instruction_and_accepted_same_work_prove_fixture_recovery(self):
        evidence = AUTONOMY.summarize_recovery({
            "first": intent(revised=False, accepted=False),
            "retry": intent(revised=True, accepted=True),
        })
        self.assertEqual(evidence["recovered_work_count"], 1)
        self.assertEqual(evidence["failed_invocation_count"], 1)

    def test_planning_failure_does_not_substitute_for_worker_failure(self):
        evidence = AUTONOMY.summarize_recovery({
            "planner": intent(revised=False, accepted=False, kind="planning"),
            "worker": intent(revised=True, accepted=True),
        })
        self.assertEqual(evidence["recovered_work_count"], 0)
        self.assertEqual(evidence["failed_invocation_count"], 0)


if __name__ == "__main__":
    unittest.main()

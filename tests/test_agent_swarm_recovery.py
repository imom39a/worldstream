"""Fixed starter and real-check preflight for instructed live recovery."""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path
from unittest.mock import patch

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))

from agent_swarm_live import run_live
from agent_swarm_recovery import (
    BASELINE_SHA256,
    DELIVERED_SOURCE,
    REMOVED_CALL,
    baseline_checker_preflight,
    frozen_baseline,
    recovery_goal,
    recovery_source_constraints,
)


def test_starter_is_exactly_one_deletion_and_survives_bounded_setup_request():
    original = DELIVERED_SOURCE.read_bytes()
    baseline = frozen_baseline()
    assert original.count(REMOVED_CALL) == 1
    assert baseline == original.replace(REMOVED_CALL, b"", 1)
    assert hashlib.sha256(baseline).hexdigest() == BASELINE_SHA256
    goal = recovery_goal(baseline)
    chunks = recovery_source_constraints(baseline)
    embedded = "".join(json.loads(chunk.split("JSON string: ", 1)[1]) for chunk in chunks)
    assert embedded.encode("utf-8") == baseline
    assert len(goal.encode("utf-8")) <= 4096
    assert all(len(chunk.encode("utf-8")) <= 2048 for chunk in chunks)
    assert all(not any(ord(char) < 32 or ord(char) == 127 for char in item)
               for item in [goal, *chunks])
    assert len(chunks) + 6 <= 64  # Five shared constraints and one recovery instruction.


@pytest.mark.skipif(sys.platform != "darwin", reason="macOS Seatbelt boundary")
def test_unchanged_production_checker_detects_declared_defect():
    preflight = baseline_checker_preflight(frozen_baseline())
    assert preflight["exit_code"] == 1
    assert preflight["output"] == {
        "cases": 26,
        "passed": 25,
        "failed": ["quote_in_unquoted_field"],
    }
    assert preflight["stderr"] == ""


def test_setup_verification_failure_closes_owned_native_profile(tmp_path):
    events = []

    class TrialWithFailedSetup:
        def __init__(self, *_args, **_kwargs):
            self.swarm_id = "swarm-under-test"
            self.native = type("Native", (), {"close": lambda _self: events.append("close")})()

        def run(self):
            raise RuntimeError("setup verification failed")

        def ctl(self, operation, swarm_id):
            events.append((operation, swarm_id))

    executable = tmp_path / "executable"
    executable.write_text("fixture")
    with (
        patch("agent_swarm_recovery.RecoveryTrial", TrialWithFailedSetup),
        pytest.raises(RuntimeError, match="setup verification failed"),
    ):
        run_live(
            None, model="gpt-5.6-sol", effort="medium", codex=executable,
            baseline_recovery=True, process_guard=executable, app=executable,
            root=tmp_path,
        )
    assert events == [("stop", "swarm-under-test"), "close"]

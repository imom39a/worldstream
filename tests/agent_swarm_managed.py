"""Focused contracts for the native Agent Swarm acceptance driver."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "worldstream_agent_swarm_managed_smoke",
    ROOT / "scripts/verify-agent-swarm-managed.py",
)
if SPEC is None or SPEC.loader is None:  # pragma: no cover
    raise RuntimeError("could not load Agent Swarm managed smoke")
MANAGED = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MANAGED
SPEC.loader.exec_module(MANAGED)


@pytest.mark.parametrize(
    ("actor", "fixture_flag"),
    [
        ({"kind": "human_coordinator"}, False),
        ({"kind": "worker", "member_key": "fixture-a"}, True),
    ],
)
def test_submit_exact_uses_controlled_worker_fixture_seam_only_for_workers(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    actor: dict[str, str],
    fixture_flag: bool,
):
    captured: list[str] = []

    def fake_run(
        command: list[str], **_kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        captured.extend(command)
        return subprocess.CompletedProcess(command, 0, '{"status":"accepted"}', "")

    monkeypatch.setattr(MANAGED, "run", fake_run)
    action = {
        "actor": actor,
        "action_id": "action-1",
        "based_on_room_seq": 1,
        "offer_id": "offer-1",
        "action_type": "confirm_initial_setup",
        "payload_schema_digest": "sha256:fixture",
        "payload": {},
    }
    receipt = MANAGED.submit_exact(
        tmp_path / "app",
        [],
        "swarm-1",
        action,
        tmp_path,
        tmp_path,
    )
    assert receipt == {"status": "accepted"}
    assert ("--allow-controlled-worker-fixture" in captured) is fixture_flag
    assert json.loads((tmp_path / "action-action-1.json").read_text()) == action


def test_cleanup_failure_is_attached_without_replacing_primary_error(
    tmp_path: Path,
):
    marker = tmp_path / "cleanup-complete"

    with pytest.raises(ValueError, match="primary startup failure") as caught:
        try:
            raise ValueError("primary startup failure")
        finally:
            MANAGED.finish_cleanup(
                root=tmp_path,
                marker=marker,
                cleanup_failures=["controller stop was unavailable"],
            )

    assert caught.value.__notes__ == [
        (
            f"managed cleanup was not proven; retain {tmp_path}: "
            "controller stop was unavailable"
        )
    ]
    assert not marker.exists()


def test_cleanup_failure_without_primary_error_fails_closed(tmp_path: Path):
    with pytest.raises(RuntimeError, match="daemon remained active"):
        MANAGED.finish_cleanup(
            root=tmp_path,
            marker=tmp_path / "cleanup-complete",
            cleanup_failures=["daemon remained active"],
        )


def test_cleanup_accepts_only_exact_retained_stopped_evidence():
    receipt = {
        "status": "unavailable",
        "code": "controller_unavailable",
        "retained_server": {
            "controller": {
                "availability": "available",
                "phase": "stopped",
                "lease": "released",
            },
            "runtime": {
                "availability": "available",
                "phase": "stopped",
                "lease": "released",
            },
            "operation": {
                "checkpoint": {"action": "stop", "stage": "complete"}
            },
        },
    }
    already_stopped = subprocess.CompletedProcess(
        ["worldstreamctl"], 3, json.dumps(receipt), "controller unavailable"
    )
    assert MANAGED.managed_stop_proven(already_stopped)
    assert MANAGED.retained_managed_stop_proven(already_stopped)
    assert not MANAGED.retained_managed_stop_proven(
        subprocess.CompletedProcess(["worldstreamctl"], 0, "{}", "")
    )

    receipt["retained_server"]["operation"]["checkpoint"] = {
        "action": "start",
        "stage": "runtime_restart",
    }
    retained_after_async_controller_stop = subprocess.CompletedProcess(
        ["worldstreamctl"], 3, json.dumps(receipt), "controller unavailable"
    )
    assert MANAGED.retained_managed_stop_proven(
        retained_after_async_controller_stop
    )

    receipt["retained_server"]["runtime"]["phase"] = "running"
    still_running = subprocess.CompletedProcess(
        ["worldstreamctl"], 3, json.dumps(receipt), "controller unavailable"
    )
    assert not MANAGED.managed_stop_proven(still_running)
    assert not MANAGED.managed_stop_proven(
        subprocess.CompletedProcess(["worldstreamctl"], 3, "not json", "failed")
    )


def test_cleanup_boundedly_waits_for_retained_stop_checkpoint(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    waiting = {
        "status": "unavailable",
        "code": "controller_unavailable",
        "retained_server": {
            "controller": {
                "availability": "available",
                "phase": "ready",
                "lease": "held",
            },
            "runtime": {
                "availability": "available",
                "phase": "stopped",
                "lease": "released",
            },
            "operation": {
                "checkpoint": {"action": "stop", "stage": "complete"}
            },
        },
    }
    stopped = json.loads(json.dumps(waiting))
    stopped["retained_server"]["controller"].update(
        {"phase": "stopped", "lease": "released"}
    )
    receipts = iter([waiting, stopped])
    sleeps: list[float] = []

    def fake_run(
        command: list[str], **_kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.CompletedProcess(
            command, 3, json.dumps(next(receipts)), "controller unavailable"
        )

    monkeypatch.setattr(MANAGED, "run", fake_run)
    monkeypatch.setattr(MANAGED.time, "sleep", sleeps.append)

    assert MANAGED.wait_for_retained_managed_stop(
        control=tmp_path / "worldstreamctl",
        managed=["--state-dir", str(tmp_path)],
        cwd=tmp_path,
        attempts=2,
        interval_seconds=0.25,
    )
    assert sleeps == [0.25]


def test_action_reobserves_only_when_stale_receipt_allows_revision(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    observed_sequences: list[int] = []
    submitted_actions: list[dict[str, object]] = []

    def fake_observe(*_args: object) -> dict[str, object]:
        room_seq = len(observed_sequences) + 1
        observed_sequences.append(room_seq)
        return {
            "room_seq": room_seq,
            "action_offers": [
                {
                    "offer_id": f"offer-{room_seq}",
                    "action_type": "propose_work_item",
                    "payload_schema_digest": "blake3:" + "a" * 64,
                }
            ],
        }

    def fake_submit(
        _app: Path,
        _local: list[str],
        _swarm_id: str,
        action: dict[str, object],
        _root: Path,
        _cwd: Path,
    ) -> dict[str, object]:
        submitted_actions.append(action)
        if len(submitted_actions) == 1:
            return {
                "status": "rejected",
                "code": "stale_room_state",
                "may_submit_revised_action": True,
            }
        return {"status": "accepted", "duplicate": False}

    monkeypatch.setattr(MANAGED, "observe_actor", fake_observe)
    monkeypatch.setattr(MANAGED, "submit_exact", fake_submit)
    receipt, observation = MANAGED.act(
        tmp_path / "app",
        [],
        "swarm-1",
        "human",
        "propose_work_item",
        {"work_id": "work-1"},
        MANAGED.ActionIds(),
        tmp_path,
        tmp_path,
    )

    assert receipt["status"] == "accepted"
    assert observation["room_seq"] == 2
    assert observed_sequences == [1, 2]
    assert [action["based_on_room_seq"] for action in submitted_actions] == [1, 2]
    assert submitted_actions[0]["action_id"] != submitted_actions[1]["action_id"]


def test_tui_update_reopen_selects_the_exact_retained_swarm_index():
    assert MANAGED.tui_reopen_inputs(2) == [
        {"input": "down"},
        {"input": "down"},
        {"input": "open"},
        {"input": "resize", "width": 72, "height": 20},
        {"input": "refresh"},
        {"input": "quit"},
    ]
    with pytest.raises(ValueError, match="index is invalid"):
        MANAGED.tui_reopen_inputs(True)

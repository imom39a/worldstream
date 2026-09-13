from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "room-history-qualification.py"


def load_module():
    spec = importlib.util.spec_from_file_location("room_history_qualification", SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_all_tiers_are_deterministic_and_keep_counters_separate():
    module = load_module()
    specs = [module.TierSpec(size) for size in module.TIER_SIZES]
    first = module.qualify(tiers=specs)
    second = module.qualify(tiers=specs)
    assert first == second
    assert first["status"] == "smoke_pass"
    assert first["pass"] is False
    assert first["skipped_required_evidence"] == ["seventy_two_hour_soak"]
    for tier, size in zip(first["tiers"], module.TIER_SIZES):
        assert tier["tier"]["transitions"] == size
        counters = tier["counters"]
        assert counters["transitions"] == size
        assert counters["frames"] == size * 3
        assert counters["models"] != counters["transitions"]
        assert tier["bounded_state"]["bounded"]
        assert tier["action"]["based_on_room_seq_exact"]


def test_checkpoint_resume_does_not_replay_completed_tiers(tmp_path: Path):
    module = load_module()
    checkpoint = tmp_path / "qualification.json"
    first = module.qualify(tiers=[module.TierSpec(1_000)], checkpoint_path=checkpoint)
    assert first["status"] == "smoke_pass"
    resumed = module.qualify(tiers=[module.TierSpec(1_000)], resume_path=checkpoint)
    assert resumed["tiers"] == first["tiers"]
    assert resumed["skipped_required_evidence"] == ["history_tiers", "seventy_two_hour_soak"]


def test_required_evidence_fail_closed_and_cli_is_machine_readable():
    completed = subprocess.run([sys.executable, str(SCRIPT), "--tier", "1000", "--compact"], cwd=ROOT, text=True, capture_output=True)
    assert completed.returncode == 0, completed.stderr
    report = json.loads(completed.stdout)
    assert report["schema"] == "worldstream/room-history-qualification/v1"
    assert report["pass"] is False
    assert report["soak"]["status"] == "skipped"
    assert "seventy_two_hour_soak" in report["skipped_required_evidence"]


def test_bounds_reject_unbounded_runner_state():
    module = load_module()
    with pytest.raises(module.QualificationError, match="state_bytes"):
        module.run_tier(module.TierSpec(1_000, state_bytes=module.MAX_RUNNER_STATE_BYTES + 1))


def test_backend_gate_rejects_modeled_only_and_skipped_evidence():
    module = load_module()
    assert module.backend_can_qualify(
        {"sqlite": {"status": "completed", "source": "deterministic_model"}},
        require_sqlite=True,
        require_postgres=False,
    ) is False
    assert module.backend_can_qualify(
        {"sqlite": {"status": "skipped", "source": "not_requested"}},
        require_sqlite=True,
        require_postgres=False,
    ) is False
    assert module.postgres_backend_hook(None)["status"] == "skipped"
    assert module.postgres_backend_hook("postgres://redacted")["status"] == "failed"


def test_qualification_rejects_modeled_or_skipped_backend(monkeypatch):
    module = load_module()
    monkeypatch.setattr(
        module,
        "run_sqlite_backend",
        lambda **_: {"status": "completed", "source": "deterministic_model", "qualification_eligible": True},
    )
    monkeypatch.setattr(
        module,
        "run_sqlite_soak",
        lambda **_: {"status": "skipped", "source": "not_requested"},
    )
    report = module.qualify(tiers=[module.TierSpec(1_000)], real_sqlite=True)
    assert report["pass"] is False
    assert "backend_evidence" in report["failures"]
    assert any(item["id"] == "backend_evidence" and item["status"] == "failed" for item in report["required_evidence"])


def test_real_sqlite_soak_reuses_completed_checkpoint(monkeypatch):
    module = load_module()
    prior = {
        "status": "completed",
        "source": "production_sqlite_core_storage",
        "requested_seconds": 1.0,
        "observed_seconds": 1.002,
        "iterations": [{"status": "completed", "source": "production_sqlite_core_storage"}],
    }
    monkeypatch.setattr(module, "run_sqlite_backend", lambda **_: pytest.fail("completed soak should not rerun"))
    assert module.run_sqlite_soak(seconds=1.0, transitions=1_000, timeout_seconds=1.0, prior=prior) == prior

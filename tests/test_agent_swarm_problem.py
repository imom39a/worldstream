"""Custom live-problem validation and binding without native admission."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

from agent_swarm_autonomous_delivery import autonomous_delivery_policy
from agent_swarm_problem import ProblemTrial, load_problem


def fixture(
    tmp_path,
    *,
    source="def answer():\n    return 1\n",
    checker="assert True\n",
    **changes,
):
    (tmp_path / "starter.py").write_bytes(source.encode("utf-8"))
    (tmp_path / "checker.py").write_bytes(checker.encode("utf-8"))
    document = {
        "goal": "Implement answer in the requested file.",
        "constraints": ["Use standard Python syntax."],
        "acceptance_criterion": "The trusted checker passes.",
        "target": "answer.py",
        "initial_source_file": "starter.py",
        "checker_file": "checker.py",
    }
    document.update(changes)
    config = tmp_path / "problem.json"
    config.write_text(json.dumps(document), encoding="utf-8")
    return config


def test_exact_source_roundtrip_and_policy_binding(tmp_path, monkeypatch):
    source = "def answer():\n    return 'é'\n"
    config = fixture(tmp_path, source=source, target="answer.md")
    problem = load_problem(config)
    assert problem.source == source.encode("utf-8")
    assert (
        "".join(
            json.loads(part.split("JSON string: ", 1)[1])
            for part in problem.source_parts
        ).encode("utf-8")
        == problem.source
    )
    assert problem.require_parallel is False
    assert problem.summary()["source_parts"] >= 1
    trial = ProblemTrial.__new__(ProblemTrial)
    trial.problem = problem
    trial.guard = tmp_path / "guard"
    trial.guard.write_text("fixture")
    monkeypatch.setattr(
        "agent_swarm_autonomous_delivery.authorized_checker_command",
        lambda script: ("python", ["-c", script]),
    )
    policy = trial.planning_policy()
    assert policy["delivery"]["target"] == "answer.md"
    assert policy["delivery"]["resource_id"] == trial.resource_id()
    assert policy["delivery"]["checks"][0]["arguments"] == ["-c", problem.checker]
    assert trial.goal_text() == problem.goal_text()
    assert trial.initial_source().encode("utf-8") == problem.source


def test_dry_run_needs_no_workspace_or_model(tmp_path):
    config = fixture(tmp_path)
    command = [
        sys.executable,
        str(ROOT / "scripts/verify-agent-swarm-managed.py"),
        "--validate-problem",
        str(config),
    ]
    result = subprocess.run(
        command, cwd=ROOT, capture_output=True, text=True, check=True
    )
    assert (
        json.loads(result.stdout)["status"] == "validated_no_model_or_managed_services"
    )


def test_invalid_live_problem_rejected_before_managed_setup(tmp_path):
    config = fixture(tmp_path, source="x = '${BAD}'\n")
    command = [
        sys.executable,
        str(ROOT / "scripts/verify-agent-swarm-managed.py"),
        "--workspace",
        str(tmp_path / "missing-workspace"),
        "--root",
        str(tmp_path / "missing-root"),
        "--live-codex-report",
        str(tmp_path / "report.json"),
        "--live-problem",
        str(config),
        "--codex-path",
        str(tmp_path / "missing-codex"),
        "--live-model",
        "fixture-model",
        "--live-effort",
        "medium",
        "--acknowledge-live-moving-alias",
    ]
    result = subprocess.run(
        command, cwd=ROOT, capture_output=True, text=True, check=False
    )
    assert result.returncode != 0
    assert "setup-forbidden string" in result.stderr
    assert not (tmp_path / "missing-root").exists()


def test_invalid_checker_syntax_has_bounded_cli_error(tmp_path):
    config = fixture(tmp_path, checker="def broken(:\n    secret_payload = 1\n")
    command = [
        sys.executable,
        str(ROOT / "scripts/verify-agent-swarm-managed.py"),
        "--validate-problem",
        str(config),
    ]
    result = subprocess.run(
        command, cwd=ROOT, capture_output=True, text=True, check=False
    )
    assert result.returncode == 1
    assert "checker_file has invalid Python syntax at line 1" in result.stderr
    assert "secret_payload" not in result.stderr
    assert "Traceback" not in result.stderr


@pytest.mark.parametrize("target", ["../escape.py", "a.exe", "a/b.py", "-bad.py"])
def test_invalid_target(tmp_path, target):
    with pytest.raises(ValueError, match="target"):
        load_problem(fixture(tmp_path, target=target))


def test_input_paths_and_bounds_fail_closed(tmp_path):
    config = fixture(tmp_path, initial_source_file="../escape.py")
    with pytest.raises(ValueError, match="under the problem directory"):
        load_problem(config)
    config = fixture(tmp_path, source="x" * 8193)
    with pytest.raises(ValueError, match="8192"):
        load_problem(config)
    config = fixture(tmp_path, checker="x" * 4097)
    with pytest.raises(ValueError, match="4096"):
        load_problem(config)
    config = fixture(tmp_path)
    (tmp_path / "starter.py").unlink()
    (tmp_path / "starter.py").symlink_to(tmp_path / "checker.py")
    with pytest.raises(ValueError, match="symlink"):
        load_problem(config)


def test_setup_forbidden_strings_and_control_fail_closed(tmp_path):
    with pytest.raises(ValueError, match="setup-forbidden"):
        load_problem(fixture(tmp_path, source="x = '${VALUE}'\n"))
    with pytest.raises(ValueError, match="setup-forbidden"):
        load_problem(fixture(tmp_path, constraints=["line\nbreak"]))


def test_default_csv_policy_unchanged(monkeypatch, tmp_path):
    guard = tmp_path / "guard"
    guard.write_text("fixture")
    monkeypatch.setattr(
        "agent_swarm_autonomous_delivery.authorized_checker_command",
        lambda script: ("python", ["-c", script]),
    )
    policy = autonomous_delivery_policy(guard)
    assert policy["delivery"]["target"] == "csv_tool.py"
    assert policy["delivery"]["resource_id"] == "csv-target"
    assert policy["max_work_items"] == 4

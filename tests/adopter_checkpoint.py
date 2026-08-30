from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
KIT = ROOT / "qualification/outside-adopter-v1"


def load_module(path: Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


CHECKPOINT = load_module(KIT / "checkpoint.py", "worldstream_adopter_checkpoint")
TRIAL = load_module(
    ROOT / "scripts/adopter-trial.py", "worldstream_adopter_trial_checkpoint"
)


def test_artifact_checkpoint_captures_exact_bytes_without_path(tmp_path: Path) -> None:
    artifact = tmp_path / "release-manifest.json"
    artifact.write_bytes(b"exact release manifest bytes\n")

    checkpoint = CHECKPOINT.artifact_checkpoint("release_manifest", artifact)

    assert checkpoint["schema"] == CHECKPOINT.SCHEMA
    assert checkpoint["release_evidence"] is False
    assert checkpoint["receipt_row"] == {
        "name": "release_manifest",
        "sha256": "sha256:"
        + hashlib.sha256(b"exact release manifest bytes\n").hexdigest(),
        "size_bytes": 29,
    }
    assert str(tmp_path) not in json.dumps(checkpoint)


def test_command_checkpoint_redacts_argv_and_raw_output() -> None:
    checkpoint, exit_code = CHECKPOINT.command_checkpoint(
        "install",
        [sys.executable, "-c", "print('private-output')"],
        10,
    )

    encoded = json.dumps(checkpoint)
    assert exit_code == 0
    assert "private-output" not in encoded
    assert "print(" not in encoded
    assert checkpoint["receipt_row"]["stdout_sha256"] == (
        "sha256:" + hashlib.sha256(b"private-output\n").hexdigest()
    )
    assert checkpoint["capture"]["stdout_size_bytes"] == 15


def test_failed_command_is_captured_but_cannot_be_a_passing_row() -> None:
    checkpoint, exit_code = CHECKPOINT.command_checkpoint(
        "install", [sys.executable, "-c", "raise SystemExit(7)"], 10
    )

    assert exit_code == 7
    assert checkpoint["receipt_row"]["exit_code"] == 7


def test_command_capture_fails_closed_at_output_bound(monkeypatch) -> None:
    monkeypatch.setattr(CHECKPOINT, "MAX_CAPTURE_BYTES", 4)
    with pytest.raises(CHECKPOINT.CheckpointError, match="capture exceeds"):
        CHECKPOINT.command_checkpoint(
            "install", [sys.executable, "-c", "print('too-large')"], 10
        )


def test_checkpoint_refuses_symlink_and_overwrite(tmp_path: Path) -> None:
    artifact = tmp_path / "artifact"
    artifact.write_bytes(b"bytes")
    linked = tmp_path / "linked"
    linked.symlink_to(artifact)
    with pytest.raises(CHECKPOINT.CheckpointError, match="non-symlink"):
        CHECKPOINT.artifact_checkpoint("release_manifest", linked)

    output = tmp_path / "checkpoint.json"
    CHECKPOINT.exclusive_write(output, b"first\n")
    with pytest.raises(CHECKPOINT.CheckpointError, match="overwrite"):
        CHECKPOINT.exclusive_write(output, b"second\n")
    assert output.read_bytes() == b"first\n"


def test_unrun_template_is_rejected_as_a_trial_receipt(tmp_path: Path) -> None:
    with pytest.raises(TRIAL.TrialError, match="field inventory"):
        TRIAL.validate_receipt(
            KIT / "receipt.template.json",
            "0" * 64,
        )

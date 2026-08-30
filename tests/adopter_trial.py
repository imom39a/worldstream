from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]


def load_module():
    spec = importlib.util.spec_from_file_location(
        "worldstream_adopter_trial", ROOT / "scripts/adopter-trial.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


TRIAL = load_module()


def canonical(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()


def release_manifest(path: Path) -> str:
    content = canonical(
        {
            "schema": "worldstream/release-artifact-manifest/v2",
            "artifacts": {},
            "evidence": {},
        }
    )
    path.write_bytes(content)
    return hashlib.sha256(content).hexdigest()


def receipt(
    trial_id: str,
    participant: str,
    journey: str,
    route: str,
    profile: str,
    manifest_sha256: str,
) -> dict[str, Any]:
    checkpoints = (
        sorted(
            TRIAL.PACK_CHECKPOINTS
            | ({TRIAL.PROMPT_CHECKPOINT} if route == "prompt_assisted" else set())
        )
        if journey == "pack_author"
        else sorted(TRIAL.INTEGRATOR_CHECKPOINTS)
    )
    elapsed = 1200 if journey == "pack_author" else 900
    return {
        "schema": TRIAL.SCHEMA,
        "trial_id": trial_id,
        "participant": {
            "participant_id": participant,
            "non_contributor": True,
            "prior_contribution": False,
        },
        "journey": journey,
        "route": route,
        "installation_profile": profile,
        "release_manifest_sha256": manifest_sha256,
        "started_at": "2026-08-30T12:00:00Z",
        "completed_at": "2026-08-30T12:20:00Z"
        if elapsed == 1200
        else "2026-08-30T12:15:00Z",
        "elapsed_seconds": elapsed,
        "outcome": "passed",
        "constraints": {
            "clean_environment": True,
            "released_artifacts_only": True,
            "source_or_adr_access": False,
            "linear_or_chat_access": False,
            "rust_or_runtime_rebuild": False,
            "manual_database_access": False,
            "paid_dependency": False,
            "maintainer_interventions": 0,
        },
        "checkpoints": checkpoints,
        "artifacts": [
            {
                "name": "release_manifest",
                "sha256": "sha256:" + manifest_sha256,
                "size_bytes": 128,
            },
            {
                "name": "starter_distribution",
                "sha256": "sha256:" + "a" * 64,
                "size_bytes": 1024,
            },
        ],
        "commands": [
            {
                "argv_sha256": "sha256:" + "b" * 64,
                "duration_ms": 1000,
                "exit_code": 0,
                "name": checkpoint,
                "stderr_sha256": "sha256:" + "c" * 64,
                "stdout_sha256": "sha256:" + "d" * 64,
            }
            for checkpoint in checkpoints
        ],
        "diagnostics": [],
    }


def write_complete_set(root: Path, manifest_sha256: str) -> None:
    rows = [
        (
            "trial_author-a",
            "person-a",
            "pack_author",
            "deterministic",
            "native-linux-x86_64",
        ),
        (
            "trial_author-b",
            "person-b",
            "pack_author",
            "prompt_assisted",
            "native-windows-x64",
        ),
        (
            "trial_author-c",
            "person-c",
            "pack_author",
            "deterministic",
            "native-linux-x86_64",
        ),
        (
            "trial_integrator-a",
            "person-d",
            "application_integrator",
            "official_negotiate",
            "native-linux-x86_64",
        ),
        (
            "trial_integrator-b",
            "person-e",
            "application_integrator",
            "official_negotiate",
            "native-windows-x64",
        ),
        (
            "trial_integrator-c",
            "person-f",
            "application_integrator",
            "official_negotiate",
            "native-linux-x86_64",
        ),
    ]
    for index, row in enumerate(rows):
        (root / f"{index}.json").write_bytes(canonical(receipt(*row, manifest_sha256)))


def test_qualifies_exact_six_outside_adopter_receipts(tmp_path: Path) -> None:
    manifest = tmp_path / "release-manifest.json"
    digest = release_manifest(manifest)
    receipts = tmp_path / "receipts"
    receipts.mkdir()
    write_complete_set(receipts, digest)

    summary = TRIAL.qualify(receipts, manifest)

    assert summary["status"] == "qualified"
    assert summary["trial_count"] == 6
    assert summary["pack_author_count"] == 3
    assert summary["application_integrator_count"] == 3
    assert summary["installation_profiles"] == [
        "native-linux-x86_64",
        "native-windows-x64",
    ]


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        (
            lambda value: value["constraints"].__setitem__(
                "maintainer_interventions", 1
            ),
            "constraints",
        ),
        (lambda value: value.__setitem__("elapsed_seconds", 4000), "elapsed"),
        (
            lambda value: value.__setitem__("release_manifest_sha256", "0" * 64),
            "different release",
        ),
        (lambda value: value["checkpoints"].pop(), "checkpoint"),
        (
            lambda value: value["commands"][0].__setitem__("exit_code", 1),
            "failed command",
        ),
        (lambda value: value["commands"].pop(), "command inventory"),
        (lambda value: value["artifacts"].pop(), "artifact inventory"),
        (
            lambda value: value["artifacts"][0].__setitem__(
                "sha256", "sha256:" + "0" * 64
            ),
            "release artifact disagrees",
        ),
        (
            lambda value: value["commands"][0].__setitem__(
                "duration_ms", value["elapsed_seconds"] * 1000
            ),
            "durations exceed",
        ),
        (
            lambda value: value["diagnostics"].append(
                {"code": "failure", "resolution": "Bearer wsb1:secret"}
            ),
            "secret-like",
        ),
    ],
)
def test_rejects_nonqualifying_or_sensitive_receipt(
    tmp_path: Path, mutation, message: str
) -> None:
    manifest = tmp_path / "release-manifest.json"
    digest = release_manifest(manifest)
    candidate = receipt(
        "trial_author-a",
        "person-a",
        "pack_author",
        "deterministic",
        "native-linux-x86_64",
        digest,
    )
    mutation(candidate)
    path = tmp_path / "candidate.json"
    path.write_bytes(canonical(candidate))

    with pytest.raises(TRIAL.TrialError, match=message):
        TRIAL.validate_receipt(path, digest)


def test_does_not_self_certify_missing_people_or_profiles(tmp_path: Path) -> None:
    manifest = tmp_path / "release-manifest.json"
    digest = release_manifest(manifest)
    receipts = tmp_path / "receipts"
    receipts.mkdir()
    write_complete_set(receipts, digest)
    (receipts / "5.json").unlink()

    with pytest.raises(
        TRIAL.TrialError, match="exactly three Pack Authors and three integrators"
    ):
        TRIAL.qualify(receipts, manifest)

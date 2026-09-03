from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


def load_module():
    name = "worldstream_test_release_qualification"
    existing = sys.modules.get(name)
    if existing is not None:
        return existing
    path = ROOT / "scripts/release-qualification.py"
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


QUALIFICATION = load_module()


def canonical(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()


def adopter_summary(release_sha256: str) -> dict:
    rows = [
        ("trial_author-a", "pack_author", "deterministic", "native-linux-x86_64"),
        (
            "trial_author-b",
            "pack_author",
            "prompt_assisted",
            "native-windows-x64",
        ),
        ("trial_author-c", "pack_author", "deterministic", "native-linux-x86_64"),
        (
            "trial_integrator-a",
            "application_integrator",
            "official_negotiate",
            "native-linux-x86_64",
        ),
        (
            "trial_integrator-b",
            "application_integrator",
            "official_negotiate",
            "native-windows-x64",
        ),
        (
            "trial_integrator-c",
            "application_integrator",
            "official_negotiate",
            "native-linux-x86_64",
        ),
    ]
    return {
        "schema": QUALIFICATION.SUMMARY_SCHEMA,
        "status": "qualified",
        "release_manifest_sha256": release_sha256,
        "trial_count": 6,
        "pack_author_count": 3,
        "application_integrator_count": 3,
        "installation_profiles": ["native-linux-x86_64", "native-windows-x64"],
        "maximum_pack_author_seconds": 1200,
        "maximum_application_integrator_seconds": 900,
        "receipts": [
            {
                "trial_id": trial_id,
                "journey": journey,
                "route": route,
                "installation_profile": profile,
                "receipt_sha256": f"{index + 1:064x}",
            }
            for index, (trial_id, journey, route, profile) in enumerate(rows)
        ],
    }


def structural_layout(
    root: Path, monkeypatch, profile=QUALIFICATION.INVENTORY.LEGACY
) -> Path:
    release_value = {"schema": profile.manifest_schema, "product": "0.1.0"}
    if profile.serialized_discriminator:
        release_value["release_inventory"] = profile.identity
    release = canonical(release_value)
    release_sha = QUALIFICATION.sha256(release)
    (root / "release").mkdir(parents=True)
    (root / "release/release-manifest.json").write_bytes(release)
    (root / "release/sigstore.bundle.json").write_bytes(b"{}\n")
    (root / "starters/official").mkdir(parents=True)
    (root / "starters/custom").mkdir(parents=True)
    official = root / "starters/official/official.tar.gz"
    custom = root / "starters/custom/custom.tar.gz"
    official.write_bytes(b"official Starter fixture\n")
    custom.write_bytes(b"custom Starter fixture\n")
    (root / "inputs").mkdir()
    adopter = adopter_summary(release_sha)
    adopter_bytes = canonical(adopter)
    adopter_path = root / "inputs/outside-adopter-qualification.json"
    adopter_path.write_bytes(adopter_bytes)

    receipts = {
        "official": {
            "distribution": {
                "mode": "official",
                "profile": "native-linux-x86_64",
            }
        },
        "custom": {"distribution": {"mode": "custom", "profile": "macos-source"}},
    }

    def fake_starters(starters, expected_release_sha, *, authenticate=True):
        assert set(starters) == {"official", "custom"}
        assert expected_release_sha == release_sha
        return receipts, {
            identity: path.read_bytes() for identity, path in starters.items()
        }

    monkeypatch.setattr(QUALIFICATION, "validate_starters", fake_starters)
    reports = QUALIFICATION.evidence_reports(release_sha, receipts, adopter)
    (root / "evidence").mkdir()
    for evidence_id, content in reports.items():
        (root / f"evidence/{evidence_id}.json").write_bytes(content)

    artifacts = {
        "official": "starters/official/official.tar.gz",
        "custom": "starters/custom/custom.tar.gz",
    }
    evidence = {
        evidence_id: f"evidence/{evidence_id}.json"
        for evidence_id in QUALIFICATION.QUALIFICATION_IDS
    }
    manifest = {
        "schema": QUALIFICATION.SCHEMA,
        "product": "0.1.0",
        "release_manifest": {
            "path": "release/release-manifest.json",
            "sha256": QUALIFICATION.sha256_ref(release),
            "signature_path": "release/sigstore.bundle.json",
        },
        "artifacts": artifacts,
        "artifact_digests": {
            identity: QUALIFICATION.sha256_ref((root / relative).read_bytes())
            for identity, relative in artifacts.items()
        },
        "inputs": {
            "outside-adopter-qualification": "inputs/outside-adopter-qualification.json"
        },
        "input_digests": {
            "outside-adopter-qualification": QUALIFICATION.sha256_ref(adopter_bytes)
        },
        "evidence": evidence,
        "evidence_digests": {
            evidence_id: QUALIFICATION.sha256_ref((root / relative).read_bytes())
            for evidence_id, relative in evidence.items()
        },
        "verification_material": {
            "qualification-sigstore-bundle": {
                "path": "release-qualification-manifest.bundle.json"
            }
        },
    }
    path = root / "release-qualification-manifest.json"
    path.write_bytes(canonical(manifest))
    return path


@pytest.mark.parametrize(
    "profile", [QUALIFICATION.INVENTORY.LEGACY, QUALIFICATION.INVENTORY.CLI_FIRST]
)
def test_structural_manifest_binds_release_starters_adopters_and_evidence(
    tmp_path: Path, monkeypatch, profile
) -> None:
    structural_layout(tmp_path, monkeypatch, profile)

    value = QUALIFICATION.validate_layout(tmp_path, require_signature=False)

    assert value["schema"] == QUALIFICATION.SCHEMA
    assert set(value["evidence"]) == set(QUALIFICATION.QUALIFICATION_IDS)


@pytest.mark.parametrize(
    "value",
    [
        {"schema": QUALIFICATION.INVENTORY.CLI_FIRST.manifest_schema},
        {
            "schema": QUALIFICATION.INVENTORY.CLI_FIRST.manifest_schema,
            "release_inventory": QUALIFICATION.INVENTORY.LEGACY.identity,
        },
        {
            "schema": QUALIFICATION.INVENTORY.LEGACY.manifest_schema,
            "release_inventory": QUALIFICATION.INVENTORY.LEGACY.identity,
        },
    ],
)
def test_rejects_cross_wired_release_identity(value: dict) -> None:
    with pytest.raises(QUALIFICATION.QualificationError, match="identity"):
        QUALIFICATION.validate_release_identity(value, "release manifest")


def test_rejects_resigned_narrative_evidence_and_extra_files(
    tmp_path: Path, monkeypatch
) -> None:
    manifest_path = structural_layout(tmp_path, monkeypatch)
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    evidence_id = QUALIFICATION.QUALIFICATION_IDS[0]
    report_path = tmp_path / manifest["evidence"][evidence_id]
    report = json.loads(report_path.read_text(encoding="utf-8"))
    report["checks"]["official_subject_inventory"] = False
    report_path.write_bytes(canonical(report))
    manifest["evidence_digests"][evidence_id] = QUALIFICATION.sha256_ref(
        report_path.read_bytes()
    )
    manifest_path.write_bytes(canonical(manifest))

    with pytest.raises(QUALIFICATION.QualificationError, match="evidence is invalid"):
        QUALIFICATION.validate_layout(tmp_path, require_signature=False)

    report_path.write_bytes(
        QUALIFICATION.evidence_reports(
            QUALIFICATION.sha256(
                (tmp_path / "release/release-manifest.json").read_bytes()
            ),
            {
                "official": {
                    "distribution": {
                        "mode": "official",
                        "profile": "native-linux-x86_64",
                    }
                },
                "custom": {
                    "distribution": {"mode": "custom", "profile": "macos-source"}
                },
            },
            adopter_summary(
                QUALIFICATION.sha256(
                    (tmp_path / "release/release-manifest.json").read_bytes()
                )
            ),
        )[evidence_id]
    )
    manifest["evidence_digests"][evidence_id] = QUALIFICATION.sha256_ref(
        report_path.read_bytes()
    )
    manifest_path.write_bytes(canonical(manifest))
    (tmp_path / "narrative.txt").write_text("looks good\n", encoding="utf-8")
    with pytest.raises(QUALIFICATION.QualificationError, match="extra"):
        QUALIFICATION.validate_layout(tmp_path, require_signature=False)


def test_rejects_fabricated_adopter_counts_routes_and_mixed_release(
    tmp_path: Path,
) -> None:
    release_sha = "a" * 64
    path = tmp_path / "adopters.json"
    value = adopter_summary(release_sha)

    value["receipts"][0]["route"] = "narrative_only"
    path.write_bytes(canonical(value))
    with pytest.raises(QUALIFICATION.QualificationError, match="inconsistent"):
        QUALIFICATION.validate_adopters(path, release_sha)

    value = adopter_summary(release_sha)
    value["release_manifest_sha256"] = "b" * 64
    path.write_bytes(canonical(value))
    with pytest.raises(QUALIFICATION.QualificationError, match="another release"):
        QUALIFICATION.validate_adopters(path, release_sha)

    value = adopter_summary(release_sha)
    value["installation_profiles"][0] = "macos-source"
    value["receipts"][0]["installation_profile"] = "macos-source"
    path.write_bytes(canonical(value))
    with pytest.raises(QUALIFICATION.QualificationError, match="profiles are invalid"):
        QUALIFICATION.validate_adopters(path, release_sha)

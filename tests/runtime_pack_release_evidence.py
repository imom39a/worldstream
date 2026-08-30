from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def load_module():
    name = "worldstream_test_runtime_pack_release_evidence"
    existing = sys.modules.get(name)
    if existing is not None:
        return existing
    path = ROOT / "scripts/release-evidence-produce-runtime-packs.py"
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


EVIDENCE = load_module()


def canonical(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()


def release_contract(root: Path) -> tuple[Path, Path, dict]:
    authored = (ROOT / "compatibility.toml").read_text(encoding="utf-8")
    authored = authored.replace(
        'manifest_kind = "specification"', 'manifest_kind = "release"', 1
    ).replace("release_ready = false", "release_ready = true", 1)
    value = tomllib.loads(authored)
    toml_path = root / "compatibility.toml"
    json_path = root / "compatibility.json"
    toml_path.write_text(authored, encoding="utf-8")
    json_path.write_bytes(canonical(value))
    return toml_path, json_path, value


def write_json(path: Path, value: object) -> Path:
    path.write_bytes(canonical(value))
    return path


def diagnostic_inputs(root: Path, manifest: dict) -> SimpleNamespace:
    bundle_row = next(
        row
        for row in manifest["activity_pack_bundles"]
        if row["pack_id"] == "worldstream.negotiate"
    )
    bundle = ROOT / bundle_row["path"]
    runtime = root / "runtime.tar.gz"
    runtime.write_bytes(b"exact detached native Linux archive fixture\n")
    runtime_sha = "sha256:" + hashlib.sha256(runtime.read_bytes()).hexdigest()
    runtime_size = runtime.stat().st_size
    identity = EVIDENCE.STARTER.pack_identity(bundle.read_bytes(), "fixture bundle")
    pack_proof = json.loads(
        (ROOT / "packs/negotiate/evidence/conformance-v1.json").read_text(
            encoding="utf-8"
        )
    )["production_proof"]["complete"]

    policy = {
        "schema": "worldstream/negotiate-policy-qualification/v1",
        "status": "passed",
        "release_evidence": True,
        "artifact_identity": {
            "bundle_digest": identity["bundle_digest"],
            "revision_digest": identity["revision_digest"],
        },
        "source_policy": {
            "released_artifacts_only": True,
            "source_checkout_access": False,
            "target_debug_access": False,
            "manual_database_access": False,
        },
        "fresh_process_proof_equal": True,
        "independent_participant_network_cursors": True,
        "scope": {
            "production_bundle_verifier": True,
            "production_component_host": True,
            "production_core_registry": True,
        },
        "flow": {
            "bundle_digest": identity["bundle_digest"],
            "revision_digest": identity["revision_digest"],
            "actions": [
                "submit_proposal_revision",
                "submit_proposal_revision",
                "request_exact_approval",
                "record_exact_approval",
                "accept_current_proposal",
                "select_accepted_proposal",
                "record_agreement_signature",
                "record_agreement_signature",
                "commit_agreement",
            ],
            "outcome": "agreement_committed",
        },
        "production_proof": {"status": "passed"},
        "gates": [
            {"name": name, "status": "passed", "exit_code": 0}
            for name in sorted(
                {
                    "independent_restart_oracle",
                    "public_typescript_pack",
                    "participant_reconnect_and_privacy",
                    "dual_evidence_offline_verifier",
                }
            )
        ],
    }
    a202_artifact = root / "a202-adapter.tar.gz"
    a202_artifact.write_bytes(b"detached A202 adapter fixture\n")
    a202_binding = EVIDENCE.artifact_binding(a202_artifact, "A202 fixture")
    a202 = {
        "schema": "worldstream/a202-adapter-qualification/v1",
        "status": "passed",
        "release_evidence": True,
        "artifact_identity": a202_binding,
        "source_policy": {
            "released_artifacts_only": True,
            "source_checkout_access": False,
            "target_debug_access": False,
        },
        "profile": {
            "repository_revision": "fa85aa8b49bfe7b3f7ded487c98500a600e92e41",
            "shared_objects": "a202-commercial/0.1",
            "state_machine_rules": "1.3",
            "operated_scope": "a202-scope/operated/0.1",
            "bilateral_scope": "a202-scope/bilateral/0.1",
            "demonstration_profile": "a202-profile/calibration-service/0.1",
        },
        "checks": {
            name: True
            for name in (
                "exact_canonical_objects",
                "signature_and_mandate_verification",
                "authenticated_resolver_evidence",
                "acceptance_selection_and_independent_signatures",
                "agreement_committed_cross_index",
                "negative_mutation_matrix",
            )
        },
    }

    def restart(profile: str) -> dict:
        return {
            "schema": "worldstream/negotiate-released-artifact-acceptance/v1",
            "status": "passed",
            "release_evidence": True,
            "storage_profile": profile,
            "artifact_identity": {
                "runtime_sha256": runtime_sha,
                "runtime_size_bytes": runtime_size,
                "bundle_digest": identity["bundle_digest"],
                "revision_digest": identity["revision_digest"],
            },
            "room": {
                "pack_id": "worldstream.negotiate",
                "forced_daemon_restart": True,
                "independent_membership_cursors": 4,
                "all_memberships_reconnected": True,
                "outcome": "agreement_committed",
                "exact_executable_replay": True,
                "head_before_restart": "blake3:" + "a" * 64,
                "replayed_head": "blake3:" + "a" * 64,
                "offline_dual_evidence": True,
            },
            "policy": {
                "released_artifacts_only": True,
                "source_checkout_access": False,
                "target_debug_access": False,
                "manual_database_access": False,
            },
        }

    return SimpleNamespace(
        output_dir=root / "producers",
        component_only=False,
        bundle=bundle,
        pack_proof=write_json(root / "pack-proof.json", pack_proof),
        policy_report=write_json(root / "policy.json", policy),
        a202_report=write_json(root / "a202.json", a202),
        a202_artifact=a202_artifact,
        sqlite_restart_report=write_json(
            root / "sqlite-restart.json", restart("sqlite-bundled")
        ),
        postgres_restart_report=write_json(
            root / "postgres-restart.json", restart("postgres-primary")
        ),
        linux_archive=runtime,
    )


def test_produces_four_exact_artifact_bound_reports(tmp_path: Path) -> None:
    toml_path, json_path, manifest = release_contract(tmp_path)
    args = diagnostic_inputs(tmp_path, manifest)
    args.manifest_toml = toml_path
    args.manifest_json = json_path

    EVIDENCE.produce(args)

    assert {path.name for path in args.output_dir.iterdir()} == {
        "pack-component-conformance.json",
        "negotiate-policy.json",
        "negotiate-sqlite-restart.json",
        "negotiate-postgres-restart.json",
    }
    for path in args.output_dir.iterdir():
        value = json.loads(path.read_text(encoding="utf-8"))
        assert value["release_evidence"] is True
        assert value["fail_closed"] is False
        assert value["phase"] == "pre-sign"


def test_checkout_specification_cannot_emit_release_evidence(tmp_path: Path) -> None:
    with pytest.raises(EVIDENCE.RuntimePackEvidenceError, match="release-ready"):
        EVIDENCE.validate_release_contract(
            ROOT / "compatibility.toml", ROOT / "compatibility.json"
        )


def test_component_only_producer_needs_no_narrative_or_restart_receipt(
    tmp_path: Path,
) -> None:
    toml_path, json_path, manifest = release_contract(tmp_path)
    args = diagnostic_inputs(tmp_path, manifest)
    args.component_only = True
    args.policy_report = None
    args.a202_report = None
    args.a202_artifact = None
    args.sqlite_restart_report = None
    args.postgres_restart_report = None
    args.manifest_toml = toml_path
    args.manifest_json = json_path

    EVIDENCE.produce(args)

    assert [path.name for path in args.output_dir.iterdir()] == [
        "pack-component-conformance.json"
    ]


@pytest.mark.parametrize(
    ("target", "mutation", "message"),
    [
        (
            "pack_proof",
            lambda value: value.__setitem__("bundle_digest", "blake3:" + "0" * 64),
            "Component Host/Core proof",
        ),
        (
            "policy_report",
            lambda value: value["source_policy"].__setitem__(
                "source_checkout_access", True
            ),
            "released-artifact oracle/privacy",
        ),
        (
            "policy_report",
            lambda value: (
                value.__setitem__(
                    "schema", "worldstream/negotiate-golden-flow-acceptance/v1"
                ),
                value.__setitem__("release_evidence", False),
            ),
            "released-artifact oracle/privacy",
        ),
        (
            "a202_report",
            lambda value: value["profile"].__setitem__("state_machine_rules", "draft"),
            "pinned operated profile",
        ),
        (
            "sqlite_restart_report",
            lambda value: value["policy"].__setitem__("source_checkout_access", True),
            "restart/Replay",
        ),
    ],
)
def test_rejects_narrative_profile_and_checkout_substitution(
    tmp_path: Path, target: str, mutation, message: str
) -> None:
    _toml, _json, manifest = release_contract(tmp_path)
    args = diagnostic_inputs(tmp_path, manifest)
    path = getattr(args, target)
    value = json.loads(path.read_text(encoding="utf-8"))
    mutation(value)
    path.write_bytes(canonical(value))
    identity = EVIDENCE.STARTER.pack_identity(args.bundle.read_bytes(), "fixture")
    runtime = EVIDENCE.artifact_binding(args.linux_archive, "runtime")

    with pytest.raises(EVIDENCE.RuntimePackEvidenceError, match=message):
        if target == "pack_proof":
            EVIDENCE.validate_pack(args.bundle, path, manifest)
        elif target == "policy_report":
            EVIDENCE.validate_policy(path, identity)
        elif target == "a202_report":
            EVIDENCE.validate_a202(
                path, EVIDENCE.artifact_binding(args.a202_artifact, "A202 fixture")
            )
        else:
            EVIDENCE.validate_restart(path, "sqlite-bundled", identity, runtime)

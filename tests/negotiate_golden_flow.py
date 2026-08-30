from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


def load_module():
    spec = importlib.util.spec_from_file_location(
        "worldstream_negotiate_golden_flow",
        ROOT / "scripts/negotiate-golden-flow.py",
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


FLOW = load_module()


def test_static_contract_binds_the_complete_nine_action_release_flow() -> None:
    report = FLOW.validate_static_contract(FLOW.DEFAULT_BUNDLE)
    assert report["actions"] == FLOW.EXPECTED_ACTIONS
    assert report["restart_after_step"] == 4
    assert report["outcome"] == "agreement_committed"
    assert report["evidence_cross_index_entries"] >= 10
    assert report["bundle_digest"].endswith(FLOW.DEFAULT_BUNDLE.stem.rsplit("-", 1)[1])


def test_proof_requires_every_production_seam_fact() -> None:
    static = FLOW.validate_static_contract(FLOW.DEFAULT_BUNDLE)
    proof = {
        "proof_type": "complete",
        "status": "passed",
        "pack_id": "worldstream.negotiate",
        "bundle_digest": static["bundle_digest"],
        "revision_digest": static["revision_digest"],
        "transcript_digest": f"blake3:{'c' * 64}",
        "accepted_action": True,
        "declared_rejection": True,
        "retained_old_revision": True,
        "private_views": 4,
        "roles": 4,
    }
    FLOW.validate_proof(proof, static)
    with pytest.raises(FLOW.AcceptanceFailure, match="frozen contract"):
        FLOW.validate_proof({**proof, "retained_old_revision": False}, static)


def test_static_contract_rejects_a_substituted_bundle_path(tmp_path: Path) -> None:
    substituted = tmp_path / "different.wspack"
    substituted.write_bytes(b"not-the-release")
    with pytest.raises(FLOW.AcceptanceFailure, match="does not bind"):
        FLOW.validate_static_contract(substituted)


def test_static_contract_hashes_the_exact_release_bytes() -> None:
    static = FLOW.validate_static_contract(FLOW.DEFAULT_BUNDLE)
    assert FLOW.exact_blake3_digest(FLOW.DEFAULT_BUNDLE) == static["bundle_digest"]
    assert FLOW.is_blake3_digest(static["bundle_digest"])
    assert not FLOW.is_blake3_digest("blake3:short")

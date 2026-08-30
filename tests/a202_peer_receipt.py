from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]
KIT = ROOT / "interop/a202-peer-v1"


def load_module():
    spec = importlib.util.spec_from_file_location(
        "worldstream_a202_peer_receipt", KIT / "validate-receipt.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


PEER = load_module()


def canonical(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()


def invitation() -> tuple[dict[str, Any], bytes]:
    content = (KIT / "exchange.json").read_bytes()
    return json.loads(content), content


def valid_receipt() -> dict[str, Any]:
    value, content = invitation()
    objects = {item["id"]: item for item in value["objects"]}
    cases = []
    for case in value["cases"]:
        expected = case["expected"]
        source = objects[case["input"]["object_id"]]
        cases.append(
            {
                "case_id": case["case_id"],
                "fresh_signature_required": expected["fresh_signature_required"],
                "outcomes": expected["outcomes"],
                "received_exact_bytes_sha256": source["exact_bytes_sha256"],
                "received_media_type": case["input"]["media_type"],
                "retry_decision": expected["retry_decision"],
                "returned_exact_bytes_sha256": expected["returned_exact_bytes_sha256"],
            }
        )
    return {
        "attestation": {
            "independent_implementation": True,
            "non_contributor": True,
            "receipt_origin": "external_peer",
            "released_artifact_only": True,
            "worldstream_serializer_used": False,
            "worldstream_source_dependency": False,
        },
        "cases": cases,
        "completed_at": "2026-08-30T13:20:00Z",
        "invitation_sha256": "sha256:" + hashlib.sha256(content).hexdigest(),
        "participant": {
            "implementation_artifact_sha256": "sha256:" + "7" * 64,
            "implementation_id": "outside-peer",
            "implementation_language": "TypeScript",
            "implementation_version": "0.1.0",
            "participant_id": "peer-participant-001",
        },
        "schema": PEER.RECEIPT_SCHEMA,
        "started_at": "2026-08-30T13:00:00Z",
    }


def write_receipt(path: Path, value: dict[str, Any]) -> None:
    path.write_bytes(canonical(value))


def test_invitation_is_a_bounded_specification_not_evidence() -> None:
    value, content = PEER.validate_invitation(KIT / "exchange.json")

    assert value["evidence_class"] == "specification_invitation"
    assert value["release_evidence"] is False
    assert content == canonical(value)
    assert {case["expected"]["outcomes"]["signature"] for case in value["cases"]} == {
        "verified",
        "failed",
        "not_checkable",
    }


def test_valid_external_receipt_remains_only_a_candidate(tmp_path: Path) -> None:
    path = tmp_path / "receipt.json"
    write_receipt(path, valid_receipt())

    result = PEER.validate_receipt(KIT / "exchange.json", path)

    assert result["status"] == "valid_external_receipt_candidate"
    assert result["case_count"] == 8
    assert result["release_evidence"] is False
    assert result["independent_evidence_established"] is False


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        (
            lambda value: value["attestation"].__setitem__(
                "worldstream_serializer_used", True
            ),
            "independent release-artifact-only",
        ),
        (
            lambda value: value.__setitem__("invitation_sha256", "sha256:" + "0" * 64),
            "different invitation",
        ),
        (lambda value: value["cases"].pop(), "case inventory"),
        (
            lambda value: value["cases"][0]["outcomes"].__setitem__(
                "signature", "not_checkable"
            ),
            "tri-state outcomes",
        ),
        (
            lambda value: value["cases"][5].__setitem__(
                "returned_exact_bytes_sha256", "sha256:" + "9" * 64
            ),
            "returned_exact_bytes_sha256",
        ),
        (
            lambda value: value["participant"].__setitem__(
                "implementation_version", "Bearer secret"
            ),
            "secret-like",
        ),
    ],
)
def test_rejects_fabricated_or_drifted_receipt(
    tmp_path: Path, mutation, message: str
) -> None:
    value = valid_receipt()
    mutation(value)
    path = tmp_path / "receipt.json"
    write_receipt(path, value)

    with pytest.raises(PEER.PeerReceiptError, match=message):
        PEER.validate_receipt(KIT / "exchange.json", path)


def test_rejects_duplicate_keys_and_symlink_receipts(tmp_path: Path) -> None:
    duplicate = tmp_path / "duplicate.json"
    duplicate.write_text('{"schema":"a","schema":"b"}', encoding="utf-8")
    with pytest.raises(PEER.PeerReceiptError, match="duplicate JSON key"):
        PEER.validate_receipt(KIT / "exchange.json", duplicate)

    non_json_number = tmp_path / "nan.json"
    non_json_number.write_text('{"value":NaN}', encoding="utf-8")
    with pytest.raises(PEER.PeerReceiptError, match="non-JSON numeric"):
        PEER.strict_json(non_json_number.read_bytes(), "NaN fixture")

    actual = tmp_path / "actual.json"
    write_receipt(actual, valid_receipt())
    linked = tmp_path / "linked.json"
    linked.symlink_to(actual)
    with pytest.raises(PEER.PeerReceiptError, match="non-symlink"):
        PEER.validate_receipt(KIT / "exchange.json", linked)


def test_incomplete_template_cannot_validate_as_a_receipt() -> None:
    with pytest.raises(PEER.PeerReceiptError, match="field inventory"):
        PEER.validate_receipt(KIT / "exchange.json", KIT / "receipt.template.json")

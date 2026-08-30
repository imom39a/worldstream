#!/usr/bin/env python3
"""Validate the A202 peer invitation and an independently returned receipt.

This program never generates a peer receipt and never upgrades a local vector,
fixture, or schema-valid receipt into independent interoperability evidence.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
import stat
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

INVITATION_SCHEMA = "worldstream/a202-peer-exchange-invitation/v1"
RECEIPT_SCHEMA = "worldstream/a202-peer-exchange-receipt/v1"
VALIDATION_SCHEMA = "worldstream/a202-peer-exchange-receipt-validation/v1"
MAX_INVITATION_BYTES = 512 * 1024
MAX_RECEIPT_BYTES = 128 * 1024
MAX_TEXT_BYTES = 160
MAX_CASES = 32
SAFE_CODE = re.compile(r"[a-z][a-z0-9._-]{0,127}\Z")
SHA256 = re.compile(r"sha256:[0-9a-f]{64}\Z")
BLAKE3 = re.compile(r"blake3:[0-9a-f]{64}\Z")
BASE64URL = re.compile(r"[A-Za-z0-9_-]+\Z")
OUTCOMES = frozenset({"verified", "failed", "not_checkable"})
OUTCOME_FIELDS = {
    "a202_content_hash",
    "exact_byte_digest",
    "logical_head",
    "media_type",
    "signature",
    "signer_status",
}
CASE_IDS = (
    "action_initial_head",
    "offer_missing_key",
    "offer_tampered_content",
    "offer_valid",
    "offer_wrong_media_type",
    "retry_room_head_only",
    "retry_session_head_changed",
    "retry_transaction_head_changed",
)
PROFILE = {
    "bilateral_scope": "a202-scope/bilateral/0.1",
    "operated_scope": "a202-scope/operated/0.1",
    "repository_revision": "fa85aa8b49bfe7b3f7ded487c98500a600e92e41",
    "rules_version": "1.3",
    "spec_version": "a202-commercial/0.1",
    "transaction_profile": "a202-profile/calibration-service/0.1",
}


class PeerReceiptError(RuntimeError):
    """The invitation or returned peer receipt is unsafe or incomplete."""


def fail(message: str) -> None:
    raise PeerReceiptError(message)


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def object_value(value: object, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{label} must be an object")
    return value


def list_value(value: object, label: str) -> list[Any]:
    if not isinstance(value, list) or len(value) > MAX_CASES:
        fail(f"{label} must be a bounded list")
    return value


def exact_fields(value: dict[str, Any], fields: set[str], label: str) -> None:
    if set(value) != fields:
        fail(f"{label} has an unexpected field inventory")


def safe_code(value: object, label: str) -> str:
    if not isinstance(value, str) or SAFE_CODE.fullmatch(value) is None:
        fail(f"{label} must be a bounded safe code")
    return value


def bounded_text(value: object, label: str) -> str:
    if (
        not isinstance(value, str)
        or not value
        or len(value.encode("utf-8")) > MAX_TEXT_BYTES
        or any(ord(character) < 0x20 or ord(character) == 0x7F for character in value)
        or re.search(r"(?i)(bearer\s+|wsb1:|api[_-]?key|password|secret)", value)
    ):
        fail(f"{label} is invalid or contains secret-like text")
    return value


def utc_second(value: object, label: str) -> datetime:
    if not isinstance(value, str) or not value.endswith("Z"):
        fail(f"{label} must be canonical UTC-second text")
    try:
        parsed = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(
            tzinfo=timezone.utc
        )
    except ValueError as error:
        raise PeerReceiptError(f"{label} must be canonical UTC-second text") from error
    return parsed


def strict_json(content: bytes, label: str) -> dict[str, Any]:
    def no_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                fail(f"{label} contains duplicate JSON key {key!r}")
            result[key] = value
        return result

    try:
        value = json.loads(
            content,
            object_pairs_hook=no_duplicates,
            parse_constant=lambda constant: fail(
                f"{label} contains non-JSON numeric constant {constant!r}"
            ),
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise PeerReceiptError(f"{label} is not strict JSON") from error
    return object_value(value, label)


def regular_bytes(path: Path, label: str, maximum: int) -> bytes:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise PeerReceiptError(f"{label} is unavailable") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail(f"{label} must be a regular non-symlink file")
    if metadata.st_size <= 0 or metadata.st_size > maximum:
        fail(f"{label} is outside the bounded size limit")
    try:
        content = path.read_bytes()
        finished = path.lstat()
    except OSError as error:
        raise PeerReceiptError(f"{label} could not be read") from error
    before = (metadata.st_dev, metadata.st_ino, metadata.st_size, metadata.st_mtime_ns)
    after = (finished.st_dev, finished.st_ino, finished.st_size, finished.st_mtime_ns)
    if before != after or len(content) != metadata.st_size:
        fail(f"{label} changed while it was read")
    return content


def base64url(value: object, label: str) -> bytes:
    if not isinstance(value, str) or BASE64URL.fullmatch(value) is None or "=" in value:
        fail(f"{label} is not canonical unpadded base64url")
    try:
        decoded = base64.urlsafe_b64decode(value + "=" * (-len(value) % 4))
    except (ValueError, TypeError) as error:
        raise PeerReceiptError(f"{label} is not base64url") from error
    if base64.urlsafe_b64encode(decoded).rstrip(b"=").decode("ascii") != value:
        fail(f"{label} is not canonical unpadded base64url")
    return decoded


def validate_sha256(value: object, label: str) -> str:
    if not isinstance(value, str) or SHA256.fullmatch(value) is None:
        fail(f"{label} must be a lowercase SHA-256 reference")
    return value


def validate_basis(value: object, label: str) -> None:
    basis = object_value(value, label)
    exact_fields(basis, {"room", "session", "transaction"}, label)
    room = object_value(basis["room"], f"{label}.room")
    exact_fields(room, {"digest", "sequence"}, f"{label}.room")
    if type(room["sequence"]) is not int or room["sequence"] < 0:
        fail(f"{label}.room.sequence must be a nonnegative integer")
    if not isinstance(room["digest"], str) or BLAKE3.fullmatch(room["digest"]) is None:
        fail(f"{label}.room.digest must be a BLAKE3 reference")
    for name in ("session", "transaction"):
        head = object_value(basis[name], f"{label}.{name}")
        exact_fields(head, {"event_hash", "sequence"}, f"{label}.{name}")
        if type(head["sequence"]) is not int or head["sequence"] < 0:
            fail(f"{label}.{name}.sequence must be a nonnegative integer")
        validate_sha256(head["event_hash"], f"{label}.{name}.event_hash")


def validate_invitation(path: Path) -> tuple[dict[str, Any], bytes]:
    content = regular_bytes(path, "A202 peer invitation", MAX_INVITATION_BYTES)
    invitation = strict_json(content, "A202 peer invitation")
    exact_fields(
        invitation,
        {
            "cases",
            "evidence_class",
            "objects",
            "profile",
            "reference_material",
            "release_evidence",
            "schema",
            "session",
            "verification_key",
        },
        "A202 peer invitation",
    )
    if content != canonical_json(invitation):
        fail("A202 peer invitation is not canonical JSON")
    if (
        invitation["schema"] != INVITATION_SCHEMA
        or invitation["evidence_class"] != "specification_invitation"
        or invitation["release_evidence"] is not False
        or invitation["profile"] != PROFILE
    ):
        fail("A202 peer invitation makes an unsupported profile or evidence claim")

    session = object_value(invitation["session"], "invitation session")
    exact_fields(session, {"session_id", "transaction_id"}, "invitation session")
    safe_code(session["session_id"], "session_id")
    safe_code(session["transaction_id"], "transaction_id")
    key = object_value(invitation["verification_key"], "verification key")
    exact_fields(
        key,
        {
            "current_status",
            "key_id",
            "public_key_sec1_base64url",
            "status_at",
            "status_at_signing_time",
            "subject_id",
        },
        "verification key",
    )
    if key["current_status"] != "active" or key["status_at_signing_time"] != "active":
        fail("verification key is not active in the invitation")
    safe_code(key["key_id"], "verification key id")
    safe_code(key["subject_id"], "verification key subject")
    utc_second(key["status_at"], "verification key status_at")
    if (
        len(base64url(key["public_key_sec1_base64url"], "verification public key"))
        != 65
    ):
        fail("verification public key is not an uncompressed P-256 SEC1 point")

    objects: dict[str, dict[str, Any]] = {}
    object_rows = list_value(invitation["objects"], "invitation objects")
    for raw in object_rows:
        item = object_value(raw, "invitation object")
        exact_fields(
            item,
            {
                "a202_content_hash",
                "exact_bytes_base64url",
                "exact_bytes_sha256",
                "expected_signature",
                "id",
                "media_type",
                "object_type",
                "wire_digest",
            },
            "invitation object",
        )
        object_id = safe_code(item["id"], "invitation object id")
        if object_id in objects:
            fail("invitation contains duplicate object ids")
        exact = base64url(item["exact_bytes_base64url"], f"{object_id} exact bytes")
        expected_sha = validate_sha256(
            item["exact_bytes_sha256"], f"{object_id} SHA-256"
        )
        if "sha256:" + hashlib.sha256(exact).hexdigest() != expected_sha:
            fail(f"{object_id} exact bytes do not match their SHA-256")
        if (
            not isinstance(item["wire_digest"], str)
            or BLAKE3.fullmatch(item["wire_digest"]) is None
        ):
            fail(f"{object_id} has an invalid BLAKE3 wire digest")
        if item["wire_digest"] == "blake3:" + "0" * 64:
            fail(f"{object_id} retains an unresolved BLAKE3 wire digest")
        if item["media_type"] != "application/a202-commercial+json":
            fail(f"{object_id} has the wrong A202 media type")
        if (
            not isinstance(item["a202_content_hash"], str)
            or re.fullmatch(r"[0-9a-f]{64}", item["a202_content_hash"]) is None
        ):
            fail(f"{object_id} has an invalid A202 content hash")
        signature = object_value(item["expected_signature"], f"{object_id} signature")
        exact_fields(
            signature, {"key_id", "purpose", "subject_id"}, f"{object_id} signature"
        )
        safe_code(signature["key_id"], f"{object_id} signature key")
        safe_code(signature["purpose"], f"{object_id} signature purpose")
        safe_code(signature["subject_id"], f"{object_id} signature subject")
        objects[object_id] = item
    if list(objects) != sorted(objects) or set(objects) != {
        "action_valid",
        "offer_tampered",
        "offer_valid",
    }:
        fail("invitation object inventory is incomplete or non-canonical")

    references = list_value(invitation["reference_material"], "reference material")
    reference_ids: list[str] = []
    for raw in references:
        reference = object_value(raw, "reference material")
        exact_fields(
            reference,
            {
                "canonical_content_bytes_base64url",
                "object_id",
                "signature_message_base64url",
            },
            "reference material",
        )
        object_id = safe_code(reference["object_id"], "reference object id")
        if object_id not in objects:
            fail("reference material names an unknown object")
        base64url(
            reference["canonical_content_bytes_base64url"], f"{object_id} content bytes"
        )
        base64url(
            reference["signature_message_base64url"], f"{object_id} signature message"
        )
        reference_ids.append(object_id)
    if reference_ids != ["action_valid", "offer_valid"]:
        fail("reference material inventory is incomplete or non-canonical")

    cases: dict[str, dict[str, Any]] = {}
    for raw in list_value(invitation["cases"], "invitation cases"):
        case = object_value(raw, "invitation case")
        exact_fields(case, {"case_id", "expected", "input"}, "invitation case")
        case_id = safe_code(case["case_id"], "case_id")
        if case_id in cases:
            fail("invitation contains duplicate case ids")
        case_input = object_value(case["input"], f"{case_id} input")
        exact_fields(
            case_input,
            {
                "key_set",
                "media_type",
                "object_id",
                "observed_basis",
                "submission_basis",
            },
            f"{case_id} input",
        )
        if case_input["key_set"] not in {"active", "empty"}:
            fail(f"{case_id} has an invalid key set")
        if case_input["object_id"] not in objects:
            fail(f"{case_id} names an unknown object")
        if case_input["media_type"] not in {
            "application/a202-commercial+json",
            "application/json",
        }:
            fail(f"{case_id} has an unsupported media type")
        for basis_name in ("submission_basis", "observed_basis"):
            if case_input[basis_name] is not None:
                validate_basis(case_input[basis_name], f"{case_id}.{basis_name}")
        expected = object_value(case["expected"], f"{case_id} expected")
        exact_fields(
            expected,
            {
                "fresh_signature_required",
                "outcomes",
                "retry_decision",
                "returned_exact_bytes_sha256",
            },
            f"{case_id} expected",
        )
        outcomes = object_value(expected["outcomes"], f"{case_id} outcomes")
        if set(outcomes) != OUTCOME_FIELDS or any(
            value not in OUTCOMES for value in outcomes.values()
        ):
            fail(f"{case_id} has an invalid tri-state outcome inventory")
        if (
            expected["retry_decision"]
            not in {
                "not_applicable",
                "reuse_exact_bytes",
                "rebuild_and_resign",
            }
            or type(expected["fresh_signature_required"]) is not bool
        ):
            fail(f"{case_id} has an invalid retry expectation")
        returned = expected["returned_exact_bytes_sha256"]
        if returned is not None:
            validate_sha256(returned, f"{case_id} returned exact bytes")
        cases[case_id] = case
    if tuple(cases) != CASE_IDS:
        fail("invitation case inventory is incomplete or non-canonical")
    return invitation, content


def validate_receipt(invitation_path: Path, receipt_path: Path) -> dict[str, Any]:
    invitation, invitation_bytes = validate_invitation(invitation_path)
    receipt_bytes = regular_bytes(receipt_path, "peer receipt", MAX_RECEIPT_BYTES)
    receipt = strict_json(receipt_bytes, "peer receipt")
    exact_fields(
        receipt,
        {
            "attestation",
            "cases",
            "completed_at",
            "invitation_sha256",
            "participant",
            "schema",
            "started_at",
        },
        "peer receipt",
    )
    if receipt["schema"] != RECEIPT_SCHEMA:
        fail("peer receipt has the wrong schema")
    invitation_sha = "sha256:" + hashlib.sha256(invitation_bytes).hexdigest()
    if receipt["invitation_sha256"] != invitation_sha:
        fail("peer receipt names a different invitation")
    started = utc_second(receipt["started_at"], "peer receipt started_at")
    completed = utc_second(receipt["completed_at"], "peer receipt completed_at")
    elapsed = int((completed - started).total_seconds())
    if elapsed < 0 or elapsed > 7200:
        fail("peer receipt duration is negative or exceeds two hours")

    participant = object_value(receipt["participant"], "peer participant")
    exact_fields(
        participant,
        {
            "implementation_artifact_sha256",
            "implementation_id",
            "implementation_language",
            "implementation_version",
            "participant_id",
        },
        "peer participant",
    )
    participant_id = safe_code(participant["participant_id"], "participant_id")
    implementation_id = safe_code(participant["implementation_id"], "implementation_id")
    bounded_text(participant["implementation_language"], "implementation_language")
    bounded_text(participant["implementation_version"], "implementation_version")
    validate_sha256(
        participant["implementation_artifact_sha256"],
        "implementation artifact SHA-256",
    )

    attestation = object_value(receipt["attestation"], "peer attestation")
    required_attestation = {
        "independent_implementation": True,
        "non_contributor": True,
        "receipt_origin": "external_peer",
        "released_artifact_only": True,
        "worldstream_serializer_used": False,
        "worldstream_source_dependency": False,
    }
    if attestation != required_attestation:
        fail(
            "peer receipt does not attest an independent release-artifact-only implementation"
        )

    invitation_cases = {
        case["case_id"]: case
        for case in list_value(invitation["cases"], "invitation cases")
    }
    objects = {
        item["id"]: item
        for item in list_value(invitation["objects"], "invitation objects")
    }
    observed_ids: list[str] = []
    for raw in list_value(receipt["cases"], "peer receipt cases"):
        observed = object_value(raw, "peer receipt case")
        exact_fields(
            observed,
            {
                "case_id",
                "fresh_signature_required",
                "outcomes",
                "received_exact_bytes_sha256",
                "received_media_type",
                "retry_decision",
                "returned_exact_bytes_sha256",
            },
            "peer receipt case",
        )
        case_id = safe_code(observed["case_id"], "peer receipt case_id")
        if case_id in observed_ids or case_id not in invitation_cases:
            fail("peer receipt contains a duplicate or unknown case")
        expected_case = invitation_cases[case_id]
        expected = expected_case["expected"]
        source = objects[expected_case["input"]["object_id"]]
        if observed["received_media_type"] != expected_case["input"]["media_type"]:
            fail(f"{case_id} reports a substituted media type")
        if observed["received_exact_bytes_sha256"] != source["exact_bytes_sha256"]:
            fail(f"{case_id} reports substituted exact bytes")
        if observed["outcomes"] != expected["outcomes"]:
            fail(f"{case_id} tri-state outcomes differ from the invitation")
        for field in (
            "retry_decision",
            "returned_exact_bytes_sha256",
            "fresh_signature_required",
        ):
            if observed[field] != expected[field]:
                fail(f"{case_id} {field} differs from the invitation")
        observed_ids.append(case_id)
    if observed_ids != sorted(invitation_cases):
        fail("peer receipt case inventory is incomplete or non-canonical")

    return {
        "case_count": len(observed_ids),
        "elapsed_seconds": elapsed,
        "implementation_id": implementation_id,
        "independent_evidence_established": False,
        "invitation_sha256": invitation_sha,
        "participant_id": participant_id,
        "receipt_sha256": "sha256:" + hashlib.sha256(receipt_bytes).hexdigest(),
        "release_evidence": False,
        "schema": VALIDATION_SCHEMA,
        "status": "valid_external_receipt_candidate",
    }


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    subcommands = command.add_subparsers(dest="command", required=True)
    inspect = subcommands.add_parser("validate-invitation")
    inspect.add_argument("--invitation", required=True, type=Path)
    receipt = subcommands.add_parser("validate-receipt")
    receipt.add_argument("--invitation", required=True, type=Path)
    receipt.add_argument("--receipt", required=True, type=Path)
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "validate-invitation":
            invitation, content = validate_invitation(args.invitation)
            result = {
                "case_count": len(invitation["cases"]),
                "evidence_class": "specification_invitation",
                "invitation_sha256": "sha256:" + hashlib.sha256(content).hexdigest(),
                "release_evidence": False,
                "schema": "worldstream/a202-peer-exchange-invitation-validation/v1",
                "status": "valid_invitation",
            }
        else:
            result = validate_receipt(args.invitation, args.receipt)
    except PeerReceiptError as error:
        print(f"A202 peer receipt validation failed: {error}", file=sys.stderr)
        return 1
    os.write(sys.stdout.fileno(), canonical_json(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

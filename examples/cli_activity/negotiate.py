"""Submit the reviewed first proposal to a Negotiate 0.2 Room."""

from __future__ import annotations

import argparse
import asyncio
import json
from pathlib import Path
from typing import Any

from .credentials import CredentialError, load_membership, sdk_base_url

ACTION_TYPE = "submit_proposal_revision"
EXPECTED_PACK = {
    "id": "worldstream.negotiate",
    "version": "0.2.0",
    "digest": "blake3:651a04711a61bbdb263da5869a48d9829bc37b3be315042404587b00d52c127c",
}
ACTION_ID = "act_01_buyer_proposal"
VALID_UNTIL = 4_102_440_000


def first_buyer_proposal() -> dict[str, Any]:
    """Return the exact public Action payload for the reviewed setup example."""
    offer_content_hash = (
        "f0355964099f46e7abf4c9c04f1250f4e87b19d1e3b85e3ed0f0577433963798"
    )
    offer_wire_digest = (
        "blake3:1f6ff1b5f7ae670400d04c97ac6b6794d5b4d1f8a0e3fa17f7d10eb24ccdfc45"
    )
    offer_canonical_json = _canonical(
        {
            "content_hash": offer_content_hash,
            "id": "off_worldstream_buyer_01",
            "object_type": "offer",
            "payload": {
                "offeree": "org_delta",
                "offeror": "org_northstar",
                "session_id": "ses_northstar_delta_worldstream_01",
                "supersedes_offer_id": None,
                "terms": {
                    "core": {
                        "description": (
                            "Calibration and digital certificates for 20 pressure transmitters"
                        ),
                        "quantity": "20",
                        "total": {"amount": "3350.00", "currency": "EUR"},
                        "unit_code": "H87",
                        "unit_name": "piece",
                    },
                    "profile": "a202-profile/calibration-service/0.1",
                    "profile_terms": {
                        "acceptance": {
                            "certificate_required": True,
                            "machine_readable_result_required": True,
                            "qualification_standard": "ISO/IEC 17025:2017",
                        },
                        "completion": {
                            "business_calendar": "NL",
                            "business_days_after_collection": 15,
                        },
                        "payment": {
                            "balance_trigger": "buyer_acceptance",
                            "prepayment_percent": "20",
                        },
                        "rework": {"included_attempts": 1},
                    },
                },
                "valid_until": VALID_UNTIL,
            },
            "spec_version": "a202-commercial/0.1",
            "transaction_id": "txn_calibration_worldstream_01",
            "version": 1,
        }
    )
    event_content_hash = (
        "599c82ead58812aab8811fe76c00d37668cdbff4ad6f80bef8d36469854b54cd"
    )
    event_wire_digest = (
        "blake3:8c4336d88c1783061dc9ed1a02f27453deb33f38afd007c0dea683899f58a916"
    )
    event_canonical_json = _canonical(
        {
            "content_hash": event_content_hash,
            "id": "evt_session_offer_02",
            "object_type": "transaction_event",
            "payload": {
                "event_type": "offer.submitted",
                "previous_event_hash": "sha256:" + "2" * 64,
                "sequence": 2,
                "stream": {
                    "id": "ses_northstar_delta_worldstream_01",
                    "kind": "session",
                },
            },
            "spec_version": "a202-commercial/0.1",
            "transaction_id": "txn_calibration_worldstream_01",
            "version": 1,
        }
    )
    return {
        "action": ACTION_TYPE,
        "action_id": ACTION_ID,
        "actor": "buyer_agent",
        "proposal": {
            "author": "buyer_agent",
            "offer": {
                "object_id": "off_worldstream_buyer_01",
                "object_type": "offer",
                "declared_content_hash": offer_content_hash,
                "canonical_json": offer_canonical_json,
                "wire_digest": offer_wire_digest,
                "signature": {
                    "signer_id": "agent:northstar:buyer",
                    "signer_role": "buyer_agent",
                    "purpose": "offer_submission",
                    "signed_wire_digest": offer_wire_digest,
                    "proof": (
                        "blake3:d768c57774630064caa66d7132e9ba854cf320f3b5243f54594d75eb787690cb"
                    ),
                },
            },
            "session_event": {
                "object_id": "evt_session_offer_02",
                "object_type": "transaction_event",
                "declared_content_hash": event_content_hash,
                "canonical_json": event_canonical_json,
                "wire_digest": event_wire_digest,
                "signature": {
                    "signer_id": "agent:worldstream:venue_signer",
                    "signer_role": "venue_signer",
                    "purpose": "event_append",
                    "signed_wire_digest": event_wire_digest,
                    "proof": (
                        "blake3:88d6f95847beb0e40694efec99235e18b923689589a20408a47c4a34ead7269d"
                    ),
                },
            },
            "valid_until": VALID_UNTIL,
            "supersedes_offer_id": None,
        },
    }


def _canonical(value: dict[str, Any]) -> str:
    return json.dumps(
        value,
        allow_nan=False,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    )


async def run(membership_file: Path, timeout_seconds: float) -> dict[str, Any]:
    """Use one exported Membership credential to submit one public Room Action."""
    from worldstream_sdk import Client, LostActionReply

    membership = load_membership(membership_file)
    if membership["pack"] != EXPECTED_PACK or membership["role"] != "buyer_agent":
        raise CredentialError("credential_negotiate_buyer_required")

    client = Client(sdk_base_url(membership), membership["bearer"])
    room = await client.open_room(membership["room_id"], membership["member_id"])
    try:
        if room.attached is None or room.attached.get("pack") != EXPECTED_PACK:
            raise CredentialError("attached_negotiate_revision_mismatch")
        await room.sync()
        projection = await client.projection(membership["room_id"])
        if not _is_offered(projection, ACTION_TYPE):
            raise CredentialError("negotiate_first_proposal_not_offered")
        room_head = projection.get("room_head")
        expected = room_head.get("room_seq") if isinstance(room_head, dict) else None
        if isinstance(expected, bool) or not isinstance(expected, int) or expected < 0:
            raise CredentialError("negotiate_room_head_invalid")
        payload = first_buyer_proposal()
        try:
            receipt = await room.act(
                ACTION_TYPE,
                payload,
                expected_room_seq=expected,
                timeout=timeout_seconds,
            )
        except LostActionReply as error:
            receipt = await error.retry()
        transition_id = receipt.get("transition_id")
        result_head = receipt.get("room_head")
        room_seq = (
            result_head.get("room_seq") if isinstance(result_head, dict) else None
        )
        duplicate = receipt.get("duplicate")
        if (
            not isinstance(duplicate, bool)
            or not isinstance(transition_id, str)
            or not transition_id
            or isinstance(room_seq, bool)
            or not isinstance(room_seq, int)
        ):
            return {
                "status": "rejected",
                "action": ACTION_TYPE,
                "code": receipt.get("code", "action_not_accepted"),
                "room_id": membership["room_id"],
            }
        return {
            "status": "accepted",
            "action": ACTION_TYPE,
            "room_id": membership["room_id"],
            "room_seq": room_seq,
            "transition_id": transition_id,
        }
    finally:
        await room.close()


def _is_offered(projection: dict[str, Any], action_type: str) -> bool:
    body = projection.get("projection")
    offers = body.get("action_offers") if isinstance(body, dict) else None
    return isinstance(offers, list) and any(
        isinstance(offer, dict) and offer.get("action_type") == action_type
        for offer in offers
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--membership-file", required=True, type=Path)
    parser.add_argument("--timeout-seconds", type=float, default=15.0)
    args = parser.parse_args()
    if not 1 <= args.timeout_seconds <= 60:
        parser.error("timeout-seconds must be between 1 and 60")
    try:
        from worldstream_sdk import ProtocolError

        result = asyncio.run(run(args.membership_file, args.timeout_seconds))
    except (CredentialError, OSError, ProtocolError, TimeoutError, ValueError) as error:
        result = {"status": "error", "code": type(error).__name__}
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0 if result["status"] == "accepted" else 3


if __name__ == "__main__":
    raise SystemExit(main())

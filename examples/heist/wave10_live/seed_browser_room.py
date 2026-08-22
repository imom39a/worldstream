#!/usr/bin/env python3
"""Seed one disposable Heist membership for the real browser console smoke.

The helper uses the public operator HTTP capability route and the Python SDK
projection route. It writes the browser bootstrap only to an owner-readable
0600 file and never prints a bearer or private projection context.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import pathlib
import re
import sys
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "sdk/python/src"))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from run_absent_broker_live import (
    MEMBER_IDEMPOTENCY_KEYS,
    PACK_DIGEST,
    PACK_ID,
    PACK_VERSION,
    PRINCIPAL_IDS,
    BlockedStory,
    _json_request,
    issue_runner_capability,
)
from worldstream_sdk import Client, ProtocolError

ULID = re.compile(r"^[0-9A-HJKMNP-TV-Z]{26}$")
BEARER = re.compile(r"^wsb1:[0-9a-f]{64}$")
BROWSER_RUNNER_ID = "01JZ7K0B5R9A7C8D9E0F1G2H33"
BROWSER_RUNNER_KEYS = (
    "01JZ7K0B5R9A7C8D9E0F1G2H44",
    "01JZ7K0B5R9A7C8D9E0F1G2H55",
    "01JZ7K0B5R9A7C8D9E0F1G2H66",
)


def _read_bearer(path: pathlib.Path) -> str:
    bearer = path.read_text(encoding="utf-8").strip()
    if not BEARER.fullmatch(bearer):
        raise RuntimeError("operator_bearer_file_invalid")
    return bearer


def _write_private_json(path: pathlib.Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    descriptor = os.open(path, flags, 0o600)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            descriptor = -1
            json.dump(value, output, ensure_ascii=True, separators=(",", ":"))
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
    finally:
        if descriptor >= 0:
            os.close(descriptor)
    os.chmod(path, 0o600)


async def seed(args: argparse.Namespace) -> None:
    operator_bearer = _read_bearer(pathlib.Path(args.operator_bearer_file))
    operator = Client(args.base_url, operator_bearer)
    print("seed_stage=create_room", file=sys.stderr, flush=True)
    created = await operator.create_room(
        {
            "pack": {"id": PACK_ID, "version": PACK_VERSION, "digest": PACK_DIGEST},
            "configuration": {
                "pack_id": PACK_ID,
                "pack_schema": 1,
                "roles": ["navigator", "insider", "broker"],
                "briefing_duration_seconds": 30,
                "negotiation_duration_seconds": 90,
                "commitment_duration_seconds": 30,
                "commitment_reminder_seconds_before_deadline": 10,
                "result_duration_seconds": 20,
                "maximum_plans": 12,
                "maximum_open_offers_per_role": 4,
            },
            "members": [
                {
                    "principal_id": PRINCIPAL_IDS[index],
                    "principal_kind": "agent",
                    "role": role,
                    "access_mode": "participant",
                }
                for index, role in enumerate(("navigator", "insider", "broker"))
            ],
            "idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FGR",  # gitleaks:allow
        }
    )
    room_id = created.get("room_id")
    member_ids = created.get("member_ids")
    if not isinstance(room_id, str) or not ULID.fullmatch(room_id):
        raise RuntimeError("seeded_room_id_invalid")
    if (
        not isinstance(member_ids, list)
        or not member_ids
        or not all(
            isinstance(item, str) and ULID.fullmatch(item) for item in member_ids
        )
    ):
        raise RuntimeError("seeded_member_ids_invalid")
    member_id = member_ids[0]
    print("seed_stage=runner_capability", file=sys.stderr, flush=True)
    runner_capability = issue_runner_capability(
        args.base_url,
        operator_bearer,
        room_id,
        member_ids[2],
        PRINCIPAL_IDS[2],
        runner_id=BROWSER_RUNNER_ID,
        change_keys=BROWSER_RUNNER_KEYS,
    )
    runner_bearer = runner_capability.get("bearer")
    if not isinstance(runner_bearer, str) or not BEARER.fullmatch(runner_bearer):
        raise RuntimeError("seeded_runner_bearer_invalid")
    print("seed_stage=member_capabilities", file=sys.stderr, flush=True)
    member_bearers: list[str] = []
    for index, seeded_member_id in enumerate(member_ids):
        capability = _json_request(
            args.base_url,
            operator_bearer,
            "/v1/operator/member-capabilities",
            {
                "room_id": room_id,
                "member_id": seeded_member_id,
                "principal_id": PRINCIPAL_IDS[index],
                "scopes": [
                    "room:attach",
                    "room:observe_member",
                    "room:act",
                    "room:replay",
                ],
                "idempotency_key": MEMBER_IDEMPOTENCY_KEYS[index],
                "expires_at": None,
            },
        )
        bearer = capability.get("bearer")
        if not isinstance(bearer, str) or not BEARER.fullmatch(bearer):
            raise RuntimeError("seeded_member_bearer_invalid")
        member_bearers.append(bearer)
    bearer = member_bearers[0]
    member = Client(args.base_url, bearer)
    try:
        projection = await member.projection(room_id)
    except (ProtocolError, OSError, TimeoutError) as error:
        raise RuntimeError("seeded_projection_unavailable") from error
    projection_body = projection.get("projection")
    activity = (
        projection_body.get("activity") if isinstance(projection_body, dict) else {}
    )
    offers = activity.get("action_offers") if isinstance(activity, dict) else []
    offer_count = len(offers) if isinstance(offers, list) else 0
    room_head = projection.get("room_head")
    room_seq = room_head.get("room_seq") if isinstance(room_head, dict) else None
    if not isinstance(room_seq, int):
        raise TypeError("seeded_room_head_invalid")
    _write_private_json(
        pathlib.Path(args.output),
        {
            "version": "worldstream.console.live.v1",
            "endpoint": args.base_url.replace("http://", "ws://", 1) + "/v1/stream",
            "clientName": "worldstream-console-browser-smoke",
            "clientVersion": "0.1.0",
            "roomId": room_id,
            "memberId": member_id,
            "bearer": bearer,
            "expectedActionType": "inspect_clue",
            "expectedRoomSeq": room_seq,
            "actionOfferCount": offer_count,
        },
    )
    if args.driver_output:
        _write_private_json(
            pathlib.Path(args.driver_output),
            {
                "version": "worldstream.heist.browser-driver.v1",
                "endpoint": args.base_url,
                "room_id": room_id,
                "member_ids": member_ids,
                "member_bearers": member_bearers,
                "runner_id": BROWSER_RUNNER_ID,
                "runner_bearer": runner_bearer,
            },
        )
    print(f"seeded room={room_id} member={member_id} action_offers={offer_count}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--operator-bearer-file", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--driver-output")
    args = parser.parse_args()
    try:
        asyncio.run(seed(args))
    except BlockedStory as error:
        safe = {
            key: value
            for key, value in error.details.items()
            if key in {"error_code", "http_status", "retryable"}
        }
        print(
            f"blocked:{error.reason_code}:{json.dumps(safe, sort_keys=True)}",
            file=sys.stderr,
        )
        return 2
    except ProtocolError as error:
        print(f"blocked:{error.code}", file=sys.stderr)
        return 2
    except (OSError, RuntimeError) as error:
        print(f"blocked: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

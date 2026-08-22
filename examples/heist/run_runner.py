#!/usr/bin/env python3
"""Exercise the Runner Activation wire path with explicit live prerequisites.

The default ``--self-test`` is deterministic and fixture-only. Live polling is
deliberately opt-in and exits non-zero when a base URL, bearer, or target
membership is missing. This harness does not mint capabilities or fabricate
Activation offers.
"""

from __future__ import annotations

import argparse
import asyncio
import json
from typing import Any

from worldstream_sdk import Client, ProtocolError


def self_test() -> dict[str, Any]:
    """Return a stable offline plan without claiming a live server result."""

    return {
        "status": "ok",
        "mode": "runner",
        "fixture_only": True,
        "live_evidence": False,
        "stream_path": "/v1/runner/stream",
        "steps": [
            "runner.hello",
            "activation.offer",
            "activation.claim",
            "activation.renew",
            "activation.release",
            "activation.complete",
        ],
    }


async def run_live(args: argparse.Namespace) -> dict[str, Any]:
    if not args.base_url or not args.bearer or not args.runner_id:
        return {
            "status": "blocked",
            "fixture_only": False,
            "live_evidence": False,
            "reason": "base_url_bearer_and_runner_id_are_required",
        }
    if not args.room_id or not args.member_id:
        return {
            "status": "blocked",
            "fixture_only": False,
            "live_evidence": False,
            "reason": "room_id_and_member_id_are_required_for_polling",
        }
    try:
        client = Client(args.base_url, args.bearer)
        runner = await client.open_runner(
            args.runner_id,
            args.maximum_concurrent_activations,
            args.supported_pack_id,
        )
        try:
            offers = await runner.poll_offers(args.room_id, args.member_id)
            result: dict[str, Any] = {
                "status": "ok",
                "fixture_only": False,
                "live_evidence": True,
                "runner_id": args.runner_id,
                "offer_count": len(offers["offers"]),
                "offers": offers["offers"],
            }
            if args.claim_first and offers["offers"]:
                offer = offers["offers"][0]
                claimed = await runner.claim(
                    offer["activation_id"], args.lease_ms, args.claim_id
                )
                result["claim"] = {
                    "code": claimed["code"],
                    "state": claimed["state"],
                    "lease_generation": claimed["lease_generation"],
                    "context_present": claimed["context"] is not None,
                }
                if (
                    args.complete_disposition
                    and claimed["lease_generation"] is not None
                ):
                    completed = await runner.complete(
                        offer["activation_id"],
                        claimed["claim_id"] or args.claim_id or "",
                        claimed["lease_generation"],
                        args.complete_disposition,
                    )
                    result["complete"] = {
                        "code": completed["code"],
                        "state": completed["state"],
                    }
            return result
        finally:
            await runner.close()
    except (ProtocolError, ValueError, OSError, TimeoutError) as error:
        # ProtocolError already redacts bearer-shaped values. Emit only typed
        # status fields so a diagnostic cannot accidentally become a secret log.
        if isinstance(error, ProtocolError):
            return {
                "status": "error",
                "fixture_only": False,
                "live_evidence": False,
                "error_code": error.code,
                "retryable": error.retryable,
            }
        return {
            "status": "error",
            "fixture_only": False,
            "live_evidence": False,
            "error_code": type(error).__name__,
        }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--self-test", action="store_true", help="print the offline wire plan"
    )
    parser.add_argument("--base-url", help="live WorldStream HTTP(S) base URL")
    parser.add_argument("--bearer", help="live Runner capability; never printed")
    parser.add_argument("--runner-id", help="authorized Runner identity")
    parser.add_argument("--room-id", help="authorized Agent room")
    parser.add_argument("--member-id", help="authorized Agent Membership")
    parser.add_argument(
        "--supported-pack-id", action="append", default=["worldstream.agent-heist"]
    )
    parser.add_argument("--maximum-concurrent-activations", type=int, default=1)
    parser.add_argument("--claim-first", action="store_true")
    parser.add_argument("--claim-id")
    parser.add_argument("--lease-ms", type=int, default=5000)
    parser.add_argument("--complete-disposition")
    args = parser.parse_args()

    if args.self_test:
        print(json.dumps(self_test(), sort_keys=True, separators=(",", ":")))
        return 0
    result = asyncio.run(run_live(args))
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0 if result["status"] == "ok" else 2


if __name__ == "__main__":
    raise SystemExit(main())

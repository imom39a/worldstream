"""Fail-closed UI/live Heist acceptance probe.

This probe exercises only the public Runner control path. A zero-offer poll is
reported as a missing transition-producing prerequisite; it never fabricates an
Activation, advances a Room, or claims that a live Heist completed.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import urllib.error
import urllib.request
from typing import Any

from worldstream_sdk import Client, ProtocolError

TRANSITION_PREREQUISITE = (
    "a public transition-producing Heist operation or a pre-seeded eligible "
    "Invocation that creates an Activation"
)


def classify_offer_probe(*, readyz: int, offers: object) -> dict[str, Any]:
    if readyz != 200:
        return {
            "status": "blocked",
            "reason_code": "daemon_not_ready",
            "readyz": readyz,
            "live_evidence": False,
        }
    if not isinstance(offers, list):
        return {
            "status": "blocked",
            "reason_code": "invalid_offer_response",
            "readyz": readyz,
            "live_evidence": False,
        }
    if not offers:
        return {
            "status": "blocked",
            "reason_code": "activation_transition_producer_missing",
            "prerequisite": TRANSITION_PREREQUISITE,
            "evidence": "authorized Runner poll returned zero Activation offers",
            "readyz": readyz,
            "offer_count": 0,
            "live_evidence": True,
            "fabricated_offer": False,
        }
    return {
        "status": "activation_available",
        "readyz": readyz,
        "offer_count": len(offers),
        "live_evidence": True,
        "fabricated_offer": False,
    }


async def run_live(args: argparse.Namespace) -> dict[str, Any]:
    missing = [
        name
        for name, value in (
            ("base_url", args.base_url),
            ("bearer", args.bearer),
            ("runner_id", args.runner_id),
            ("room_id", args.room_id),
            ("member_id", args.member_id),
        )
        if not value
    ]
    if missing:
        return {
            "status": "blocked",
            "reason_code": "live_inputs_missing",
            "missing": missing,
            "live_evidence": False,
        }
    try:
        readyz = readiness_status(args.base_url)
        if readyz != 200:
            return classify_offer_probe(readyz=readyz, offers=[])
        client = Client(args.base_url, args.bearer)
        runner = await client.open_runner(
            args.runner_id, args.maximum_concurrent_activations, args.supported_pack_id
        )
        try:
            result: dict[str, Any] | None = None
            for attempt in range(args.poll_count):
                offers = await runner.poll_offers(args.room_id, args.member_id)
                result = classify_offer_probe(
                    readyz=readyz, offers=offers.get("offers")
                )
                result["poll_attempt"] = attempt + 1
                if result["status"] == "activation_available":
                    break
                if attempt + 1 < args.poll_count:
                    await asyncio.sleep(args.poll_interval)
            assert result is not None
            result.update(
                {
                    "runner_id": args.runner_id,
                    "room_id": args.room_id,
                    "member_id": args.member_id,
                    "stream_path": "/v1/runner/stream",
                    "secrets": "not_emitted",
                }
            )
            if result["status"] == "activation_available":
                result["boundary"] = (
                    "An offer exists; claim/participant-action/recovery orchestration still requires corresponding real protocol receipts."
                )
            return result
        finally:
            await runner.close()
    except (ProtocolError, ValueError, OSError, TimeoutError) as error:
        if isinstance(error, ProtocolError):
            return {
                "status": "error",
                "reason_code": error.code,
                "retryable": error.retryable,
                "live_evidence": False,
                "secrets": "not_emitted",
            }
        return {
            "status": "error",
            "reason_code": type(error).__name__,
            "live_evidence": False,
            "secrets": "not_emitted",
        }


def readiness_status(base_url: str) -> int:
    try:
        with urllib.request.urlopen(
            f"{base_url.rstrip('/')}/readyz", timeout=2
        ) as response:
            return response.status
    except urllib.error.HTTPError as error:
        return error.code
    except OSError:
        return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url")
    parser.add_argument("--bearer", help="live Runner capability; never printed")
    parser.add_argument("--runner-id")
    parser.add_argument("--room-id")
    parser.add_argument("--member-id")
    parser.add_argument(
        "--supported-pack-id", action="append", default=["worldstream.agent-heist"]
    )
    parser.add_argument("--maximum-concurrent-activations", type=int, default=1)
    parser.add_argument("--poll-count", type=int, default=1)
    parser.add_argument("--poll-interval", type=float, default=0.25)
    args = parser.parse_args()
    if (
        args.poll_count < 1
        or args.poll_count > 120
        or args.poll_interval < 0
        or args.poll_interval > 60
    ):
        parser.error("poll-count must be 1..120 and poll-interval must be 0..60")
    result = asyncio.run(run_live(args))
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0 if result["status"] == "activation_available" else 2


if __name__ == "__main__":
    raise SystemExit(main())

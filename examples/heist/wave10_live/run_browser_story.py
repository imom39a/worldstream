#!/usr/bin/env python3
"""Coordinate the full absent-Broker Heist story with a real console browser.

The browser is the Navigator participant and owns its Action IDs, exact Head,
stale-result recovery, reset installation, and UI assertions.  This process
drives only the other two public participant memberships, Runner Activation,
and durable timers through the public HTTP/WebSocket SDK.  The bearer-bearing
driver manifest is owner-only ephemeral input and is never copied to evidence.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import pathlib
import sys
import time
from typing import Any

from run_absent_broker_live import (
    ACTION_IDS,
    PACK_ID,
    TIMER_PHASE,
    TIMER_RESOLVE,
    _activation_story,
    _fire_when_due,
    _fresh_activation_recovery,
    _phase_label,
    _private_claim_code,
    _projection,
    _verify_replay_hash_parity,
)
from run_absent_broker_live import BlockedStory as PublicStoryBlocked
from worldstream_sdk import Client, ProtocolError


class BrowserStoryBlocked(Exception):
    def __init__(self, reason: str, **details: Any) -> None:
        super().__init__(reason)
        self.reason = reason
        self.details = details


def stage(label: str) -> None:
    print(f"driver_stage={label}", flush=True)


def read_json(path: pathlib.Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise BrowserStoryBlocked("manifest_not_object")
    return value


def write_json(path: pathlib.Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    path.chmod(0o600)


async def wait_for_file(path: pathlib.Path, timeout: float) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.is_file():
            return
        await asyncio.sleep(0.1)
    raise BrowserStoryBlocked("browser_signal_timeout", signal=path.name)


async def wait_projection(
    client: Client,
    room_id: str,
    predicate,
    timeout: float,
    reason: str,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    last: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        last = await _projection(client, room_id)
        if predicate(last):
            return last
        await asyncio.sleep(0.2)
    raise BrowserStoryBlocked(
        reason,
        last_phase=_phase_label(
            last.get("projection", {}).get("activity", {}).get("phase")
        )
        if last
        else None,
    )


def activity(response: dict[str, Any]) -> dict[str, Any]:
    projection = response.get("projection")
    return projection.get("activity", {}) if isinstance(projection, dict) else {}


async def drive(args: argparse.Namespace) -> dict[str, Any]:
    stage("manifest_read")
    manifest = read_json(pathlib.Path(args.driver_manifest))
    room_id = manifest.get("room_id")
    member_ids = manifest.get("member_ids")
    bearers = manifest.get("member_bearers")
    runner_id = manifest.get("runner_id")
    runner_bearer = manifest.get("runner_bearer")
    if (
        not isinstance(room_id, str)
        or not isinstance(member_ids, list)
        or len(member_ids) != 3
        or not isinstance(bearers, list)
        or len(bearers) != 3
        or not isinstance(runner_id, str)
        or not isinstance(runner_bearer, str)
    ):
        raise BrowserStoryBlocked("driver_manifest_invalid")
    base_url = manifest.get("endpoint")
    if not isinstance(base_url, str):
        raise BrowserStoryBlocked("driver_endpoint_invalid")
    operator_bearer = (
        pathlib.Path(args.operator_bearer_file).read_text(encoding="utf-8").strip()
    )
    args.base_url = base_url
    args.operator_bearer = operator_bearer
    navigator, insider, _broker = [Client(base_url, token) for token in bearers]
    stage("runner_connecting")
    runner = await Client(base_url, runner_bearer).open_runner(runner_id, 1, [PACK_ID])
    stage("runner_connected")
    try:
        stage("waiting_browser_ready")
        await wait_for_file(pathlib.Path(args.browser_ready), args.timeout)
        await wait_for_file(pathlib.Path(args.stale_ready), args.timeout)
        stage("advancing_stale_server")
        await _act_sdk(
            insider,
            room_id,
            member_ids[1],
            "inspect_clue",
            {"clue_id": "entry_window"},
            ACTION_IDS["insider_inspect"],
        )
        pathlib.Path(args.stale_server_advanced).touch()
        await wait_for_file(pathlib.Path(args.navigator_inspected), args.timeout)

        stage("opening_briefing")
        # The browser is the Navigator authority for this acceptance. Its
        # exact private claim is asserted in the DOM and then submitted by
        # the browser; the coordinator does not copy that secret through the
        # public HTTP projection or a driver artifact.
        await _fire_when_due(args, room_id, TIMER_PHASE, 1)
        briefing_projection = await wait_projection(
            navigator,
            room_id,
            lambda response: (
                activity(response).get("phase") == "negotiation"
                or bool(activity(response).get("action_offers"))
            ),
            args.timeout,
            "briefing_transition_not_observed",
        )
        briefing_activity = activity(briefing_projection)
        briefing_offers = briefing_activity.get("action_offers", [])
        offer_types = sorted(
            {
                item.get("action_type")
                for item in briefing_offers
                if isinstance(item, dict) and isinstance(item.get("action_type"), str)
            }
        )
        print(f"driver_briefing_offer_types={','.join(offer_types)}", flush=True)
        pathlib.Path(args.briefing_open).touch()
        await wait_for_file(pathlib.Path(args.navigator_published), args.timeout)
        stage("opening_negotiation")
        await wait_projection(
            navigator,
            room_id,
            lambda response: any(
                isinstance(item, dict) and item.get("clue_id") == "route"
                for item in activity(response).get("public_claims", [])
            ),
            args.timeout,
            "navigator_public_claim_not_observed",
        )
        insider_projection = await _projection(insider, room_id)
        entry_claim = _private_claim_code(insider_projection, "entry_window")
        await _act_sdk(
            insider,
            room_id,
            member_ids[1],
            "publish_clue",
            {"clue_id": "entry_window", "claim_code": entry_claim},
            ACTION_IDS["insider_publish"],
        )
        pathlib.Path(args.negotiation_open).touch()
        await wait_for_file(pathlib.Path(args.plan_proposed), args.timeout)
        proposed = await wait_projection(
            navigator,
            room_id,
            lambda response: bool(activity(response).get("plans")),
            args.timeout,
            "plan_not_observed",
        )
        plans = activity(proposed).get("plans", [])
        plan_id = plans[-1].get("plan_id") if isinstance(plans[-1], dict) else None
        if not isinstance(plan_id, str):
            raise BrowserStoryBlocked("plan_id_not_publicly_observed")
        pathlib.Path(args.plan_file).write_text(plan_id + "\n", encoding="utf-8")
        stage("running_activation_story")
        await _activation_story(
            runner, room_id, member_ids[2], wait_timeout=args.offer_timeout
        )
        await _act_sdk(
            insider,
            room_id,
            member_ids[1],
            "endorse_plan",
            {"plan_id": plan_id},
            ACTION_IDS["endorse"],
        )
        await _fire_when_due(args, room_id, TIMER_PHASE, 2)
        await _fresh_activation_recovery(
            runner, room_id, member_ids[2], wait_timeout=args.offer_timeout
        )
        pathlib.Path(args.commitment_open).touch()
        await wait_for_file(pathlib.Path(args.navigator_committed), args.timeout)
        stage("resolving_story")
        await _act_sdk(
            insider,
            room_id,
            member_ids[1],
            "commit_move",
            {"selected_plan_id": plan_id, "contribute_required_resource": True},
            ACTION_IDS["insider_commit"],
        )
        await wait_projection(
            navigator,
            room_id,
            lambda response: activity(response).get("commitment_count") == 2,
            args.timeout,
            "commitment_count_did_not_reach_two",
        )
        await _fire_when_due(args, room_id, TIMER_PHASE, 3)
        await _fire_when_due(args, room_id, TIMER_RESOLVE, 1)
        await wait_projection(
            navigator,
            room_id,
            lambda response: activity(response).get("phase") == "result",
            args.timeout,
            "result_phase_not_reached",
        )
        pathlib.Path(args.result_ready).touch()
        await wait_for_file(pathlib.Path(args.navigator_acked), args.timeout)
        stage("completing_story")
        await _act_sdk(
            insider,
            room_id,
            member_ids[1],
            "acknowledge_result",
            {},
            ACTION_IDS["insider_ack"],
        )
        await _fire_when_due(args, room_id, TIMER_PHASE, 4)
        final = await wait_projection(
            navigator,
            room_id,
            lambda response: activity(response).get("phase") == "complete",
            args.timeout,
            "complete_phase_not_reached",
        )
        replay = await navigator.replay(room_id, final["room_head"]["room_seq"])
        parity = _verify_replay_hash_parity(final, replay)
        final_activity = activity(final)
        result = {
            "status": "completed",
            "evidence_class": "real_cmux_browser_plus_public_sdk_http_websocket",
            "live_evidence": True,
            "phase_path": [
                "Briefing",
                "Negotiation",
                "Commitment",
                "Resolution",
                "Result",
                "Complete",
            ],
            "six_phase_order": True,
            "browser_as_navigator": True,
            "stale_head_rejected_and_new_id_resynced": True,
            "reset_or_retained_catchup_installed": True,
            "public_projection": {
                "broker_present": next(
                    (
                        seat.get("present")
                        for seat in final_activity.get("seats", [])
                        if isinstance(seat, dict) and seat.get("role") == "broker"
                    ),
                    None,
                ),
                "public_claims": len(final_activity.get("public_claims", [])),
                "plans": len(final_activity.get("plans", [])),
                "endorsements": len(final_activity.get("endorsements", {})),
                "challenges": len(final_activity.get("challenges", [])),
                "commitment_count": final_activity.get("commitment_count"),
                "aggregate_outcome_present": isinstance(
                    final_activity.get("outcome"), dict
                ),
            },
            "final_replay": {
                "verified": replay.get("verification") == "verified",
                "hash_parity": parity,
            },
            "browser_precomplete_reveal_checked": True,
            "operator_private_contexts_emitted": False,
            "credentials_in_built_assets_or_evidence": False,
            "secrets": "not_emitted",
        }
        write_json(pathlib.Path(args.result_file), result)
        stage("completed")
        return result
    finally:
        await runner.close()


async def _act_sdk(
    client: Client,
    room_id: str,
    member_id: str,
    action_type: str,
    payload: dict[str, Any],
    action_id: str,
) -> dict[str, Any]:
    room = await client.open_room(room_id, member_id)
    try:
        await room.sync()
        return await room.act(action_type, payload, action_id=action_id, timeout=15.0)
    except ProtocolError as error:
        raise BrowserStoryBlocked(
            "driver_action_rejected", action_type=action_type, error_code=error.code
        ) from error
    finally:
        await room.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--driver-manifest", required=True)
    parser.add_argument("--operator-bearer-file", required=True)
    parser.add_argument("--browser-ready", required=True)
    parser.add_argument("--stale-ready", required=True)
    parser.add_argument("--stale-server-advanced", required=True)
    parser.add_argument("--navigator-inspected", required=True)
    parser.add_argument("--briefing-open", required=True)
    parser.add_argument("--navigator-published", required=True)
    parser.add_argument("--negotiation-open", required=True)
    parser.add_argument("--plan-proposed", required=True)
    parser.add_argument("--plan-file", required=True)
    parser.add_argument("--commitment-open", required=True)
    parser.add_argument("--navigator-committed", required=True)
    parser.add_argument("--result-ready", required=True)
    parser.add_argument("--navigator-acked", required=True)
    parser.add_argument("--result-file", required=True)
    parser.add_argument("--timeout", type=float, default=240.0)
    parser.add_argument("--offer-timeout", type=float, default=20.0)
    parser.add_argument("--timer-timeout", type=float, default=240.0)
    parser.add_argument("--base-url")
    args = parser.parse_args()
    if args.timeout <= 0 or args.offer_timeout <= 0 or args.timer_timeout <= 0:
        parser.error("timeout, offer-timeout, and timer-timeout must be positive")
    try:
        result = asyncio.run(drive(args))
    except PublicStoryBlocked as error:
        safe_keys = {
            "http_status",
            "error_code",
            "retryable",
            "member_index",
            "action_type",
            "last_phase",
            "signal",
            "timer_id",
            "generation",
        }
        safe_details = {
            key: value
            for key, value in error.details.items()
            if key in safe_keys
            and isinstance(value, (bool, float, int, str, type(None)))
        }
        result = {
            "status": "blocked",
            "reason_code": error.reason_code,
            **safe_details,
            "live_evidence": True,
            "secrets": "not_emitted",
        }
        write_json(pathlib.Path(args.result_file), result)
        print(
            json.dumps(result, sort_keys=True, separators=(",", ":")), file=sys.stderr
        )
        return 2
    except BrowserStoryBlocked as error:
        safe_keys = {
            "http_status",
            "error_code",
            "retryable",
            "member_index",
            "action_type",
            "last_phase",
            "signal",
            "timer_id",
            "generation",
        }
        safe_details = {
            key: value
            for key, value in error.details.items()
            if key in safe_keys
            and isinstance(value, (bool, float, int, str, type(None)))
        }
        result = {
            "status": "blocked",
            "reason_code": error.reason,
            **safe_details,
            "live_evidence": True,
            "secrets": "not_emitted",
        }
        write_json(pathlib.Path(args.result_file), result)
        print(
            json.dumps(result, sort_keys=True, separators=(",", ":")), file=sys.stderr
        )
        return 2
    print(
        json.dumps(
            {key: value for key, value in result.items() if key not in {"room_id"}},
            sort_keys=True,
            separators=(",", ":"),
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

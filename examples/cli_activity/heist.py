"""Run the deterministic Heist 0.2 external Insider from CLI credential files."""

from __future__ import annotations

import argparse
import asyncio
import json
import time
from pathlib import Path
from typing import Any

from .credentials import (
    CredentialError,
    load_membership,
    load_runner,
    sdk_base_url,
    validate_pair,
)

PACK_ID = "worldstream.agent-heist"
PACK_VERSION = "0.2.0"
PACK_DIGEST = "blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820"
EXPECTED_PACK = {"id": PACK_ID, "version": PACK_VERSION, "digest": PACK_DIGEST}
ACTION_PRIORITY = (
    "inspect_clue",
    "publish_clue",
    "endorse_plan",
    "commit_move",
    "acknowledge_result",
)
SPONTANEOUS_INSIDER_ACTIONS = frozenset({"inspect_clue", "publish_clue"})
ATTENTION_ACTIONS = {
    "offer_received": frozenset({"accept_exchange"}),
    "endorsement_requested": frozenset({"endorse_plan", "challenge_plan"}),
    "commitment_opened": frozenset({"commit_move"}),
    "required_action_deadline": frozenset({"commit_move"}),
    "round_result_available": frozenset({"acknowledge_result"}),
}


def select_action(projection: dict[str, Any]) -> tuple[str, dict[str, Any]] | None:
    """Select only an currently offered deterministic Insider action."""
    body = projection.get("projection")
    if not isinstance(body, dict):
        return None
    activity = body.get("activity")
    offers = body.get("action_offers")
    if not isinstance(activity, dict) or not isinstance(offers, list):
        return None
    offered = {offer.get("action_type") for offer in offers if isinstance(offer, dict)}
    phase = activity.get("phase")
    for action in ACTION_PRIORITY:
        if action not in offered:
            continue
        if action == "inspect_clue":
            return action, {"clue_id": "entry_window"}
        if action == "publish_clue":
            if _claim(activity.get("public_claims"), "entry_window") is not None:
                continue
            claim = _entry_claim(activity)
            if claim:
                return action, {"clue_id": "entry_window", "claim_code": claim}
        if action == "endorse_plan":
            plan = _plan_id(activity)
            if plan:
                return action, {"plan_id": plan}
        if action == "commit_move" and phase == "commitment":
            plan = _plan_id(activity)
            if plan:
                return action, {
                    "selected_plan_id": plan,
                    "contribute_required_resource": True,
                }
        if action == "acknowledge_result" and phase == "result":
            return action, {}
    return None


def select_spontaneous_insider_action(
    projection: dict[str, Any],
) -> tuple[str, dict[str, Any]] | None:
    """Select Insider setup work that does not answer an Attention."""
    selected = select_action(projection)
    if selected is None or selected[0] not in SPONTANEOUS_INSIDER_ACTIONS:
        return None
    return selected


def select_activation_action(
    projection: dict[str, Any], reason_code: str
) -> tuple[str, dict[str, Any]] | None:
    """Select only work that satisfies the claimed Attention reason."""
    allowed = ATTENTION_ACTIONS.get(reason_code)
    body = projection.get("projection")
    if allowed is None or not isinstance(body, dict):
        return None
    offers = body.get("action_offers")
    if not isinstance(offers, list):
        return None
    filtered = [
        offer
        for offer in offers
        if isinstance(offer, dict) and offer.get("action_type") in allowed
    ]
    scoped = projection | {"projection": body | {"action_offers": filtered}}
    return select_action(scoped)


def select_navigator_action(
    projection: dict[str, Any],
) -> tuple[str, dict[str, Any]] | None:
    """Select the next direct-SDK Navigator action from authorized data."""
    body = projection.get("projection")
    if not isinstance(body, dict):
        return None
    activity = body.get("activity")
    offers = body.get("action_offers")
    if not isinstance(activity, dict) or not isinstance(offers, list):
        return None
    offered = {offer.get("action_type") for offer in offers if isinstance(offer, dict)}
    if "inspect_clue" in offered:
        return "inspect_clue", {"clue_id": "route"}
    if (
        "publish_clue" in offered
        and _claim(activity.get("public_claims"), "route") is None
    ):
        claim = _claim(activity.get("private_clues"), "route")
        if claim is not None:
            return "publish_clue", {"clue_id": "route", "claim_code": claim}
    if "propose_plan" in offered:
        if _plan_id(activity) is not None:
            return None
        route = _claim(activity.get("public_claims"), "route")
        entry = _claim(activity.get("public_claims"), "entry_window")
        plan = _plan_from_claims(route, entry)
        if plan is not None:
            return "propose_plan", plan
    if "commit_move" in offered:
        plan = _plan_id(activity)
        if plan is not None:
            return "commit_move", {
                "selected_plan_id": plan,
                "contribute_required_resource": False,
            }
    if "acknowledge_result" in offered:
        return "acknowledge_result", {}
    return None


def _entry_claim(activity: dict[str, Any]) -> str | None:
    value = _claim(activity.get("private_clues"), "entry_window")
    return value if value is not None and value.startswith("entry_window_") else None


def _claim(clues: Any, clue_id: str) -> str | None:
    if not isinstance(clues, list):
        return None
    for clue in clues:
        if isinstance(clue, dict) and clue.get("clue_id") == clue_id:
            value = clue.get("claim_code")
            return value if isinstance(value, str) and value else None
    return None


def _plan_from_claims(
    route_claim: str | None, entry_claim: str | None
) -> dict[str, str] | None:
    if route_claim is None or entry_claim is None:
        return None
    if not route_claim.startswith("route_") or not entry_claim.startswith(
        "entry_window_"
    ):
        return None
    route = route_claim.removeprefix("route_")
    entry = entry_claim.removeprefix("entry_window_")
    completion = {
        ("canal", "late"): ("disguise", "van"),
        ("service", "early"): ("thermal_key", "boat"),
        ("roof", "middle"): ("jammer", "motorbike"),
    }.get((route, entry))
    if completion is None:
        return None
    required_tool, extraction = completion
    return {
        "route": route,
        "entry_window": entry,
        "required_tool": required_tool,
        "extraction": extraction,
    }


def _plan_id(activity: dict[str, Any]) -> str | None:
    plans = activity.get("plans")
    if not isinstance(plans, list):
        return None
    values = [plan.get("plan_id") for plan in plans if isinstance(plan, dict)]
    return next((value for value in values if isinstance(value, str) and value), None)


async def run(
    navigator_membership_file: Path,
    membership_file: Path,
    runner_file: Path,
    timeout_seconds: float,
) -> dict[str, Any]:
    """Drive one direct Navigator and one assignment-bound external Insider."""
    from worldstream_sdk import Client, ProtocolError

    navigator = load_membership(navigator_membership_file)
    membership = load_membership(membership_file)
    runner_doc = load_runner(runner_file)
    validate_pair(membership, runner_doc)
    pack = membership["pack"]
    if pack != EXPECTED_PACK or membership["role"] != "insider":
        raise CredentialError("credential_heist_insider_required")
    if (
        navigator["pack"] != pack
        or navigator["operation"] != membership["operation"]
        or navigator["room_id"] != membership["room_id"]
        or navigator["role"] != "navigator"
    ):
        raise CredentialError("credential_heist_navigator_required")
    navigator_client = Client(sdk_base_url(navigator), navigator["bearer"])
    member_client = Client(sdk_base_url(membership), membership["bearer"])
    runner_client = Client(sdk_base_url(runner_doc), runner_doc["bearer"])
    navigator_room = await navigator_client.open_room(
        navigator["room_id"], navigator["member_id"]
    )
    room = await member_client.open_room(membership["room_id"], membership["member_id"])
    runner = None
    navigator_actions = 0
    insider_actions = 0
    activations = 0
    try:
        if (
            navigator_room.attached is None
            or navigator_room.attached.get("pack") != EXPECTED_PACK
            or room.attached is None
            or room.attached.get("pack") != EXPECTED_PACK
        ):
            raise CredentialError("attached_heist_revision_mismatch")
        runner = await runner_client.open_runner(
            runner_doc["runner_id"],
            1,
            [pack["id"]],
            [pack],
        )
        await navigator_room.sync()
        await room.sync()
        deadline = time.monotonic() + timeout_seconds
        while time.monotonic() < deadline:
            try:
                navigator_projection = await navigator_client.projection(
                    membership["room_id"]
                )
                activity = navigator_projection["projection"]["activity"]
                if isinstance(activity, dict) and activity.get("phase") == "complete":
                    outcome = activity.get("outcome")
                    if not isinstance(outcome, dict) or not outcome:
                        raise CredentialError("heist_terminal_outcome_missing")
                    return {
                        "status": "complete",
                        "navigator_actions": navigator_actions,
                        "insider_actions": insider_actions,
                        "activations": activations,
                        "outcome": outcome,
                        "room_id": membership["room_id"],
                    }
                navigator_acted = await submit_current_action(
                    navigator_room,
                    navigator_projection,
                    select_navigator_action,
                )
                navigator_actions += int(navigator_acted)
                projection = await member_client.projection(membership["room_id"])
                insider_acted, handled = await drive_tick(
                    room,
                    runner,
                    member_client,
                    membership["room_id"],
                    membership["member_id"],
                    projection,
                )
                insider_actions += int(insider_acted) + int(handled)
                activations += int(handled)
            except ProtocolError as error:
                if not error.retryable:
                    raise
                navigator_acted = insider_acted = handled = False
            if not navigator_acted and not insider_acted and not handled:
                await asyncio.sleep(0.2)
        return {
            "status": "timeout",
            "navigator_actions": navigator_actions,
            "insider_actions": insider_actions,
            "activations": activations,
            "room_id": membership["room_id"],
        }
    finally:
        if runner is not None:
            await runner.close()
        await room.close()
        await navigator_room.close()


async def drive_tick(
    room: Any,
    runner: Any,
    client: Any,
    room_id: str,
    member_id: str,
    projection: dict[str, Any],
) -> tuple[bool, bool]:
    """Prefer assignment work, then advance spontaneous Insider setup work."""
    body = projection.get("projection")
    activity = body.get("activity") if isinstance(body, dict) else None
    if isinstance(activity, dict) and activity.get("phase") == "lobby":
        return False, False
    handled = await handle_activation(room, runner, client, room_id, member_id)
    acted = False
    if not handled:
        acted = await submit_current_action(
            room, projection, select_spontaneous_insider_action
        )
    return acted, handled


async def submit_current_action(
    room: Any,
    projection: dict[str, Any],
    selector: Any = select_action,
) -> bool:
    """Submit one offered action; stale rejections require a fresh later tick."""
    from worldstream_sdk import LostActionReply

    selection = selector(projection)
    if selection is None:
        return False
    action, payload = selection
    try:
        room_head = projection.get("room_head")
        expected = room_head.get("room_seq") if isinstance(room_head, dict) else None
        result = await room.act(action, payload, expected_room_seq=expected)
    except LostActionReply as error:
        result = await error.retry()
    # SDK participant fixtures document rejected Actions as ordinary reply bodies.
    if result.get("code") in {"stale_head", "stale_room_state"}:
        await room.resync()
        return False
    transition_id = result.get("transition_id")
    room_head = result.get("room_head")
    room_seq = room_head.get("room_seq") if isinstance(room_head, dict) else None
    return (
        isinstance(transition_id, str)
        and bool(transition_id)
        and isinstance(result.get("duplicate"), bool)
        and not isinstance(room_seq, bool)
        and isinstance(room_seq, int)
        and room_seq >= 0
    )


async def handle_activation(
    room: Any, runner: Any, client: Any, room_id: str, member_id: str
) -> bool:
    """Claim one offer, attempt its current participant work, then complete it."""
    from worldstream_sdk import LostRunnerReply

    offers = await runner.poll_offers(room_id, member_id)
    if not offers["offers"]:
        return False
    activation = offers["offers"][0]
    activation_id = activation.get("activation_id")
    reason_code = activation.get("reason_code")
    if (
        not isinstance(activation_id, str)
        or not activation_id
        or activation.get("room_id") != room_id
        or activation.get("member_id") != member_id
        or reason_code not in ATTENTION_ACTIONS
    ):
        return False
    try:
        claimed = await runner.claim(activation_id, 30_000)
    except LostRunnerReply as error:
        claimed = await error.retry()
    context = claimed.get("context")
    claim_id = claimed.get("claim_id")
    lease_generation = claimed.get("lease_generation")
    if (
        claimed.get("code") != "granted"
        or claimed.get("activation_id") != activation_id
        or not isinstance(claim_id, str)
        or not claim_id
        or isinstance(lease_generation, bool)
        or not isinstance(lease_generation, int)
        or lease_generation < 1
        or not isinstance(context, dict)
        or context.get("activation_id") != activation_id
        or context.get("claim_id") != claim_id
        or context.get("lease_generation") != lease_generation
        or context.get("reason_code") != reason_code
        or not isinstance(context.get("room_head"), dict)
        or context["room_head"].get("room_id") != room_id
    ):
        return False
    activation_projection = None
    if isinstance(context.get("projection"), dict):
        activation_projection = {
            "room_head": context.get("room_head"),
            "projection": context["projection"]
            | {"action_offers": context.get("action_offers", [])},
        }
    projection = activation_projection or await client.projection(room_id)
    acted = await submit_current_action(
        room, projection, lambda value: select_activation_action(value, reason_code)
    )
    if not acted and activation_projection is not None:
        acted = await submit_current_action(
            room,
            await client.projection(room_id),
            lambda value: select_activation_action(value, reason_code),
        )
    if not acted:
        # The lease expires durably; never claim successful handling for work
        # that this deterministic example could not perform.
        return False
    try:
        completed = await runner.complete(
            activation_id, claim_id, lease_generation, "handled"
        )
    except LostRunnerReply as error:
        completed = await error.retry()
    return (
        completed.get("code") == "completed"
        and completed.get("state") == "completed"
        and completed.get("activation_id") == activation_id
        and completed.get("claim_id") == claim_id
        and completed.get("lease_generation") == lease_generation
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--navigator-membership-file", required=True, type=Path)
    parser.add_argument("--membership-file", required=True, type=Path)
    parser.add_argument("--runner-file", required=True, type=Path)
    parser.add_argument("--timeout-seconds", type=float, default=300.0)
    args = parser.parse_args()
    if not 1 <= args.timeout_seconds <= 900:
        parser.error("timeout-seconds must be between 1 and 900")
    try:
        from worldstream_sdk import ProtocolError

        result = asyncio.run(
            run(
                args.navigator_membership_file,
                args.membership_file,
                args.runner_file,
                args.timeout_seconds,
            )
        )
    except (CredentialError, OSError, ProtocolError, TimeoutError, ValueError) as error:
        result = {"status": "error", "code": type(error).__name__}
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0 if result["status"] == "complete" else 3


if __name__ == "__main__":
    raise SystemExit(main())

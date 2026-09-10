"""Shared bounded Runner machinery for Midnight Archive companion policies.

Role adapters supply a deterministic plan selector and an expected Role. This
module owns credential, Activation, Context, Head, Action, and completion
fencing so every companion uses the same authority path.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import re
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from worldstream_sdk import Client, LostActionReply, LostRunnerReply, ProtocolError

from examples.cli_activity.credentials import (
    load_membership,
    load_runner,
    sdk_base_url,
    validate_pair,
)

PACK_ID = "worldstream.midnight-archive"
ACTION = "submit_companion_plan"
ACTION_OFFER_DOMAIN = "worldstream/action-offer/v1"
REASON = "companion_plan_requested"
PROJECTION_SCHEMA = "worldstream.midnight-archive/participant-observation/v2"
DIGEST = re.compile(r"blake3:[0-9a-f]{64}\Z")

PlanSelector = Callable[[dict], dict]
RoleRunner = Callable[..., Any]


@dataclass(frozen=True)
class CompanionRunnerPolicy:
    """The small Role-specific interface used by the shared Runner."""

    expected_role: str
    select_plan: PlanSelector
    contract_error: type[ValueError]
    mismatch_code: str


def require(policy: CompanionRunnerPolicy, condition: bool) -> None:
    if not condition:
        raise policy.contract_error(policy.mismatch_code)


def integer(value: Any, minimum: int, maximum: int) -> bool:
    return type(value) is int and minimum <= value <= maximum


def shortest_route(
    policy: CompanionRunnerPolicy,
    activity: dict,
    origin: str,
    target: str,
    locations: set[str],
) -> list[str]:
    """Return one deterministic route through only currently open authored edges."""
    gates = activity.get("gates")
    require(
        policy,
        isinstance(gates, dict)
        and set(gates) == {"archive_gate", "service_hatch"}
        and all(value in ("open", "closed") for value in gates.values()),
    )
    chart = activity.get("map")
    require(policy, isinstance(chart, dict) and set(chart) == {"locations", "connections"})
    authored_locations = chart["locations"]
    require(
        policy,
        isinstance(authored_locations, list)
        and len(authored_locations) == len(locations)
        and all(
            isinstance(item, dict) and isinstance(item.get("id"), str)
            for item in authored_locations
        )
        and {item["id"] for item in authored_locations} == locations,
    )
    edges = chart["connections"]
    require(policy, isinstance(edges, list) and len(edges) == 6)
    adjacency: dict[str, set[str]] = {location: set() for location in locations}
    seen: set[tuple[str, str]] = set()
    for edge in edges:
        require(policy, isinstance(edge, dict) and set(edge) == {"from", "to", "gate"})
        left, right, gate = edge["from"], edge["to"], edge["gate"]
        require(
            policy,
            isinstance(left, str)
            and left in locations
            and isinstance(right, str)
            and right in locations
            and left != right
            and gate in (None, "archive_gate", "service_hatch"),
        )
        key = tuple(sorted((left, right)))
        require(policy, key not in seen)
        seen.add(key)
        if gate is None or gates[gate] == "open":
            adjacency[left].add(right)
            adjacency[right].add(left)
    pending = [(origin, [])]
    visited = {origin}
    for location, route in pending:
        if location == target:
            return route
        for destination in sorted(adjacency[location]):
            if destination not in visited:
                visited.add(destination)
                pending.append((destination, [*route, destination]))
    raise policy.contract_error(policy.mismatch_code)


async def wait_for_matching_offer(
    room: Any,
    runner: Any,
    policy: CompanionRunnerPolicy,
    wait_seconds: int,
    poll_interval: float = 0.1,
) -> dict | None:
    """Keep one Runner attached for one bounded, exact-Membership opportunity."""
    require(policy, integer(wait_seconds, 0, 120) and 0 <= poll_interval <= 1)
    deadline = asyncio.get_running_loop().time() + wait_seconds
    while True:
        polled = await runner.poll_offers(room.room_id, room.member_id)
        matching = [
            offer
            for offer in polled["offers"]
            if offer["reason_code"] == REASON
            and offer["room_id"] == room.room_id
            and offer["member_id"] == room.member_id
        ]
        if matching:
            require(policy, len(matching) == 1)
            return matching[0]
        remaining = deadline - asyncio.get_running_loop().time()
        if remaining <= 0:
            return None
        await asyncio.sleep(min(poll_interval, remaining))


async def answer_once(
    room: Any,
    runner: Any,
    client: Any,
    pack: dict[str, str],
    policy: CompanionRunnerPolicy,
    *,
    wait_seconds: int = 0,
) -> dict:
    """Wait boundedly, then claim and answer at most one exact Activation."""
    offer = await wait_for_matching_offer(room, runner, policy, wait_seconds)
    if offer is None:
        return {"status": "no_opportunity", "submitted_actions": 0}
    claimed = await runner.claim(offer["activation_id"], 30_000)
    if claimed["code"] != "granted":
        return {"status": "not_claimed", "submitted_actions": 0}
    context = claimed.get("context")
    require(policy, isinstance(context, dict))
    require(
        policy,
        context.get("activation_id") == offer["activation_id"]
        and claimed.get("activation_id") == offer["activation_id"]
        and context.get("claim_id") == claimed.get("claim_id")
        and isinstance(claimed.get("claim_id"), str)
        and context.get("lease_generation") == claimed.get("lease_generation")
        and integer(context.get("lease_generation"), 1, 2**63 - 1)
        and integer(claimed.get("lease_generation"), 1, 2**63 - 1)
        and context.get("reason_code") == REASON
        and context.get("cause_room_seq") == offer["cause_room_seq"]
        and context.get("deadline") == offer.get("deadline")
        and context.get("projection_schema") == PROJECTION_SCHEMA,
    )
    # Read through the companion's Membership authority, never Runner authority.
    # A newer Head ends this Invocation; the policy does not silently rebase.
    current = await client.projection(room.room_id)
    require(policy, current.get("room_head") == context.get("room_head"))
    head = current["room_head"]
    require(
        policy,
        head.get("room_id") == room.room_id
        and head.get("pack_digest") == pack["digest"]
        and integer(head.get("room_seq"), 0, 2**63 - 1),
    )
    require(policy, context.get("cause_room_seq") == head["room_seq"])
    projection = current.get("projection")
    require(policy, isinstance(projection, dict))
    require(policy, projection == context.get("projection"))
    require(policy, projection.get("action_offers") == context.get("action_offers"))
    payload = policy.select_plan(projection)
    receipt = await room.act(ACTION, payload, expected_room_seq=head["room_seq"])
    if "transition_id" not in receipt:
        return {"status": "action_rejected", "submitted_actions": 1}
    require(
        policy,
        isinstance(receipt["transition_id"], str)
        and receipt.get("room_head", {}).get("room_seq") == head["room_seq"] + 1
        and receipt.get("room_head", {}).get("pack_digest") == pack["digest"],
    )
    completed = await runner.complete(
        offer["activation_id"],
        claimed["claim_id"],
        claimed["lease_generation"],
        "handled",
    )
    confirmed = (
        completed.get("code") == "completed"
        and completed.get("state") == "completed"
        and completed.get("activation_id") == offer["activation_id"]
        and completed.get("claim_id") == claimed["claim_id"]
        and completed.get("lease_generation") == claimed["lease_generation"]
    )
    return {
        "status": "handled" if confirmed else "completion_unconfirmed",
        "submitted_actions": 1,
    }


async def run_for_role(
    policy: CompanionRunnerPolicy,
    membership_file: Path,
    runner_file: Path,
    revision: str,
    *,
    wait_seconds: int = 45,
) -> dict:
    """Use only one exact external companion assignment's exported capabilities."""
    require(
        policy,
        isinstance(revision, str)
        and DIGEST.fullmatch(revision) is not None
        and integer(wait_seconds, 1, 120),
    )
    membership = load_membership(membership_file)
    runner_document = load_runner(runner_file)
    validate_pair(membership, runner_document)
    pack = {"id": PACK_ID, "version": "0.1.0", "digest": revision}
    require(
        policy,
        membership["role"] == policy.expected_role
        and membership["seat"] == runner_document["seat"]
        and membership["pack"] == pack
        and runner_document["owner_principal_id"] == membership["principal_id"]
        and sdk_base_url(membership) == sdk_base_url(runner_document)
        and membership["bearer"] != runner_document["bearer"]
        and runner_document["permitted_memberships"]
        == [{"room_id": membership["room_id"], "member_id": membership["member_id"]}],
    )
    client = Client(sdk_base_url(membership), membership["bearer"])
    runner_client = Client(sdk_base_url(runner_document), runner_document["bearer"])
    room = await client.open_room(membership["room_id"], membership["member_id"])
    runner = None
    try:
        require(policy, room.attached is not None and room.attached.get("pack") == pack)
        require(
            policy,
            room.welcome is not None
            and room.welcome.get("authenticated_principal")
            == {"principal_id": membership["principal_id"], "kind": "agent"},
        )
        await room.sync()
        runner = await runner_client.open_runner(
            runner_document["runner_id"], 1, [PACK_ID], [pack]
        )
        return await answer_once(
            room,
            runner,
            client,
            pack,
            policy,
            wait_seconds=wait_seconds,
        )
    finally:
        if runner is not None:
            await runner.close()
        await room.close()


def run_cli(
    policy: CompanionRunnerPolicy,
    description: str | None = None,
    role_runner: RoleRunner | None = None,
) -> int:
    parser = argparse.ArgumentParser(description=description)
    parser.add_argument("--membership-file", required=True, type=Path)
    parser.add_argument("--runner-file", required=True, type=Path)
    parser.add_argument("--pack-revision", required=True)
    parser.add_argument("--wait-seconds", type=int, default=45)
    args = parser.parse_args()
    try:
        operation = run_for_role(
            policy,
            args.membership_file,
            args.runner_file,
            args.pack_revision,
            wait_seconds=args.wait_seconds,
        ) if role_runner is None else role_runner(
            args.membership_file,
            args.runner_file,
            args.pack_revision,
            wait_seconds=args.wait_seconds,
        )
        result = asyncio.run(
            asyncio.wait_for(
                operation,
                timeout=args.wait_seconds + 15,
            )
        )
    except (
        ProtocolError,
        LostActionReply,
        LostRunnerReply,
        OSError,
        ValueError,
        TimeoutError,
        RuntimeError,
        KeyError,
        TypeError,
    ):
        # Protocol diagnostics, paths and exception causes can contain private
        # input. Deliberately emit no exception string, traceback, or raw receipt.
        result = {"status": "failed", "submitted_actions": "unconfirmed"}
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0 if result["status"] in {"handled", "no_opportunity"} else 2

"""Wait boundedly, then answer one Mira planning opportunity through scoped SDK connections.

Run as ``python -m examples.midnight_archive.run_mira`` from the repository root.
Credential documents are owner-only CLI exports. No input Projection, credential,
discovery, or plan is written to disk or included in the bounded result report.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import re
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
DIGEST = re.compile(r"blake3:[0-9a-f]{64}\Z")
LOCATIONS = {"atrium", "records", "conservation", "plant", "vault"}


class MiraContractError(ValueError):
    """A closed rejection without input data or authority material."""


def require(condition: bool) -> None:
    if not condition:
        raise MiraContractError("mira_contract_mismatch")


def integer(value: Any, minimum: int, maximum: int) -> bool:
    return type(value) is int and minimum <= value <= maximum


def exact(value: Any, fields: set[str]) -> dict:
    require(isinstance(value, dict) and set(value) == fields)
    return value


def select_plan(projection: dict) -> dict:
    """Choose up to three zero-power investigation steps from Mira's own view.

    Known map edges determine movement. The policy never reads candidate IDs,
    authenticity, source contents, verifier results, or another Role's memory.
    """
    exact(projection, {"core", "activity", "action_offers"})
    offers = projection["action_offers"]
    require(isinstance(offers, list) and len(offers) == 1)
    require(isinstance(offers[0], dict)
            and offers[0].get("domain") == ACTION_OFFER_DOMAIN
            and offers[0].get("action_type") == ACTION)
    activity = projection["activity"]
    require(isinstance(activity, dict) and activity.get("phase") == "active")
    companion = exact(activity.get("mira"), {
        "presence", "location", "mode", "task", "planning", "preparation",
        "knowledge", "last_contribution",
    })
    require(companion["presence"] == "active" and companion["mode"] == "tasked")
    location = companion["location"]
    require(isinstance(location, str) and location in LOCATIONS)
    task = exact(companion["task"], {
        "status", "revision", "kind", "power_allowance", "power_spent",
    })
    require(task["status"] == "assigned"
            and task["kind"] in ("investigate_records", "investigate_conservation")
            and integer(task["revision"], 1, 2**53 - 1)
            and integer(task["power_allowance"], 0, 1)
            and integer(task["power_spent"], 0, task["power_allowance"]))
    planning = exact(companion["planning"], {
        "status", "opportunity_revision", "plan_revision", "steps_total",
        "steps_completed", "deadline",
    })
    require(planning["status"] == "waiting"
            and integer(planning["opportunity_revision"], 1, 2**53 - 1)
            and integer(planning["plan_revision"], 0, 2**53 - 1)
            and integer(planning["steps_total"], 0, 3)
            and integer(planning["steps_completed"], 0, 3)
            and planning["steps_total"] == planning["steps_completed"]
            and isinstance(planning["deadline"], str)
            and re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z",
                             planning["deadline"]) is not None)
    # Deadline eligibility is decided by Pack Semantic Time at admission.
    # A local wall clock must never manufacture a fresh opportunity.
    knowledge = exact(companion["knowledge"], {"records", "conservation", "verifier_result"})
    require(all(knowledge[source] in ("unknown", "private", "shared")
                for source in ("records", "conservation")))
    target = task["kind"].removeprefix("investigate_")
    require(knowledge[target] != "shared")
    route = shortest_route(activity, location, target)
    steps = [step("move", destination=destination) for destination in route]
    if knowledge[target] == "unknown":
        steps.append(step("inspect_source", source=target))
    steps.append(step("share_source", source=target))
    return {
        "task_revision": task["revision"],
        "opportunity_revision": planning["opportunity_revision"],
        "steps": steps[:3],
    }


def step(kind: str, *, destination: str = "none", source: str = "none") -> dict:
    return {"step_type": kind, "destination": destination, "source_id": source, "power_cost": 0}


def shortest_route(activity: dict, origin: str, target: str) -> list[str]:
    gates = exact(activity.get("gates"), {"archive_gate", "service_hatch"})
    require(all(value in ("open", "closed") for value in gates.values()))
    chart = exact(activity.get("map"), {"locations", "connections"})
    locations = chart["locations"]
    require(isinstance(locations, list) and len(locations) == len(LOCATIONS))
    require(all(isinstance(item, dict) and isinstance(item.get("id"), str)
                for item in locations))
    require({item["id"] for item in locations} == LOCATIONS)
    edges = chart["connections"]
    require(isinstance(edges, list) and len(edges) == 6)
    adjacency: dict[str, set[str]] = {location: set() for location in LOCATIONS}
    seen: set[tuple[str, str]] = set()
    for edge in edges:
        exact(edge, {"from", "to", "gate"})
        left, right, gate = edge["from"], edge["to"], edge["gate"]
        require(isinstance(left, str) and left in LOCATIONS
                and isinstance(right, str) and right in LOCATIONS and left != right
                and gate in (None, "archive_gate", "service_hatch"))
        key = tuple(sorted((left, right)))
        require(key not in seen)
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
    raise MiraContractError("mira_contract_mismatch")


async def wait_for_matching_offer(
    room: Any, runner: Any, wait_seconds: int, poll_interval: float = 0.1,
) -> dict | None:
    """Keep the Runner attached for one bounded matching Activation opportunity."""
    require(integer(wait_seconds, 0, 120) and 0 <= poll_interval <= 1)
    deadline = asyncio.get_running_loop().time() + wait_seconds
    while True:
        polled = await runner.poll_offers(room.room_id, room.member_id)
        matching = [
            offer for offer in polled["offers"]
            if offer["reason_code"] == REASON
            and offer["room_id"] == room.room_id
            and offer["member_id"] == room.member_id
        ]
        if matching:
            require(len(matching) == 1)
            return matching[0]
        remaining = deadline - asyncio.get_running_loop().time()
        if remaining <= 0:
            return None
        await asyncio.sleep(min(poll_interval, remaining))


async def answer_once(
    room: Any, runner: Any, client: Any, pack: dict[str, str], *, wait_seconds: int = 0,
) -> dict:
    """Wait boundedly, then claim and answer at most one exact Activation."""
    offer = await wait_for_matching_offer(room, runner, wait_seconds)
    if offer is None:
        return {"status": "no_opportunity", "submitted_actions": 0}
    claimed = await runner.claim(offer["activation_id"], 30_000)
    if claimed["code"] != "granted":
        return {"status": "not_claimed", "submitted_actions": 0}
    context = claimed.get("context")
    require(isinstance(context, dict))
    require(
        context.get("activation_id") == offer["activation_id"]
        and claimed.get("activation_id") == offer["activation_id"]
        and context.get("claim_id") == claimed.get("claim_id")
        and isinstance(claimed.get("claim_id"), str)
        and context.get("lease_generation") == claimed.get("lease_generation")
        and integer(context.get("lease_generation"), 1, 2**63 - 1)
        and integer(claimed.get("lease_generation"), 1, 2**63 - 1)
        and context.get("reason_code") == REASON
        and context.get("cause_room_seq") == offer["cause_room_seq"]
    )
    # Read through Mira's Membership authority, never Runner authority. A newer
    # Head ends this invocation; the policy does not silently answer a new task.
    current = await client.projection(room.room_id)
    require(current.get("room_head") == context.get("room_head"))
    head = current["room_head"]
    require(
        head.get("room_id") == room.room_id
        and head.get("pack_digest") == pack["digest"]
        and integer(head.get("room_seq"), 0, 2**63 - 1)
    )
    projection = current.get("projection")
    require(isinstance(projection, dict))
    require(projection == context.get("projection"))
    require(projection.get("action_offers") == context.get("action_offers"))
    payload = select_plan(projection)
    receipt = await room.act(ACTION, payload, expected_room_seq=head["room_seq"])
    if "transition_id" not in receipt:
        return {"status": "action_rejected", "submitted_actions": 1}
    require(
        isinstance(receipt["transition_id"], str)
        and receipt.get("room_head", {}).get("room_seq") == head["room_seq"] + 1
        and receipt.get("room_head", {}).get("pack_digest") == pack["digest"]
    )
    completed = await runner.complete(
        offer["activation_id"], claimed["claim_id"], claimed["lease_generation"], "handled"
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


async def run(
    membership_file: Path, runner_file: Path, revision: str, *, wait_seconds: int = 45,
) -> dict:
    """Use only the exact external Mira assignment's two exported capabilities."""
    require(
        isinstance(revision, str)
        and DIGEST.fullmatch(revision) is not None
        and integer(wait_seconds, 1, 120)
    )
    membership = load_membership(membership_file)
    runner_document = load_runner(runner_file)
    validate_pair(membership, runner_document)
    pack = {"id": PACK_ID, "version": "0.1.0", "digest": revision}
    require(
        membership["role"] == "mira"
        and membership["pack"] == pack
        and runner_document["owner_principal_id"] == membership["principal_id"]
        and sdk_base_url(membership) == sdk_base_url(runner_document)
        and membership["bearer"] != runner_document["bearer"]
        and runner_document["permitted_memberships"] == [
            {"room_id": membership["room_id"], "member_id": membership["member_id"]}
        ]
    )
    client = Client(sdk_base_url(membership), membership["bearer"])
    runner_client = Client(sdk_base_url(runner_document), runner_document["bearer"])
    room = await client.open_room(membership["room_id"], membership["member_id"])
    runner = None
    try:
        require(room.attached is not None and room.attached.get("pack") == pack)
        require(room.welcome is not None and room.welcome.get("authenticated_principal") == {
            "principal_id": membership["principal_id"], "kind": "agent",
        })
        await room.sync()
        runner = await runner_client.open_runner(runner_document["runner_id"], 1, [PACK_ID], [pack])
        return await answer_once(room, runner, client, pack, wait_seconds=wait_seconds)
    finally:
        if runner is not None:
            await runner.close()
        await room.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--membership-file", required=True, type=Path)
    parser.add_argument("--runner-file", required=True, type=Path)
    parser.add_argument("--pack-revision", required=True)
    parser.add_argument("--wait-seconds", type=int, default=45)
    args = parser.parse_args()
    try:
        result = asyncio.run(asyncio.wait_for(
            run(
                args.membership_file, args.runner_file, args.pack_revision,
                wait_seconds=args.wait_seconds,
            ), timeout=args.wait_seconds + 15,
        ))
    except (ProtocolError, LostActionReply, LostRunnerReply, OSError, ValueError,
            TimeoutError, RuntimeError, KeyError, TypeError):
        # Protocol diagnostics, paths and exception causes can contain private
        # input. Deliberately emit no exception string, traceback, or raw receipt.
        result = {"status": "failed", "submitted_actions": "unconfirmed"}
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0 if result["status"] in {"handled", "no_opportunity"} else 2


if __name__ == "__main__":
    raise SystemExit(main())

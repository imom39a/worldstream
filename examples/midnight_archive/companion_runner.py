"""Shared bounded Runner machinery for Midnight Archive companion policies.

Role adapters supply a deterministic plan selector and an expected Role. This
module owns credential, Activation, Context, Head, Action, and completion
fencing so every companion uses the same authority path.

Successful and no-opportunity reports retain ``status`` and ``submitted_actions``.
Terminal failure reports add ``provider_attempts_consumed`` (zero or one for this
Invocation); durable ten-attempt House allowance remains outside this example.
"""

from __future__ import annotations

import argparse
import asyncio
import copy
import json
import re
from collections.abc import Callable
from contextlib import suppress
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Protocol

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
PROJECTION_SCHEMA = "worldstream.midnight-archive/participant-projection/v4"
DIGEST = re.compile(r"blake3:[0-9a-f]{64}\Z")
UTC_TIMESTAMP = re.compile(
    r"(?P<year>[0-9]{4})-(?P<month>[0-9]{2})-(?P<day>[0-9]{2})"
    r"T(?P<hour>[0-9]{2}):(?P<minute>[0-9]{2}):(?P<second>[0-9]{2})"
    r"(?:\.(?P<fraction>[0-9]{1,9}))?Z\Z"
)
RUNTIME_COMPLETION_MARGIN_MS = 5_000
PLAN_STEP_TYPES = {
    "move",
    "inspect_source",
    "share_source",
    "use_verifier",
    "open_service_hatch",
    "collect_assay_sample",
    "complete_field_assay",
}
PLAN_LOCATIONS = {"none", "atrium", "records", "conservation", "plant", "vault"}
PLAN_SOURCES = {"none", "records", "conservation"}
TERMINAL_FAILURE_STATUSES = {
    "provider_timeout",
    "provider_rejected",
    "provider_attempt_rejected",
    "stale_head",
    "cancelled",
    "expired",
    "ineligible",
    "malformed",
    "action_rejected",
}

PlanSelector = Callable[[dict], dict]
RoleRunner = Callable[..., Any]


@dataclass(frozen=True)
class ProviderRequest:
    """One provider request bound to an exact claimed Activation context."""

    activation_id: str
    claim_id: str
    lease_generation: int
    room_head: dict
    projection: dict


class ProviderAdapter(Protocol):
    """External planning seam; implementations receive no Runner authority."""

    async def propose(self, request: ProviderRequest) -> dict: ...


@dataclass
class ProviderInvocationAttempt:
    """One invocation-local dispatch marker, separate from Host allowance."""

    consumed: bool = False
    in_flight: bool = False
    _activation_id: str | None = field(default=None, init=False, repr=False)

    def begin(self, activation_id: str) -> None:
        if (
            self.consumed
            or self.in_flight
            or self._activation_id is not None
        ):
            raise ProviderAttemptRejected
        self._activation_id = activation_id
        self.consumed = True
        self.in_flight = True

    def finish(self, activation_id: str) -> None:
        if self._activation_id == activation_id:
            self.in_flight = False


class ProviderAttemptRejected(RuntimeError):
    """The provider boundary declined or could not admit this exact request."""


class ProviderRejected(RuntimeError):
    """The selected provider rejected one already-consumed attempt."""


@dataclass(frozen=True)
class PolicyProviderAdapter:
    """Default offline adapter around the deterministic Role policy."""

    selector: PlanSelector

    async def propose(self, request: ProviderRequest) -> dict:
        return self.selector(request.projection)


@dataclass(frozen=True)
class ScriptedProviderAdapter:
    """Explicit offline provider behavior for deterministic qualification."""

    selector: PlanSelector
    mode: str = "deterministic"
    delay_seconds: float = 0

    def __post_init__(self) -> None:
        if (
            self.mode not in {"deterministic", "delayed", "missing", "rejected"}
            or isinstance(self.delay_seconds, bool)
            or not isinstance(self.delay_seconds, (int, float))
            or not 0 <= self.delay_seconds <= 30
        ):
            raise ValueError("invalid scripted provider configuration")

    async def propose(self, request: ProviderRequest) -> dict:
        if self.mode == "missing":
            await asyncio.Event().wait()
        if self.mode == "rejected":
            raise ProviderRejected
        if self.mode == "delayed":
            await asyncio.sleep(self.delay_seconds)
        return self.selector(request.projection)


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


def canonical_utc_timestamp(value: Any) -> bool:
    """Match Core's canonical UTC RFC 3339 second timestamp contract."""
    if not isinstance(value, str):
        return False
    match = UTC_TIMESTAMP.fullmatch(value)
    if match is None:
        return False
    fraction = match.group("fraction")
    if fraction is not None and fraction.endswith("0"):
        return False
    try:
        datetime(
            int(match.group("year")),
            int(match.group("month")),
            int(match.group("day")),
            int(match.group("hour")),
            int(match.group("minute")),
            int(match.group("second")),
            tzinfo=UTC,
        )
    except ValueError:
        return False
    return True


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
    # This is only the local idle-offer ceiling. The Pack's 15-second planning
    # opportunity and any future provider-call timeout remain separate fences.
    require(policy, integer(wait_seconds, 0, 180) and 0 <= poll_interval <= 1)
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


def claim_failure_status(code: Any) -> str:
    """Map an unclaimed Activation reply to one closed public status."""
    return {
        "expired": "expired",
        "cancelled": "cancelled",
        "fenced": "stale_head",
        "stale_lease": "stale_head",
    }.get(code, "activation_rejected")


def validate_execution_bounds(
    policy: CompanionRunnerPolicy,
    context: dict,
    provider_timeout_seconds: float,
    claim_received_at: float,
) -> int:
    """Validate exact Runtime witnesses and the separate provider-call bound."""
    budget = context.get("runner_budget")
    limits = context.get("runner_limits")
    require(
        policy,
        isinstance(budget, dict)
        and set(budget) == {"schema", "max_action_submissions"}
        and budget.get("schema") == "worldstream/runner-budget/v1"
        and budget.get("max_action_submissions") == 1,
    )
    require(
        policy,
        isinstance(limits, dict)
        and set(limits) == {"schema", "max_runtime_ms", "max_result_bytes"}
        and limits.get("schema") == "worldstream/runner-limits/v1"
        and integer(limits.get("max_runtime_ms"), 1, 86_400_000)
        and integer(limits.get("max_result_bytes"), 1, 65_536),
    )
    require(
        policy,
        not isinstance(provider_timeout_seconds, bool)
        and isinstance(provider_timeout_seconds, (int, float))
        and provider_timeout_seconds > 0,
    )
    elapsed_ms = max(
        0, int((asyncio.get_running_loop().time() - claim_received_at) * 1000)
    )
    require(
        policy,
        int(provider_timeout_seconds * 1000)
        <= limits["max_runtime_ms"] - elapsed_ms - RUNTIME_COMPLETION_MARGIN_MS,
    )
    return limits["max_result_bytes"]


def valid_plan_payload(
    payload: Any,
    max_result_bytes: int,
    projection: dict,
    policy: CompanionRunnerPolicy,
) -> bool:
    """Check the closed provider result envelope before participant submission."""
    if not isinstance(payload, dict) or set(payload) != {
        "task_revision", "opportunity_revision", "steps", "dialogue",
    }:
        return False
    if not (
        integer(payload["task_revision"], 1, 2**53 - 1)
        and integer(payload["opportunity_revision"], 1, 2**53 - 1)
        and isinstance(payload["steps"], list)
        and 1 <= len(payload["steps"]) <= 3
    ):
        return False
    activity = projection.get("activity")
    companion = activity.get(policy.expected_role) if isinstance(activity, dict) else None
    task = companion.get("task") if isinstance(companion, dict) else None
    planning = companion.get("planning") if isinstance(companion, dict) else None
    if not (
        isinstance(task, dict)
        and isinstance(planning, dict)
        and payload["task_revision"] == task.get("revision")
        and payload["opportunity_revision"] == planning.get("opportunity_revision")
    ):
        return False
    text = payload["dialogue"]
    if not isinstance(text, str) or any(ord(char) < 32 or 0xD800 <= ord(char) <= 0xDFFF for char in text):
        return False
    if len(text.encode("utf-8")) > 160 or len(json.dumps(json.dumps(text, ensure_ascii=False), ensure_ascii=False).encode("utf-8")) > 192:
        return False
    if text and companion.get("dialogue_allowed") is not True:
        return False
    for step in payload["steps"]:
        if not (
            isinstance(step, dict)
            and set(step) == {"step_type", "destination", "source_id", "power_cost"}
            and isinstance(step["step_type"], str)
            and step["step_type"] in PLAN_STEP_TYPES
            and isinstance(step["destination"], str)
            and step["destination"] in PLAN_LOCATIONS
            and isinstance(step["source_id"], str)
            and step["source_id"] in PLAN_SOURCES
            and integer(step["power_cost"], 0, 2)
        ):
            return False
    try:
        encoded = json.dumps(
            payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False
        ).encode("utf-8")
    except (TypeError, ValueError):
        return False
    return len(encoded) <= max_result_bytes


def projection_unavailable_status(
    projection: Any, policy: CompanionRunnerPolicy,
) -> str | None:
    """Return why the projected task cannot begin provider execution."""
    if not isinstance(projection, dict):
        return "malformed"
    activity = projection.get("activity")
    if not isinstance(activity, dict):
        return "malformed"
    companion = activity.get(policy.expected_role)
    if not isinstance(companion, dict):
        return "malformed"
    task = companion.get("task")
    planning = companion.get("planning")
    if not isinstance(task, dict) or not isinstance(planning, dict):
        return "malformed"
    if task.get("status") == "cancelled":
        return "cancelled"
    if planning.get("status") == "expired":
        return "expired"
    if task.get("status") == "assigned" and planning.get("status") == "not_requested":
        return "cancelled"
    if (
        activity.get("phase") != "active"
        or companion.get("presence") != "active"
        or companion.get("mode") != "tasked"
        or task.get("status") != "assigned"
        or planning.get("status") != "waiting"
    ):
        return "ineligible"
    return None


def changed_projection_status(
    projection: Any, policy: CompanionRunnerPolicy,
) -> str:
    """Explain why a provider reply can no longer target its claimed Head."""
    return projection_unavailable_status(projection, policy) or "stale_head"


def action_rejection_status(receipt: dict) -> str:
    """Map Pack/Head rejections to one closed Runner terminal status."""
    code = receipt.get("code")
    details = receipt.get("details")
    declared_code = details.get("declared_code") if isinstance(details, dict) else None
    if code in {"stale_head", "stale_room_state", "stale_plan"}:
        return "stale_head"
    if code == "deadline_passed":
        return "expired"
    if declared_code == "stale_plan":
        return "stale_head"
    if code in {"inactive", "role_violation", "task_violation", "companion_unavailable"}:
        return "ineligible"
    if declared_code in {"inactive", "role_violation", "task_violation", "companion_unavailable"}:
        return "ineligible"
    if code in {"invalid_payload", "plan_invalid"} or declared_code in {
        "invalid_payload", "plan_invalid",
    }:
        return "malformed"
    return "action_rejected"


async def answer_once(
    room: Any,
    runner: Any,
    client: Any,
    pack: dict[str, str],
    policy: CompanionRunnerPolicy,
    *,
    wait_seconds: int = 0,
    provider: ProviderAdapter | None = None,
    provider_attempt: ProviderInvocationAttempt | None = None,
    provider_timeout_seconds: float = 10,
) -> dict:
    """Wait boundedly, then claim and answer at most one exact Activation."""
    observation_drain = asyncio.create_task(
        drain_membership_observations(room, policy)
    )
    try:
        offer = await wait_for_matching_offer(room, runner, policy, wait_seconds)
    finally:
        observation_drain.cancel()
        with suppress(asyncio.CancelledError):
            await observation_drain
    if offer is None:
        return {"status": "no_opportunity", "submitted_actions": 0}
    claimed = await runner.claim(offer["activation_id"], 30_000)
    if claimed["code"] != "granted":
        return {
            "status": claim_failure_status(claimed["code"]),
            "submitted_actions": 0,
            "provider_attempts_consumed": 0,
        }
    claim_received_at = asyncio.get_running_loop().time()
    if not (
        claimed.get("activation_id") == offer["activation_id"]
        and isinstance(claimed.get("claim_id"), str)
        and integer(claimed.get("lease_generation"), 1, 2**63 - 1)
    ):
        return {
            "status": "malformed",
            "submitted_actions": 0,
            "provider_attempts_consumed": 0,
        }
    context = claimed.get("context")
    try:
        require(policy, isinstance(context, dict))
        require(
            policy,
            context.get("activation_id") == offer["activation_id"]
            and context.get("claim_id") == claimed["claim_id"]
            and context.get("lease_generation") == claimed["lease_generation"]
            and integer(context.get("lease_generation"), 1, 2**63 - 1)
            and context.get("reason_code") == REASON
            and context.get("cause_room_seq") == offer["cause_room_seq"]
            and context.get("deadline") == offer.get("deadline")
            and context.get("projection_schema") == PROJECTION_SCHEMA,
        )
        result_limit = validate_execution_bounds(
            policy,
            context,
            provider_timeout_seconds,
            claim_received_at,
        )
        # Read through the companion's Membership authority, never Runner authority.
        current = await client.projection(room.room_id)
        if current.get("room_head") != context.get("room_head"):
            return await complete_terminal(
                runner,
                offer,
                claimed,
                status="stale_head",
                disposition="failed",
                submitted_actions=0,
                provider_attempts_consumed=0,
            )
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
    except policy.contract_error:
        return await complete_terminal(
            runner,
            offer,
            claimed,
            status="malformed",
            disposition="failed",
            submitted_actions=0,
            provider_attempts_consumed=0,
        )
    unavailable_status = projection_unavailable_status(projection, policy)
    if unavailable_status is not None:
        return await complete_terminal(
            runner,
            offer,
            claimed,
            status=unavailable_status,
            disposition=(
                "failed" if unavailable_status == "malformed" else "declined"
            ),
            submitted_actions=0,
            provider_attempts_consumed=0,
        )
    provider = provider or PolicyProviderAdapter(policy.select_plan)
    provider_attempt = provider_attempt or ProviderInvocationAttempt()
    attempt_was_consumed = provider_attempt.consumed
    try:
        payload = await invoke_provider(
            provider,
            ProviderRequest(
                activation_id=offer["activation_id"],
                claim_id=claimed["claim_id"],
                lease_generation=claimed["lease_generation"],
                room_head=copy.deepcopy(head),
                projection=copy.deepcopy(projection),
            ),
            provider_attempt,
            provider_timeout_seconds,
        )
    except TimeoutError:
        return await complete_terminal(
            runner,
            offer,
            claimed,
            status="provider_timeout",
            disposition="failed",
            submitted_actions=0,
            provider_attempts_consumed=(
                int(provider_attempt.consumed) - int(attempt_was_consumed)
            ),
        )
    except ProviderRejected:
        return await complete_terminal(
            runner,
            offer,
            claimed,
            status="provider_rejected",
            disposition="declined",
            submitted_actions=0,
            provider_attempts_consumed=(
                int(provider_attempt.consumed) - int(attempt_was_consumed)
            ),
        )
    except ProviderAttemptRejected:
        return await complete_terminal(
            runner,
            offer,
            claimed,
            status="provider_attempt_rejected",
            disposition="declined",
            submitted_actions=0,
            provider_attempts_consumed=0,
        )
    except policy.contract_error:
        return await complete_terminal(
            runner,
            offer,
            claimed,
            status="malformed",
            disposition="failed",
            submitted_actions=0,
            provider_attempts_consumed=(
                int(provider_attempt.consumed) - int(attempt_was_consumed)
            ),
        )
    if not valid_plan_payload(payload, result_limit, projection, policy):
        return await complete_terminal(
            runner,
            offer,
            claimed,
            status="malformed",
            disposition="failed",
            submitted_actions=0,
            provider_attempts_consumed=(
                int(provider_attempt.consumed) - int(attempt_was_consumed)
            ),
        )
    latest = await client.projection(room.room_id)
    if latest.get("room_head") != head:
        latest_status = changed_projection_status(latest.get("projection"), policy)
        return await complete_terminal(
            runner,
            offer,
            claimed,
            status=latest_status,
            disposition=(
                "declined"
                if latest_status in {"cancelled", "expired", "ineligible"}
                else "failed"
            ),
            submitted_actions=0,
            provider_attempts_consumed=(
                int(provider_attempt.consumed) - int(attempt_was_consumed)
            ),
        )
    if latest.get("projection") != projection:
        return await complete_terminal(
            runner,
            offer,
            claimed,
            status="malformed",
            disposition="failed",
            submitted_actions=0,
            provider_attempts_consumed=(
                int(provider_attempt.consumed) - int(attempt_was_consumed)
            ),
        )
    receipt = await room.act(ACTION, payload, expected_room_seq=head["room_seq"])
    if "transition_id" not in receipt:
        rejection_status = action_rejection_status(receipt)
        return await complete_terminal(
            runner,
            offer,
            claimed,
            status=rejection_status,
            disposition=(
                "declined"
                if rejection_status in {"expired", "ineligible"}
                else "failed"
            ),
            submitted_actions=1,
            provider_attempts_consumed=(
                int(provider_attempt.consumed) - int(attempt_was_consumed)
            ),
        )
    if not (
        isinstance(receipt["transition_id"], str)
        and receipt.get("room_head", {}).get("room_seq") == head["room_seq"] + 1
        and receipt.get("room_head", {}).get("pack_digest") == pack["digest"]
    ):
        return await complete_terminal(
            runner,
            offer,
            claimed,
            status="malformed",
            disposition="failed",
            submitted_actions=1,
            provider_attempts_consumed=(
                int(provider_attempt.consumed) - int(attempt_was_consumed)
            ),
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


async def complete_terminal(
    runner: Any,
    offer: dict,
    claimed: dict,
    *,
    status: str,
    disposition: str,
    submitted_actions: int,
    provider_attempts_consumed: int,
) -> dict:
    """Persist one closed Activation disposition and return a redacted result."""
    if (
        status not in TERMINAL_FAILURE_STATUSES
        or disposition not in {"declined", "failed"}
        or submitted_actions not in {0, 1}
        or provider_attempts_consumed not in {0, 1}
    ):
        raise ValueError("invalid terminal Runner result")
    completed = await runner.complete(
        offer["activation_id"],
        claimed["claim_id"],
        claimed["lease_generation"],
        disposition,
    )
    confirmed = (
        completed.get("code") == "completed"
        and completed.get("state") == "completed"
        and completed.get("activation_id") == offer["activation_id"]
        and completed.get("claim_id") == claimed["claim_id"]
        and completed.get("lease_generation") == claimed["lease_generation"]
    )
    return {
        "status": status if confirmed else "completion_unconfirmed",
        "submitted_actions": submitted_actions,
        "provider_attempts_consumed": provider_attempts_consumed,
    }


async def invoke_provider(
    provider: ProviderAdapter,
    request: ProviderRequest,
    attempt: ProviderInvocationAttempt,
    timeout_seconds: float,
) -> dict:
    """Consume and serialize one bounded provider attempt for this Invocation."""
    if (
        isinstance(timeout_seconds, bool)
        or not isinstance(timeout_seconds, (int, float))
        or not 0 < timeout_seconds <= 30
    ):
        raise ProviderAttemptRejected
    attempt.begin(request.activation_id)
    try:
        return await asyncio.wait_for(
            provider.propose(request), timeout=float(timeout_seconds)
        )
    finally:
        attempt.finish(request.activation_id)


async def drain_membership_observations(
    room: Any, policy: CompanionRunnerPolicy,
) -> None:
    """Keep the exact Membership stream current while its Runner waits idle."""
    async for delivery in room.events():
        require(
            policy,
            isinstance(delivery, dict)
            and integer(delivery.get("frame_seq"), 1, 2**63 - 1),
        )
        await room.ack(delivery["frame_seq"])


async def run_for_role(
    policy: CompanionRunnerPolicy,
    membership_file: Path,
    runner_file: Path,
    revision: str,
    *,
    wait_seconds: int = 45,
    provider: ProviderAdapter | None = None,
    provider_attempt: ProviderInvocationAttempt | None = None,
    provider_timeout_seconds: float = 10,
) -> dict:
    """Use only one exact external companion assignment's exported capabilities."""
    require(
        policy,
        isinstance(revision, str)
        and DIGEST.fullmatch(revision) is not None
        and integer(wait_seconds, 1, 180)
        and not isinstance(provider_timeout_seconds, bool)
        and isinstance(provider_timeout_seconds, (int, float))
        and 0 < provider_timeout_seconds <= 30,
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
            provider=provider,
            provider_attempt=provider_attempt,
            provider_timeout_seconds=provider_timeout_seconds,
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
    parser.add_argument(
        "--provider-mode",
        choices=("deterministic", "delayed", "missing", "rejected"),
        default="deterministic",
    )
    parser.add_argument("--provider-delay-seconds", type=float, default=0)
    parser.add_argument("--provider-timeout-seconds", type=float, default=10)
    args = parser.parse_args()
    try:
        provider = ScriptedProviderAdapter(
            policy.select_plan, args.provider_mode, args.provider_delay_seconds
        )
        provider_attempt = ProviderInvocationAttempt()
        operation = run_for_role(
            policy,
            args.membership_file,
            args.runner_file,
            args.pack_revision,
            wait_seconds=args.wait_seconds,
            provider=provider,
            provider_attempt=provider_attempt,
            provider_timeout_seconds=args.provider_timeout_seconds,
        ) if role_runner is None else role_runner(
            args.membership_file,
            args.runner_file,
            args.pack_revision,
            wait_seconds=args.wait_seconds,
            provider=provider,
            provider_attempt=provider_attempt,
            provider_timeout_seconds=args.provider_timeout_seconds,
        )
        result = asyncio.run(
            asyncio.wait_for(
                operation,
                timeout=args.wait_seconds + args.provider_timeout_seconds + 10,
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

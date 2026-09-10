"""Wait boundedly, then answer one Mira planning opportunity through scoped SDK connections.

Run as ``python -m examples.midnight_archive.run_mira`` from the repository root.
Credential documents are owner-only CLI exports. No input Projection, credential,
discovery, or plan is written to disk or included in the bounded result report.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from examples.midnight_archive import companion_runner

ACTION = companion_runner.ACTION
ACTION_OFFER_DOMAIN = companion_runner.ACTION_OFFER_DOMAIN
DIGEST = companion_runner.DIGEST
PACK_ID = companion_runner.PACK_ID
REASON = companion_runner.REASON

LOCATIONS = {"atrium", "records", "conservation", "plant", "vault"}
CANDIDATES = {"ledger-amber", "ledger-cobalt", "ledger-violet"}


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
    """Choose up to three bounded steps from Mira's own authorized view."""
    exact(projection, {"core", "activity", "action_offers"})
    offers = projection["action_offers"]
    require(isinstance(offers, list) and len(offers) == 1)
    require(
        isinstance(offers[0], dict)
        and offers[0].get("domain") == ACTION_OFFER_DOMAIN
        and offers[0].get("action_type") == ACTION
    )
    activity = projection["activity"]
    require(isinstance(activity, dict) and activity.get("phase") == "active")
    companion = exact(
        activity.get("mira"),
        {
            "presence", "location", "mode", "task", "planning", "preparation",
            "knowledge", "field_assay", "last_contribution",
        },
    )
    require(companion["presence"] == "active" and companion["mode"] == "tasked")
    location = companion["location"]
    require(isinstance(location, str) and location in LOCATIONS)
    task = exact(
        companion["task"],
        {"status", "revision", "kind", "power_allowance", "power_spent"},
    )
    require(
        task["status"] == "assigned"
        and task["kind"] in (
            "investigate_records", "investigate_conservation", "open_service_hatch",
            "field_assay",
        )
        and integer(task["revision"], 1, 2**53 - 1)
        and integer(task["power_allowance"], 0, 2)
        and integer(task["power_spent"], 0, task["power_allowance"])
    )
    planning = exact(
        companion["planning"],
        {
            "status", "opportunity_revision", "plan_revision", "steps_total",
            "steps_completed", "deadline",
        },
    )
    require(
        planning["status"] == "waiting"
        and integer(planning["opportunity_revision"], 1, 2**53 - 1)
        and integer(planning["plan_revision"], 0, 2**53 - 1)
        and integer(planning["steps_total"], 0, 3)
        and integer(planning["steps_completed"], 0, 3)
        and planning["steps_total"] == planning["steps_completed"]
        and companion_runner.canonical_utc_timestamp(planning["deadline"])
    )
    # Deadline eligibility is decided by Pack Semantic Time at admission.
    # A local wall clock must never manufacture a fresh opportunity.
    knowledge = exact(companion["knowledge"], {"records", "conservation", "verifier_result"})
    require(all(knowledge[source] in ("unknown", "private", "shared")
                for source in ("records", "conservation")))
    assay = exact(companion["field_assay"], {"steps_completed", "result"})
    require(integer(assay["steps_completed"], 0, 2))
    if assay["result"] is not None:
        result = exact(assay["result"], {"candidate_id", "confidence"})
        require(result["candidate_id"] in CANDIDATES and result["confidence"] == "verified")

    if task["kind"] == "field_assay":
        require(task["power_allowance"] == 0 and task["power_spent"] == 0
                and assay["result"] is None)
        steps = [
            step("move", destination=destination)
            for destination in shortest_route(activity, location, "vault")
        ]
        if assay["steps_completed"] == 0:
            steps.extend([step("collect_assay_sample"), step("complete_field_assay")])
        else:
            require(assay["steps_completed"] == 1)
            steps.append(step("complete_field_assay"))
        steps = steps[:3]
    elif task["kind"] == "open_service_hatch":
        gates = exact(activity.get("gates"), {"archive_gate", "service_hatch"})
        require(gates["service_hatch"] == "closed"
                and task["power_allowance"] == 2 and task["power_spent"] == 0)
        steps = [
            step("move", destination=destination)
            for destination in shortest_route(activity, location, "plant")
        ]
        steps.append(step("open_service_hatch", power=2))
        steps = steps[:3]
    else:
        require(task["power_allowance"] <= 1)
        require(assay["steps_completed"] in (0, 1, 2))
        target = task["kind"].removeprefix("investigate_")
        require(knowledge[target] != "shared")
        route = shortest_route(activity, location, target)
        steps = [step("move", destination=destination) for destination in route]
        if knowledge[target] == "unknown":
            steps.append(step("inspect_source", source=target))
        steps.append(step("share_source", source=target))
        steps = steps[:3]
    return {
        "task_revision": task["revision"],
        "opportunity_revision": planning["opportunity_revision"],
        "steps": steps,
    }


def step(
    kind: str, *, destination: str = "none", source: str = "none", power: int = 0
) -> dict:
    return {
        "step_type": kind,
        "destination": destination,
        "source_id": source,
        "power_cost": power,
    }


def shortest_route(activity: dict, origin: str, target: str) -> list[str]:
    return companion_runner.shortest_route(POLICY, activity, origin, target, LOCATIONS)


POLICY = companion_runner.CompanionRunnerPolicy(
    expected_role="mira",
    select_plan=select_plan,
    contract_error=MiraContractError,
    mismatch_code="mira_contract_mismatch",
)


async def wait_for_matching_offer(
    room: Any, runner: Any, wait_seconds: int, poll_interval: float = 0.1,
) -> dict | None:
    return await companion_runner.wait_for_matching_offer(
        room, runner, POLICY, wait_seconds, poll_interval
    )


async def answer_once(
    room: Any,
    runner: Any,
    client: Any,
    pack: dict[str, str],
    *,
    wait_seconds: int = 0,
    provider: companion_runner.ProviderAdapter | None = None,
    provider_attempt: companion_runner.ProviderInvocationAttempt | None = None,
    provider_timeout_seconds: float = 10,
) -> dict:
    return await companion_runner.answer_once(
        room,
        runner,
        client,
        pack,
        POLICY,
        wait_seconds=wait_seconds,
        provider=provider,
        provider_attempt=provider_attempt,
        provider_timeout_seconds=provider_timeout_seconds,
    )


async def run(
    membership_file: Path,
    runner_file: Path,
    revision: str,
    *,
    wait_seconds: int = 45,
    provider: companion_runner.ProviderAdapter | None = None,
    provider_attempt: companion_runner.ProviderInvocationAttempt | None = None,
    provider_timeout_seconds: float = 10,
) -> dict:
    return await companion_runner.run_for_role(
        POLICY,
        membership_file,
        runner_file,
        revision,
        wait_seconds=wait_seconds,
        provider=provider,
        provider_attempt=provider_attempt,
        provider_timeout_seconds=provider_timeout_seconds,
    )


def main() -> int:
    return companion_runner.run_cli(POLICY, __doc__, run)


if __name__ == "__main__":
    raise SystemExit(main())

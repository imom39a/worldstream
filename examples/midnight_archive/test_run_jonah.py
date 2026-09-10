from __future__ import annotations

import copy

import pytest

from examples.midnight_archive import run_jonah as jonah


def projection() -> dict:
    return {
        "core": {},
        "action_offers": [{
            "domain": jonah.ACTION_OFFER_DOMAIN,
            "action_type": jonah.ACTION,
            "payload_schema_digest": "blake3:" + "2" * 64,
        }],
        "activity": {
            "phase": "active",
            "gates": {"archive_gate": "closed", "service_hatch": "closed"},
            "map": {
                "locations": [
                    {"id": location, "name": location.title(), "description": "Authored location."}
                    for location in sorted(jonah.LOCATIONS)
                ],
                "connections": [
                    {"from": "atrium", "to": "records", "gate": None},
                    {"from": "atrium", "to": "conservation", "gate": None},
                    {"from": "records", "to": "conservation", "gate": None},
                    {"from": "records", "to": "plant", "gate": None},
                    {"from": "conservation", "to": "vault", "gate": "archive_gate"},
                    {"from": "plant", "to": "vault", "gate": "service_hatch"},
                ],
            },
            "jonah": {
                "presence": "active",
                "location": "atrium",
                "mode": "tasked",
                "task": {
                    "status": "assigned",
                    "revision": 4,
                    "kind": "open_service_hatch",
                    "power_allowance": 1,
                    "power_spent": 0,
                },
                "planning": {
                    "status": "waiting",
                    "opportunity_revision": 5,
                    "plan_revision": 0,
                    "steps_total": 0,
                    "steps_completed": 0,
                    "deadline": "2026-09-09T12:00:15Z",
                },
                "knowledge": {
                    "records": "unknown",
                    "conservation": "unknown",
                    "verifier_result": None,
                },
                "field_assay": {"steps_completed": 0, "result": None},
                "preparation": {},
                "last_contribution": None,
            },
        },
    }


def test_atrium_hatch_plan_is_two_moves_then_the_one_charge_specialist_step():
    view = projection()
    original = copy.deepcopy(view)
    assert jonah.select_plan(view) == {
        "task_revision": 4,
        "opportunity_revision": 5,
        "steps": [
            jonah.step("move", destination="records"),
            jonah.step("move", destination="plant"),
            jonah.step("open_service_hatch", power=1),
        ],
    }
    assert view == original


def test_plant_hatch_plan_contains_only_the_specialist_step():
    view = projection()
    view["activity"]["jonah"]["location"] = "plant"
    assert jonah.select_plan(view)["steps"] == [
        jonah.step("open_service_hatch", power=1)
    ]


def test_ordinary_investigation_uses_the_same_closed_zero_power_steps():
    view = projection()
    companion = view["activity"]["jonah"]
    companion["task"].update(kind="investigate_conservation", power_allowance=0)
    assert jonah.select_plan(view)["steps"] == [
        jonah.step("move", destination="conservation"),
        jonah.step("inspect_source", source="conservation"),
        jonah.step("share_source", source="conservation"),
    ]


@pytest.mark.parametrize("mutation", [
    "assay_task",
    "open_hatch",
    "no_allowance",
    "spent_allowance",
    "assay_progress",
    "assay_result",
    "wrong_role_node",
    "malformed_map",
])
def test_ineligible_or_malformed_jonah_work_fails_closed(mutation):
    view = projection()
    companion = view["activity"]["jonah"]
    if mutation == "assay_task":
        companion["task"]["kind"] = "field_assay"
    elif mutation == "open_hatch":
        view["activity"]["gates"]["service_hatch"] = "open"
    elif mutation == "no_allowance":
        companion["task"]["power_allowance"] = 0
    elif mutation == "spent_allowance":
        companion["task"]["power_spent"] = 1
    elif mutation == "assay_progress":
        companion["field_assay"]["steps_completed"] = 1
    elif mutation == "assay_result":
        companion["field_assay"]["result"] = {
            "candidate_id": "ledger-violet",
            "confidence": "verified",
        }
    elif mutation == "wrong_role_node":
        view["activity"]["mira"] = view["activity"].pop("jonah")
    else:
        view["activity"]["map"]["connections"][0]["to"] = "outside"
    with pytest.raises(jonah.JonahContractError, match="^jonah_contract_mismatch$"):
        jonah.select_plan(view)


def test_other_companion_private_fields_are_never_read():
    view = projection()
    view["activity"]["mira"] = {"private_discovery": "private-canary"}
    payload = jonah.select_plan(view)
    assert "private-canary" not in repr(payload)


@pytest.mark.parametrize("deadline", [
    "2026-09-09T12:00:15Z",
    "2026-09-09T12:00:15.1Z",
    "2026-09-09T12:00:15.12Z",
    "2026-09-09T12:00:15.123456789Z",
])
def test_jonah_accepts_every_canonical_core_fraction_width(deadline):
    view = projection()
    view["activity"]["jonah"]["planning"]["deadline"] = deadline
    assert jonah.select_plan(view)["opportunity_revision"] == 5


@pytest.mark.parametrize("deadline", [
    "2026-09-09T12:00:15.0Z",
    "2026-09-09T12:00:15.120Z",
    "2026-09-09T12:00:15.1234567890Z",
    "2026-02-29T12:00:15Z",
    "2026-09-09T12:00:60Z",
    "2026-09-09T12:00:15+00:00",
])
def test_jonah_rejects_noncanonical_core_timestamps(deadline):
    view = projection()
    view["activity"]["jonah"]["planning"]["deadline"] = deadline
    with pytest.raises(jonah.JonahContractError, match="^jonah_contract_mismatch$"):
        jonah.select_plan(view)

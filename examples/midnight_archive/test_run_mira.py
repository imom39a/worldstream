from __future__ import annotations

import asyncio
import copy
import json
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock

import pytest
from worldstream_sdk.client import _loads

from examples.midnight_archive import companion_runner
from examples.midnight_archive import run_mira as mira

PACK = {"id": mira.PACK_ID, "version": "0.1.0", "digest": "blake3:" + "1" * 64}


def projection():
    return {
        "core": {},
        "action_offers": [{"domain": mira.ACTION_OFFER_DOMAIN, "action_type": mira.ACTION,
                           "payload_schema_digest": "blake3:" + "2" * 64}],
        "activity": {
            "phase": "active", "gates": {"archive_gate": "closed", "service_hatch": "closed"},
            "map": {"locations": [
                {"id": location, "name": location.title(), "description": "Authored location."}
                for location in sorted(mira.LOCATIONS)
            ], "connections": [
                {"from": "atrium", "to": "records", "gate": None},
                {"from": "atrium", "to": "conservation", "gate": None},
                {"from": "records", "to": "conservation", "gate": None},
                {"from": "records", "to": "plant", "gate": None},
                {"from": "conservation", "to": "vault", "gate": "archive_gate"},
                {"from": "plant", "to": "vault", "gate": "service_hatch"},
            ]},
            "mira": {
                "presence": "active", "location": "atrium", "mode": "tasked",
                "task": {"status": "assigned", "revision": 2, "kind": "investigate_records",
                         "power_allowance": 0, "power_spent": 0},
                "planning": {"status": "waiting", "opportunity_revision": 3, "plan_revision": 0,
                             "steps_total": 0, "steps_completed": 0,
                             "deadline": "2026-09-09T12:00:15.000Z"},
                "knowledge": {"records": "unknown", "conservation": "unknown",
                              "verifier_result": None},
                "field_assay": {"steps_completed": 0, "result": None},
                "preparation": {}, "last_contribution": None,
            },
        },
    }


def test_exact_records_plan_has_three_closed_zero_power_steps():
    view = projection()
    original = copy.deepcopy(view)
    assert mira.select_plan(view) == {
        "task_revision": 2, "opportunity_revision": 3,
        "steps": [mira.step("move", destination="records"),
                  mira.step("inspect_source", source="records"),
                  mira.step("share_source", source="records")],
    }
    assert view == original


def test_conservation_and_private_knowledge_select_only_needed_work():
    view = projection()
    companion = view["activity"]["mira"]
    companion["task"]["kind"] = "investigate_conservation"
    companion["task"]["power_allowance"] = 1
    companion["task"]["power_spent"] = 1
    companion["location"] = "conservation"
    assert mira.select_plan(view)["steps"] == [
        mira.step("inspect_source", source="conservation"),
        mira.step("share_source", source="conservation"),
    ]
    companion["knowledge"]["conservation"] = "private"
    assert mira.select_plan(view)["steps"] == [mira.step("share_source", source="conservation")]


def test_long_route_clips_to_three_and_respects_closed_gate():
    view = projection()
    view["activity"]["mira"]["location"] = "vault"
    view["activity"]["gates"]["service_hatch"] = "open"
    assert mira.select_plan(view)["steps"] == [
        mira.step("move", destination="plant"), mira.step("move", destination="records"),
        mira.step("inspect_source", source="records"),
    ]
    view["activity"]["gates"]["service_hatch"] = "closed"
    with pytest.raises(mira.MiraContractError):
        mira.select_plan(view)


def test_field_assay_uses_two_zero_power_steps_and_resumes_from_recorded_progress():
    view = projection()
    companion = view["activity"]["mira"]
    companion["task"]["kind"] = "field_assay"
    companion["location"] = "vault"
    assert mira.select_plan(view)["steps"] == [
        mira.step("collect_assay_sample"),
        mira.step("complete_field_assay"),
    ]
    companion["field_assay"]["steps_completed"] = 1
    assert mira.select_plan(view)["steps"] == [mira.step("complete_field_assay")]


def test_field_assay_routes_to_the_vault_before_collecting_when_a_gate_is_open():
    view = projection()
    companion = view["activity"]["mira"]
    companion["task"]["kind"] = "field_assay"
    view["activity"]["gates"]["archive_gate"] = "open"
    assert mira.select_plan(view)["steps"] == [
        mira.step("move", destination="conservation"),
        mira.step("move", destination="vault"),
        mira.step("collect_assay_sample"),
    ]


def test_mira_can_plan_the_two_charge_ordinary_service_hatch_method():
    view = projection()
    companion = view["activity"]["mira"]
    companion["task"].update(kind="open_service_hatch", power_allowance=2)
    assert mira.select_plan(view)["steps"] == [
        mira.step("move", destination="records"),
        mira.step("move", destination="plant"),
        mira.step("open_service_hatch", power=2),
    ]


@pytest.mark.parametrize("field,value", [
    ("steps_completed", 3),
    ("result", {"candidate_id": "ledger-amber", "confidence": "unverified"}),
])
def test_field_assay_contract_fails_closed(field, value):
    view = projection()
    view["activity"]["mira"]["field_assay"][field] = value
    with pytest.raises(mira.MiraContractError, match="^mira_contract_mismatch$"):
        mira.select_plan(view)


@pytest.mark.parametrize("section,field,value", [
    ("task", "status", "cancelled"), ("task", "kind", "none"),
    ("task", "revision", True), ("task", "power_allowance", 2),
    ("task", "power_spent", 1), ("planning", "status", "expired"),
    ("planning", "opportunity_revision", 0), ("planning", "plan_revision", -1),
    ("planning", "steps_total", 4), ("planning", "steps_total", 1),
    ("planning", "deadline", "none"), ("knowledge", "records", "shared"),
    ("knowledge", "records", "invented"),
])
def test_invalid_task_and_opportunity_fail_closed(section, field, value):
    view = projection()
    view["activity"]["mira"][section][field] = value
    with pytest.raises(mira.MiraContractError, match="^mira_contract_mismatch$"):
        mira.select_plan(view)


@pytest.mark.parametrize("field,value", [("presence", "suspended"), ("mode", "following"),
                                        ("location", "outside")])
def test_ineligible_mira_cannot_plan(field, value):
    view = projection()
    view["activity"]["mira"][field] = value
    with pytest.raises(mira.MiraContractError):
        mira.select_plan(view)


def test_missing_offer_and_unknown_task_fields_are_rejected():
    view = projection()
    view["action_offers"] = []
    with pytest.raises(mira.MiraContractError):
        mira.select_plan(view)
    view = projection()
    view["activity"]["mira"]["task"]["arbitrary_script"] = "secret-canary"
    with pytest.raises(mira.MiraContractError, match="^mira_contract_mismatch$"):
        mira.select_plan(view)


def test_malformed_map_cannot_authorize_a_route():
    view = projection()
    view["activity"]["map"]["locations"] = []
    with pytest.raises(mira.MiraContractError):
        mira.select_plan(view)
    view = projection()
    view["activity"]["map"]["connections"][0]["to"] = "outside"
    with pytest.raises(mira.MiraContractError):
        mira.select_plan(view)


def transport(projection: dict):
    async def no_events():
        await asyncio.Event().wait()
        yield {}

    head = {
        "room_id": "room", "room_seq": 8, "pack_digest": PACK["digest"],
        "core_schema_version": "worldstream.core-room-state.v1",
        **{key: "blake3:" + "3" * 64 for key in (
            "genesis_or_transition_hash", "core_state_hash", "activity_state_hash",
            "authoritative_state_hash",
        )},
    }
    offer = {
        "activation_id": "activation", "room_id": "room", "member_id": "mira",
        "reason_code": mira.REASON, "cause_room_seq": 8,
        "deadline": "2026-09-09T12:00:15.000Z",
    }
    context = {
        "activation_id": "activation", "claim_id": "claim", "lease_generation": 1,
        "reason_code": mira.REASON, "cause_room_seq": 8, "room_head": head,
        "projection": projection, "action_offers": projection["action_offers"],
        "lease_until": "2026-09-09T12:00:30.000Z", "deadline": "2026-09-09T12:00:15.000Z",
        "integrity_generation": 1, "policy_revision": 1, "authority_generation": 1,
        "membership_generation": 1, "frame_head": 8, "retained_floor": 0, "cursor": None,
        "projection_schema": companion_runner.PROJECTION_SCHEMA,
        "runner_budget": {
            "schema": "worldstream/runner-budget/v1", "max_action_submissions": 1,
        },
        "runner_limits": {
            "schema": "worldstream/runner-limits/v1", "max_runtime_ms": 30_000,
            "max_result_bytes": 65_536,
        },
        "artifact_references": [],
        "delivery": {"kind": "projection_reset", "baseline_frame_head": 8, "reason": "initial"},
    }
    runner = SimpleNamespace(
        poll_offers=AsyncMock(return_value={"offers": [offer]}),
        claim=AsyncMock(return_value={
            "code": "granted", "activation_id": "activation", "claim_id": "claim",
            "lease_generation": 1, "context": context,
            "operation_id": "claim", "runner_id": "runner", "state": "leased",
            "context_hash": "blake3:" + "4" * 64,
        }),
        complete=AsyncMock(return_value={
            "code": "completed", "state": "completed", "activation_id": "activation",
            "claim_id": "claim", "lease_generation": 1,
        }),
    )
    client = SimpleNamespace(projection=AsyncMock(return_value={
        "room_head": copy.deepcopy(head), "projection": copy.deepcopy(projection),
    }))
    room = SimpleNamespace(
        room_id="room", member_id="mira",
        events=Mock(side_effect=no_events), ack=AsyncMock(),
        act=AsyncMock(return_value={
            "transition_id": "transition", "room_head": {**head, "room_seq": 9},
        }),
    )
    return room, runner, client


def test_production_context_shape_contains_full_projection_and_separate_offers():
    # Matches sqlite_backend.rs::activation_context (also PostgreSQL): the full
    # wire Projection includes offers, with the same offers repeated in Context.
    view = projection()
    assert view["action_offers"][0]["domain"] == "worldstream/action-offer/v1"
    room, runner, client = transport(view)
    body = runner.claim.return_value
    wire = _loads(json.dumps({
        "protocol": "0.1", "type": "activation.claimed",
        "message_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV", "body": body,
    }))
    runner.claim.return_value = wire["body"]
    result = asyncio.run(mira.answer_once(room, runner, client, PACK))
    assert result["status"] == "handled"


def test_one_plan_uses_exact_head_and_never_retains_discoveries(capsys):
    view = projection()
    view["activity"]["candidates"] = [{"private_text": "discovery-canary"}]
    room, runner, client = transport(view)
    result = asyncio.run(mira.answer_once(room, runner, client, PACK))
    assert result == {"status": "handled", "submitted_actions": 1}
    room.act.assert_awaited_once_with(mira.ACTION, mira.select_plan(view), expected_room_seq=8)
    runner.poll_offers.assert_awaited_once_with("room", "mira")
    runner.claim.assert_awaited_once_with("activation", 30_000)
    runner.complete.assert_awaited_once_with("activation", "claim", 1, "handled")
    assert "discovery-canary" not in json.dumps(result)
    assert "discovery-canary" not in repr(room.act.call_args)
    assert capsys.readouterr().out == ""


def test_stale_action_receipt_is_not_retried_or_completed():
    room, runner, client = transport(projection())
    room.act.return_value = {"code": "stale_head", "current_room_seq": 9}
    result = asyncio.run(mira.answer_once(room, runner, client, PACK))
    assert result == {
        "status": "stale_head",
        "submitted_actions": 1,
        "provider_attempts_consumed": 1,
    }
    room.act.assert_awaited_once()
    runner.complete.assert_awaited_once_with("activation", "claim", 1, "failed")


def test_lost_action_reply_is_not_retried_or_completed():
    room, runner, client = transport(projection())
    room.act.side_effect = TimeoutError("private transport detail")
    with pytest.raises(TimeoutError):
        asyncio.run(mira.answer_once(room, runner, client, PACK))
    room.act.assert_awaited_once()
    runner.complete.assert_not_called()


def test_nonmatching_attention_never_claims_or_submits():
    room, runner, client = transport({"action_offers": []})
    runner.poll_offers.return_value["offers"][0]["reason_code"] = "arbitrary_other_reason"
    result = asyncio.run(mira.answer_once(room, runner, client, PACK))
    assert result == {"status": "no_opportunity", "submitted_actions": 0}
    runner.claim.assert_not_called()
    client.projection.assert_not_called()
    room.act.assert_not_called()


def test_waits_boundedly_for_one_matching_opportunity_before_claiming():
    room, runner, client = transport(projection())
    nonmatching = copy.deepcopy(runner.poll_offers.return_value)
    nonmatching["offers"][0]["reason_code"] = "another_reason"
    matching = copy.deepcopy(runner.poll_offers.return_value)
    runner.poll_offers.side_effect = [nonmatching, matching]
    result = asyncio.run(mira.answer_once(room, runner, client, PACK, wait_seconds=1))
    assert result == {"status": "handled", "submitted_actions": 1}
    assert runner.poll_offers.await_count == 2
    runner.claim.assert_awaited_once_with("activation", 30_000)
    room.act.assert_awaited_once()


def test_bounded_wait_without_matching_opportunity_never_claims_or_submits():
    room, runner, client = transport({"action_offers": []})
    runner.poll_offers.return_value = {"offers": []}
    result = asyncio.run(mira.answer_once(room, runner, client, PACK, wait_seconds=0))
    assert result == {"status": "no_opportunity", "submitted_actions": 0}
    runner.poll_offers.assert_awaited_once_with("room", "mira")
    runner.claim.assert_not_called()
    room.act.assert_not_called()


@pytest.mark.parametrize("field,value", [
    ("reason_code", "other"), ("cause_room_seq", 7), ("claim_id", "other"),
    ("activation_id", "other"), ("lease_generation", True),
])
def test_malformed_claim_context_records_failure_without_submitting(field, value):
    room, runner, client = transport({"action_offers": []})
    runner.claim.return_value["context"][field] = value
    result = asyncio.run(mira.answer_once(room, runner, client, PACK))
    assert result == {
        "status": "malformed",
        "submitted_actions": 0,
        "provider_attempts_consumed": 0,
    }
    room.act.assert_not_called()
    runner.complete.assert_awaited_once_with("activation", "claim", 1, "failed")


def test_newer_head_never_rebases_or_retries():
    room, runner, client = transport({"action_offers": []})
    client.projection.return_value["room_head"]["room_seq"] = 9
    result = asyncio.run(mira.answer_once(room, runner, client, PACK))
    assert result == {
        "status": "stale_head",
        "submitted_actions": 0,
        "provider_attempts_consumed": 0,
    }
    room.act.assert_not_called()
    runner.claim.assert_awaited_once()
    client.projection.assert_awaited_once()
    runner.complete.assert_awaited_once_with("activation", "claim", 1, "failed")


def test_cli_error_output_never_retains_private_inputs(monkeypatch, capsys, tmp_path):
    secret = "wsb1:" + "a" * 64
    private = "private-discovery-canary"
    monkeypatch.setattr(mira, "run", AsyncMock(side_effect=ValueError(f"{secret} {private}")))
    monkeypatch.setattr("sys.argv", [
        "run_mira", "--membership-file", str(tmp_path / "membership"),
        "--runner-file", str(tmp_path / "runner"), "--pack-revision", PACK["digest"],
    ])
    assert mira.main() == 2
    captured = capsys.readouterr()
    assert json.loads(captured.out) == {"status": "failed", "submitted_actions": "unconfirmed"}
    assert captured.err == ""
    assert secret not in captured.out
    assert private not in captured.out
    assert list(tmp_path.iterdir()) == []


def test_cli_exposes_explicit_scripted_provider_configuration(monkeypatch, capsys, tmp_path):
    terminal = {
        "status": "provider_rejected",
        "submitted_actions": 0,
        "provider_attempts_consumed": 1,
    }
    run = AsyncMock(return_value=terminal)
    monkeypatch.setattr(mira, "run", run)
    monkeypatch.setattr("sys.argv", [
        "run_mira",
        "--membership-file", str(tmp_path / "membership"),
        "--runner-file", str(tmp_path / "runner"),
        "--pack-revision", PACK["digest"],
        "--provider-mode", "rejected",
        "--provider-timeout-seconds", "2",
    ])

    assert mira.main() == 2
    assert json.loads(capsys.readouterr().out) == terminal
    kwargs = run.await_args.kwargs
    assert kwargs["provider"].mode == "rejected"
    assert kwargs["provider_timeout_seconds"] == 2
    assert isinstance(
        kwargs["provider_attempt"], companion_runner.ProviderInvocationAttempt
    )


def credentials():
    membership = {
        "schema": "worldstream/membership-credentials/v1", "operation": "operation",
        "seat": "mira", "room_id": "room", "member_id": "mira", "principal_id": "agent",
        "runtime_url": "ws://127.0.0.1:9000/v1/stream", "role": "mira", "pack": PACK,
        "bearer": "wsb1:" + "a" * 64,
    }
    runner = {
        "schema": "worldstream/runner-credentials/v1", "operation": "operation",
        "seat": "mira", "runner_id": "runner", "pack": PACK,
        "owner_principal_id": "agent",
        "runtime_url": "ws://127.0.0.1:9000/v1/runner/stream",
        "permitted_memberships": [{"room_id": "room", "member_id": "mira"}],
        "bearer": "wsb1:" + "b" * 64,
    }
    return membership, runner


@pytest.mark.parametrize("invalid", [
    "human_role", "wrong_seat", "foreign_runner_owner", "another_origin", "shared_bearer",
    "broad_runner",
])
def test_credentials_fail_closed_before_connecting(monkeypatch, tmp_path, invalid):
    membership, runner = credentials()
    if invalid == "human_role":
        membership["role"] = "lead"
    elif invalid == "wrong_seat":
        runner["seat"] = "jonah"
    elif invalid == "foreign_runner_owner":
        runner["owner_principal_id"] = "different-agent"
    elif invalid == "another_origin":
        runner["runtime_url"] = "ws://127.0.0.1:9001/v1/runner/stream"
    elif invalid == "shared_bearer":
        runner["bearer"] = membership["bearer"]
    else:
        runner["permitted_memberships"].append({"room_id": "room", "member_id": "lead"})
    monkeypatch.setattr(companion_runner, "load_membership", lambda _: membership)
    monkeypatch.setattr(companion_runner, "load_runner", lambda _: runner)
    factory = AsyncMock()
    monkeypatch.setattr(companion_runner, "Client", factory)
    with pytest.raises(mira.MiraContractError):
        asyncio.run(mira.run(tmp_path / "membership", tmp_path / "runner", PACK["digest"]))
    factory.assert_not_called()

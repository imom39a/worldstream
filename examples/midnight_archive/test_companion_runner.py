from __future__ import annotations

import asyncio
import copy
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock

import pytest

from examples.midnight_archive import companion_runner

PACK = {
    "id": companion_runner.PACK_ID,
    "version": "0.1.0",
    "digest": "blake3:" + "1" * 64,
}


class RunnerContractError(ValueError):
    pass


def policy(selector: Mock | None = None) -> companion_runner.CompanionRunnerPolicy:
    return companion_runner.CompanionRunnerPolicy(
        expected_role="mira",
        select_plan=selector or Mock(return_value={"task_revision": 1}),
        contract_error=RunnerContractError,
        mismatch_code="test_contract_mismatch",
    )


def transport() -> tuple[SimpleNamespace, SimpleNamespace, SimpleNamespace, dict]:
    async def no_events():
        await asyncio.Event().wait()
        yield {}

    projection = {
        "core": {},
        "activity": {"phase": "active", "private_canary": "never-returned"},
        "action_offers": [{
            "domain": companion_runner.ACTION_OFFER_DOMAIN,
            "action_type": companion_runner.ACTION,
            "payload_schema_digest": "blake3:" + "2" * 64,
        }],
    }
    head = {
        "room_id": "room",
        "room_seq": 8,
        "pack_digest": PACK["digest"],
    }
    offer = {
        "activation_id": "activation",
        "room_id": "room",
        "member_id": "mira-member",
        "reason_code": companion_runner.REASON,
        "cause_room_seq": 8,
        "deadline": "2026-09-09T12:00:15.000Z",
    }
    context = {
        "activation_id": "activation",
        "claim_id": "claim",
        "lease_generation": 1,
        "reason_code": companion_runner.REASON,
        "cause_room_seq": 8,
        "deadline": "2026-09-09T12:00:15.000Z",
        "projection_schema": companion_runner.PROJECTION_SCHEMA,
        "room_head": copy.deepcopy(head),
        "projection": copy.deepcopy(projection),
        "action_offers": copy.deepcopy(projection["action_offers"]),
    }
    runner = SimpleNamespace(
        poll_offers=AsyncMock(return_value={"offers": [offer]}),
        claim=AsyncMock(return_value={
            "code": "granted",
            "activation_id": "activation",
            "claim_id": "claim",
            "lease_generation": 1,
            "context": context,
        }),
        complete=AsyncMock(return_value={
            "code": "completed",
            "state": "completed",
            "activation_id": "activation",
            "claim_id": "claim",
            "lease_generation": 1,
        }),
    )
    client = SimpleNamespace(projection=AsyncMock(return_value={
        "room_head": copy.deepcopy(head),
        "projection": copy.deepcopy(projection),
    }))
    room = SimpleNamespace(
        room_id="room",
        member_id="mira-member",
        events=Mock(side_effect=no_events),
        ack=AsyncMock(),
        act=AsyncMock(return_value={
            "transition_id": "transition",
            "room_head": {**head, "room_seq": 9},
        }),
    )
    return room, runner, client, projection


def test_shared_activation_path_submits_once_at_the_claimed_head_then_completes():
    room, runner, client, projection = transport()
    selector = Mock(return_value={"task_revision": 1, "steps": []})
    result = asyncio.run(
        companion_runner.answer_once(room, runner, client, PACK, policy(selector))
    )
    assert result == {"status": "handled", "submitted_actions": 1}
    selector.assert_called_once_with(projection)
    room.act.assert_awaited_once_with(
        companion_runner.ACTION,
        {"task_revision": 1, "steps": []},
        expected_room_seq=8,
    )
    runner.complete.assert_awaited_once_with("activation", "claim", 1, "handled")


@pytest.mark.parametrize("field,value", [
    ("reason_code", "other"),
    ("cause_room_seq", 7),
    ("claim_id", "other"),
    ("lease_generation", True),
    ("deadline", "2026-09-09T12:00:16.000Z"),
    ("projection_schema", "worldstream.midnight-archive/participant-projection/v1"),
])
def test_shared_claim_context_mismatch_never_submits_or_completes(field, value):
    room, runner, client, _ = transport()
    runner.claim.return_value["context"][field] = value
    with pytest.raises(RunnerContractError, match="^test_contract_mismatch$"):
        asyncio.run(companion_runner.answer_once(room, runner, client, PACK, policy()))
    room.act.assert_not_called()
    runner.complete.assert_not_called()


def test_offer_for_another_companion_membership_is_never_claimed():
    room, runner, client, _ = transport()
    runner.poll_offers.return_value["offers"][0]["member_id"] = "jonah-member"
    result = asyncio.run(
        companion_runner.answer_once(room, runner, client, PACK, policy())
    )
    assert result == {"status": "no_opportunity", "submitted_actions": 0}
    runner.claim.assert_not_called()
    client.projection.assert_not_called()
    room.act.assert_not_called()


def test_idle_offer_wait_acks_multiple_observations_and_stops_before_action():
    room, runner, client, _ = transport()
    offer = runner.poll_offers.return_value["offers"][0]
    observations_acked = asyncio.Event()
    drain_stopped = asyncio.Event()
    acknowledged: list[int] = []

    async def events_until_cancelled():
        try:
            yield {"frame_seq": 9}
            yield {"frame_seq": 10}
            await asyncio.Event().wait()
        finally:
            drain_stopped.set()

    async def ack(frame_seq: int):
        acknowledged.append(frame_seq)
        if acknowledged == [9, 10]:
            observations_acked.set()

    async def offer_after_observations(*_args):
        await observations_acked.wait()
        return {"offers": [offer]}

    async def act_after_drain(*_args, **_kwargs):
        assert drain_stopped.is_set()
        return {
            "transition_id": "transition",
            "room_head": {
                "room_id": "room",
                "room_seq": 9,
                "pack_digest": PACK["digest"],
            },
        }

    room.events = Mock(side_effect=events_until_cancelled)
    room.ack.side_effect = ack
    room.act.side_effect = act_after_drain
    runner.poll_offers.side_effect = offer_after_observations
    result = asyncio.run(
        companion_runner.answer_once(
            room, runner, client, PACK, policy(), wait_seconds=1
        )
    )
    assert result == {"status": "handled", "submitted_actions": 1}
    assert acknowledged == [9, 10]
    assert drain_stopped.is_set()


def test_no_opportunity_cancels_and_awaits_the_membership_drain():
    room, runner, client, _ = transport()
    drain_started = asyncio.Event()
    drain_stopped = asyncio.Event()

    async def events_until_cancelled():
        try:
            drain_started.set()
            await asyncio.Event().wait()
            yield {}
        finally:
            drain_stopped.set()

    async def no_offer_after_drain_started(*_args):
        await drain_started.wait()
        return {"offers": []}

    room.events = Mock(side_effect=events_until_cancelled)
    runner.poll_offers.side_effect = no_offer_after_drain_started
    result = asyncio.run(
        companion_runner.answer_once(
            room, runner, client, PACK, policy(), wait_seconds=0
        )
    )
    assert result == {"status": "no_opportunity", "submitted_actions": 0}
    assert drain_stopped.is_set()
    runner.claim.assert_not_called()
    room.act.assert_not_called()


def test_membership_drain_failure_prevents_claim_and_submission():
    room, runner, client, _ = transport()
    invalid_delivery_started = asyncio.Event()
    offer = runner.poll_offers.return_value["offers"][0]

    async def invalid_events():
        invalid_delivery_started.set()
        yield {"frame_seq": 0}

    async def offer_after_invalid_delivery(*_args):
        await invalid_delivery_started.wait()
        await asyncio.sleep(0)
        return {"offers": [offer]}

    room.events = Mock(side_effect=invalid_events)
    runner.poll_offers.side_effect = offer_after_invalid_delivery
    with pytest.raises(RunnerContractError, match="^test_contract_mismatch$"):
        asyncio.run(
            companion_runner.answer_once(
                room, runner, client, PACK, policy(), wait_seconds=1
            )
        )
    runner.claim.assert_not_called()
    client.projection.assert_not_called()
    room.act.assert_not_called()


def test_idle_offer_wait_accepts_180_seconds_but_rejects_a_broader_window():
    room, runner, _, _ = transport()
    offer = asyncio.run(
        companion_runner.wait_for_matching_offer(room, runner, policy(), 180)
    )
    assert offer is runner.poll_offers.return_value["offers"][0]
    with pytest.raises(RunnerContractError, match="^test_contract_mismatch$"):
        asyncio.run(
            companion_runner.wait_for_matching_offer(room, runner, policy(), 181)
        )


def credentials() -> tuple[dict, dict]:
    membership = {
        "schema": "worldstream/membership-credentials/v1",
        "operation": "operation",
        "seat": "mira-seat",
        "room_id": "room",
        "member_id": "mira-member",
        "principal_id": "mira-principal",
        "runtime_url": "ws://127.0.0.1:9000/v1/stream",
        "role": "mira",
        "pack": PACK,
        "bearer": "wsb1:" + "a" * 64,
    }
    runner = {
        "schema": "worldstream/runner-credentials/v1",
        "operation": "operation",
        "seat": "mira-seat",
        "runner_id": "mira-runner",
        "pack": PACK,
        "owner_principal_id": "mira-principal",
        "runtime_url": "ws://127.0.0.1:9000/v1/runner/stream",
        "permitted_memberships": [{"room_id": "room", "member_id": "mira-member"}],
        "bearer": "wsb1:" + "b" * 64,
    }
    return membership, runner


def test_valid_role_pinned_pair_opens_only_its_membership_and_runner(monkeypatch, tmp_path):
    membership, runner_document = credentials()
    room, runner, membership_client, _ = transport()
    room.attached = {"pack": PACK}
    room.welcome = {
        "authenticated_principal": {"principal_id": "mira-principal", "kind": "agent"}
    }
    room.sync = AsyncMock()
    room.close = AsyncMock()
    runner.close = AsyncMock()
    membership_client.open_room = AsyncMock(return_value=room)
    runner_client = SimpleNamespace(open_runner=AsyncMock(return_value=runner))
    clients = Mock(side_effect=[membership_client, runner_client])
    monkeypatch.setattr(companion_runner, "load_membership", lambda _: membership)
    monkeypatch.setattr(companion_runner, "load_runner", lambda _: runner_document)
    monkeypatch.setattr(companion_runner, "Client", clients)

    result = asyncio.run(
        companion_runner.run_for_role(
            policy(),
            tmp_path / "membership",
            tmp_path / "runner",
            PACK["digest"],
        )
    )

    assert result == {"status": "handled", "submitted_actions": 1}
    membership_client.open_room.assert_awaited_once_with("room", "mira-member")
    runner_client.open_runner.assert_awaited_once_with(
        "mira-runner", 1, [companion_runner.PACK_ID], [PACK]
    )
    runner.close.assert_awaited_once()
    room.close.assert_awaited_once()


@pytest.mark.parametrize("invalid", [
    "wrong_role",
    "wrong_seat",
    "swapped_runner",
    "broad_runner",
])
def test_role_and_single_membership_authority_fail_before_connecting(
    monkeypatch, tmp_path: Path, invalid: str,
):
    membership, runner = credentials()
    if invalid == "wrong_role":
        membership["role"] = "jonah"
    elif invalid == "wrong_seat":
        runner["seat"] = "jonah-seat"
    elif invalid == "swapped_runner":
        runner.update(seat="jonah-seat", owner_principal_id="jonah-principal")
        runner["permitted_memberships"] = [{"room_id": "room", "member_id": "jonah-member"}]
    else:
        runner["permitted_memberships"].append(
            {"room_id": "room", "member_id": "lead-member"}
        )
    monkeypatch.setattr(companion_runner, "load_membership", lambda _: membership)
    monkeypatch.setattr(companion_runner, "load_runner", lambda _: runner)
    client = AsyncMock()
    monkeypatch.setattr(companion_runner, "Client", client)
    with pytest.raises((RunnerContractError, ValueError)):
        asyncio.run(
            companion_runner.run_for_role(
                policy(),
                tmp_path / "membership",
                tmp_path / "runner",
                PACK["digest"],
            )
        )
    client.assert_not_called()

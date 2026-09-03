import asyncio
import json
import os
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from examples.cli_activity.heist import (
    EXPECTED_PACK,
    PACK_DIGEST,
    drive_tick,
    handle_activation,
    run,
    select_action,
    select_navigator_action,
    select_spontaneous_insider_action,
    submit_current_action,
)


def _protected_membership(path: Path, *, role: str, digest: str) -> None:
    path.write_text(
        json.dumps(
            {
                "schema": "worldstream/membership-credentials/v1",
                "operation": "setup-1",
                "seat": role,
                "runtime_url": "ws://127.0.0.1:19410/v1/stream",
                "room_id": "room-1",
                "member_id": f"{role}-member",
                "principal_id": f"{role}-principal",
                "pack": {
                    "id": "worldstream.agent-heist",
                    "version": "0.2.0",
                    "digest": digest,
                },
                "role": role,
                "scopes": ["room:act"],
                "bearer": "wsb1:" + "b" * 64,
            }
        ),
        encoding="utf-8",
    )
    if os.name != "nt":
        path.chmod(0o600)


def _protected_runner(path: Path, *, digest: str) -> None:
    path.write_text(
        json.dumps(
            {
                "schema": "worldstream/runner-credentials/v1",
                "operation": "setup-1",
                "seat": "insider",
                "runtime_url": "ws://127.0.0.1:19410/v1/runner/stream",
                "runner_id": "runner-1",
                "owner_principal_id": "insider-principal",
                "pack": {
                    "id": "worldstream.agent-heist",
                    "version": "0.2.0",
                    "digest": digest,
                },
                "permitted_memberships": [
                    {"room_id": "room-1", "member_id": "insider-member"}
                ],
                "scopes": ["runner:activate"],
                "bearer": "wsb1:" + "c" * 64,
            }
        ),
        encoding="utf-8",
    )
    if os.name != "nt":
        path.chmod(0o600)


def projection(activity, offered):
    return {
        "projection": {
            "activity": activity,
            "action_offers": [{"action_type": action} for action in offered],
            "core": {},
        }
    }


def test_selects_only_current_offered_insider_actions() -> None:
    assert select_action(projection({"phase": "briefing"}, ["inspect_clue"])) == (
        "inspect_clue",
        {"clue_id": "entry_window"},
    )
    assert select_action(
        projection(
            {
                "phase": "briefing",
                "private_clues": [
                    {"clue_id": "entry_window", "claim_code": "entry_window_late"}
                ],
            },
            ["publish_clue"],
        )
    ) == (
        "publish_clue",
        {"clue_id": "entry_window", "claim_code": "entry_window_late"},
    )
    assert (
        select_action(
            projection(
                {
                    "phase": "negotiation",
                    "private_clues": [
                        {"clue_id": "entry_window", "claim_code": "entry_window_late"}
                    ],
                    "public_claims": [
                        {"clue_id": "entry_window", "claim_code": "entry_window_late"}
                    ],
                },
                ["publish_clue"],
            )
        )
        is None
    )
    assert select_action(
        projection(
            {"phase": "negotiation", "plans": [{"plan_id": "plan"}]},
            ["endorse_plan"],
        )
    ) == ("endorse_plan", {"plan_id": "plan"})
    assert select_action(
        projection(
            {"phase": "commitment", "plans": [{"plan_id": "plan"}]},
            ["commit_move"],
        )
    ) == (
        "commit_move",
        {"selected_plan_id": "plan", "contribute_required_resource": True},
    )
    assert select_action(projection({"phase": "result"}, ["acknowledge_result"])) == (
        "acknowledge_result",
        {},
    )
    assert (
        select_action(projection({"phase": "negotiation"}, ["challenge_plan"])) is None
    )


def test_navigator_builds_a_plan_only_from_public_claims() -> None:
    activity = {
        "phase": "negotiation",
        "public_claims": [
            {"clue_id": "route", "claim_code": "route_service"},
            {"clue_id": "entry_window", "claim_code": "entry_window_early"},
        ],
    }
    assert select_navigator_action(projection(activity, ["propose_plan"])) == (
        "propose_plan",
        {
            "route": "service",
            "entry_window": "early",
            "required_tool": "thermal_key",
            "extraction": "boat",
        },
    )
    activity["public_claims"].pop()
    assert select_navigator_action(projection(activity, ["propose_plan"])) is None
    activity["public_claims"].append(
        {"clue_id": "entry_window", "claim_code": "entry_window_early"}
    )
    activity["plans"] = [{"plan_id": "already-created"}]
    assert select_navigator_action(projection(activity, ["propose_plan"])) is None


def test_insider_spontaneous_work_does_not_consume_attention_actions() -> None:
    current = projection(
        {"phase": "negotiation", "plans": [{"plan_id": "plan"}]},
        ["endorse_plan"],
    )
    assert select_action(current) == ("endorse_plan", {"plan_id": "plan"})
    assert select_spontaneous_insider_action(current) is None


def test_participant_action_does_not_wait_for_a_runner_offer() -> None:
    class Room:
        def __init__(self) -> None:
            self.calls = []

        async def act(self, action, payload, *, expected_room_seq=None):
            self.calls.append((action, payload, expected_room_seq))
            return {
                "transition_id": "transition",
                "room_head": {"room_seq": 1},
                "duplicate": False,
            }

    room = Room()

    class Runner:
        async def poll_offers(self, _room, _member):
            return {"offers": []}

    class Client:
        pass

    acted, handled = asyncio.run(
        drive_tick(
            room,
            Runner(),
            Client(),
            "room",
            "member",
            projection({"phase": "briefing"}, ["inspect_clue"]),
        )
    )
    assert acted is True
    assert handled is False
    assert room.calls == [("inspect_clue", {"clue_id": "entry_window"}, None)]


def test_action_receipt_accepts_exact_duplicates_but_not_duplicate_rejections() -> None:
    replies = [
        {
            "code": "wrong_phase",
            "current_room_seq": 1,
            "duplicate": True,
        },
        {
            "transition_id": "transition-1",
            "room_head": {"room_seq": 2},
            "duplicate": True,
        },
    ]

    class Room:
        async def act(self, *_args, **_kwargs):
            return replies.pop(0)

    current = projection({"phase": "briefing"}, ["inspect_clue"])
    assert asyncio.run(submit_current_action(Room(), current)) is False
    assert asyncio.run(submit_current_action(Room(), current)) is True


def test_lobby_waits_for_launch_without_polling_runner_work() -> None:
    class Runner:
        async def poll_offers(self, _room, _member):
            raise AssertionError("Lobby must not poll Activation offers")

    acted, handled = asyncio.run(
        drive_tick(
            object(),
            Runner(),
            object(),
            "room",
            "member",
            projection({"phase": "lobby"}, []),
        )
    )

    assert acted is False
    assert handled is False


def test_run_rejects_an_unreviewed_heist_revision_before_connecting(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    wrong_digest = "blake3:" + "0" * 64
    navigator = tmp_path / "navigator.json"
    insider = tmp_path / "insider.json"
    runner = tmp_path / "runner.json"
    _protected_membership(navigator, role="navigator", digest=wrong_digest)
    _protected_membership(insider, role="insider", digest=wrong_digest)
    _protected_runner(runner, digest=wrong_digest)

    class Client:
        def __init__(self, *_args):
            raise AssertionError("an unreviewed Pack revision must not connect")

    monkeypatch.setitem(
        sys.modules,
        "worldstream_sdk",
        SimpleNamespace(Client=Client, ProtocolError=RuntimeError),
    )

    with pytest.raises(ValueError, match="credential_heist_insider_required"):
        asyncio.run(run(navigator, insider, runner, 5.0))


def test_reviewed_heist_digest_is_frozen() -> None:
    assert PACK_DIGEST == (
        "blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820"
    )


def test_activation_performs_only_work_allowed_by_its_attention_reason() -> None:
    calls: list[tuple] = []
    activation_id = "activation-1"
    claim_id = "claim-1"
    activation_projection = projection(
        {"phase": "negotiation", "plans": [{"plan_id": "plan-1"}]},
        ["inspect_clue", "endorse_plan"],
    )["projection"]

    class Room:
        async def act(self, action, payload, *, expected_room_seq=None):
            calls.append(("act", action, payload, expected_room_seq))
            return {
                "transition_id": "transition-1",
                "room_head": {"room_seq": 5},
                "duplicate": False,
            }

    class Runner:
        async def poll_offers(self, room_id, member_id):
            return {
                "offers": [
                    {
                        "activation_id": activation_id,
                        "room_id": room_id,
                        "member_id": member_id,
                        "reason_code": "endorsement_requested",
                    }
                ]
            }

        async def claim(self, requested_activation_id, _lease_ms):
            assert requested_activation_id == activation_id
            return {
                "code": "granted",
                "activation_id": activation_id,
                "claim_id": claim_id,
                "lease_generation": 1,
                "context": {
                    "activation_id": activation_id,
                    "claim_id": claim_id,
                    "lease_generation": 1,
                    "reason_code": "endorsement_requested",
                    "room_head": {"room_id": "room", "room_seq": 4},
                    "projection": activation_projection,
                    "action_offers": activation_projection["action_offers"],
                },
            }

        async def complete(self, *args):
            calls.append(("complete", *args))
            return {
                "code": "completed",
                "state": "completed",
                "activation_id": activation_id,
                "claim_id": claim_id,
                "lease_generation": 1,
            }

    class Client:
        async def projection(self, _room_id):
            raise AssertionError("claim context contains the required projection")

    handled = asyncio.run(
        handle_activation(Room(), Runner(), Client(), "room", "member")
    )
    assert handled is True
    assert calls[0] == ("act", "endorse_plan", {"plan_id": "plan-1"}, 4)
    assert calls[1][0] == "complete"


def test_activation_does_not_report_handled_when_completion_is_fenced() -> None:
    activation_id = "activation-1"
    claim_id = "claim-1"
    activation_projection = projection({"phase": "result"}, ["acknowledge_result"])[
        "projection"
    ]

    class Room:
        async def act(self, *_args, **_kwargs):
            return {
                "transition_id": "transition-1",
                "room_head": {"room_seq": 10},
                "duplicate": False,
            }

    class Runner:
        async def poll_offers(self, room_id, member_id):
            return {
                "offers": [
                    {
                        "activation_id": activation_id,
                        "room_id": room_id,
                        "member_id": member_id,
                        "reason_code": "round_result_available",
                    }
                ]
            }

        async def claim(self, *_args):
            return {
                "code": "granted",
                "activation_id": activation_id,
                "claim_id": claim_id,
                "lease_generation": 1,
                "context": {
                    "activation_id": activation_id,
                    "claim_id": claim_id,
                    "lease_generation": 1,
                    "reason_code": "round_result_available",
                    "room_head": {"room_id": "room", "room_seq": 9},
                    "projection": activation_projection,
                    "action_offers": activation_projection["action_offers"],
                },
            }

        async def complete(self, *_args):
            return {
                "code": "fenced",
                "state": "leased",
                "activation_id": activation_id,
                "claim_id": claim_id,
                "lease_generation": 2,
            }

    handled = asyncio.run(
        handle_activation(Room(), Runner(), object(), "room", "member")
    )
    assert handled is False


def test_run_rejects_wrong_live_attachment_before_runner_handshake(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    navigator = tmp_path / "navigator.json"
    insider = tmp_path / "insider.json"
    runner = tmp_path / "runner.json"
    _protected_membership(navigator, role="navigator", digest=PACK_DIGEST)
    _protected_membership(insider, role="insider", digest=PACK_DIGEST)
    _protected_runner(runner, digest=PACK_DIGEST)
    closed: list[str] = []

    class Room:
        def __init__(self, member_id: str) -> None:
            self.member_id = member_id
            pack = (
                EXPECTED_PACK
                if member_id == "insider-member"
                else {
                    **EXPECTED_PACK,
                    "digest": "blake3:" + "0" * 64,
                }
            )
            self.attached = {"pack": pack}

        async def close(self):
            closed.append(self.member_id)

    class Client:
        def __init__(self, *_args):
            pass

        async def open_room(self, _room_id, member_id):
            return Room(member_id)

        async def open_runner(self, *_args):
            raise AssertionError("wrong Pack attachment must not start a Runner")

    monkeypatch.setitem(
        sys.modules,
        "worldstream_sdk",
        SimpleNamespace(Client=Client, ProtocolError=RuntimeError),
    )

    with pytest.raises(ValueError, match="attached_heist_revision_mismatch"):
        asyncio.run(run(navigator, insider, runner, 5.0))
    assert closed == ["insider-member", "navigator-member"]

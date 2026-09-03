import asyncio
import json
import os
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest
from blake3 import blake3

from examples.cli_activity.credentials import CredentialError
from examples.cli_activity.negotiate import (
    ACTION_TYPE,
    EXPECTED_PACK,
    first_buyer_proposal,
    run,
)


def protected_membership(path: Path, *, digest: str = EXPECTED_PACK["digest"]) -> None:
    path.write_text(
        json.dumps(
            {
                "schema": "worldstream/membership-credentials/v1",
                "operation": "setup-1",
                "seat": "buyer-agent",
                "runtime_url": "ws://127.0.0.1:19410/v1/stream",
                "room_id": "room-1",
                "member_id": "buyer-member",
                "principal_id": "buyer-principal",
                "pack": EXPECTED_PACK | {"digest": digest},
                "role": "buyer_agent",
                "scopes": ["room:act"],
                "bearer": "wsb1:" + "b" * 64,
            }
        ),
        encoding="utf-8",
    )
    if os.name != "nt":
        path.chmod(0o600)


def test_first_proposal_is_public_and_pinned_to_long_lived_example_bytes() -> None:
    payload = first_buyer_proposal()

    assert set(payload) == {"action", "action_id", "actor", "proposal"}
    assert "admitted_at" not in payload
    assert "basis" not in payload
    assert payload["action"] == ACTION_TYPE
    assert payload["actor"] == "buyer_agent"
    assert payload["proposal"]["valid_until"] == 4_102_440_000
    assert (
        payload["proposal"]["offer"]["wire_digest"]
        == "blake3:1f6ff1b5f7ae670400d04c97ac6b6794d5b4d1f8a0e3fa17f7d10eb24ccdfc45"
    )
    for exact_object in (
        payload["proposal"]["offer"],
        payload["proposal"]["session_event"],
    ):
        assert exact_object["wire_digest"] == (
            "blake3:" + blake3(exact_object["canonical_json"].encode()).hexdigest()
        )
        assert (
            exact_object["signature"]["signed_wire_digest"]
            == exact_object["wire_digest"]
        )
    offer = payload["proposal"]["offer"]
    proof_input = "\0".join(
        (
            "worldstream/negotiate-fixture-proof/v1",
            "buyer_agent",
            "agent:northstar:buyer",
            "offer_submission",
            offer["wire_digest"],
        )
    )
    assert (
        offer["signature"]["proof"]
        == "blake3:" + blake3(proof_input.encode()).hexdigest()
    )


def test_run_submits_one_offered_action_only_through_the_public_room_sdk(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    credential = tmp_path / "buyer.json"
    protected_membership(credential)
    calls: list[tuple] = []

    class Room:
        def __init__(self) -> None:
            self.attached = {"pack": EXPECTED_PACK}

        async def sync(self):
            calls.append(("sync",))
            return []

        async def act(self, action_type, payload, **options):
            assert "action_id" not in options
            calls.append(("act", action_type, payload, options))
            return {
                "action_id": payload["action_id"],
                "transition_id": "transition-1",
                "duplicate": False,
                "room_head": {"room_seq": 1},
            }

        async def close(self):
            calls.append(("close",))

    class Client:
        def __init__(self, base_url, bearer):
            calls.append(("client", base_url, bearer))

        async def open_room(self, room_id, member_id):
            calls.append(("open_room", room_id, member_id))
            return Room()

        async def projection(self, room_id):
            calls.append(("projection", room_id))
            return {
                "room_id": room_id,
                "room_head": {"room_seq": 0},
                "projection": {
                    "activity": {"phase": "formation_open"},
                    "action_offers": [
                        {
                            "action_type": ACTION_TYPE,
                            "payload_schema_digest": "blake3:" + "a" * 64,
                        }
                    ],
                },
            }

    class LostActionReply(Exception):
        pass

    monkeypatch.setitem(
        sys.modules,
        "worldstream_sdk",
        SimpleNamespace(Client=Client, LostActionReply=LostActionReply),
    )

    result = asyncio.run(run(credential, 5.0))

    assert result == {
        "status": "accepted",
        "action": ACTION_TYPE,
        "room_id": "room-1",
        "room_seq": 1,
        "transition_id": "transition-1",
    }
    act = next(call for call in calls if call[0] == "act")
    assert act[1] == ACTION_TYPE
    assert act[3] == {"expected_room_seq": 0, "timeout": 5.0}
    assert "admitted_at" not in act[2]
    assert "basis" not in act[2]
    assert calls[-1] == ("close",)


def test_lost_reply_retry_accepts_the_exact_retained_duplicate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    credential = tmp_path / "buyer.json"
    protected_membership(credential)

    class LostActionReply(Exception):
        async def retry(self):
            return {
                "transition_id": "transition-1",
                "duplicate": True,
                "room_head": {"room_seq": 1},
            }

    class Room:
        def __init__(self) -> None:
            self.attached = {"pack": EXPECTED_PACK}

        async def sync(self):
            return []

        async def act(self, *_args, **_kwargs):
            raise LostActionReply()

        async def close(self):
            return None

    class Client:
        def __init__(self, *_args):
            pass

        async def open_room(self, *_args):
            return Room()

        async def projection(self, room_id):
            return {
                "room_id": room_id,
                "room_head": {"room_seq": 0},
                "projection": {
                    "activity": {"phase": "formation_open"},
                    "action_offers": [{"action_type": ACTION_TYPE}],
                },
            }

    monkeypatch.setitem(
        sys.modules,
        "worldstream_sdk",
        SimpleNamespace(Client=Client, LostActionReply=LostActionReply),
    )

    result = asyncio.run(run(credential, 5.0))
    assert result["status"] == "accepted"
    assert result["transition_id"] == "transition-1"


def test_run_rejects_an_unexpected_revision_before_connecting(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    credential = tmp_path / "buyer.json"
    protected_membership(credential, digest="blake3:" + "0" * 64)

    class Client:
        def __init__(self, *_args):
            raise AssertionError("invalid credentials must not connect")

    monkeypatch.setitem(
        sys.modules,
        "worldstream_sdk",
        SimpleNamespace(Client=Client, LostActionReply=RuntimeError),
    )

    with pytest.raises(CredentialError, match="credential_negotiate_buyer_required"):
        asyncio.run(run(credential, 5.0))

import asyncio
import copy
import json

import pytest

from worldstream_sdk import Client, LostActionReply, ProtocolError
from worldstream_sdk.client import (
    MAX_MESSAGE_BYTES,
    WS_SUBPROTOCOL,
    Room,
    _loads,
    _protocol_error_from_http,
)

VALID_BEARER = "wsb1:" + "ab" * 32
VALID_MESSAGE_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
VALID_DIGEST = "blake3:" + "00" * 32
VALID_ROOM_HEAD = {
    "room_id": "room",
    "room_seq": 0,
    "genesis_or_transition_hash": VALID_DIGEST,
    "core_schema_version": "worldstream.core-room-state.v1",
    "pack_digest": VALID_DIGEST,
    "core_state_hash": VALID_DIGEST,
    "activity_state_hash": VALID_DIGEST,
    "authoritative_state_hash": VALID_DIGEST,
}
VALID_PROJECTION = {"core": {}, "activity": {}, "action_offers": []}


class FakeWebSocket:
    def __init__(self) -> None:
        self.sent: list[dict] = []
        self.incoming: asyncio.Queue[str] = asyncio.Queue()

    async def send(self, raw: str) -> None:
        self.sent.append(json.loads(raw))

    async def recv(self) -> str:
        return await self.incoming.get()

    async def close(self) -> None:
        return None


def message(message_type: str, body: dict, request_id: str | None = None) -> str:
    defaults = {
        "server.welcome": {
            "session_id": VALID_MESSAGE_ID,
            "selected_protocol": "0.1",
            "server_version": "test",
            "heartbeat_interval_ms": 30_000,
            "maximum_message_bytes": 512 * 1024,
            "authenticated_principal": {"principal_id": "principal", "kind": "agent"},
        },
        "room.attached": {
            "room_id": "room",
            "member_id": "member",
            "principal_kind": "agent",
            "access_mode": "participant",
            "role": None,
            "membership_status": "enabled",
            "room_status": "active",
            "room_health": "healthy",
            "integrity_generation": 1,
            "room_head": VALID_ROOM_HEAD,
            "cursor": None,
            "frame_head": 0,
            "retained_floor": 0,
            "sync_token": "sync-token",
            "sync": {"kind": "projection_reset", "baseline_frame_head": 0, "reason": "test"},
            "pack": {"id": "worldstream.counter", "version": "0.1.0", "digest": VALID_DIGEST},
        },
        "projection.reset": {
            "room_id": "room",
            "member_id": "member",
            "room_head": VALID_ROOM_HEAD,
            "room_health": "healthy",
            "integrity_generation": 1,
            "baseline_frame_head": 0,
            "reset_reason": "test",
            "projection_schema": "test",
            "projection": VALID_PROJECTION,
            "projection_hash": VALID_DIGEST,
        },
        "observation.deliver": {
            "room_id": "room",
            "member_id": "member",
            "frame_seq": 0,
            "cause_room_seq": 0,
            "frame_kind": "transition",
            "observation_schema": "test",
            "observation": {},
            "frame_payload_hash": VALID_DIGEST,
        },
        "observation.acked": {"room_id": "room", "member_id": "member", "cursor": 0},
        "room.sync_acked": {"through_frame_head": 0},
        "action.accepted": {
            "room_id": "room",
            "member_id": "member",
            "action_id": "action",
            "transition_id": "transition",
            "admitted_at": "2026-01-01T00:00:00Z",
            "room_head": VALID_ROOM_HEAD,
            "duplicate": False,
        },
        "action.rejected": {
            "room_id": "room",
            "member_id": "member",
            "action_id": "action",
            "admitted_at": "2026-01-01T00:00:00Z",
            "code": "stale_head",
            "message": "stale",
            "current_room_seq": 0,
            "action_offers": [],
            "retryable_with_same_action_id": False,
            "may_submit_revised_action": True,
            "duplicate": False,
            "details": {},
        },
        "error": {"code": "internal", "message": "failed", "retryable": False},
    }
    merged = {**defaults.get(message_type, {}), **body}
    result = {
        "protocol": "0.1",
        "type": message_type,
        "message_id": VALID_MESSAGE_ID,
        "body": merged,
    }
    if request_id:
        result["request_id"] = request_id
    return json.dumps(result)


def test_strict_loader_rejects_duplicate_keys_and_floats() -> None:
    with pytest.raises(ValueError):
        _loads('{"protocol":"0.1","protocol":"0.1","type":"x","message_id":"m","body":{}}')
    with pytest.raises(ValueError):
        _loads('{"protocol":"0.1","type":"x","message_id":"m","body":{"n":1.5}}')


def test_action_payload_bound_is_checked_before_send() -> None:
    room = Room(Client("http://localhost", VALID_BEARER), "room", "member")
    with pytest.raises(ValueError):
        room._action_request("increment", "x" * (256 * 1024), None, 0)
    with pytest.raises(ValueError):
        room._action_request("increment", {"value": 1.5}, None, 0)
    with pytest.raises(ValueError):
        room._action_request("increment", {"value": 9_007_199_254_740_992}, None, 0)


def test_websocket_negotiates_bounded_worldstream_transport() -> None:
    fake = FakeWebSocket()
    captured: dict = {}

    async def ws_factory(*args, **kwargs):
        captured["url"] = args[0]
        captured.update(kwargs)
        return fake

    fake.incoming.put_nowait(message("server.welcome", {}))
    fake.incoming.put_nowait(message("room.attached", {}))

    async def run() -> None:
        await Room(
            Client("http://localhost", VALID_BEARER, ws_factory=ws_factory), "room", "member"
        ).connect()

    asyncio.run(run())
    assert captured["additional_headers"] == {"Authorization": f"Bearer {VALID_BEARER}"}
    assert captured["url"] == "ws://localhost/v1/stream"
    assert captured["subprotocols"] == (WS_SUBPROTOCOL,)
    assert captured["compression"] is None
    assert captured["max_size"] == MAX_MESSAGE_BYTES
    assert captured["max_queue"] == 16


def test_empty_stream_allows_next_retained_floor() -> None:
    raw = json.loads(message("room.attached", {"frame_head": 0, "retained_floor": 1}))
    _loads(json.dumps(raw))


def test_empty_stream_allows_ack_without_a_persisted_cursor() -> None:
    raw = json.loads(message("observation.acked", {"cursor": None}))
    _loads(json.dumps(raw))


def test_nested_head_and_projection_shapes_are_fail_closed() -> None:
    raw = json.loads(message("action.accepted", {"room_head": {}}))
    with pytest.raises(ProtocolError) as caught:
        _loads(json.dumps(raw))
    assert caught.value.code == "invalid_envelope"

    raw = json.loads(message("projection.reset", {"projection": {}}))
    with pytest.raises(ProtocolError) as caught:
        _loads(json.dumps(raw))
    assert caught.value.code == "invalid_envelope"


def test_protocol_errors_redact_bearer_values() -> None:
    token = VALID_BEARER
    error = ProtocolError("internal", f"failed for {token}", False, {"sync_token": token})
    assert token not in str(error)
    assert token not in repr(error.details)


def test_http_error_details_redact_bearer_values() -> None:
    error = _protocol_error_from_http(
        {
            "error": {
                "code": "forbidden",
                "message": f"Bearer {VALID_BEARER} is not authorized",
                "retryable": False,
                "details": {"authorization": VALID_BEARER},
            }
        }
    )
    assert VALID_BEARER not in str(error)
    assert VALID_BEARER not in repr(error.details)


@pytest.mark.parametrize("payload", [{}, "é" * 20_000])
def test_lost_action_reply_reuses_exact_identity(payload) -> None:
    fake = FakeWebSocket()
    client = Client("http://localhost", VALID_BEARER, ws_factory=lambda *_args, **_kwargs: fake)
    room = Room(client, "room", "member")
    room.websocket = fake
    room.attached = {"room_head": {"room_seq": 0}}
    room.live = True

    async def run() -> None:
        with pytest.raises(LostActionReply) as caught:
            await room.act("increment", payload, timeout=0.001)
        lost = caught.value
        fake.incoming.put_nowait(
            message(
                "action.accepted",
                {
                    "room_id": "room",
                    "member_id": "member",
                    "action_id": lost.request["body"]["action_id"],
                },
                lost.request["request_id"],
            )
        )
        await lost.retry()
        assert fake.sent[-1]["body"]["action_id"] == lost.request["body"]["action_id"]
        assert fake.sent[-1]["body"] == fake.sent[0]["body"]

    asyncio.run(run())


def test_action_reply_preserves_live_observation_push_for_events() -> None:
    fake = FakeWebSocket()
    room = Room(Client("http://localhost", VALID_BEARER), "room", "member")
    room.websocket = fake
    room.attached = {"room_head": {"room_seq": 0}, "frame_head": 0}
    room.live = True
    room._sync_complete = True

    async def run() -> None:
        async def send_with_live_push(raw: str) -> None:
            envelope = json.loads(raw)
            await FakeWebSocket.send(fake, raw)
            if envelope["type"] == "action.submit":
                fake.incoming.put_nowait(
                    message(
                        "action.accepted",
                        {
                            "action_id": envelope["body"]["action_id"],
                            "room_head": {**VALID_ROOM_HEAD, "room_seq": 1},
                        },
                        envelope["request_id"],
                    )
                )
                fake.incoming.put_nowait(
                    message(
                        "observation.deliver",
                        {"frame_seq": 1, "cause_room_seq": 1},
                    )
                )

        fake.send = send_with_live_push
        result = await room.act("increment", {}, action_id="action", expected_room_seq=0)
        assert result["action_id"] == "action"
        assert (await room.events().__anext__())["frame_seq"] == 1

    asyncio.run(run())


def test_protocol_error_is_typed() -> None:
    with pytest.raises(ProtocolError) as caught:
        _loads(json.dumps({"protocol": "0.2", "type": "x", "message_id": "m", "body": {}}))
    assert caught.value.code == "unsupported_protocol"


def test_capability_and_envelope_ids_are_canonical() -> None:
    with pytest.raises(ValueError):
        Client("http://localhost", "token")
    with pytest.raises(ProtocolError) as caught:
        _loads(message("room.sync_acked", {"through_frame_head": 0}).replace(VALID_MESSAGE_ID, "m"))
    assert caught.value.code == "invalid_envelope"


def test_known_body_schema_rejects_unknown_fields() -> None:
    raw = json.loads(message("room.sync_acked", {"through_frame_head": 0}))
    raw["body"]["unexpected"] = True
    with pytest.raises(ProtocolError) as caught:
        _loads(json.dumps(raw))
    assert caught.value.code == "invalid_envelope"


def test_sync_waits_for_server_barrier_and_drains_reset_and_frames() -> None:
    fake = FakeWebSocket()

    async def ws_factory(*_args, **_kwargs):
        return fake

    client = Client("http://localhost", VALID_BEARER, ws_factory=ws_factory)
    room = Room(client, "room", "member")

    async def run() -> None:
        fake.incoming.put_nowait(message("server.welcome", {}))
        fake.incoming.put_nowait(
            message(
                "room.attached",
                {
                    "room_id": "room",
                    "member_id": "member",
                    "frame_head": 4,
                    "sync_token": "token",
                    "sync": {
                        "kind": "projection_reset",
                        "baseline_frame_head": 4,
                        "reason": "retention_gap",
                    },
                },
            )
        )
        await room.connect()

        ack_saw_installed: list[bool] = []

        async def send_with_observation(raw: str) -> None:
            envelope = json.loads(raw)
            if envelope["type"] == "room.sync_ack":
                ack_saw_installed.append(room.last_projection_reset is not None)
            await FakeWebSocket.send(fake, raw)

        fake.send = send_with_observation
        fake.incoming.put_nowait(
            message(
                "projection.reset",
                {
                    "room_id": "room",
                    "member_id": "member",
                    "baseline_frame_head": 4,
                    "projection": VALID_PROJECTION,
                },
            )
        )
        fake.incoming.put_nowait(
            message(
                "observation.deliver",
                {"room_id": "room", "member_id": "member", "frame_seq": 5},
            )
        )
        fake.incoming.put_nowait(message("room.sync_acked", {"through_frame_head": 4}))
        delivered = await room.sync()
        assert [item for item in delivered if "frame_seq" in item] == []
        assert room.live is True
        assert fake.sent[-1]["type"] == "room.sync_ack"
        assert ack_saw_installed == [True]
        assert (await room.events().__anext__())["frame_seq"] == 5

    asyncio.run(run())


def test_reconnect_uses_last_acknowledged_cursor() -> None:
    fake = FakeWebSocket()

    async def ws_factory(*_args, **_kwargs):
        return fake

    client = Client("http://localhost", VALID_BEARER, ws_factory=ws_factory)
    room = Room(client, "room", "member", after_frame_seq=2)
    room.websocket = fake
    room.cursor = 7

    async def run() -> None:
        fake.incoming.put_nowait(message("server.welcome", {}))
        fake.incoming.put_nowait(
            message(
                "room.attached",
                {"room_id": "room", "member_id": "member"},
            )
        )
        await room.reconnect()
        attach = [sent for sent in fake.sent if sent["type"] == "room.attach"][-1]
        assert attach["body"]["after_frame_seq"] == 7

    asyncio.run(run())


def test_resync_reconnects_from_cursor_and_completes_the_new_barrier() -> None:
    fake = FakeWebSocket()

    async def ws_factory(*_args, **_kwargs):
        return fake

    room = Room(
        Client("http://localhost", VALID_BEARER, ws_factory=ws_factory),
        "room",
        "member",
    )
    room.websocket = fake
    room.cursor = 2
    room.attached = {"room_head": VALID_ROOM_HEAD}
    fake.incoming.put_nowait(message("server.welcome", {}))
    fake.incoming.put_nowait(
        message(
            "room.attached",
            {
                "cursor": 2,
                "frame_head": 3,
                "sync": {"kind": "retained_frames", "cursor_exclusive": 2, "through_frame_head": 3},
            },
        )
    )
    fake.incoming.put_nowait(message("observation.deliver", {"frame_seq": 3}))
    fake.incoming.put_nowait(message("room.sync_acked", {"through_frame_head": 3}))

    async def run() -> None:
        batch = await room.resync()
        assert [frame["frame_seq"] for frame in batch] == [3]
        assert room.live is True
        attach = [sent for sent in fake.sent if sent["type"] == "room.attach"][-1]
        assert attach["body"]["after_frame_seq"] == 2
        sync_ack = [sent for sent in fake.sent if sent["type"] == "room.sync_ack"][-1]
        assert sync_ack["body"]["through_frame_head"] == 3

    asyncio.run(run())


def test_reconnect_rejects_a_frame_replayed_at_or_before_the_acknowledged_cursor() -> None:
    fake = FakeWebSocket()
    room = Room(Client("http://localhost", VALID_BEARER), "room", "member")
    room.websocket = fake
    room.attached = {"room_id": "room", "member_id": "member", "frame_head": 7, "cursor": 7}
    room.cursor = 7
    room._sync_complete = True
    room.live = True
    room._last_delivered_frame_seq = 7
    fake.incoming.put_nowait(
        message("observation.deliver", {"room_id": "room", "member_id": "member", "frame_seq": 7})
    )

    async def run() -> None:
        with pytest.raises(ProtocolError) as caught:
            await room.events().__anext__()
        assert caught.value.code == "invalid_envelope"
        assert room.live is True

    asyncio.run(run())


def test_cursor_advances_only_after_server_acknowledgement() -> None:
    fake = FakeWebSocket()
    room = Room(Client("http://localhost", VALID_BEARER), "room", "member")
    room.websocket = fake
    room.attached = {"room_id": "room", "member_id": "member", "frame_head": 7, "cursor": 2}
    room.cursor = 2

    async def run() -> None:
        fake.incoming.put_nowait(message("error", {}))
        with pytest.raises(ProtocolError):
            await room.ack(7)
        assert room.cursor == 2

        fake.incoming.put_nowait(
            message("observation.acked", {"room_id": "room", "member_id": "member", "cursor": 7})
        )
        await room.ack(7)
        assert room.cursor == 7

    asyncio.run(run())


def test_membership_frames_cannot_cross_two_memberships() -> None:
    fake = FakeWebSocket()
    room = Room(Client("http://localhost", VALID_BEARER), "room", "member-a")
    room.websocket = fake
    fake.incoming.put_nowait(
        message(
            "observation.deliver",
            {"room_id": "room", "member_id": "member-b", "frame_seq": 1},
        )
    )

    async def run() -> None:
        with pytest.raises(ProtocolError) as caught:
            await room._receive()
        assert caught.value.code == "forbidden"

    asyncio.run(run())


def test_duplicate_and_stale_action_outcomes_are_returned_without_reinterpretation() -> None:
    fake = FakeWebSocket()
    room = Room(Client("http://localhost", VALID_BEARER), "room", "member")
    room.websocket = fake
    room.attached = {"room_head": {"room_seq": 4}}

    async def run() -> None:
        request = room._action_request("increment", {}, "stable-action", 4)
        fake.incoming.put_nowait(
            message(
                "action.accepted",
                {
                    "room_id": "room",
                    "member_id": "member",
                    "action_id": "stable-action",
                    "duplicate": True,
                },
                request["request_id"],
            )
        )
        duplicate = await room.retry_action(request)
        assert duplicate["duplicate"] is True

        stale_request = room._action_request("increment", {}, "stale-action", 1)
        fake.incoming.put_nowait(
            message(
                "action.rejected",
                {
                    "room_id": "room",
                    "member_id": "member",
                    "action_id": "stale-action",
                    "code": "stale_head",
                },
                stale_request["request_id"],
            )
        )
        stale = await room.retry_action(stale_request)
        assert stale["code"] == "stale_head"

        conflict_request = room._action_request("increment", {}, "conflict-action", 4)
        fake.incoming.put_nowait(
            message(
                "error",
                {
                    "code": "idempotency_conflict",
                    "message": "action identity conflicts",
                    "retryable": False,
                },
                conflict_request["request_id"],
            )
        )
        with pytest.raises(ProtocolError) as caught:
            await room.retry_action(conflict_request)
        assert caught.value.code == "idempotency_conflict"

    asyncio.run(run())


def test_same_action_identity_rejects_a_changed_canonical_body_before_send() -> None:
    fake = FakeWebSocket()
    room = Room(Client("http://localhost", VALID_BEARER), "room", "member")
    room.websocket = fake
    room.attached = {"room_head": VALID_ROOM_HEAD}
    request = room._action_request("increment", {"delta": 1}, "stable-action", 0)
    room._remember_action(request)
    changed = copy.deepcopy(request)
    changed["body"]["payload"] = {"delta": 2}

    async def run() -> None:
        with pytest.raises(ProtocolError) as caught:
            await room.retry_action(changed)
        assert caught.value.code == "idempotency_conflict"
        assert fake.sent == []

    asyncio.run(run())


def test_accepted_head_advances_locally_but_a_stale_duplicate_never_rewinds_it() -> None:
    fake = FakeWebSocket()
    room = Room(Client("http://localhost", VALID_BEARER), "room", "member")
    room.websocket = fake
    room.attached = {"room_head": VALID_ROOM_HEAD}
    request = room._action_request("increment", {}, "stable-action", 0)
    advanced_head = {**VALID_ROOM_HEAD, "room_seq": 2, "genesis_or_transition_hash": "next"}
    old_head = {**VALID_ROOM_HEAD, "room_seq": 1}

    async def run() -> None:
        fake.incoming.put_nowait(
            message(
                "action.accepted",
                {"action_id": "stable-action", "room_head": advanced_head},
                request["request_id"],
            )
        )
        await room.retry_action(request)
        assert room.attached["room_head"]["room_seq"] == 2

        fake.incoming.put_nowait(
            message(
                "action.accepted",
                {"action_id": "stable-action", "duplicate": True, "room_head": old_head},
                request["request_id"],
            )
        )
        duplicate = await room.retry_action(request)
        assert duplicate["duplicate"] is True
        assert room.attached["room_head"]["room_seq"] == 2

    asyncio.run(run())


def test_stale_result_requires_resync_before_a_new_action_identity() -> None:
    fake = FakeWebSocket()
    room = Room(Client("http://localhost", VALID_BEARER), "room", "member")
    room.websocket = fake
    room.attached = {"room_head": VALID_ROOM_HEAD}
    request = room._action_request("increment", {}, "stale-action", 0)

    async def run() -> None:
        fake.incoming.put_nowait(
            message(
                "action.rejected",
                {"action_id": "stale-action", "current_room_seq": 2, "code": "stale_head"},
                request["request_id"],
            )
        )
        result = await room.retry_action(request)
        assert result["code"] == "stale_head"
        with pytest.raises(ProtocolError) as caught:
            await room.act("increment", {}, action_id="new-action")
        assert caught.value.code == "resync_required"

    asyncio.run(run())


def test_sync_install_is_atomic_when_reset_or_retained_range_is_invalid() -> None:
    fake = FakeWebSocket()
    room = Room(Client("http://localhost", VALID_BEARER), "room", "member")
    room.websocket = fake
    room.attached = {
        "room_id": "room",
        "member_id": "member",
        "frame_head": 2,
        "cursor": None,
        "sync_token": "token",
        "sync": {"kind": "projection_reset", "baseline_frame_head": 2, "reason": "test"},
    }
    room._sync_token = "token"

    async def run() -> None:
        fake.incoming.put_nowait(
            message(
                "projection.reset",
                {
                    "room_id": "room",
                    "member_id": "member",
                    "baseline_frame_head": 1,
                    "projection": VALID_PROJECTION,
                },
            )
        )
        with pytest.raises(ProtocolError) as caught:
            await room.sync()
        assert caught.value.code == "sync_barrier_mismatch"
        assert room.last_projection_reset is None
        assert room.last_sync_frames == []
        assert room.live is False
        assert len(room._pending) == 1

    asyncio.run(run())


def test_projection_reset_must_match_the_captured_complete_head() -> None:
    fake = FakeWebSocket()
    room = Room(Client("http://localhost", VALID_BEARER), "room", "member")
    room.websocket = fake
    room.attached = {
        "room_id": "room",
        "member_id": "member",
        "frame_head": 2,
        "cursor": None,
        "sync_token": "token",
        "sync": {"kind": "projection_reset", "baseline_frame_head": 2, "reason": "test"},
        "room_head": VALID_ROOM_HEAD,
    }
    room._sync_token = "token"
    reset_head = {**VALID_ROOM_HEAD, "room_seq": 1}
    fake.incoming.put_nowait(
        message(
            "projection.reset",
            {"baseline_frame_head": 2, "room_head": reset_head, "projection": VALID_PROJECTION},
        )
    )

    async def run() -> None:
        with pytest.raises(ProtocolError) as caught:
            await room.sync()
        assert caught.value.code == "sync_barrier_mismatch"
        assert room.last_projection_reset is None
        assert room.live is False

    asyncio.run(run())


def test_retained_sync_installs_captured_range_and_leaves_no_cursor_side_effect() -> None:
    fake = FakeWebSocket()
    room = Room(Client("http://localhost", VALID_BEARER), "room", "member", after_frame_seq=2)
    room.websocket = fake
    room.attached = {
        "room_id": "room",
        "member_id": "member",
        "frame_head": 4,
        "cursor": 2,
        "sync_token": "session-token",
        "sync": {"kind": "retained_frames", "cursor_exclusive": 2, "through_frame_head": 4},
    }
    room._sync_token = "session-token"

    async def run() -> None:
        for frame_seq in (3, 4, 5):
            fake.incoming.put_nowait(
                message(
                    "observation.deliver",
                    {"room_id": "room", "member_id": "member", "frame_seq": frame_seq},
                )
            )
        fake.incoming.put_nowait(message("room.sync_acked", {"through_frame_head": 4}))
        delivered = await room.sync()
        assert [item["frame_seq"] for item in delivered] == [3, 4]
        assert room.cursor == 2
        assert room.live is True
        assert fake.sent[-1]["body"]["sync_token"] == "session-token"
        assert (await room.events().__anext__())["frame_seq"] == 5
        with pytest.raises(RuntimeError):
            await room.sync()

    asyncio.run(run())


def test_slow_consumer_closure_is_resumable_from_cursor() -> None:
    fake = FakeWebSocket()

    async def ws_factory(*_args, **_kwargs):
        return fake

    room = Room(Client("http://localhost", VALID_BEARER, ws_factory=ws_factory), "room", "member")
    room.websocket = fake
    room.attached = {"room_id": "room", "member_id": "member", "frame_head": 4, "cursor": 2}
    room.cursor = 2
    room._sync_complete = True
    room.live = True
    fake.incoming.put_nowait(message("error", {"code": "slow_consumer", "retryable": True}))

    async def run() -> None:
        with pytest.raises(ProtocolError) as caught:
            await room.events().__anext__()
        assert caught.value.code == "slow_consumer"
        assert room.live is False
        assert room.websocket is None
        room.websocket = fake
        fake.incoming.put_nowait(message("server.welcome", {}))
        fake.incoming.put_nowait(
            message("room.attached", {"room_id": "room", "member_id": "member"})
        )
        await room.reconnect()
        attach = [sent for sent in fake.sent if sent["type"] == "room.attach"][-1]
        assert attach["body"]["after_frame_seq"] == 2

    asyncio.run(run())


def test_action_result_with_wrong_identity_is_rejected_fail_closed() -> None:
    fake = FakeWebSocket()
    room = Room(Client("http://localhost", VALID_BEARER), "room", "member")
    room.websocket = fake
    room.attached = {"room_head": {"room_seq": 0}}

    async def run() -> None:
        request = room._action_request("increment", {}, "known-action", 0)
        fake.incoming.put_nowait(
            message(
                "action.accepted",
                {"room_id": "room", "member_id": "member", "action_id": "other-action"},
                request["request_id"],
            )
        )
        with pytest.raises(ProtocolError) as caught:
            await room.retry_action(request)
        assert caught.value.code == "invalid_envelope"

    asyncio.run(run())


def test_replay_is_read_only_and_validates_the_authorized_response_shape(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    client = Client("http://localhost", VALID_BEARER)
    paths: list[str] = []

    def get_json(path: str) -> dict:
        paths.append(path)
        return {
            "room_id": "room",
            "pack": {"id": "worldstream.counter", "version": "0.1.0", "digest": VALID_DIGEST},
            "requested_room_seq": 3,
            "room_head": {**VALID_ROOM_HEAD, "room_seq": 3},
            "projection": VALID_PROJECTION,
            "projection_hash": VALID_DIGEST,
            "verification": "verified",
            "room_health": "healthy",
            "integrity_generation": 1,
        }

    monkeypatch.setattr(Client, "_get_json", staticmethod(get_json))

    async def run() -> None:
        replay = await client.replay("room", 3)
        assert replay["requested_room_seq"] == 3
        assert paths == ["/v1/rooms/room/replay?at_room_seq=3"]
        with pytest.raises(ValueError):
            await client.replay("room", -1)
        with pytest.raises(ProtocolError):
            Client._validate_http_response(
                "/v1/rooms/room/replay?at_room_seq=3",
                {**replay, "member_id": "member"},
            )
        bad_replay = {**replay, "requested_room_seq": 4}
        monkeypatch.setattr(Client, "_get_json", staticmethod(lambda _path: bad_replay))
        with pytest.raises(ProtocolError) as caught:
            await client.replay("room", 3)
        assert caught.value.code == "invalid_envelope"
        bad_verification = {**replay, "verification": "unverified"}
        monkeypatch.setattr(Client, "_get_json", staticmethod(lambda _path: bad_verification))
        with pytest.raises(ProtocolError) as caught:
            await client.replay("room", 3)
        assert caught.value.code == "replay_unverified"

    asyncio.run(run())

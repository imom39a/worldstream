from __future__ import annotations

import asyncio
import json

import pytest

from worldstream_sdk import Client, LostRunnerReply, ProtocolError, Runner
from worldstream_sdk.client import WS_SUBPROTOCOL, _loads

BEARER = "wsb1:" + "a" * 64
MESSAGE_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
SESSION_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAW"


def envelope(message_type: str, body: dict, request_id: str | None = None) -> str:
    value = {
        "protocol": "0.1",
        "type": message_type,
        "message_id": MESSAGE_ID,
        "body": body,
    }
    if request_id is not None:
        value["request_id"] = request_id
    return json.dumps(value, separators=(",", ":"))


class RunnerFakeWebSocket:
    def __init__(self) -> None:
        self.incoming: asyncio.Queue[str] = asyncio.Queue()
        self.sent: list[dict] = []
        self.drop_next_complete = False
        self.error_next = False
        self.url: str | None = None

    async def send(self, raw: str) -> None:
        value = json.loads(raw)
        self.sent.append(value)
        request_id = value.get("request_id")
        if value["type"] == "client.hello":
            self.incoming.put_nowait(
                envelope(
                    "server.welcome",
                    {
                        "session_id": SESSION_ID,
                        "selected_protocol": "0.1",
                        "server_version": "0.1.0",
                        "heartbeat_interval_ms": 1000,
                        "maximum_message_bytes": 512 * 1024,
                        "authenticated_principal": {"principal_id": "runner", "kind": "agent"},
                    },
                )
            )
        elif value["type"] == "runner.hello":
            self.incoming.put_nowait(
                envelope("runner.ready", {"runner_id": "runner-a"}, request_id)
            )
        elif value["type"] == "activation.offer":
            self.incoming.put_nowait(
                envelope(
                    "activation.offers",
                    {
                        "operation_id": value["body"]["operation_id"],
                        "runner_id": "runner-a",
                        "offers": [],
                    },
                    request_id,
                )
            )
        elif value["type"] in {
            "activation.claim",
            "activation.renew",
            "activation.release",
            "activation.complete",
        }:
            if self.error_next:
                self.error_next = False
                self.incoming.put_nowait(
                    envelope(
                        "error",
                        {
                            "code": "forbidden",
                            "message": "bearer wsb1:" + "a" * 64 + " is not authorized",
                            "retryable": False,
                            "details": {"authorization": "wsb1:" + "a" * 64},
                        },
                        request_id,
                    )
                )
                return
            if value["type"] == "activation.complete" and self.drop_next_complete:
                self.drop_next_complete = False
                return
            response_type = {
                "activation.claim": "activation.claimed",
                "activation.renew": "activation.renewed",
                "activation.release": "activation.released",
                "activation.complete": "activation.completed",
            }[value["type"]]
            body = {
                "operation_id": value["body"].get("operation_id", value["body"].get("claim_id")),
                "activation_id": value["body"].get("activation_id"),
                "claim_id": value["body"].get("claim_id"),
                "runner_id": "runner-a",
                "code": {
                    "activation.claim": "granted",
                    "activation.renew": "renewed",
                    "activation.release": "released",
                    "activation.complete": "completed",
                }[value["type"]],
                "state": {
                    "activation.claim": "leased",
                    "activation.renew": "leased",
                    "activation.release": "pending",
                    "activation.complete": "completed",
                }[value["type"]],
                "lease_generation": 1,
                "context_hash": None,
                "context": None,
            }
            self.incoming.put_nowait(envelope(response_type, body, request_id))

    async def recv(self) -> str:
        return await self.incoming.get()

    async def close(self) -> None:
        return None


def make_runner(fake: RunnerFakeWebSocket) -> Runner:
    async def ws_factory(*_args, **kwargs):
        fake.url = _args[0]
        assert kwargs["subprotocols"] == (WS_SUBPROTOCOL,)
        assert kwargs["max_queue"] == 16
        return fake

    return Runner(
        Client("http://localhost", BEARER, ws_factory=ws_factory),
        "runner-a",
        4,
        ["worldstream.heist"],
    )


def test_runner_handshake_and_offer_poll_preserve_correlated_request_ids() -> None:
    fake = RunnerFakeWebSocket()
    runner = make_runner(fake)

    async def run() -> None:
        ready = await runner.connect()
        offers = await runner.poll_offers("room-a", "member-a", "offer-op")
        assert ready == {"runner_id": "runner-a"}
        assert offers == {"operation_id": "offer-op", "runner_id": "runner-a", "offers": []}

    asyncio.run(run())
    assert fake.url == "ws://localhost/v1/runner/stream"
    assert [item["type"] for item in fake.sent] == [
        "client.hello",
        "runner.hello",
        "activation.offer",
    ]
    assert fake.sent[0]["body"]["mode"] == "runner"
    assert fake.sent[1]["request_id"]
    assert fake.sent[2]["request_id"]
    assert fake.sent[2]["body"] == {
        "operation_id": "offer-op",
        "runner_id": "runner-a",
        "room_id": "room-a",
        "member_id": "member-a",
    }


def test_runner_handshake_can_declare_exact_supported_pack_revisions() -> None:
    fake = RunnerFakeWebSocket()
    revision = {
        "id": "worldstream.agent-heist",
        "version": "0.2.0",
        "digest": "blake3:" + "b" * 64,
    }
    runner = Runner(
        Client("http://localhost", BEARER, ws_factory=lambda *_args, **_kwargs: None),
        "runner-a",
        4,
        ["worldstream.agent-heist"],
        [revision],
    )

    async def ws_factory(*_args, **_kwargs):
        return fake

    runner.client.ws_factory = ws_factory
    asyncio.run(runner.connect())
    assert fake.sent[1]["body"]["supported_pack_revisions"] == [revision]


def test_runner_rejects_duplicate_or_malformed_exact_pack_revisions() -> None:
    client = Client("http://localhost", BEARER)
    revision = {
        "id": "worldstream.agent-heist",
        "version": "0.2.0",
        "digest": "blake3:" + "b" * 64,
    }
    with pytest.raises(ProtocolError):
        Runner(client, "runner-a", 1, ["worldstream.agent-heist"], [revision, revision])
    with pytest.raises(ProtocolError):
        Runner(client, "runner-a", 1, ["worldstream.agent-heist"], [{"id": "missing"}])
    with pytest.raises(ProtocolError):
        Runner(
            client,
            "runner-a",
            1,
            ["worldstream.agent-heist"],
            [{**revision, "digest": "blake3:not-a-digest"}],
        )
    with pytest.raises(ProtocolError):
        Runner(
            client,
            "runner-a",
            1,
            ["worldstream.agent-heist"],
            [{**revision, "version": "v" * 65}],
        )


def test_runner_uses_dedicated_control_stream_url() -> None:
    client = Client("https://example.test/prefix", BEARER)
    assert client.ws_url() == "wss://example.test/prefix/v1/stream"
    assert client.runner_ws_url() == "wss://example.test/prefix/v1/runner/stream"


def test_runner_validation_is_strict_and_bounded() -> None:
    with pytest.raises(ProtocolError):
        Runner(Client("http://localhost", BEARER), "runner-a", 0, ["pack"])
    with pytest.raises(ProtocolError):
        Runner(Client("http://localhost", BEARER), "runner-a", 1, ["pack", "pack"])
    with pytest.raises(ProtocolError):
        _loads(
            envelope(
                "activation.offers",
                {"operation_id": "op", "runner_id": "runner-a", "offers": [{"bad": True}]},
            )
        )


def test_runner_lease_operations_use_wire_shapes_and_redact_protocol_errors() -> None:
    fake = RunnerFakeWebSocket()
    runner = make_runner(fake)
    secret = BEARER

    async def run() -> None:
        await runner.connect()
        claimed = await runner.claim("activation-a", 5000, "claim-a")
        renewed = await runner.renew("activation-a", "claim-a", 1, 6000, "renew-op")
        released = await runner.release("activation-a", "claim-a", 1, "release-op")
        completed = await runner.complete("activation-a", "claim-a", 1, "success", "complete-op")
        assert claimed["code"] == "granted"
        assert renewed["code"] == "renewed"
        assert released["code"] == "released"
        assert completed["code"] == "completed"
        assert [item["type"] for item in fake.sent[-4:]] == [
            "activation.claim",
            "activation.renew",
            "activation.release",
            "activation.complete",
        ]
        assert fake.sent[-4]["body"] == {
            "activation_id": "activation-a",
            "runner_id": "runner-a",
            "claim_id": "claim-a",
            "requested_lease_ms": 5000,
        }
        assert fake.sent[-1]["body"]["disposition"] == "success"

        fake.error_next = True
        fake.sent.clear()
        with pytest.raises(ProtocolError) as caught:
            await runner.release("activation-a", "claim-a", 1, "forbidden-op")
        assert secret not in str(caught.value)
        assert secret not in repr(caught.value.details)

    asyncio.run(run())


def test_lost_runner_reply_retries_exact_idempotency_identity() -> None:
    fake = RunnerFakeWebSocket()
    runner = make_runner(fake)

    async def run() -> None:
        await runner.connect()
        fake.drop_next_complete = True
        with pytest.raises(LostRunnerReply) as caught:
            await runner.complete("activation-a", "claim-a", 1, "success", "stable-op", 0.001)
        lost = caught.value
        assert lost.request["body"]["operation_id"] == "stable-op"
        fake.incoming.put_nowait(
            envelope(
                "activation.completed",
                {
                    "operation_id": "stable-op",
                    "activation_id": "activation-a",
                    "claim_id": "claim-a",
                    "runner_id": "runner-a",
                    "code": "completed",
                    "state": "completed",
                    "lease_generation": 1,
                    "context_hash": None,
                    "context": None,
                },
                lost.request["request_id"],
            )
        )
        await lost.retry()
        assert fake.sent[-1]["request_id"] == lost.request["request_id"]
        assert fake.sent[-1]["body"] == lost.request["body"]

    asyncio.run(run())

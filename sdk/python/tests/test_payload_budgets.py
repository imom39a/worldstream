import asyncio
import copy
import json

import pytest

from worldstream_sdk import (
    PAYLOAD_BUDGET_V1_ID,
    Client,
    canonical_payload_bytes,
    check_declared_artifact_reference,
    check_fresh_payload,
)


def creation():
    return {
        "pack": {"id": "counter", "version": "1", "digest": "blake3:" + "00" * 32},
        "configuration": {},
        "members": [
            {
                "principal_id": "principal",
                "principal_kind": "human",
                "role": "counter",
                "access_mode": "participant",
            }
        ],
        "idempotency_key": "same-create",
    }


def test_create_selector_preserves_legacy_bytes_and_creation_identity(monkeypatch):
    sent = []

    def post(_self, path, body):
        assert path == "/v1/rooms"
        sent.append(json.dumps(body, ensure_ascii=False, separators=(",", ":")))
        return {"room_id": "created"}

    monkeypatch.setattr(Client, "_post_json", post)
    client = Client("http://localhost", "wsb1:" + "ab" * 32)

    async def run():
        original = creation()
        assert await client.create_room(original) == {"room_id": "created"}
        explicit = copy.deepcopy(original)
        explicit["canonical_history_format"] = "worldstream/transition/v1"
        await client.create_room(explicit)
        assert sent[0] == sent[1]
        assert explicit["canonical_history_format"] == "worldstream/transition/v1"
        compact = copy.deepcopy(original)
        compact["canonical_history_format"] = "worldstream/transition/v2"
        await client.create_room(compact)
        assert json.loads(sent[-1])["canonical_history_format"] == "worldstream/transition/v2"
        assert json.loads(sent[-1])["idempotency_key"] == original["idempotency_key"]

    asyncio.run(run())


@pytest.mark.parametrize("selector", [None, "v2", "worldstream/transition/v3", 0, True, [], {}])
def test_client_rejects_closed_selector_before_http(monkeypatch, selector):
    def fail(*_args):
        pytest.fail("invalid selector reached HTTP")

    monkeypatch.setattr(Client, "_post_json", fail)
    request = creation()
    request["canonical_history_format"] = selector
    with pytest.raises(ValueError):
        asyncio.run(Client("http://localhost", "wsb1:" + "ab" * 32).create_room(request))


def test_create_extra_field_is_rejected():
    request = creation()
    request["unexpected"] = True
    with pytest.raises(ValueError):
        Client._validate_create_request(request)


@pytest.mark.parametrize("delta", [-1, 0, 1])
def test_fresh_utf8_limit_is_inclusive_and_separate_from_wire(delta):
    payload = "é" * 16_382 + "x" * (delta + 2)
    assert len(canonical_payload_bytes(payload)) == 32_768 + delta
    if delta <= 0:
        assert (
            check_fresh_payload(payload, kind="action_payload", policy_id=PAYLOAD_BUDGET_V1_ID)
            == 32_768 + delta
        )
    else:
        with pytest.raises(ValueError):
            check_fresh_payload(payload, kind="action_payload", policy_id=PAYLOAD_BUDGET_V1_ID)


def test_canonical_order_and_aggregate_accounting():
    assert (
        canonical_payload_bytes({"😀": False, "2": 1, "10": "é", "\uffff": None})
        == '{"10":"é","2":1,"\uffff":null,"😀":false}'.encode()
    )
    items = ["x" * 5000] * 60
    for item in items:
        check_fresh_payload(item, kind="domain_event_item", policy_id=PAYLOAD_BUDGET_V1_ID)
    with pytest.raises(ValueError):
        check_fresh_payload(items, kind="domain_events_array", policy_id=PAYLOAD_BUDGET_V1_ID)


def test_policy_and_artifact_classification_are_explicit():
    for kind, policy in [
        ("unknown", PAYLOAD_BUDGET_V1_ID),
        ("action_payload", "future"),
        ("artifact_reference", PAYLOAD_BUDGET_V1_ID),
    ]:
        with pytest.raises(ValueError):
            check_fresh_payload({}, kind=kind, policy_id=policy)
    # A normal JSON object is never scanned for reference-shaped fields.
    assert (
        check_fresh_payload(
            {"artifact": "x" * 3000}, kind="action_payload", policy_id=PAYLOAD_BUDGET_V1_ID
        )
        > 2048
    )
    with pytest.raises(ValueError):
        check_declared_artifact_reference({}, schema_id="", policy_id=PAYLOAD_BUDGET_V1_ID)
    assert (
        check_declared_artifact_reference(
            {}, schema_id="app/reference/exact-v1", policy_id=PAYLOAD_BUDGET_V1_ID
        )
        == 2
    )

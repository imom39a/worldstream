"""Small async client for the public WorldStream Room protocol."""

from __future__ import annotations

import asyncio
import copy
import json
import re
import secrets
import time
import urllib.error
import urllib.request
from collections.abc import AsyncIterator, Callable
from dataclasses import dataclass
from typing import Any
from urllib.parse import urlsplit

import websockets
from websockets.exceptions import ConnectionClosed, WebSocketException

PROTOCOL = "0.1"
MAX_MESSAGE_BYTES = 512 * 1024
MAX_ACTION_PAYLOAD_BYTES = 256 * 1024
MAX_ACTION_TYPE_BYTES = 256
_MAX_SAFE_INTEGER = 9_007_199_254_740_991
_ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
_ULID_RE = re.compile(r"[0-7][0-9A-HJKMNP-TV-Z]{25}\Z")
_BEARER_RE = re.compile(r"wsb1:[0-9a-f]{64}\Z")
_BEARER_IN_TEXT_RE = re.compile(r"(?i)(?:bearer\s+)?wsb1:[0-9a-f]{64}")
WS_SUBPROTOCOL = "worldstream.json.v0.1"
_REQUIRED_CAPABILITIES = ("cursor_ack", "projection_reset")
_RUNNER_MAX_CONCURRENT_ACTIVATIONS = 64
_RUNNER_MAX_SUPPORTED_PACKS = 64
_RUNNER_MAX_OFFERS = 1024
_RUNNER_MAX_LEASE_MS = 86_400_000
_RUNNER_MAX_PENDING_MESSAGES = 256
_ACTIVATION_RESULT_CODES = frozenset(
    {
        "granted",
        "renewed",
        "released",
        "completed",
        "not_available",
        "expired",
        "cancelled",
        "fenced",
        "stale_lease",
        "idempotency_conflict",
        "result_retired",
    }
)
_ACTIVATION_STATES = frozenset({"pending", "leased", "completed", "expired", "cancelled"})
_KNOWN_MESSAGE_TYPES = frozenset(
    {
        "client.hello",
        "room.attach",
        "room.sync_ack",
        "observation.ack",
        "action.submit",
        "server.welcome",
        "room.attached",
        "projection.reset",
        "observation.deliver",
        "observation.acked",
        "room.sync_acked",
        "action.accepted",
        "action.rejected",
        "error",
        "server.ping",
        "client.pong",
        "runner.hello",
        "runner.ready",
        "activation.offer",
        "activation.offers",
        "activation.claim",
        "activation.claimed",
        "activation.renew",
        "activation.renewed",
        "activation.release",
        "activation.released",
        "activation.complete",
        "activation.completed",
    }
)
_BODY_FIELDS: dict[str, tuple[frozenset[str], frozenset[str]]] = {
    "client.hello": (
        frozenset(("client_name", "client_version", "mode", "supported_protocols", "capabilities")),
        frozenset(("client_name", "client_version", "mode", "supported_protocols", "capabilities")),
    ),
    "room.attach": (
        frozenset(("room_id", "member_id", "after_frame_seq")),
        frozenset(("room_id", "member_id", "after_frame_seq")),
    ),
    "room.sync_ack": (
        frozenset(("room_id", "member_id", "through_frame_head", "sync_token")),
        frozenset(("room_id", "member_id", "through_frame_head", "sync_token")),
    ),
    "observation.ack": (
        frozenset(("room_id", "member_id", "through_frame_seq")),
        frozenset(("room_id", "member_id", "through_frame_seq")),
    ),
    "action.submit": (
        frozenset(
            ("room_id", "member_id", "action_id", "based_on_room_seq", "action_type", "payload")
        ),
        frozenset(
            ("room_id", "member_id", "action_id", "based_on_room_seq", "action_type", "payload")
        ),
    ),
    "server.welcome": (
        frozenset(
            (
                "session_id",
                "selected_protocol",
                "server_version",
                "heartbeat_interval_ms",
                "maximum_message_bytes",
                "authenticated_principal",
            )
        ),
        frozenset(
            (
                "session_id",
                "selected_protocol",
                "server_version",
                "heartbeat_interval_ms",
                "maximum_message_bytes",
                "authenticated_principal",
            )
        ),
    ),
    "room.attached": (
        frozenset(
            (
                "room_id",
                "member_id",
                "principal_kind",
                "access_mode",
                "role",
                "membership_status",
                "room_status",
                "room_health",
                "integrity_generation",
                "room_head",
                "cursor",
                "frame_head",
                "retained_floor",
                "sync_token",
                "sync",
                "pack",
            )
        ),
        frozenset(
            (
                "room_id",
                "member_id",
                "principal_kind",
                "access_mode",
                "role",
                "membership_status",
                "room_status",
                "room_health",
                "integrity_generation",
                "room_head",
                "cursor",
                "frame_head",
                "retained_floor",
                "sync_token",
                "sync",
                "pack",
            )
        ),
    ),
    "projection.reset": (
        frozenset(
            (
                "room_id",
                "member_id",
                "room_head",
                "room_health",
                "integrity_generation",
                "baseline_frame_head",
                "reset_reason",
                "projection_schema",
                "projection",
                "projection_hash",
            )
        ),
        frozenset(
            (
                "room_id",
                "member_id",
                "room_head",
                "room_health",
                "integrity_generation",
                "baseline_frame_head",
                "reset_reason",
                "projection_schema",
                "projection",
                "projection_hash",
            )
        ),
    ),
    "observation.deliver": (
        frozenset(
            (
                "room_id",
                "member_id",
                "frame_seq",
                "cause_room_seq",
                "frame_kind",
                "observation_schema",
                "observation",
                "frame_payload_hash",
            )
        ),
        frozenset(
            (
                "room_id",
                "member_id",
                "frame_seq",
                "cause_room_seq",
                "frame_kind",
                "observation_schema",
                "observation",
                "frame_payload_hash",
            )
        ),
    ),
    "observation.acked": (
        frozenset(("room_id", "member_id", "cursor")),
        frozenset(("room_id", "member_id", "cursor")),
    ),
    "room.sync_acked": (
        frozenset(("through_frame_head",)),
        frozenset(("through_frame_head",)),
    ),
    "action.accepted": (
        frozenset(
            (
                "room_id",
                "member_id",
                "action_id",
                "transition_id",
                "admitted_at",
                "room_head",
                "duplicate",
            )
        ),
        frozenset(
            (
                "room_id",
                "member_id",
                "action_id",
                "transition_id",
                "admitted_at",
                "room_head",
                "duplicate",
            )
        ),
    ),
    "action.rejected": (
        frozenset(
            (
                "room_id",
                "member_id",
                "action_id",
                "admitted_at",
                "code",
                "message",
                "current_room_seq",
                "action_offers",
                "retryable_with_same_action_id",
                "may_submit_revised_action",
                "duplicate",
                "details",
            )
        ),
        frozenset(
            (
                "room_id",
                "member_id",
                "action_id",
                "admitted_at",
                "code",
                "message",
                "current_room_seq",
                "action_offers",
                "retryable_with_same_action_id",
                "may_submit_revised_action",
                "duplicate",
                "details",
            )
        ),
    ),
    "error": (
        frozenset(("code", "message", "retryable")),
        frozenset(("code", "message", "retryable", "details")),
    ),
    "server.ping": (frozenset(), frozenset()),
    "client.pong": (frozenset(), frozenset()),
    "runner.hello": (
        frozenset(("runner_id", "maximum_concurrent_activations", "supported_pack_ids")),
        frozenset(
            (
                "runner_id",
                "maximum_concurrent_activations",
                "supported_pack_ids",
                "supported_pack_revisions",
            )
        ),
    ),
    "runner.ready": (frozenset(("runner_id",)), frozenset(("runner_id",))),
    "activation.offer": (
        frozenset(("operation_id", "runner_id", "room_id", "member_id")),
        frozenset(("operation_id", "runner_id", "room_id", "member_id")),
    ),
    "activation.offers": (
        frozenset(("operation_id", "runner_id", "offers")),
        frozenset(("operation_id", "runner_id", "offers")),
    ),
    "activation.claim": (
        frozenset(("activation_id", "runner_id", "claim_id", "requested_lease_ms")),
        frozenset(("activation_id", "runner_id", "claim_id", "requested_lease_ms")),
    ),
    "activation.claimed": (
        frozenset(
            (
                "operation_id",
                "activation_id",
                "claim_id",
                "runner_id",
                "code",
                "state",
                "lease_generation",
                "context_hash",
                "context",
            )
        ),
        frozenset(
            (
                "operation_id",
                "activation_id",
                "claim_id",
                "runner_id",
                "code",
                "state",
                "lease_generation",
                "context_hash",
                "context",
            )
        ),
    ),
}

for _activation_request_type, _activation_response_type in (
    ("activation.renew", "activation.renewed"),
    ("activation.release", "activation.released"),
    ("activation.complete", "activation.completed"),
):
    _BODY_FIELDS[_activation_request_type] = (
        frozenset(("activation_id", "runner_id", "claim_id", "operation_id", "lease_generation")),
        frozenset(
            (
                "activation_id",
                "runner_id",
                "claim_id",
                "operation_id",
                "lease_generation",
                "requested_lease_ms",
                "disposition",
            )
        ),
    )
    _BODY_FIELDS[_activation_response_type] = _BODY_FIELDS["activation.claimed"]


def _validate_ulid(value: Any, label: str) -> str:
    if not isinstance(value, str) or _ULID_RE.fullmatch(value) is None:
        raise ProtocolError("invalid_envelope", f"{label} is not a canonical ULID", False)
    return value


def _validate_nonnegative_integer(value: Any, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise ProtocolError("invalid_payload", f"{label} is invalid", False)
    return value


def _validate_canonical_value(value: Any, label: str = "value") -> None:
    """Reject values that cannot participate in a canonical JSON request."""

    if value is None or isinstance(value, (bool, str)):
        return
    if type(value) is int:
        if not -_MAX_SAFE_INTEGER <= value <= _MAX_SAFE_INTEGER:
            raise ValueError(f"{label} contains an integer outside the canonical safe range")
        return
    if isinstance(value, float):
        raise ValueError(f"{label} contains a floating-point value")  # noqa: TRY004
    if isinstance(value, list):
        for index, item in enumerate(value):
            _validate_canonical_value(item, f"{label}[{index}]")
        return
    if isinstance(value, dict):
        for key, item in value.items():
            if not isinstance(key, str):
                raise ValueError(f"{label} contains a non-string object key")  # noqa: TRY004
            _validate_canonical_value(item, f"{label}.{key}")
        return
    raise ValueError(f"{label} contains a value that is not valid JSON")


def _wire_string(value: Any, label: str, *, allow_empty: bool = False, max_bytes: int = 512) -> str:
    if not isinstance(value, str) or (not allow_empty and not value):
        raise ProtocolError("invalid_payload", f"{label} is invalid", False)
    if len(value.encode("utf-8")) > max_bytes:
        raise ProtocolError("invalid_payload", f"{label} is invalid", False)
    return value


def _wire_scope(value: Any, label: str) -> str:
    value = _wire_string(value, label)
    if any(char in value for char in "/?#\x00\r\n"):
        raise ProtocolError("invalid_payload", f"{label} is invalid", False)
    return value


def _optional_wire_string(value: Any, label: str) -> None:
    if value is not None:
        _wire_string(value, label)


def _redact_text(value: str) -> str:
    return _BEARER_IN_TEXT_RE.sub("[redacted bearer]", value)


def _redact_details(value: Any, key: str | None = None) -> Any:
    if key is not None and any(
        marker in key.lower()
        for marker in ("token", "secret", "password", "authorization", "bearer")
    ):
        return "[redacted]"
    if isinstance(value, str):
        return _redact_text(value)
    if isinstance(value, list):
        return [_redact_details(item) for item in value]
    if isinstance(value, dict):
        return {name: _redact_details(item, name) for name, item in value.items()}
    return value


def _validate_hash_text(value: Any, label: str) -> str:
    # The selected hash suite is part of the server's negotiated/versioned
    # manifest.  The SDK therefore validates the exact non-empty field shape
    # without pretending to recompute a provider-specific digest here.
    return _wire_string(value, label, max_bytes=256)


def _validate_pack(value: Any, label: str = "pack") -> None:
    if not isinstance(value, dict) or set(value) != {"id", "version", "digest"}:
        raise ProtocolError("invalid_envelope", f"{label} is invalid", False)
    _wire_string(value["id"], f"{label}.id")
    _wire_string(value["version"], f"{label}.version")
    _validate_hash_text(value["digest"], f"{label}.digest")


def _validate_runner_pack_revision(value: Any, label: str) -> None:
    """Validate the exact PackReference bounds enforced by Runner admission."""

    if not isinstance(value, dict) or set(value) != {"id", "version", "digest"}:
        raise ProtocolError("invalid_envelope", f"{label} is invalid", False)
    _wire_string(value["id"], f"{label}.id", max_bytes=256)
    _wire_string(value["version"], f"{label}.version", max_bytes=64)
    digest = value["digest"]
    if (
        not isinstance(digest, str)
        or not digest.startswith("blake3:")
        or len(digest) != 71
        or any(character not in "0123456789abcdef" for character in digest[7:])
    ):
        raise ProtocolError("invalid_envelope", f"{label}.digest is invalid", False)


_ROOM_HEAD_FIELDS = frozenset(
    (
        "room_id",
        "room_seq",
        "genesis_or_transition_hash",
        "core_schema_version",
        "pack_digest",
        "core_state_hash",
        "activity_state_hash",
        "authoritative_state_hash",
    )
)


def _validate_room_head(value: Any, label: str = "room_head", room_id: str | None = None) -> None:
    if not isinstance(value, dict) or set(value) != _ROOM_HEAD_FIELDS:
        raise ProtocolError("invalid_envelope", f"{label} is invalid", False)
    actual_room_id = _wire_scope(value["room_id"], f"{label}.room_id")
    if room_id is not None and actual_room_id != room_id:
        raise ProtocolError("forbidden", f"{label} is addressed to another room", False)
    _validate_nonnegative_integer(value["room_seq"], f"{label}.room_seq")
    _wire_string(value["core_schema_version"], f"{label}.core_schema_version")
    for field in (
        "genesis_or_transition_hash",
        "pack_digest",
        "core_state_hash",
        "activity_state_hash",
        "authoritative_state_hash",
    ):
        _validate_hash_text(value[field], f"{label}.{field}")


def _validate_action_offer(value: Any, label: str) -> None:
    fields = {"domain", "action_type", "payload_schema_digest", "eligibility_window"}
    if (
        not isinstance(value, dict)
        or set(value) - fields
        or not {"domain", "action_type", "payload_schema_digest"} <= set(value)
    ):
        raise ProtocolError("invalid_envelope", f"{label} is invalid", False)
    _wire_string(value["domain"], f"{label}.domain")
    _wire_string(value["action_type"], f"{label}.action_type", max_bytes=MAX_ACTION_TYPE_BYTES)
    _validate_hash_text(value["payload_schema_digest"], f"{label}.payload_schema_digest")
    if "eligibility_window" in value:
        _validate_canonical_value(value["eligibility_window"], f"{label}.eligibility_window")


def _validate_projection(value: Any, label: str = "projection") -> None:
    if not isinstance(value, dict) or set(value) != {"core", "activity", "action_offers"}:
        raise ProtocolError("invalid_envelope", f"{label} is invalid", False)
    _validate_canonical_value(value["core"], f"{label}.core")
    _validate_canonical_value(value["activity"], f"{label}.activity")
    offers = value["action_offers"]
    if not isinstance(offers, list) or len(offers) > 1024:
        raise ProtocolError("invalid_envelope", f"{label}.action_offers is invalid", False)
    for index, offer in enumerate(offers):
        _validate_action_offer(offer, f"{label}.action_offers[{index}]")


def _validate_runner_id(value: Any, label: str = "runner_id") -> str:
    return _wire_scope(value, label)


def _validate_activation_id(value: Any, label: str) -> str:
    return _wire_scope(value, label)


def _validate_lease_ms(value: Any, label: str, *, optional: bool = False) -> None:
    if value is None and optional:
        return
    lease_ms = _validate_nonnegative_integer(value, label)
    if lease_ms == 0 or lease_ms > _RUNNER_MAX_LEASE_MS:
        raise ProtocolError("invalid_payload", f"{label} is invalid", False)


def _validate_activation_offer(value: Any, label: str) -> None:
    fields = {
        "activation_id",
        "room_id",
        "member_id",
        "cause_room_seq",
        "reason_code",
        "priority",
        "deadline",
        "lease_duration_ms",
    }
    if not isinstance(value, dict) or set(value) != fields:
        raise ProtocolError("invalid_envelope", f"{label} is invalid", False)
    _validate_activation_id(value["activation_id"], f"{label}.activation_id")
    _wire_scope(value["room_id"], f"{label}.room_id")
    _wire_scope(value["member_id"], f"{label}.member_id")
    _validate_nonnegative_integer(value["cause_room_seq"], f"{label}.cause_room_seq")
    _wire_string(value["reason_code"], f"{label}.reason_code", max_bytes=128)
    _validate_nonnegative_integer(value["priority"], f"{label}.priority")
    _optional_wire_string(value["deadline"], f"{label}.deadline")
    _validate_lease_ms(value["lease_duration_ms"], f"{label}.lease_duration_ms")


def _validate_activation_context(value: Any, label: str) -> None:
    fields = {
        "activation_id",
        "claim_id",
        "cause_room_seq",
        "reason_code",
        "lease_generation",
        "lease_until",
        "deadline",
        "room_head",
        "integrity_generation",
        "policy_revision",
        "authority_generation",
        "membership_generation",
        "frame_head",
        "retained_floor",
        "cursor",
        "projection_schema",
        "projection",
        "action_offers",
        "runner_budget",
        "runner_limits",
        "artifact_references",
        "delivery",
    }
    if not isinstance(value, dict) or set(value) != fields:
        raise ProtocolError("invalid_envelope", f"{label} is invalid", False)
    _validate_activation_id(value["activation_id"], f"{label}.activation_id")
    _wire_scope(value["claim_id"], f"{label}.claim_id")
    _validate_nonnegative_integer(value["cause_room_seq"], f"{label}.cause_room_seq")
    _wire_string(value["reason_code"], f"{label}.reason_code", max_bytes=128)
    _validate_nonnegative_integer(value["lease_generation"], f"{label}.lease_generation")
    _wire_string(value["lease_until"], f"{label}.lease_until", max_bytes=128)
    _optional_wire_string(value["deadline"], f"{label}.deadline")
    room_head = value["room_head"]
    if not isinstance(room_head, dict):
        raise ProtocolError("invalid_envelope", f"{label}.room_head is invalid", False)
    room_id = _wire_scope(room_head.get("room_id"), f"{label}.room_head.room_id")
    _validate_room_head(room_head, f"{label}.room_head", room_id)
    for field in (
        "integrity_generation",
        "policy_revision",
        "authority_generation",
        "membership_generation",
        "frame_head",
        "retained_floor",
    ):
        _validate_nonnegative_integer(value[field], f"{label}.{field}")
    if value["cursor"] is not None:
        _validate_nonnegative_integer(value["cursor"], f"{label}.cursor")
    _wire_string(value["projection_schema"], f"{label}.projection_schema")
    _validate_canonical_value(value["projection"], f"{label}.projection")
    offers = value["action_offers"]
    if not isinstance(offers, list) or len(offers) > _RUNNER_MAX_OFFERS:
        raise ProtocolError("invalid_envelope", f"{label}.action_offers is invalid", False)
    for index, offer in enumerate(offers):
        _validate_action_offer(offer, f"{label}.action_offers[{index}]")
    for field in ("runner_budget", "runner_limits"):
        _validate_canonical_value(value[field], f"{label}.{field}")
    artifacts = value["artifact_references"]
    if not isinstance(artifacts, list) or len(artifacts) > _RUNNER_MAX_OFFERS:
        raise ProtocolError("invalid_envelope", f"{label}.artifact_references is invalid", False)
    _validate_canonical_value(artifacts, f"{label}.artifact_references")
    delivery = value["delivery"]
    if not isinstance(delivery, dict) or delivery.get("kind") not in {
        "retained_frames",
        "projection_reset",
    }:
        raise ProtocolError("invalid_envelope", f"{label}.delivery is invalid", False)
    if delivery["kind"] == "retained_frames":
        if set(delivery) != {"kind", "cursor_exclusive", "through_frame_head", "frames"}:
            raise ProtocolError("invalid_envelope", f"{label}.delivery is invalid", False)
        _validate_nonnegative_integer(
            delivery["cursor_exclusive"], f"{label}.delivery.cursor_exclusive"
        )
        _validate_nonnegative_integer(
            delivery["through_frame_head"], f"{label}.delivery.through_frame_head"
        )
        frames = delivery["frames"]
        if not isinstance(frames, list) or len(frames) > _RUNNER_MAX_OFFERS:
            raise ProtocolError("invalid_envelope", f"{label}.delivery.frames is invalid", False)
        for index, frame in enumerate(frames):
            if not isinstance(frame, dict) or set(frame) != {
                "frame_seq",
                "cause_room_seq",
                "payload_hash",
                "payload",
            }:
                raise ProtocolError(
                    "invalid_envelope", f"{label}.delivery.frames[{index}] is invalid", False
                )
            _validate_nonnegative_integer(
                frame["frame_seq"], f"{label}.delivery.frames[{index}].frame_seq"
            )
            _validate_nonnegative_integer(
                frame["cause_room_seq"], f"{label}.delivery.frames[{index}].cause_room_seq"
            )
            _validate_hash_text(
                frame["payload_hash"], f"{label}.delivery.frames[{index}].payload_hash"
            )
            _validate_canonical_value(frame["payload"], f"{label}.delivery.frames[{index}].payload")
    else:
        if set(delivery) != {"kind", "baseline_frame_head", "reason"}:
            raise ProtocolError("invalid_envelope", f"{label}.delivery is invalid", False)
        _validate_nonnegative_integer(
            delivery["baseline_frame_head"], f"{label}.delivery.baseline_frame_head"
        )
        _wire_string(delivery["reason"], f"{label}.delivery.reason")


def _validate_activation_reply(value: Any, label: str) -> None:
    required = {
        "operation_id",
        "activation_id",
        "claim_id",
        "runner_id",
        "code",
        "state",
        "lease_generation",
        "context_hash",
        "context",
    }
    if not isinstance(value, dict) or set(value) != required:
        raise ProtocolError("invalid_envelope", f"{label} is invalid", False)
    _wire_scope(value["operation_id"], f"{label}.operation_id")
    for field in ("activation_id", "claim_id"):
        if value[field] is not None:
            _validate_activation_id(value[field], f"{label}.{field}")
    _validate_runner_id(value["runner_id"], f"{label}.runner_id")
    if value["code"] not in _ACTIVATION_RESULT_CODES:
        raise ProtocolError("invalid_envelope", f"{label}.code is invalid", False)
    if value["state"] is not None and value["state"] not in _ACTIVATION_STATES:
        raise ProtocolError("invalid_envelope", f"{label}.state is invalid", False)
    if value["lease_generation"] is not None:
        _validate_nonnegative_integer(value["lease_generation"], f"{label}.lease_generation")
    if value["context_hash"] is not None:
        _validate_hash_text(value["context_hash"], f"{label}.context_hash")
    if value["context"] is not None:
        _validate_activation_context(value["context"], f"{label}.context")


def _validate_body(message_type: str, body: Any) -> dict[str, Any]:
    body = _validate_body_schema(message_type, body)
    if message_type == "client.hello":
        _wire_string(body["client_name"], "client_name")
        _wire_string(body["client_version"], "client_version")
        if body["mode"] not in {"participant", "spectator", "operator", "runner"}:
            raise ProtocolError("invalid_payload", "mode is invalid", False)
        protocols = body["supported_protocols"]
        capabilities = body["capabilities"]
        if (
            not isinstance(protocols, list)
            or not protocols
            or any(not isinstance(item, str) or not item for item in protocols)
        ):
            raise ProtocolError("invalid_payload", "supported_protocols is invalid", False)
        if (
            not isinstance(capabilities, list)
            or len(set(capabilities)) != len(capabilities)
            or any(not isinstance(item, str) or not item for item in capabilities)
        ):
            raise ProtocolError("invalid_payload", "capabilities is invalid", False)
        if any(required not in capabilities for required in _REQUIRED_CAPABILITIES):
            raise ProtocolError("invalid_payload", "required capabilities are missing", False)
    elif message_type == "runner.hello":
        _validate_runner_id(body["runner_id"])
        maximum = _validate_nonnegative_integer(
            body["maximum_concurrent_activations"], "maximum_concurrent_activations"
        )
        if maximum == 0 or maximum > _RUNNER_MAX_CONCURRENT_ACTIVATIONS:
            raise ProtocolError(
                "invalid_payload", "maximum_concurrent_activations is invalid", False
            )
        packs = body["supported_pack_ids"]
        if not isinstance(packs, list) or len(packs) > _RUNNER_MAX_SUPPORTED_PACKS:
            raise ProtocolError("invalid_payload", "supported_pack_ids is invalid", False)
        if len(set(packs)) != len(packs):
            raise ProtocolError("invalid_payload", "supported_pack_ids is invalid", False)
        for index, pack_id in enumerate(packs):
            _wire_string(pack_id, f"supported_pack_ids[{index}]", max_bytes=256)
        revisions = body.get("supported_pack_revisions")
        if revisions is not None:
            if not isinstance(revisions, list) or len(revisions) > _RUNNER_MAX_SUPPORTED_PACKS:
                raise ProtocolError("invalid_payload", "supported_pack_revisions is invalid", False)
            identities: set[tuple[str, str, str]] = set()
            for index, revision in enumerate(revisions):
                _validate_runner_pack_revision(revision, f"supported_pack_revisions[{index}]")
                identity = (revision["id"], revision["version"], revision["digest"])
                if identity in identities:
                    raise ProtocolError(
                        "invalid_payload", "supported_pack_revisions is invalid", False
                    )
                identities.add(identity)
    elif message_type == "runner.ready":
        _validate_runner_id(body["runner_id"])
    elif message_type == "activation.offer":
        for field in ("operation_id", "room_id", "member_id"):
            _wire_scope(body[field], field)
        _validate_runner_id(body["runner_id"])
    elif message_type == "activation.offers":
        _wire_scope(body["operation_id"], "operation_id")
        _validate_runner_id(body["runner_id"])
        offers = body["offers"]
        if not isinstance(offers, list) or len(offers) > _RUNNER_MAX_OFFERS:
            raise ProtocolError("invalid_envelope", "offers is invalid", False)
        for index, offer in enumerate(offers):
            _validate_activation_offer(offer, f"offers[{index}]")
    elif message_type == "activation.claim":
        for field in ("activation_id", "claim_id"):
            _validate_activation_id(body[field], field)
        _validate_runner_id(body["runner_id"])
        _validate_lease_ms(body["requested_lease_ms"], "requested_lease_ms")
    elif message_type in {
        "activation.claimed",
        "activation.renewed",
        "activation.released",
        "activation.completed",
    }:
        _validate_activation_reply(body, message_type)
    elif message_type in {"activation.renew", "activation.release", "activation.complete"}:
        for field in ("activation_id", "claim_id", "operation_id"):
            _validate_activation_id(body[field], field)
        _validate_runner_id(body["runner_id"])
        _validate_nonnegative_integer(body["lease_generation"], "lease_generation")
        _validate_lease_ms(body.get("requested_lease_ms"), "requested_lease_ms", optional=True)
        if body.get("disposition") is not None:
            _wire_string(body["disposition"], "disposition", max_bytes=256)
    elif message_type == "room.attach":
        _wire_scope(body["room_id"], "room_id")
        _wire_scope(body["member_id"], "member_id")
        if body["after_frame_seq"] is not None:
            _validate_nonnegative_integer(body["after_frame_seq"], "after_frame_seq")
    elif message_type == "room.sync_ack":
        _wire_scope(body["room_id"], "room_id")
        _wire_scope(body["member_id"], "member_id")
        _validate_nonnegative_integer(body["through_frame_head"], "through_frame_head")
        _wire_string(body["sync_token"], "sync_token", max_bytes=1024)
    elif message_type == "observation.ack":
        _wire_scope(body["room_id"], "room_id")
        _wire_scope(body["member_id"], "member_id")
        _validate_nonnegative_integer(body["through_frame_seq"], "through_frame_seq")
    elif message_type == "action.submit":
        _wire_scope(body["room_id"], "room_id")
        _wire_scope(body["member_id"], "member_id")
        _wire_string(body["action_id"], "action_id", max_bytes=256)
        _validate_nonnegative_integer(body["based_on_room_seq"], "based_on_room_seq")
        _wire_string(body["action_type"], "action_type", max_bytes=MAX_ACTION_TYPE_BYTES)
        _validate_canonical_value(body["payload"], "payload")
    elif message_type == "server.welcome":
        _validate_ulid(body["session_id"], "session_id")
        if body["selected_protocol"] != PROTOCOL:
            raise ProtocolError(
                "unsupported_protocol", "server selected an incompatible protocol", False
            )
        _wire_string(body["server_version"], "server_version")
        _validate_nonnegative_integer(body["heartbeat_interval_ms"], "heartbeat_interval_ms")
        maximum = _validate_nonnegative_integer(
            body["maximum_message_bytes"], "maximum_message_bytes"
        )
        if maximum == 0 or maximum > MAX_MESSAGE_BYTES:
            raise ProtocolError("invalid_envelope", "server message bound is unsupported", False)
        principal = body["authenticated_principal"]
        if not isinstance(principal, dict) or set(principal) != {"principal_id", "kind"}:
            raise ProtocolError("invalid_envelope", "server principal is invalid", False)
        _wire_scope(principal["principal_id"], "principal_id")
        if principal["kind"] not in {"human", "agent"}:
            raise ProtocolError("invalid_envelope", "server principal is invalid", False)
    elif message_type == "room.attached":
        room_id = _wire_scope(body["room_id"], "room_id")
        _wire_scope(body["member_id"], "member_id")
        if body["principal_kind"] not in {"human", "agent"}:
            raise ProtocolError("invalid_envelope", "principal_kind is invalid", False)
        if body["access_mode"] not in {"participant", "spectator", "operator"}:
            raise ProtocolError("invalid_envelope", "access_mode is invalid", False)
        _optional_wire_string(body["role"], "role")
        for field in ("membership_status", "room_status", "room_health"):
            _wire_string(body[field], field)
        _validate_nonnegative_integer(body["integrity_generation"], "integrity_generation")
        _validate_room_head(body["room_head"], room_id=room_id)
        if body["cursor"] is not None:
            _validate_nonnegative_integer(body["cursor"], "cursor")
        frame_head = _validate_nonnegative_integer(body["frame_head"], "frame_head")
        retained_floor = _validate_nonnegative_integer(body["retained_floor"], "retained_floor")
        # An empty retained stream advertises the first not-yet-retained
        # sequence, which is legitimately one beyond the captured head.
        if retained_floor > frame_head + 1:
            raise ProtocolError("invalid_envelope", "retained floor exceeds frame head", False)
        if body["cursor"] is not None and body["cursor"] > frame_head:
            raise ProtocolError("invalid_envelope", "cursor exceeds frame head", False)
        _wire_string(body["sync_token"], "sync_token", max_bytes=1024)
        branch = body["sync"]
        if not isinstance(branch, dict):
            raise ProtocolError("invalid_envelope", "sync branch is invalid", False)
        if branch.get("kind") == "retained_frames":
            if set(branch) != {"kind", "cursor_exclusive", "through_frame_head"}:
                raise ProtocolError("invalid_envelope", "sync branch is invalid", False)
            cursor_exclusive = _validate_nonnegative_integer(
                branch["cursor_exclusive"], "cursor_exclusive"
            )
            through = _validate_nonnegative_integer(
                branch["through_frame_head"], "through_frame_head"
            )
            if body["cursor"] is not None and cursor_exclusive != body["cursor"]:
                raise ProtocolError(
                    "invalid_envelope", "sync cursor does not match attachment", False
                )
            if through < cursor_exclusive or through > frame_head:
                raise ProtocolError("invalid_envelope", "sync frame barrier is invalid", False)
        elif branch.get("kind") == "projection_reset":
            if set(branch) != {"kind", "baseline_frame_head", "reason"}:
                raise ProtocolError("invalid_envelope", "sync branch is invalid", False)
            baseline = _validate_nonnegative_integer(
                branch["baseline_frame_head"], "baseline_frame_head"
            )
            _wire_string(branch["reason"], "sync.reason")
            if baseline > frame_head:
                raise ProtocolError("invalid_envelope", "reset baseline exceeds frame head", False)
        else:
            raise ProtocolError("invalid_envelope", "sync branch is invalid", False)
        _validate_pack(body["pack"])
    elif message_type == "projection.reset":
        room_id = _wire_scope(body["room_id"], "room_id")
        _wire_scope(body["member_id"], "member_id")
        _validate_room_head(body["room_head"], room_id=room_id)
        _wire_string(body["room_health"], "room_health")
        _validate_nonnegative_integer(body["integrity_generation"], "integrity_generation")
        _validate_nonnegative_integer(body["baseline_frame_head"], "baseline_frame_head")
        _wire_string(body["reset_reason"], "reset_reason")
        _wire_string(body["projection_schema"], "projection_schema")
        _validate_projection(body["projection"])
        _validate_hash_text(body["projection_hash"], "projection_hash")
    elif message_type == "observation.deliver":
        _wire_scope(body["room_id"], "room_id")
        _wire_scope(body["member_id"], "member_id")
        _validate_nonnegative_integer(body["frame_seq"], "frame_seq")
        _validate_nonnegative_integer(body["cause_room_seq"], "cause_room_seq")
        _wire_string(body["frame_kind"], "frame_kind")
        _wire_string(body["observation_schema"], "observation_schema")
        _validate_canonical_value(body["observation"], "observation")
        _validate_hash_text(body["frame_payload_hash"], "frame_payload_hash")
    elif message_type == "observation.acked":
        _wire_scope(body["room_id"], "room_id")
        _wire_scope(body["member_id"], "member_id")
        if body["cursor"] is not None:
            _validate_nonnegative_integer(body["cursor"], "cursor")
    elif message_type == "room.sync_acked":
        _validate_nonnegative_integer(body["through_frame_head"], "through_frame_head")
    elif message_type == "action.accepted":
        room_id = _wire_scope(body["room_id"], "room_id")
        _wire_scope(body["member_id"], "member_id")
        _wire_string(body["action_id"], "action_id", max_bytes=256)
        _wire_string(body["transition_id"], "transition_id", max_bytes=256)
        _wire_string(body["admitted_at"], "admitted_at", max_bytes=128)
        _validate_room_head(body["room_head"], room_id=room_id)
        if not isinstance(body["duplicate"], bool):
            raise ProtocolError("invalid_envelope", "duplicate is invalid", False)
    elif message_type == "action.rejected":
        _wire_scope(body["room_id"], "room_id")
        _wire_scope(body["member_id"], "member_id")
        _wire_string(body["action_id"], "action_id", max_bytes=256)
        _wire_string(body["admitted_at"], "admitted_at", max_bytes=128)
        _wire_string(body["code"], "code", max_bytes=128)
        _wire_string(body["message"], "message", max_bytes=512)
        _validate_nonnegative_integer(body["current_room_seq"], "current_room_seq")
        offers = body["action_offers"]
        if not isinstance(offers, list) or len(offers) > 1024:
            raise ProtocolError("invalid_envelope", "action_offers is invalid", False)
        for index, offer in enumerate(offers):
            _validate_action_offer(offer, f"action_offers[{index}]")
        for field in ("retryable_with_same_action_id", "may_submit_revised_action", "duplicate"):
            if not isinstance(body[field], bool):
                raise ProtocolError("invalid_envelope", f"{field} is invalid", False)
        _validate_canonical_value(body["details"], "details")
    elif message_type == "error":
        _wire_string(body["code"], "code", max_bytes=128)
        _wire_string(body["message"], "message", max_bytes=512)
        if not isinstance(body["retryable"], bool):
            raise ProtocolError("invalid_envelope", "retryable is invalid", False)
        if "details" in body:
            _validate_canonical_value(body["details"], "details")
    elif message_type in {"server.ping", "client.pong"}:
        pass
    return body


def _validate_scope_id(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value or any(char in value for char in "/?#\x00\r\n"):
        raise ValueError(f"{label} must be a non-empty path-safe string")
    return value


def _validate_body_schema(message_type: str, body: Any) -> dict[str, Any]:
    if message_type not in _KNOWN_MESSAGE_TYPES:
        raise ProtocolError("invalid_envelope", "message type is not supported", False)
    if not isinstance(body, dict):
        raise ProtocolError("invalid_envelope", "message body must be an object", False)
    required, allowed = _BODY_FIELDS[message_type]
    keys = set(body)
    if keys - allowed or required - keys:
        raise ProtocolError("invalid_envelope", "message body does not match its schema", False)
    return body


def _protocol_error_from_http(value: Any) -> ProtocolError:
    if not isinstance(value, dict) or set(value) != {"error"}:
        return ProtocolError("internal", "request failed", False)
    payload = value["error"]
    if not isinstance(payload, dict):
        return ProtocolError("internal", "request failed", False)
    required = {"code", "message", "retryable"}
    if set(payload) - required - {"details"} or not required <= set(payload):
        return ProtocolError("internal", "request failed", False)
    code = payload["code"]
    message = payload["message"]
    retryable = payload["retryable"]
    if (
        not isinstance(code, str)
        or not code
        or not isinstance(message, str)
        or not message
        or len(message.encode("utf-8")) > 512
        or not isinstance(retryable, bool)
    ):
        return ProtocolError("internal", "request failed", False)
    details = _redact_details(payload.get("details"))
    return ProtocolError(code, message, retryable, details)


class ProtocolError(Exception):
    """A typed server error; callers should branch on ``code``."""

    def __init__(self, code: str, message: str, retryable: bool, details: Any = None) -> None:
        safe_message = _redact_text(message) if isinstance(message, str) else "request failed"
        safe_details = _redact_details(details)
        super().__init__(safe_message)
        self.code = code
        self.message = safe_message
        self.retryable = retryable
        self.details = safe_details


class _TransportError(ProtocolError):
    """A retryable transport failure kept private to action retry handling."""


class LostActionReply(TimeoutError):
    """The connection did not return an Action result after submission.

    The original action identity and canonical body are retained so
    :meth:`Room.retry_action` can safely resolve the server receipt.
    """

    def __init__(self, room: Room, request: dict[str, Any]) -> None:
        super().__init__(f"no reply for action {request['body']['action_id']}")
        self._room = room
        self.request = copy.deepcopy(request)

    async def retry(self) -> dict[str, Any]:
        return await self._room.retry_action(self.request)


class LostRunnerReply(TimeoutError):
    """A Runner command was sent but its correlated reply was not observed."""

    def __init__(self, runner: Runner, request: dict[str, Any]) -> None:
        super().__init__(f"no reply for runner request {request['request_id']}")
        self._runner = runner
        self.request = copy.deepcopy(request)

    async def retry(self, timeout: float = 15) -> dict[str, Any]:
        """Retry the exact request identity after the connection is usable."""

        return await self._runner.retry(self.request, timeout=timeout)


def _object_without_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON object key")
        result[key] = value
    return result


def _safe_integer(raw: str) -> int:
    value = int(raw)
    if not -_MAX_SAFE_INTEGER <= value <= _MAX_SAFE_INTEGER:
        raise ValueError("integer outside canonical safe range")
    return value


def _reject_number(_: str) -> None:
    raise ValueError("floating point values are not supported")


def _strict_value(raw: str | bytes) -> Any:
    if isinstance(raw, str):
        raw_bytes = raw.encode("utf-8")
    else:
        raw_bytes = raw
    if len(raw_bytes) > MAX_MESSAGE_BYTES:
        raise ProtocolError("message_too_large", "message exceeds the configured bound", False)
    try:
        return json.loads(
            raw_bytes,
            object_pairs_hook=_object_without_duplicates,
            parse_int=_safe_integer,
            parse_float=_reject_number,
            parse_constant=_reject_number,
        )
    except UnicodeDecodeError as error:
        raise ProtocolError("invalid_envelope", "message is not valid UTF-8", False) from error


def _loads(raw: str | bytes) -> dict[str, Any]:
    value = _strict_value(raw)
    if not isinstance(value, dict):
        raise ProtocolError("invalid_envelope", "message must be an object", False)
    required = {"protocol", "type", "message_id", "body"}
    if set(value) - required - {"request_id"} or not required <= set(value):
        raise ProtocolError("invalid_envelope", "message envelope is invalid", False)
    if value["protocol"] != PROTOCOL:
        raise ProtocolError("unsupported_protocol", "no compatible protocol version", False)
    if not isinstance(value["type"], str) or not value["type"]:
        raise ProtocolError("invalid_envelope", "message type is invalid", False)
    if len(value["type"].encode("utf-8")) > 128:
        raise ProtocolError("invalid_envelope", "message type is invalid", False)
    _validate_ulid(value["message_id"], "message_id")
    if "request_id" in value:
        _validate_ulid(value["request_id"], "request_id")
    _validate_body(value["type"], value["body"])
    return value


def _dumps(message_type: str, message_id: str, body: Any, request_id: str | None = None) -> str:
    if not isinstance(message_type, str) or not message_type:
        raise ProtocolError("invalid_envelope", "message type is invalid", False)
    if len(message_type.encode("utf-8")) > 128:
        raise ProtocolError("invalid_envelope", "message type is invalid", False)
    _validate_ulid(message_id, "message_id")
    if request_id is not None:
        _validate_ulid(request_id, "request_id")
    _validate_body(message_type, body)
    try:
        _validate_canonical_value(body, "message.body")
    except ValueError as error:
        raise ProtocolError(
            "invalid_payload", "message body is not canonical JSON", False
        ) from error
    envelope: dict[str, Any] = {
        "protocol": PROTOCOL,
        "type": message_type,
        "message_id": message_id,
        "body": body,
    }
    if request_id is not None:
        envelope["request_id"] = request_id
    try:
        raw = json.dumps(
            envelope,
            allow_nan=False,
            ensure_ascii=False,
            separators=(",", ":"),
            sort_keys=True,
        )
    except (TypeError, ValueError) as error:
        raise ProtocolError("invalid_payload", "message body is not valid JSON", False) from error
    if len(raw.encode("utf-8")) > MAX_MESSAGE_BYTES:
        raise ProtocolError("message_too_large", "message exceeds the configured bound", False)
    return raw


def _canonical_json(value: Any, label: str) -> str:
    """Return the stable local representation used for idempotency checks."""

    try:
        _validate_canonical_value(value, label)
        return json.dumps(
            value,
            allow_nan=False,
            ensure_ascii=False,
            separators=(",", ":"),
            sort_keys=True,
        )
    except (TypeError, ValueError) as error:
        raise ProtocolError("invalid_payload", f"{label} is not canonical JSON", False) from error


def _ulid() -> str:
    """Generate a valid, non-secret ULID for transport and Action identity."""

    value = (int(time.time_ns() // 1_000_000) << 80) | int.from_bytes(
        secrets.token_bytes(10), "big"
    )
    chars = []
    for shift in range(125, -1, -5):
        chars.append(_ALPHABET[(value >> shift) & 31])
    return "".join(chars[-26:])


@dataclass(slots=True)
class Client:
    """HTTP/ WebSocket entry point using one bearer capability."""

    base_url: str
    bearer: str
    ws_factory: Callable[..., Any] = websockets.connect

    def __post_init__(self) -> None:
        if not isinstance(self.bearer, str) or _BEARER_RE.fullmatch(self.bearer) is None:
            raise ValueError(
                "bearer must be the canonical wsb1 lowercase-hex capability wire value"
            )
        self.base_url = self.base_url.rstrip("/")
        parsed = urlsplit(self.base_url)
        if parsed.scheme not in {"http", "https"} or not parsed.netloc or parsed.username:
            raise ValueError("base_url must be an HTTP(S) URL without credentials")
        if parsed.query or parsed.fragment:
            raise ValueError("base_url must not contain a query or fragment")

    async def create_room(self, request: dict[str, Any]) -> dict[str, Any]:
        """Create a Counter/Activity room through the versioned HTTP API."""

        self._validate_create_request(request)
        return await asyncio.to_thread(self._post_json, "/v1/rooms", request)

    async def projection(self, room_id: str) -> dict[str, Any]:
        safe_room_id = _validate_scope_id(room_id, "room_id")
        response = await asyncio.to_thread(self._get_json, f"/v1/rooms/{safe_room_id}/projection")
        if response["room_id"] != safe_room_id:
            raise ProtocolError("invalid_envelope", "projection belongs to another room", False)
        return response

    async def replay(self, room_id: str, at_room_seq: int) -> dict[str, Any]:
        """Request read-only replay under the bearer’s present authority."""

        safe_room_id = _validate_scope_id(room_id, "room_id")
        if isinstance(at_room_seq, bool) or not isinstance(at_room_seq, int) or at_room_seq < 0:
            raise ValueError("at_room_seq must be a non-negative integer")
        response = await asyncio.to_thread(
            self._get_json, f"/v1/rooms/{safe_room_id}/replay?at_room_seq={at_room_seq}"
        )
        if response["room_id"] != safe_room_id or response["requested_room_seq"] != at_room_seq:
            raise ProtocolError(
                "invalid_envelope", "replay response does not match its request", False
            )
        if response["verification"] != "verified":
            raise ProtocolError(
                "replay_unverified", "server did not verify the requested replay", False
            )
        return response

    async def open_room(
        self, room_id: str, member_id: str, after_frame_seq: int | None = None
    ) -> Room:
        _validate_scope_id(room_id, "room_id")
        _validate_scope_id(member_id, "member_id")
        if after_frame_seq is not None:
            _validate_nonnegative_integer(after_frame_seq, "after_frame_seq")
        room = Room(self, room_id, member_id, after_frame_seq)
        await room.connect()
        return room

    async def open_runner(
        self,
        runner_id: str,
        maximum_concurrent_activations: int,
        supported_pack_ids: list[str],
        supported_pack_revisions: list[dict[str, Any]] | None = None,
    ) -> Runner:
        """Open and handshake a Runner control connection."""

        runner = Runner(
            self,
            runner_id,
            maximum_concurrent_activations,
            supported_pack_ids,
            supported_pack_revisions,
        )
        await runner.connect()
        return runner

    def _request(
        self, method: str, path: str, body: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        if body is not None and not isinstance(body, dict):
            raise ValueError("HTTP request body must be an object")
        try:
            if body is not None:
                _validate_canonical_value(body, "HTTP request body")
            data = (
                None
                if body is None
                else json.dumps(
                    body,
                    allow_nan=False,
                    ensure_ascii=False,
                    separators=(",", ":"),
                    sort_keys=True,
                ).encode("utf-8")
            )
        except (TypeError, ValueError) as error:
            raise ProtocolError(
                "invalid_payload", "request body is not valid JSON", False
            ) from error
        if data is not None and len(data) > MAX_MESSAGE_BYTES:
            raise ProtocolError("message_too_large", "request exceeds the configured bound", False)
        request = urllib.request.Request(
            f"{self.base_url}{path}",
            data=data,
            method=method,
            headers={
                "Accept": "application/json",
                "Authorization": f"Bearer {self.bearer}",
                "Content-Type": "application/json",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=15) as response:
                value = _strict_value(response.read())
                if not isinstance(value, dict):
                    raise ProtocolError(
                        "invalid_envelope", "HTTP response must be an object", False
                    )
                self._validate_http_response(path, value)
                return value
        except urllib.error.HTTPError as error:
            try:
                value = _strict_value(error.read())
                raise _protocol_error_from_http(value) from error
            except ProtocolError:
                raise
            except (TypeError, ValueError, UnicodeDecodeError) as parse_error:
                raise ProtocolError("internal", "request failed", False) from parse_error
        except (urllib.error.URLError, TimeoutError, OSError) as error:
            raise ProtocolError("storage_unavailable", "request transport failed", True) from error

    @staticmethod
    def _validate_http_response(path: str, value: dict[str, Any]) -> None:
        parsed_path = urlsplit(path).path
        if parsed_path == "/v1/rooms":
            required = {"room_id", "member_ids", "room_head"}
        elif parsed_path.endswith("/projection"):
            required = {
                "room_id",
                "room_head",
                "room_health",
                "integrity_generation",
                "projection_schema",
                "projection",
                "projection_hash",
            }
        elif parsed_path.endswith("/replay"):
            required = {
                "room_id",
                "pack",
                "requested_room_seq",
                "room_head",
                "projection",
                "projection_hash",
                "verification",
                "room_health",
                "integrity_generation",
            }
        else:
            return
        if set(value) != required:
            raise ProtocolError(
                "invalid_envelope", "HTTP response does not match its schema", False
            )
        if parsed_path == "/v1/rooms":
            _wire_scope(value["room_id"], "room_id")
            member_ids = value["member_ids"]
            if not isinstance(member_ids, list) or any(
                not isinstance(member_id, str) or not member_id for member_id in member_ids
            ):
                raise ProtocolError("invalid_envelope", "member_ids is invalid", False)
            _validate_room_head(value["room_head"], room_id=value["room_id"])
        elif parsed_path.endswith("/projection"):
            room_id = _wire_scope(value["room_id"], "room_id")
            _validate_room_head(value["room_head"], room_id=room_id)
            _wire_string(value["room_health"], "room_health")
            _validate_nonnegative_integer(value["integrity_generation"], "integrity_generation")
            _wire_string(value["projection_schema"], "projection_schema")
            _validate_projection(value["projection"])
            _validate_hash_text(value["projection_hash"], "projection_hash")
        elif parsed_path.endswith("/replay"):
            room_id = _wire_scope(value["room_id"], "room_id")
            _validate_pack(value["pack"])
            _validate_nonnegative_integer(value["requested_room_seq"], "requested_room_seq")
            _validate_room_head(value["room_head"], room_id=room_id)
            _validate_projection(value["projection"])
            _validate_hash_text(value["projection_hash"], "projection_hash")
            _wire_string(value["verification"], "verification")
            _wire_string(value["room_health"], "room_health")
            _validate_nonnegative_integer(value["integrity_generation"], "integrity_generation")
            if value["verification"] != "verified":
                raise ProtocolError(
                    "replay_unverified", "server did not verify the requested replay", False
                )

    @staticmethod
    def _validate_create_request(request: Any) -> None:
        fields = {"pack", "configuration", "members", "idempotency_key"}
        if not isinstance(request, dict) or set(request) != fields:
            raise ValueError("room creation request does not match its schema")
        _validate_pack(request["pack"])
        _validate_canonical_value(request["configuration"], "configuration")
        members = request["members"]
        if not isinstance(members, list) or not members or len(members) > 1024:
            raise ValueError("room creation members are invalid")
        for member in members:
            if not isinstance(member, dict) or set(member) != {
                "principal_id",
                "principal_kind",
                "role",
                "access_mode",
            }:
                raise ValueError("room creation member does not match its schema")
            _wire_scope(member["principal_id"], "principal_id")
            if member["principal_kind"] not in {"human", "agent"}:
                raise ValueError("principal_kind is invalid")
            _optional_wire_string(member["role"], "role")
            if member["access_mode"] not in {"participant", "spectator", "operator"}:
                raise ValueError("access_mode is invalid")
        _wire_string(request["idempotency_key"], "idempotency_key", max_bytes=256)

    def _post_json(self, path: str, body: dict[str, Any]) -> dict[str, Any]:
        return self._request("POST", path, body)

    def _get_json(self, path: str) -> dict[str, Any]:
        return self._request("GET", path)

    def ws_url(self) -> str:
        parsed = urlsplit(self.base_url)
        scheme = "wss" if parsed.scheme.lower() == "https" else "ws"
        path = parsed.path.rstrip("/") + "/v1/stream"
        return f"{scheme}://{parsed.netloc}{path}"

    def runner_ws_url(self) -> str:
        """Return the dedicated Runner control-stream URL."""

        parsed = urlsplit(self.base_url)
        scheme = "wss" if parsed.scheme.lower() == "https" else "ws"
        path = parsed.path.rstrip("/") + "/v1/runner/stream"
        return f"{scheme}://{parsed.netloc}{path}"


class Runner:
    """Bounded Runner control connection for durable Activation operations."""

    def __init__(
        self,
        client: Client,
        runner_id: str,
        maximum_concurrent_activations: int,
        supported_pack_ids: list[str],
        supported_pack_revisions: list[dict[str, Any]] | None = None,
    ) -> None:
        self.client = client
        self.runner_id = _validate_runner_id(runner_id)
        self.maximum_concurrent_activations = maximum_concurrent_activations
        self.supported_pack_ids = list(supported_pack_ids)
        self.supported_pack_revisions = (
            None if supported_pack_revisions is None else copy.deepcopy(supported_pack_revisions)
        )
        _validate_body("runner.hello", self._hello_body())
        self.websocket: Any = None
        self.welcome: dict[str, Any] | None = None
        self.ready: dict[str, Any] | None = None
        self.session_id: str | None = None
        self._pending: list[dict[str, Any]] = []

    def _hello_body(self) -> dict[str, Any]:
        body: dict[str, Any] = {
            "runner_id": self.runner_id,
            "maximum_concurrent_activations": self.maximum_concurrent_activations,
            "supported_pack_ids": self.supported_pack_ids,
        }
        if self.supported_pack_revisions is not None:
            body["supported_pack_revisions"] = copy.deepcopy(self.supported_pack_revisions)
        return body

    async def connect(self) -> dict[str, Any]:
        """Open the Runner-mode session and complete the Runner handshake."""

        self._pending.clear()
        self.welcome = None
        self.ready = None
        self.session_id = None
        try:
            self.websocket = await self.client.ws_factory(
                self.client.runner_ws_url(),
                additional_headers={"Authorization": f"Bearer {self.client.bearer}"},
                subprotocols=(WS_SUBPROTOCOL,),
                compression=None,
                max_size=MAX_MESSAGE_BYTES,
                max_queue=16,
            )
        except (ConnectionClosed, WebSocketException, OSError, TimeoutError) as error:
            raise _TransportError(
                "storage_unavailable", "WebSocket connection failed", True
            ) from error
        await self._send(
            "client.hello",
            {
                "client_name": "worldstream-sdk",
                "client_version": "0.1.0",
                "mode": "runner",
                "supported_protocols": [PROTOCOL],
                "capabilities": [*_REQUIRED_CAPABILITIES, "runner_activation_control"],
            },
        )
        self.welcome = await self._receive_type("server.welcome")
        self._validate_welcome(self.welcome)
        self.session_id = self.welcome["session_id"]
        self.ready = await self._command(
            "runner.hello",
            self._hello_body(),
            "runner.ready",
        )
        if self.ready.get("runner_id") != self.runner_id:
            raise ProtocolError("invalid_envelope", "Runner ready identity does not match", False)
        return self.ready

    async def poll_offers(
        self,
        room_id: str,
        member_id: str,
        operation_id: str | None = None,
        timeout: float = 15,
    ) -> dict[str, Any]:
        """Poll public Activation offers for one authorized Agent Membership."""

        body = {
            "operation_id": operation_id or _ulid(),
            "runner_id": self.runner_id,
            "room_id": _validate_scope_id(room_id, "room_id"),
            "member_id": _validate_scope_id(member_id, "member_id"),
        }
        return await self._command("activation.offer", body, "activation.offers", timeout)

    async def claim(
        self,
        activation_id: str,
        requested_lease_ms: int,
        claim_id: str | None = None,
        timeout: float = 15,
    ) -> dict[str, Any]:
        """Claim one offer; successful replies may contain invocation context."""

        body = {
            "activation_id": _validate_activation_id(activation_id, "activation_id"),
            "runner_id": self.runner_id,
            "claim_id": _validate_activation_id(claim_id or _ulid(), "claim_id"),
            "requested_lease_ms": requested_lease_ms,
        }
        return await self._command("activation.claim", body, "activation.claimed", timeout)

    async def renew(
        self,
        activation_id: str,
        claim_id: str,
        lease_generation: int,
        requested_lease_ms: int | None = None,
        operation_id: str | None = None,
        timeout: float = 15,
    ) -> dict[str, Any]:
        """Renew a claimed Activation using its fencing generation."""

        return await self._lease_command(
            "activation.renew",
            activation_id,
            claim_id,
            lease_generation,
            requested_lease_ms,
            operation_id,
            timeout,
        )

    async def release(
        self,
        activation_id: str,
        claim_id: str,
        lease_generation: int,
        operation_id: str | None = None,
        timeout: float = 15,
    ) -> dict[str, Any]:
        """Release a claimed Activation."""

        return await self._lease_command(
            "activation.release",
            activation_id,
            claim_id,
            lease_generation,
            None,
            operation_id,
            timeout,
        )

    async def complete(
        self,
        activation_id: str,
        claim_id: str,
        lease_generation: int,
        disposition: str | None = None,
        operation_id: str | None = None,
        timeout: float = 15,
    ) -> dict[str, Any]:
        """Complete a claimed Activation with an optional bounded disposition."""

        body = self._lease_body(
            activation_id,
            claim_id,
            lease_generation,
            None,
            operation_id,
            disposition,
        )
        return await self._command("activation.complete", body, "activation.completed", timeout)

    async def retry(self, request: dict[str, Any], timeout: float = 15) -> dict[str, Any]:
        """Retry an exact command identity after a lost reply."""

        if not isinstance(request, dict) or set(request) != {
            "message_type",
            "response_type",
            "request_id",
            "body",
        }:
            raise ProtocolError("invalid_payload", "Runner request shape is invalid", False)
        _validate_ulid(request["request_id"], "request_id")
        _validate_body(request["message_type"], request["body"])
        try:
            await self._send(request["message_type"], request["body"], request["request_id"])
            return await asyncio.wait_for(
                self._receive_reply(request["request_id"], request["response_type"]), timeout
            )
        except (_TransportError, TimeoutError) as error:
            raise LostRunnerReply(self, request) from error

    async def close(self) -> None:
        websocket, self.websocket = self.websocket, None
        self.ready = None
        if websocket is not None:
            try:
                await websocket.close()
            except (ConnectionClosed, WebSocketException, OSError, TimeoutError):
                pass

    async def _command(
        self, message_type: str, body: dict[str, Any], response_type: str, timeout: float = 15
    ) -> dict[str, Any]:
        request = {
            "message_type": message_type,
            "response_type": response_type,
            "request_id": _ulid(),
            "body": copy.deepcopy(body),
        }
        try:
            await self._send(message_type, body, request["request_id"])
            return await asyncio.wait_for(
                self._receive_reply(request["request_id"], response_type), timeout
            )
        except (_TransportError, TimeoutError) as error:
            raise LostRunnerReply(self, request) from error

    async def _lease_command(
        self,
        message_type: str,
        activation_id: str,
        claim_id: str,
        lease_generation: int,
        requested_lease_ms: int | None,
        operation_id: str | None,
        timeout: float,
    ) -> dict[str, Any]:
        body = self._lease_body(
            activation_id,
            claim_id,
            lease_generation,
            requested_lease_ms,
            operation_id,
            None,
        )
        response_type = {
            "activation.renew": "activation.renewed",
            "activation.release": "activation.released",
            "activation.complete": "activation.completed",
        }[message_type]
        return await self._command(message_type, body, response_type, timeout)

    def _lease_body(
        self,
        activation_id: str,
        claim_id: str,
        lease_generation: int,
        requested_lease_ms: int | None,
        operation_id: str | None,
        disposition: str | None,
    ) -> dict[str, Any]:
        body: dict[str, Any] = {
            "activation_id": _validate_activation_id(activation_id, "activation_id"),
            "runner_id": self.runner_id,
            "claim_id": _validate_activation_id(claim_id, "claim_id"),
            "operation_id": _validate_scope_id(operation_id or _ulid(), "operation_id"),
            "lease_generation": lease_generation,
        }
        if requested_lease_ms is not None:
            body["requested_lease_ms"] = requested_lease_ms
        if disposition is not None:
            body["disposition"] = disposition
        return body

    async def _send(self, message_type: str, body: Any, request_id: str | None = None) -> None:
        if self.websocket is None:
            raise _TransportError("storage_unavailable", "WebSocket is not connected", True)
        try:
            await self.websocket.send(_dumps(message_type, _ulid(), body, request_id))
        except (ConnectionClosed, WebSocketException, OSError, TimeoutError) as error:
            raise _TransportError("storage_unavailable", "WebSocket send failed", True) from error

    async def _receive(self) -> dict[str, Any]:
        if self.websocket is None:
            raise _TransportError("storage_unavailable", "WebSocket is not connected", True)
        try:
            raw = await self.websocket.recv()
        except (ConnectionClosed, WebSocketException, OSError, TimeoutError) as error:
            raise _TransportError(
                "storage_unavailable", "WebSocket receive failed", True
            ) from error
        message = _loads(raw)
        if message["type"] == "server.ping":
            await self._send("client.pong", {})
            return await self._receive()
        return message

    async def _receive_type(self, message_type: str) -> dict[str, Any]:
        while True:
            message = self._pending.pop(0) if self._pending else await self._receive()
            if message["type"] == "error":
                self._raise_error(message)
            if message["type"] == message_type:
                return message["body"]
            self._pending.append(message)
            if len(self._pending) > _RUNNER_MAX_PENDING_MESSAGES:
                raise ProtocolError("slow_consumer", "Runner message buffer is full", True)

    async def _receive_reply(self, request_id: str, response_type: str) -> dict[str, Any]:
        deferred: list[dict[str, Any]] = []
        try:
            while True:
                message = self._pending.pop(0) if self._pending else await self._receive()
                if message["type"] == "error":
                    if message.get("request_id") in {None, request_id}:
                        self._raise_error(message)
                    deferred.append(message)
                    continue
                if message.get("request_id") == request_id and message["type"] == response_type:
                    return message["body"]
                deferred.append(message)
                if len(deferred) + len(self._pending) > _RUNNER_MAX_PENDING_MESSAGES:
                    raise ProtocolError("slow_consumer", "Runner message buffer is full", True)
        finally:
            self._pending = deferred + self._pending

    def _validate_welcome(self, welcome: dict[str, Any]) -> None:
        _validate_ulid(welcome.get("session_id"), "session_id")
        if welcome.get("selected_protocol") != PROTOCOL:
            raise ProtocolError(
                "unsupported_protocol", "server selected an incompatible protocol", False
            )
        _wire_string(welcome.get("server_version"), "server_version")
        _validate_nonnegative_integer(welcome.get("heartbeat_interval_ms"), "heartbeat_interval_ms")
        maximum = _validate_nonnegative_integer(
            welcome.get("maximum_message_bytes"), "maximum_message_bytes"
        )
        if maximum == 0 or maximum > MAX_MESSAGE_BYTES:
            raise ProtocolError("invalid_envelope", "server message bound is unsupported", False)

    @staticmethod
    def _raise_error(message: dict[str, Any]) -> None:
        body = message["body"]
        code = body.get("code")
        message_text = body.get("message")
        retryable = body.get("retryable")
        if (
            not isinstance(code, str)
            or not code
            or not isinstance(message_text, str)
            or not message_text
            or not isinstance(retryable, bool)
        ):
            raise ProtocolError("invalid_envelope", "server error body is invalid", False)
        raise ProtocolError(code, message_text, retryable, body.get("details"))


class Room:
    """One Membership-addressed connection and its reconnect state."""

    def __init__(
        self,
        client: Client,
        room_id: str,
        member_id: str,
        after_frame_seq: int | None = None,
    ) -> None:
        self.client = client
        self.room_id = _validate_scope_id(room_id, "room_id")
        self.member_id = _validate_scope_id(member_id, "member_id")
        if after_frame_seq is not None:
            _validate_nonnegative_integer(after_frame_seq, "after_frame_seq")
        self.after_frame_seq = after_frame_seq
        self.websocket: Any = None
        self.welcome: dict[str, Any] | None = None
        self.attached: dict[str, Any] | None = None
        self.session_id: str | None = None
        self.live = False
        self.cursor = after_frame_seq
        self._pending: list[dict[str, Any]] = []
        self._sync_token: str | None = None
        self._sync_complete = False
        self._frame_head: int | None = None
        self._retained_floor: int | None = None
        self._last_delivered_frame_seq: int | None = None
        self.last_projection_reset: dict[str, Any] | None = None
        self.last_sync_frames: list[dict[str, Any]] = []
        self._action_fingerprints: dict[str, str] = {}
        self._needs_resync = False

    async def connect(self) -> dict[str, Any]:
        self.live = False
        self.welcome = None
        self.attached = None
        self.session_id = None
        self._sync_token = None
        self._sync_complete = False
        self._frame_head = None
        self._retained_floor = None
        self._last_delivered_frame_seq = None
        self.last_projection_reset = None
        self.last_sync_frames = []
        try:
            self.websocket = await self.client.ws_factory(
                self.client.ws_url(),
                additional_headers={"Authorization": f"Bearer {self.client.bearer}"},
                subprotocols=(WS_SUBPROTOCOL,),
                compression=None,
                max_size=MAX_MESSAGE_BYTES,
                max_queue=16,
            )
        except (ConnectionClosed, WebSocketException, OSError, TimeoutError) as error:
            raise _TransportError(
                "storage_unavailable", "WebSocket connection failed", True
            ) from error
        await self._send(
            "client.hello",
            {
                "client_name": "worldstream-sdk",
                "client_version": "0.1.0",
                "mode": "participant",
                "supported_protocols": [PROTOCOL],
                "capabilities": list(_REQUIRED_CAPABILITIES),
            },
        )
        self.welcome = await self._receive_type("server.welcome")
        self._validate_welcome(self.welcome)
        self.session_id = self.welcome["session_id"]
        await self._send(
            "room.attach",
            {
                "room_id": self.room_id,
                "member_id": self.member_id,
                "after_frame_seq": self.after_frame_seq,
            },
        )
        self.attached = await self._receive_type("room.attached")
        self._validate_attached(self.attached)
        self._sync_token = self.attached["sync_token"]
        self._frame_head = self.attached["frame_head"]
        self._retained_floor = self.attached["retained_floor"]
        self.cursor = (
            self.attached["cursor"] if self.attached["cursor"] is not None else self.cursor
        )
        self.live = False
        return self.attached

    async def sync(self) -> list[dict[str, Any]]:
        if self.attached is None:
            raise RuntimeError("room is not attached")
        if self._sync_complete:
            raise RuntimeError("room synchronization has already been acknowledged")
        branch = self.attached.get("sync")
        if not isinstance(branch, dict):
            raise ProtocolError("invalid_envelope", "attach sync branch is invalid", False)
        kind = branch.get("kind")
        if kind == "retained_frames":
            through = _validate_nonnegative_integer(
                branch.get("through_frame_head"), "through_frame_head"
            )
        elif kind == "projection_reset":
            through = _validate_nonnegative_integer(
                branch.get("baseline_frame_head"), "baseline_frame_head"
            )
        else:
            raise ProtocolError("invalid_envelope", "attach sync branch is invalid", False)
        sync_token = self._sync_token
        if not isinstance(sync_token, str) or not sync_token:
            raise ProtocolError("invalid_envelope", "sync token is invalid", False)
        await self._collect_sync(branch, through)
        synchronized = self._install_sync(branch, through)
        await self._send(
            "room.sync_ack",
            {
                "room_id": self.room_id,
                "member_id": self.member_id,
                "through_frame_head": through,
                "sync_token": sync_token,
            },
        )
        sync_ack = await self._receive_type("room.sync_acked")
        acknowledged_through = _validate_nonnegative_integer(
            sync_ack.get("through_frame_head"), "through_frame_head"
        )
        if acknowledged_through != through:
            raise ProtocolError(
                "sync_barrier_mismatch", "server acknowledged the wrong sync barrier", True
            )
        self._sync_complete = True
        self.live = True
        self._needs_resync = False
        return synchronized

    async def resync(self) -> list[dict[str, Any]]:
        """Reconnect, complete the captured sync barrier, and return its batch.

        A stale Action is never rebased locally. Call this before submitting a
        revised Action identity after the server reports a newer Room Head.
        """

        await self.reconnect()
        return await self.sync()

    async def ack(self, through_frame_seq: int) -> dict[str, Any] | None:
        _validate_nonnegative_integer(through_frame_seq, "through_frame_seq")
        if self.attached is None:
            raise RuntimeError("room is not attached")
        if self._frame_head is not None and through_frame_seq > self._frame_head:
            raise ProtocolError(
                "cursor_ahead", "acknowledgement exceeds the captured frame head", False
            )
        await self._send(
            "observation.ack",
            {
                "room_id": self.room_id,
                "member_id": self.member_id,
                "through_frame_seq": through_frame_seq,
            },
        )
        result = await self._receive_type("observation.acked")
        raw_cursor = result.get("cursor")
        if raw_cursor is None:
            if through_frame_seq != 0 or self.cursor is not None:
                raise ProtocolError("invalid_envelope", "acknowledgement cursor is missing", False)
            return result
        cursor = _validate_nonnegative_integer(raw_cursor, "cursor")
        if cursor < through_frame_seq:
            raise ProtocolError(
                "invalid_envelope",
                "server acknowledgement did not cover the requested cursor",
                False,
            )
        self.cursor = max(self.cursor or 0, cursor)
        return result

    async def act(
        self,
        action_type: str,
        payload: Any,
        *,
        action_id: str | None = None,
        expected_room_seq: int | None = None,
        timeout: float = 15,
    ) -> dict[str, Any]:
        if self._needs_resync:
            raise ProtocolError(
                "resync_required",
                "Room Head is stale; resync before submitting a new Action identity",
                True,
            )
        request = self._action_request(action_type, payload, action_id, expected_room_seq)
        self._remember_action(request)
        try:
            await self._send("action.submit", request["body"], request_id=request["request_id"])
            return await asyncio.wait_for(
                self._receive_action(request["request_id"], request["body"]["action_id"]), timeout
            )
        except (_TransportError, TimeoutError) as error:
            raise LostActionReply(self, request) from error

    async def retry_action(self, request: dict[str, Any], timeout: float = 15) -> dict[str, Any]:
        self._validate_action_request(request)
        self._remember_action(request)
        try:
            await self._send("action.submit", request["body"], request_id=request["request_id"])
            return await asyncio.wait_for(
                self._receive_action(request["request_id"], request["body"]["action_id"]), timeout
            )
        except (_TransportError, TimeoutError) as error:
            raise LostActionReply(self, request) from error

    async def reconnect(self) -> dict[str, Any]:
        if self.websocket is not None:
            try:
                await self.websocket.close()
            except (ConnectionClosed, WebSocketException, OSError, TimeoutError):
                pass
        self.after_frame_seq = self.cursor
        self._pending.clear()
        return await self.connect()

    async def close(self) -> None:
        """Close this Session without changing Membership or Cursor state."""

        websocket, self.websocket = self.websocket, None
        self.live = False
        if websocket is not None:
            try:
                await websocket.close()
            except (ConnectionClosed, WebSocketException, OSError, TimeoutError):
                pass

    async def events(self) -> AsyncIterator[dict[str, Any]]:
        if not self._sync_complete or not self.live:
            raise RuntimeError("room is not live; call sync() after attach")
        while True:
            try:
                message = self._pending.pop(0) if self._pending else await self._receive()
            except ProtocolError as error:
                if error.code in {"slow_consumer", "storage_unavailable"}:
                    self.live = False
                    self.websocket = None
                raise
            if message["type"] == "observation.deliver":
                body = message["body"]
                frame_seq = body["frame_seq"]
                if (
                    self._last_delivered_frame_seq is not None
                    and frame_seq <= self._last_delivered_frame_seq
                ):
                    raise ProtocolError(
                        "invalid_envelope", "observation frames are not strictly increasing", False
                    )
                self._last_delivered_frame_seq = frame_seq
                if self._frame_head is None or frame_seq > self._frame_head:
                    self._frame_head = frame_seq
                yield body
            elif message["type"] == "error":
                try:
                    self._raise_error(message)
                except ProtocolError as error:
                    if error.code in {"slow_consumer", "storage_unavailable"}:
                        self.live = False
                        self.websocket = None
                    raise

    def _action_request(
        self, action_type: str, payload: Any, action_id: str | None, expected: int | None
    ) -> dict[str, Any]:
        if not isinstance(action_type, str) or not action_type:
            raise ValueError("action_type must be non-empty")
        if len(action_type.encode("utf-8")) > MAX_ACTION_TYPE_BYTES:
            raise ValueError("action_type exceeds the configured bound")
        if action_id is not None and (not isinstance(action_id, str) or not action_id):
            raise ValueError("action_id must be non-empty")
        if expected is not None:
            _validate_nonnegative_integer(expected, "based_on_room_seq")
        try:
            payload_bytes = json.dumps(
                payload, allow_nan=False, ensure_ascii=False, separators=(",", ":")
            ).encode("utf-8")
            _validate_canonical_value(payload, "payload")
        except (TypeError, ValueError) as error:
            raise ValueError("payload must be valid JSON") from error
        if len(payload_bytes) > MAX_ACTION_PAYLOAD_BYTES:
            raise ValueError("payload exceeds the configured bound")
        request = {
            "request_id": _ulid(),
            "body": {
                "room_id": self.room_id,
                "member_id": self.member_id,
                "action_id": action_id or _ulid(),
                "based_on_room_seq": expected if expected is not None else self._room_seq(),
                "action_type": action_type,
                "payload": copy.deepcopy(payload),
            },
        }
        self._validate_action_request(request)
        return request

    def _remember_action(self, request: dict[str, Any]) -> None:
        """Reject a locally changed body for an already-used Action identity.

        The server remains authoritative across processes and reconnects. This
        bounded client-side record prevents an accidental same-process retry
        from changing the semantic request before it reaches the server.
        """

        body = request["body"]
        action_id = body["action_id"]
        fingerprint = _canonical_json(body, "action request")
        previous = self._action_fingerprints.get(action_id)
        if previous is not None and previous != fingerprint:
            raise ProtocolError(
                "idempotency_conflict",
                "Action identity was already used with a different request",
                False,
            )
        self._action_fingerprints[action_id] = fingerprint

    def _validate_action_request(self, request: Any) -> None:
        if not isinstance(request, dict) or set(request) != {"request_id", "body"}:
            raise ProtocolError("invalid_payload", "Action request shape is invalid", False)
        _validate_ulid(request.get("request_id"), "request_id")
        body = request["body"]
        if not isinstance(body, dict) or set(body) != {
            "room_id",
            "member_id",
            "action_id",
            "based_on_room_seq",
            "action_type",
            "payload",
        }:
            raise ProtocolError("invalid_payload", "Action request body is invalid", False)
        if body["room_id"] != self.room_id or body["member_id"] != self.member_id:
            raise ProtocolError(
                "forbidden", "Action request is not addressed to this membership", False
            )
        _wire_string(body["action_id"], "action_id", max_bytes=256)
        _validate_nonnegative_integer(body["based_on_room_seq"], "based_on_room_seq")
        _wire_string(body["action_type"], "action_type", max_bytes=MAX_ACTION_TYPE_BYTES)
        try:
            _validate_canonical_value(body["payload"], "payload")
            payload_bytes = json.dumps(
                body["payload"], allow_nan=False, ensure_ascii=False, separators=(",", ":")
            ).encode("utf-8")
        except (TypeError, ValueError) as error:
            raise ProtocolError("invalid_payload", "payload is not valid JSON", False) from error
        if len(payload_bytes) > MAX_ACTION_PAYLOAD_BYTES:
            raise ProtocolError("invalid_payload", "payload exceeds the configured bound", False)

    def _validate_welcome(self, welcome: dict[str, Any]) -> None:
        _validate_ulid(welcome.get("session_id"), "session_id")
        if welcome.get("selected_protocol") != PROTOCOL:
            raise ProtocolError(
                "unsupported_protocol", "server selected an incompatible protocol", False
            )
        if not isinstance(welcome.get("server_version"), str) or not welcome["server_version"]:
            raise ProtocolError("invalid_envelope", "server welcome is invalid", False)
        _validate_nonnegative_integer(welcome.get("heartbeat_interval_ms"), "heartbeat_interval_ms")
        maximum = _validate_nonnegative_integer(
            welcome.get("maximum_message_bytes"), "maximum_message_bytes"
        )
        if maximum > MAX_MESSAGE_BYTES:
            raise ProtocolError("invalid_envelope", "server message bound is unsupported", False)
        if not isinstance(welcome.get("authenticated_principal"), dict):
            raise ProtocolError("invalid_envelope", "server principal is invalid", False)
        principal = welcome["authenticated_principal"]
        if (
            set(principal) != {"principal_id", "kind"}
            or not isinstance(principal["principal_id"], str)
            or principal["kind"] not in {"human", "agent"}
        ):
            raise ProtocolError("invalid_envelope", "server principal is invalid", False)

    def _validate_attached(self, attached: dict[str, Any]) -> None:
        if attached.get("room_id") != self.room_id or attached.get("member_id") != self.member_id:
            raise ProtocolError(
                "forbidden", "attach response is not addressed to this membership", False
            )
        token = attached.get("sync_token")
        if not isinstance(token, str) or not token:
            raise ProtocolError("invalid_envelope", "attach sync token is invalid", False)
        branch = attached.get("sync")
        if not isinstance(branch, dict):
            raise ProtocolError("invalid_envelope", "attach sync branch is invalid", False)
        kind = branch.get("kind")
        if kind == "retained_frames":
            if set(branch) != {"kind", "cursor_exclusive", "through_frame_head"}:
                raise ProtocolError("invalid_envelope", "attach sync branch is invalid", False)
            _validate_nonnegative_integer(branch["cursor_exclusive"], "cursor_exclusive")
            _validate_nonnegative_integer(branch["through_frame_head"], "through_frame_head")
        elif kind == "projection_reset":
            if set(branch) != {"kind", "baseline_frame_head", "reason"}:
                raise ProtocolError("invalid_envelope", "attach sync branch is invalid", False)
            _validate_nonnegative_integer(branch["baseline_frame_head"], "baseline_frame_head")
            if not isinstance(branch["reason"], str) or not branch["reason"]:
                raise ProtocolError("invalid_envelope", "attach sync branch is invalid", False)
        else:
            raise ProtocolError("invalid_envelope", "attach sync branch is invalid", False)

    def _room_seq(self) -> int:
        if self.attached is None:
            raise RuntimeError("room is not attached")
        room_head = self.attached.get("room_head")
        if not isinstance(room_head, dict):
            raise ProtocolError("invalid_envelope", "room head is invalid", False)
        return _validate_nonnegative_integer(room_head.get("room_seq"), "room_seq")

    def _install_action_result(self, body: dict[str, Any]) -> None:
        if body.get("room_id") != self.room_id or body.get("member_id") != self.member_id:
            raise ProtocolError(
                "forbidden", "Action result is not addressed to this membership", False
            )
        if "room_head" in body:
            self._install_room_head(body["room_head"], "Action result")
            return
        current = self._room_seq()
        reported = _validate_nonnegative_integer(body["current_room_seq"], "current_room_seq")
        if reported > current or body.get("code") in {"stale_head", "stale_room_state"}:
            self._needs_resync = True

    def _install_room_head(self, candidate: Any, source: str) -> None:
        _validate_room_head(candidate, f"{source}.room_head", room_id=self.room_id)
        if self.attached is None:
            raise RuntimeError("room is not attached")
        current = self.attached.get("room_head")
        if not isinstance(current, dict):
            raise ProtocolError("invalid_envelope", "Room Head is unavailable", False)
        if set(current) != _ROOM_HEAD_FIELDS:
            self.attached["room_head"] = copy.deepcopy(candidate)
            return
        current_seq = _validate_nonnegative_integer(current.get("room_seq"), "room_seq")
        candidate_seq = candidate["room_seq"]
        if candidate_seq == current_seq and candidate != current:
            raise ProtocolError(
                "invalid_envelope",
                f"{source} changed the complete Room Head at one sequence",
                False,
            )
        if candidate_seq > current_seq:
            self.attached["room_head"] = copy.deepcopy(candidate)

    async def _send(self, message_type: str, body: Any, request_id: str | None = None) -> None:
        if self.websocket is None:
            raise _TransportError("storage_unavailable", "WebSocket is not connected", True)
        try:
            await self.websocket.send(_dumps(message_type, _ulid(), body, request_id))
        except (ConnectionClosed, WebSocketException, OSError, TimeoutError) as error:
            raise _TransportError("storage_unavailable", "WebSocket send failed", True) from error

    async def _receive(self) -> dict[str, Any]:
        if self.websocket is None:
            raise _TransportError("storage_unavailable", "WebSocket is not connected", True)
        try:
            raw = await self.websocket.recv()
        except (ConnectionClosed, WebSocketException, OSError, TimeoutError) as error:
            raise _TransportError(
                "storage_unavailable", "WebSocket receive failed", True
            ) from error
        message = _loads(raw)
        self._validate_membership_scope(message)
        if message["type"] == "server.ping":
            await self._send("client.pong", {})
            return await self._receive()
        return message

    async def _receive_type(self, message_type: str) -> dict[str, Any]:
        deferred: list[dict[str, Any]] = []
        while True:
            message = self._pending.pop(0) if self._pending else await self._receive()
            if message["type"] == "error":
                self._pending = deferred + self._pending
                self._raise_error(message)
            if message["type"] == message_type:
                self._pending = deferred + self._pending
                return message["body"]
            deferred.append(message)

    async def _receive_action(self, request_id: str, action_id: str) -> dict[str, Any]:
        deferred: list[dict[str, Any]] = []
        while True:
            message = self._pending.pop(0) if self._pending else await self._receive()
            if message["type"] == "error":
                if message.get("request_id") in {None, request_id}:
                    self._pending = deferred + self._pending
                    self._raise_error(message)
                deferred.append(message)
                continue
            if message.get("request_id") == request_id and message["type"] in {
                "action.accepted",
                "action.rejected",
            }:
                body = message["body"]
                if body.get("action_id") != action_id:
                    self._pending = deferred + self._pending
                    raise ProtocolError(
                        "invalid_envelope",
                        "Action result identity does not match the request",
                        False,
                    )
                self._pending = deferred + self._pending
                self._install_action_result(body)
                return body
            deferred.append(message)

    async def _collect_sync(self, branch: dict[str, Any], through: int) -> None:
        """Read only the captured sync batch, without consuming the ACK reply."""

        backlog, self._pending = self._pending, []
        collected: list[dict[str, Any]] = []
        try:
            while not self._sync_batch_ready(branch, through, collected):
                message = backlog.pop(0) if backlog else await self._receive()
                if message["type"] == "error":
                    collected.insert(0, message)
                    self._raise_error(message)
                if message["type"] == "room.sync_acked":
                    raise ProtocolError(
                        "sync_barrier_mismatch",
                        "server sent an unsolicited sync acknowledgement",
                        True,
                    )
                collected.append(message)
        except BaseException:
            self._pending = collected + backlog + self._pending
            raise
        self._pending = collected + backlog + self._pending

    def _sync_batch_ready(
        self, branch: dict[str, Any], through: int, pending: list[dict[str, Any]] | None = None
    ) -> bool:
        pending = self._pending if pending is None else pending
        reset_messages = [message for message in pending if message["type"] == "projection.reset"]
        if branch["kind"] == "projection_reset":
            return bool(reset_messages)
        if reset_messages:
            return True
        expected = list(range(branch["cursor_exclusive"] + 1, through + 1))
        baseline = [
            message["body"]["frame_seq"]
            for message in pending
            if message["type"] == "observation.deliver" and message["body"]["frame_seq"] <= through
        ]
        return baseline == expected

    def _install_sync(self, branch: dict[str, Any], through: int) -> list[dict[str, Any]]:
        """Validate and install one captured sync batch as a single state change."""

        pending = list(self._pending)
        reset_messages = [message for message in pending if message["type"] == "projection.reset"]
        frame_messages = [
            message for message in pending if message["type"] == "observation.deliver"
        ]
        if branch["kind"] == "projection_reset":
            if len(reset_messages) != 1:
                raise ProtocolError(
                    "invalid_envelope", "projection reset sync did not contain one reset", False
                )
            reset = reset_messages[0]["body"]
            if reset["baseline_frame_head"] != through:
                raise ProtocolError(
                    "sync_barrier_mismatch", "projection reset baseline does not match attach", True
                )
            attached_head = self.attached.get("room_head") if self.attached else None
            if reset["room_head"] != attached_head:
                raise ProtocolError(
                    "sync_barrier_mismatch",
                    "projection reset does not match the captured Room Head",
                    True,
                )
            if any(frame["body"]["frame_seq"] <= through for frame in frame_messages):
                raise ProtocolError(
                    "invalid_envelope",
                    "reset sync contains a frame at or before its baseline",
                    False,
                )
        elif reset_messages:
            raise ProtocolError(
                "invalid_envelope", "retained-frame sync unexpectedly contained a reset", False
            )

        if branch["kind"] == "retained_frames":
            cursor_exclusive = branch["cursor_exclusive"]
            expected = list(range(cursor_exclusive + 1, through + 1))
            baseline = [
                frame["body"]["frame_seq"]
                for frame in frame_messages
                if frame["body"]["frame_seq"] <= through
            ]
            if baseline != expected:
                raise ProtocolError(
                    "cursor_out_of_range",
                    "retained sync did not contain the complete captured range",
                    True,
                )

        sequences = [frame["body"]["frame_seq"] for frame in frame_messages]
        if sequences != sorted(set(sequences)):
            raise ProtocolError(
                "invalid_envelope", "observation frames are duplicated or out of order", False
            )

        reset = [message["body"] for message in reset_messages]
        frames = [message["body"] for message in frame_messages]
        installed = [*reset, *frames]
        remaining = [
            message
            for message in pending
            if message["type"] not in {"projection.reset", "observation.deliver"}
        ]
        # Commit all derived state only after every invariant above succeeds.
        self._pending = remaining
        if reset:
            self.last_projection_reset = copy.deepcopy(reset[0])
        self.last_sync_frames = copy.deepcopy(frames)
        # A reset establishes a baseline even when the server has no suffix
        # frames in this batch.  Retaining that barrier prevents a duplicate
        # frame from being accepted immediately after reconnect/reset.
        self._last_delivered_frame_seq = max(through, sequences[-1] if sequences else through)
        return copy.deepcopy(installed)

    def _validate_membership_scope(self, message: dict[str, Any]) -> None:
        """Fail closed if a server frame is addressed to another Membership."""

        if message["type"] not in {
            "room.attached",
            "projection.reset",
            "observation.deliver",
            "observation.acked",
            "action.accepted",
            "action.rejected",
        }:
            return
        body = message.get("body")
        if not isinstance(body, dict):
            raise ProtocolError("invalid_envelope", "membership frame body is invalid", False)
        if body.get("room_id") != self.room_id or body.get("member_id") != self.member_id:
            raise ProtocolError("forbidden", "frame is not addressed to this membership", False)

    @staticmethod
    def _raise_error(message: dict[str, Any]) -> None:
        body = message["body"]
        code = body.get("code")
        message_text = body.get("message")
        retryable = body.get("retryable")
        if (
            not isinstance(code, str)
            or not code
            or not isinstance(message_text, str)
            or not message_text
            or not isinstance(retryable, bool)
        ):
            raise ProtocolError("invalid_envelope", "server error body is invalid", False)
        raise ProtocolError(code, message_text, retryable, body.get("details"))

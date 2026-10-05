"""Explicit fresh-work hints for the immutable compact Room policy.

These checks grant no authority. Do not apply them to accepted retry resolution.
"""

from types import MappingProxyType
from typing import Any

PAYLOAD_BUDGET_V1_ID = "worldstream/payload-budget/v1"
PAYLOAD_BUDGET_V1_LIMITS = MappingProxyType(
    {
        "control_metadata": 4096,
        "action_payload": 32768,
        "external_input_payload": 32768,
        "creation_configuration": 32768,
        "domain_event_item": 8192,
        "domain_events_array": 262144,
        "timer_change_item": 4096,
        "timer_changes_array": 65536,
        "attention_signal_item": 4096,
        "attention_signals_array": 65536,
        "effects": 393216,
        "transition": 524288,
        "activity_state": 262144,
        "core_state": 131072,
        "authoritative_state": 524288,
        "genesis": 786432,
        "observation": 32768,
        "projection": 262144,
        "artifact_reference": 2048,
    }
)


def canonical_payload_bytes(value: Any) -> bytes:
    """Count exact canonical UTF-8, including JSON delimiters and escapes."""
    from .client import _canonical_json

    return _canonical_json(value, "fresh payload").encode("utf-8")


def check_fresh_payload(value: Any, *, kind: str, policy_id: str) -> int:
    """Check an explicitly classified fresh value; return its canonical byte count.

    Supply the Room's exact policy identity. Aggregate kinds require the complete
    array or accounting object, not a sum that omits brackets or field names.
    """
    if policy_id != PAYLOAD_BUDGET_V1_ID:
        raise ValueError("unsupported payload policy identity")
    if not isinstance(kind, str) or kind not in PAYLOAD_BUDGET_V1_LIMITS:
        raise ValueError("unknown payload kind")
    if kind == "artifact_reference":
        raise ValueError("use check_declared_artifact_reference with an explicit schema")
    count = len(canonical_payload_bytes(value))
    maximum = PAYLOAD_BUDGET_V1_LIMITS[kind]
    if count > maximum:
        raise ValueError(f"{kind} has {count} canonical bytes; limit is {maximum}")
    return count


def check_declared_artifact_reference(value: Any, *, schema_id: str, policy_id: str) -> int:
    """Check only a reference the application declares under an exact schema.

    The application validates that schema. This helper grants no download authority.
    """
    if not isinstance(schema_id, str) or not schema_id:
        raise ValueError("an explicit Artifact reference schema identity is required")
    if policy_id != PAYLOAD_BUDGET_V1_ID:
        raise ValueError("unsupported payload policy identity")
    count = len(canonical_payload_bytes(value))
    maximum = PAYLOAD_BUDGET_V1_LIMITS["artifact_reference"]
    if count > maximum:
        raise ValueError(f"artifact_reference has {count} canonical bytes; limit is {maximum}")
    return count

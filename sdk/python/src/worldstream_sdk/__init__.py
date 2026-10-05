"""Async Python client for the public WorldStream Room protocol."""

import json
from importlib.resources import files

from .client import Client, LostActionReply, LostRunnerReply, ProtocolError, Room, Runner
from .payload_budgets import (
    PAYLOAD_BUDGET_V1_ID,
    PAYLOAD_BUDGET_V1_LIMITS,
    canonical_payload_bytes,
    check_declared_artifact_reference,
    check_fresh_payload,
)

CLIENT_CONTRACT_IDENTITY = json.loads(
    files(__package__).joinpath("compatibility_identity.json").read_text(encoding="utf-8")
)

__all__ = [
    "CLIENT_CONTRACT_IDENTITY",
    "PAYLOAD_BUDGET_V1_ID",
    "PAYLOAD_BUDGET_V1_LIMITS",
    "Client",
    "LostActionReply",
    "LostRunnerReply",
    "ProtocolError",
    "Room",
    "Runner",
    "__version__",
    "canonical_payload_bytes",
    "check_declared_artifact_reference",
    "check_fresh_payload",
]

__version__ = "0.1.0"

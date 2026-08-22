"""Async Python client for the public WorldStream Room protocol."""

import json
from importlib.resources import files

from .client import Client, LostActionReply, LostRunnerReply, ProtocolError, Room, Runner

CLIENT_CONTRACT_IDENTITY = json.loads(
    files(__package__).joinpath("compatibility_identity.json").read_text(encoding="utf-8")
)

__all__ = [
    "CLIENT_CONTRACT_IDENTITY",
    "Client",
    "LostActionReply",
    "LostRunnerReply",
    "ProtocolError",
    "Room",
    "Runner",
    "__version__",
]

__version__ = "0.1.0"

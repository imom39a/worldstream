"""Strict, redacted loading of scoped CLI credential exports."""

from __future__ import annotations

import json
import os
import re
import stat
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit, urlunsplit

MAX_CREDENTIAL_BYTES = 64 * 1024
MEMBERSHIP_SCHEMA = "worldstream/membership-credentials/v1"
RUNNER_SCHEMA = "worldstream/runner-credentials/v1"
BEARER = re.compile(r"wsb1:[0-9a-f]{64}\Z")
MEMBERSHIP_KEYS = {
    "schema", "operation", "seat", "runtime_url", "room_id", "member_id",
    "principal_id", "pack", "role", "scopes", "bearer",
}
RUNNER_KEYS = {
    "schema", "operation", "seat", "runtime_url", "runner_id",
    "owner_principal_id", "pack", "permitted_memberships", "scopes", "bearer",
}


class CredentialError(ValueError):
    """A closed error that never includes credential content or a path."""


def load_membership(path: Path) -> dict[str, Any]:
    """Load one exact Membership credential document."""
    value = _load(path, MEMBERSHIP_SCHEMA, MEMBERSHIP_KEYS, "/v1/stream")
    for key in ("operation", "seat", "room_id", "member_id", "principal_id", "role"):
        _text(value.get(key))
    _pack(value.get("pack"))
    _strings(value.get("scopes"))
    return value


def load_runner(path: Path) -> dict[str, Any]:
    """Load one exact external-Runner credential document."""
    value = _load(path, RUNNER_SCHEMA, RUNNER_KEYS, "/v1/runner/stream")
    for key in ("operation", "seat", "runner_id", "owner_principal_id"):
        _text(value.get(key))
    _pack(value.get("pack"))
    _strings(value.get("scopes"))
    targets = value.get("permitted_memberships")
    if not isinstance(targets, list) or not targets or len(targets) > 256:
        raise CredentialError("credential_memberships_invalid")
    for target in targets:
        if not isinstance(target, dict) or set(target) != {"room_id", "member_id"}:
            raise CredentialError("credential_memberships_invalid")
        _text(target["room_id"])
        _text(target["member_id"])
    return value


def sdk_base_url(document: dict[str, Any]) -> str:
    """Return the validated HTTP(S) base corresponding to runtime_url."""
    endpoint = "/v1/stream" if document.get("schema") == MEMBERSHIP_SCHEMA else "/v1/runner/stream"
    return _endpoint(document.get("runtime_url"), endpoint)


def validate_pair(membership: dict[str, Any], runner: dict[str, Any]) -> None:
    """Require one external Runner authority for this exact Agent Membership."""
    target = {"room_id": membership["room_id"], "member_id": membership["member_id"]}
    if membership["operation"] != runner["operation"] or membership["pack"] != runner["pack"]:
        raise CredentialError("credential_pair_mismatch")
    if sum(candidate == target for candidate in runner["permitted_memberships"]) != 1:
        raise CredentialError("credential_pair_mismatch")


def _load(path: Path, schema: str, keys: set[str], endpoint: str) -> dict[str, Any]:
    try:
        before = path.lstat()
        if not stat.S_ISREG(before.st_mode) or (os.name != "nt" and before.st_mode & 0o077):
            raise CredentialError("credential_file_unprotected")
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(path, flags)
        with os.fdopen(descriptor, "rb") as source:
            after = os.fstat(source.fileno())
            if (before.st_dev, before.st_ino) != (after.st_dev, after.st_ino):
                raise CredentialError("credential_file_changed")
            raw = source.read(MAX_CREDENTIAL_BYTES + 1)
        if not raw or len(raw) > MAX_CREDENTIAL_BYTES:
            raise CredentialError("credential_file_invalid")
        value = json.loads(raw)
    except CredentialError:
        raise
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise CredentialError("credential_file_invalid") from error
    if not isinstance(value, dict) or set(value) != keys or value.get("schema") != schema:
        raise CredentialError("credential_document_invalid")
    _endpoint(value.get("runtime_url"), endpoint)
    if not isinstance(value.get("bearer"), str) or BEARER.fullmatch(value["bearer"]) is None:
        raise CredentialError("credential_bearer_invalid")
    return value


def _endpoint(value: object, endpoint: str) -> str:
    if not isinstance(value, str) or len(value) > 2048:
        raise CredentialError("credential_endpoint_invalid")
    parsed = urlsplit(value)
    if (parsed.scheme not in {"ws", "wss"} or not parsed.netloc or parsed.username
            or parsed.password or parsed.path != endpoint or parsed.query or parsed.fragment):
        raise CredentialError("credential_endpoint_invalid")
    scheme = "http" if parsed.scheme == "ws" else "https"
    return urlunsplit((scheme, parsed.netloc, "", "", ""))


def _text(value: object) -> str:
    if not isinstance(value, str) or not value or len(value.encode()) > 4096 or any(ord(c) < 32 for c in value):
        raise CredentialError("credential_document_invalid")
    return value


def _strings(value: object) -> None:
    if not isinstance(value, list) or not value or len(value) > 32:
        raise CredentialError("credential_document_invalid")
    for item in value:
        _text(item)


def _pack(value: object) -> None:
    if not isinstance(value, dict) or set(value) != {"id", "version", "digest"}:
        raise CredentialError("credential_document_invalid")
    for item in value.values():
        _text(item)

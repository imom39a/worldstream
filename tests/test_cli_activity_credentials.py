import json
import os
from pathlib import Path

import pytest

from examples.cli_activity.credentials import (
    CredentialError,
    load_membership,
    load_runner,
    sdk_base_url,
    validate_pair,
)


def document(schema: str) -> dict:
    common = {"schema": schema, "operation": "setup-1", "seat": "insider",
              "pack": {"id": "worldstream.agent-heist", "version": "0.2.0", "digest": "blake3:" + "a" * 64},
              "scopes": ["scope"], "bearer": "wsb1:" + "b" * 64}
    if "membership" in schema:
        return common | {"runtime_url": "ws://127.0.0.1:19410/v1/stream", "room_id": "room",
                         "member_id": "member", "principal_id": "principal", "role": "insider"}
    return common | {"runtime_url": "ws://127.0.0.1:19410/v1/runner/stream", "runner_id": "runner",
                     "owner_principal_id": "principal",
                     "permitted_memberships": [{"room_id": "room", "member_id": "member"}]}


def protected(path: Path, value: dict) -> None:
    path.write_text(json.dumps(value), encoding="utf-8")
    if os.name != "nt":
        path.chmod(0o600)


def test_loads_exact_separate_exports_and_converts_known_endpoints(tmp_path: Path) -> None:
    membership_path, runner_path = tmp_path / "member.json", tmp_path / "runner.json"
    protected(membership_path, document("worldstream/membership-credentials/v1"))
    protected(runner_path, document("worldstream/runner-credentials/v1"))
    membership, runner = load_membership(membership_path), load_runner(runner_path)
    validate_pair(membership, runner)
    assert sdk_base_url(membership) == "http://127.0.0.1:19410"
    assert sdk_base_url(runner) == "http://127.0.0.1:19410"


def test_pair_requires_one_exact_room_membership_tuple(tmp_path: Path) -> None:
    membership, runner = document("worldstream/membership-credentials/v1"), document("worldstream/runner-credentials/v1")
    runner["permitted_memberships"] = [
        {"room_id": "room", "member_id": "other"}, {"room_id": "other", "member_id": "member"}
    ]
    with pytest.raises(CredentialError, match="credential_pair_mismatch"):
        validate_pair(membership, runner)


def test_rejects_unknown_fields_permissions_and_endpoint_without_echo(tmp_path: Path) -> None:
    for change in (lambda value: value.update({"unexpected": True}),
                   lambda value: value.update({"bearer": "wsb1_" + "b" * 64}),
                   lambda value: value.update({"runtime_url": "ws://127.0.0.1:19410/v1/stream?secret=canary"})):
        path = tmp_path / f"credential-{len(list(tmp_path.iterdir()))}.json"
        value = document("worldstream/membership-credentials/v1")
        change(value)
        protected(path, value)
        with pytest.raises(CredentialError) as raised:
            load_membership(path)
        assert "canary" not in str(raised.value)
    if os.name != "nt":
        path = tmp_path / "open.json"
        protected(path, document("worldstream/membership-credentials/v1"))
        path.chmod(0o644)
        with pytest.raises(CredentialError, match="credential_file_unprotected"):
            load_membership(path)

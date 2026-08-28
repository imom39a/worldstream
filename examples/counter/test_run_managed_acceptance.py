from __future__ import annotations

import importlib.util
import json
import sqlite3
import tempfile
from pathlib import Path

path = Path(__file__).with_name("run_managed_acceptance.py")
spec = importlib.util.spec_from_file_location("managed_counter_acceptance", path)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def test_redaction_removes_bearers_and_run_ids() -> None:
    result = module.redact("wsb1:" + "a" * 64 + " 01ARZ3NDEKTSV4RRFFQ69G5FC2")
    assert "a" * 64 not in result
    assert "01ARZ3NDEKTSV4RRFFQ69G5FC2" not in result


def test_profile_uses_a_named_credential_not_a_secret_reference() -> None:
    profile = module._profile(19431)
    assert profile["schema"].endswith("/v2")
    assert profile["managed_provider_credential_id"] == "local-openai"
    assert "managed_provider_credential_reference" not in profile
    assert "secret_settings" not in profile


def test_public_dto_boundary_rejects_a_bearer() -> None:
    try:
        module._assert_public({"bearer": "wsb1:" + "b" * 64})
    except module.AcceptanceFailure as error:
        assert error.code == "public_dto_contains_sensitive_field"
    else:
        raise AssertionError("bearer accepted")


def test_public_dto_boundary_keeps_safe_operational_labels() -> None:
    module._assert_public(
        {
            "schema": "worldstream/studio-task-agent-attention/v1",
            "next_action": "restore_runner_authority",
            "credential_id": "local-openai",
        }
    )


def test_human_action_uses_the_exact_observed_private_ack_offer() -> None:
    offer_id, digest = module._private_ack_offer(
        {
            "room_head": {"room_seq": 0},
            "delivery": [
                {
                    "kind": "projection_reset",
                    "body": {
                        "projection": {
                            "action_offers": [
                                {
                                    "action_type": "increment",
                                    "payload_schema_digest": "blake3:one",
                                },
                                {
                                    "action_type": "private_ack",
                                    "payload_schema_digest": "blake3:two",
                                },
                            ]
                        }
                    },
                }
            ],
        }
    )
    assert offer_id == "0:private_ack:1"
    assert digest == "blake3:two"


def test_pending_attention_accepts_stopped_managed_reference_inventory() -> None:
    status = {
        "seats": [
            {
                "seat_id": "managed-counter",
                "activation": {"state": "attention", "waiting": 1, "leased": 0},
            }
        ]
    }
    assert module._pending_activation_state(status, "managed-counter") == "attention"
    assert module._pending_activation_state(status, "human-counter") is None


def test_provider_request_counts_come_from_one_status_snapshot() -> None:
    before_restart = {"received_requests": 1, "accepted_requests": 1}
    final = {"received_requests": 2, "accepted_requests": 2}

    assert module._provider_request_counts(final) == {
        "received": 2,
        "accepted": 2,
    }
    assert module._provider_request_counts(final) != module._provider_request_counts(
        before_restart
    )


def test_failure_diagnostic_allowlists_only_safe_operational_aggregates() -> None:
    diagnostic = module._failure_diagnostic_from_public(
        {
            "state": "running",
            "failure": {"message": "private context must not escape"},
            "attempts": 2,
            "activation": {
                "state": "leased",
                "last_confirmed_disposition": "handled",
                "token": "wsb1:" + "a" * 64,
            },
            "assignment_id": "01ARZ3NDEKTSV4RRFFQ69G5FC2",
        },
        {
            "runners": [{"token": "leak"}],
            "managed_hosts": [{"context": "private"}],
            "restart_attempts": [{"response": "private"}],
            "unexpected": "wsb1:" + "b" * 64,
        },
        {
            "received_requests": 2,
            "accepted_requests": 2,
            "credential": "private",
            "response": "wsb1:" + "c" * 64,
        },
    )

    assert diagnostic == {
        "schema": module.SCHEMA,
        "event": "failure_diagnostic",
        "managed_host": {
            "state": "running",
            "failure_present": True,
            "attempts": 2,
        },
        "activation": {
            "state": "leased",
            "last_confirmed_disposition": "handled",
        },
        "runner_attention": {
            "runners": 1,
            "managed_hosts": 1,
            "restart_attempts": 1,
        },
        "provider": {"received": 2, "accepted": 2},
    }
    emitted = json.dumps(diagnostic, sort_keys=True)
    for forbidden in ("token", "context", "credential", "response", "01ARZ"):
        assert forbidden not in emitted
    assert "a" * 64 not in emitted
    assert "b" * 64 not in emitted
    assert "c" * 64 not in emitted


def test_failure_diagnostic_replaces_invalid_fields_with_safe_defaults() -> None:
    diagnostic = module._failure_diagnostic_from_public(
        {
            "state": "private_state",
            "failure": "private failure",
            "attempts": module.MAX_FAILURE_DIAGNOSTIC_COUNT + 1,
            "activation": {
                "state": "private_state",
                "last_confirmed_disposition": "private_disposition",
            },
        },
        {"runners": [object()] * (module.MAX_FAILURE_DIAGNOSTIC_COUNT + 1)},
        {"received_requests": True, "accepted_requests": -1},
    )

    assert diagnostic["managed_host"] == {
        "state": "unavailable",
        "failure_present": False,
        "attempts": 0,
    }
    assert diagnostic["activation"] == {
        "state": "unavailable",
        "last_confirmed_disposition": "none",
    }
    assert diagnostic["runner_attention"] == {
        "runners": 0,
        "managed_hosts": 0,
        "restart_attempts": 0,
    }
    assert diagnostic["provider"] == {"received": 0, "accepted": 0}


def _completion_database(root: Path, room_id: str) -> None:
    database = root / "data" / "worldstream.sqlite3"
    database.parent.mkdir(parents=True)
    connection = sqlite3.connect(database)
    try:
        connection.execute(
            "CREATE TABLE activation_operation_receipts ("
            "room_id TEXT NOT NULL, operation_id TEXT NOT NULL, "
            "operation_kind TEXT NOT NULL, canonical_request_hash BLOB NOT NULL, "
            "activation_id TEXT, result_code TEXT NOT NULL)"
        )
        connection.execute(
            "INSERT INTO activation_operation_receipts VALUES (?, ?, ?, ?, ?, ?)",
            (
                room_id,
                "complete-action-1",
                "complete",
                b"a" * 32,
                module.HUMAN,
                "completed",
            ),
        )
        connection.commit()
    finally:
        connection.close()
    database.chmod(0o600)


def test_daemon_completion_receipt_is_single_and_identity_bound() -> None:
    room_id = "01ARZ3NDEKTSV4RRFFQ69G5FB3"
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        _completion_database(root, room_id)
        before = module._daemon_operational_completion_receipts(root, room_id)
        after = module._daemon_operational_completion_receipts(root, room_id)
    assert len(before) == 1
    assert after == before


def test_activation_launch_reference_is_only_a_secret_absence_canary() -> None:
    assignment = module.AGENT
    launch_reference = "a" * 64
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        active = (
            root / "studio" / "assignment-mcp-launches" / f"{assignment}.active.json"
        )
        active.parent.mkdir(parents=True)
        active.write_text(json.dumps({"launch_reference": launch_reference}))
        first = module._activation_launch_reference_canary(
            root, assignment, "activation-launch-reference-before-restart"
        )
        second = module._activation_launch_reference_canary(
            root, assignment, "activation-launch-reference-after-restart"
        )
        assert first.read_bytes() == second.read_bytes()
        assert first.name == "activation-launch-reference-before-restart"


def test_publish_report_copies_only_the_owned_candidate() -> None:
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        candidate = root / "candidate.json"
        destination = root / "published" / "report.json"
        candidate.write_text('{"status":"completed"}\n', encoding="utf-8")
        candidate.chmod(0o600)
        module._publish_report(candidate, destination)
        assert destination.read_text(encoding="utf-8") == candidate.read_text(
            encoding="utf-8"
        )
        assert destination.stat().st_mode & 0o077 == 0


def test_protected_task_setup_requires_owner_only_durable_record() -> None:
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        record = root / "studio" / "task-setups" / "managed-counter.json"
        record.parent.mkdir(parents=True)
        record.write_text('{"state":"ready","room_id":"room"}', encoding="utf-8")
        record.chmod(0o600)
        assert module._protected_task_setup(root, "managed-counter") == {
            "state": "ready",
            "room_id": "room",
        }

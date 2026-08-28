"""Read-only evidence validator for the UI-driven Counter Studio acceptance."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request
from itertools import chain
from pathlib import Path
from typing import Any

REPOSITORY = Path(__file__).resolve().parents[2]
SCHEMA = "worldstream/counter-studio-browser-verification/v1"
CONTROL_SCHEMA = "worldstream/counter-studio-demo-control/v1"
COUNTER_V4_DIGEST = "blake3:2a1d2e493cbaffa3803724dfef42d35c167db2237aa9b1e113dfb79679e9c052"
ULID = re.compile(r"^[0-9A-HJKMNP-TV-Z]{26}$")
HASH = re.compile(r"blake3:[0-9a-f]{64}")
CONSOLE_FORBIDDEN = re.compile(r"(?i)ws[bh]1:[0-9a-f]+|\b(?:room_id|member_id|membership_id|bearer|token_hash|secret_reference|host_authority|invocation_context|provider_response|private_context_canary)\b")
STRUCTURAL_PRIVATE_FIELD = re.compile(r'(?i)["\'](?:prompt|memory|private_context|provider_response)["\']\s*:')
URL_EVIDENCE_SCHEMA = "worldstream/counter-browser-url-evidence/v1"
PUBLIC_CONTEXT_LEAVES = {"increment", "private_ack", "handled"}
PRIVATE_CONTEXT_FIELD = re.compile(
    r"(?:private|secret|token|bearer|credential|authority|context|prompt|memory|"
    r"instruction|observation|payload)",
    re.IGNORECASE,
)
PUBLIC_CONTEXT_FIELD = re.compile(
    r"(?:schema|digest|pack|domain|action_type|offer_id|reason|projection_schema)$",
    re.IGNORECASE,
)


class VerificationFailure(RuntimeError):
    pass


def safe_json(path: Path, code: str) -> dict[str, Any]:
    try:
        metadata = path.lstat()
        if (
            not stat.S_ISREG(metadata.st_mode)
            or stat.S_ISLNK(metadata.st_mode)
            or metadata.st_mode & 0o077
        ):
            raise VerificationFailure(code)
        value = json.loads(path.read_text("utf-8"))
    except (OSError, ValueError, json.JSONDecodeError) as error:
        raise VerificationFailure(code) from error
    if not isinstance(value, dict):
        raise VerificationFailure(code)
    return value


def safe_bytes(path: Path, code: str) -> bytes:
    try:
        metadata = path.lstat()
        if not stat.S_ISREG(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
            raise VerificationFailure(code)
        return path.read_bytes()
    except OSError as error:
        raise VerificationFailure(code) from error


def loopback_url(value: Any, code: str) -> str:
    if not isinstance(value, str):
        raise VerificationFailure(code)
    try:
        from urllib.parse import urlparse

        parsed = urlparse(value)
    except ValueError as error:
        raise VerificationFailure(code) from error
    if (
        parsed.scheme != "http"
        or parsed.hostname not in {"127.0.0.1", "localhost", "::1"}
        or parsed.username is not None
        or parsed.password is not None
        or parsed.query
        or parsed.fragment
    ):
        raise VerificationFailure(code)
    return value.rstrip("/")


def control_metadata(path: Path) -> dict[str, str]:
    value = safe_json(path, "control_file_invalid")
    urls = value.get("urls")
    if (
        value.get("schema") != CONTROL_SCHEMA
        or value.get("status") != "ready"
        or not isinstance(urls, dict)
    ):
        raise VerificationFailure("control_file_not_ready")
    return {
        "supervisor": loopback_url(urls.get("supervisor"), "control_supervisor_invalid"),
        "studio": loopback_url(urls.get("studio"), "control_studio_invalid"),
        "console": loopback_url(urls.get("console"), "control_console_invalid"),
    }


def public_get(base: str, path: str) -> dict[str, Any]:
    try:
        with urllib.request.urlopen(
            urllib.request.Request(base + path, headers={"Accept": "application/json"}),
            timeout=5,
        ) as response:
            value = json.loads(response.read())
    except (OSError, ValueError, urllib.error.HTTPError, json.JSONDecodeError) as error:
        raise VerificationFailure("safe_status_unavailable") from error
    if not isinstance(value, dict):
        raise VerificationFailure("safe_status_invalid")
    return value


def setup_for_draft(state_dir: Path, draft_name: str) -> dict[str, Any]:
    if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,127}", draft_name):
        raise VerificationFailure("draft_name_invalid")
    return safe_json(state_dir / "task-setups" / f"{draft_name}.json", "task_setup_missing")


def setup_bindings(setup: dict[str, Any]) -> tuple[str, str, str, str]:
    room_id = setup.get("room_id")
    seats = setup.get("seats")
    if not isinstance(room_id, str) or not ULID.fullmatch(room_id) or not isinstance(seats, list):
        raise VerificationFailure("task_setup_invalid")
    human = next((seat for seat in seats if isinstance(seat, dict) and seat.get("principal_kind") == "human"), None)
    agent = next((seat for seat in seats if isinstance(seat, dict) and seat.get("principal_kind") == "agent"), None)
    if not isinstance(human, dict) or not isinstance(agent, dict):
        raise VerificationFailure("distinct_memberships_missing")
    human_member, agent_member = human.get("member_id"), agent.get("member_id")
    runner = agent.get("runner")
    runner_id = runner.get("runner_id") if isinstance(runner, dict) else None
    if (
        not all(isinstance(value, str) and ULID.fullmatch(value) for value in (human_member, agent_member, runner_id))
        or human_member == agent_member
    ):
        raise VerificationFailure("distinct_memberships_or_runner_invalid")
    return room_id, human_member, agent_member, runner_id


def records(root: Path, code: str) -> list[dict[str, Any]]:
    try:
        entries = sorted(root.glob("*.json"))
    except OSError as error:
        raise VerificationFailure(code) from error
    if not entries:
        raise VerificationFailure(code)
    return [safe_json(path, code) for path in entries]


def exact_template_lineage(state_dir: Path, draft_name: str, setup: dict[str, Any]) -> bool:
    usages = [value for value in records(state_dir / "task-templates" / "usages", "template_usage_missing") if isinstance(value.get("draft"), dict) and value["draft"].get("draft_id") == draft_name]
    if len(usages) != 1:
        raise VerificationFailure("editable_draft_lineage_ambiguous")
    usage = usages[0]
    template_id, revision_id, instantiated = usage.get("template_id"), usage.get("revision"), usage.get("draft")
    revisions = [value for value in records(state_dir / "task-templates" / "revisions", "template_revision_missing") if value.get("template_id") == template_id and value.get("revision") == revision_id]
    if len(revisions) != 1 or not isinstance(instantiated, dict):
        raise VerificationFailure("exact_template_missing")
    template = revisions[0]
    pack, configuration, seats = template.get("pack"), template.get("configuration"), template.get("seats")
    if (
        template.get("schema") != "worldstream/studio-task-template/v1"
        or not isinstance(pack, dict) or pack != {"id": "worldstream.counter", "version": "4.0.0", "digest": COUNTER_V4_DIGEST}
        or not isinstance(configuration, dict) or configuration.get("initial_value") != 0 or configuration.get("maximum_value") != 3
        or template.get("source_draft_id") == draft_name or not isinstance(seats, list)
    ):
        raise VerificationFailure("exact_counter_template_invalid")
    human = [seat for seat in seats if isinstance(seat, dict) and seat.get("principal_kind") == "human"]
    managed = [seat for seat in seats if isinstance(seat, dict) and seat.get("principal_kind") == "agent" and seat.get("agent_assignment") == "managed"]
    if (
        len(human) != 1 or len(managed) != 1 or human[0].get("required") is not True
        or managed[0].get("required") is not True
        or not isinstance(managed[0].get("agent_profile"), dict)
        or not isinstance(managed[0].get("runner_template"), dict)
    ):
        raise VerificationFailure("template_seat_bindings_invalid")
    setup_seats = setup.get("seats")
    setup_agent = next((seat for seat in setup_seats if isinstance(seat, dict) and seat.get("principal_kind") == "agent"), None) if isinstance(setup_seats, list) else None
    setup_runner = setup_agent.get("runner") if isinstance(setup_agent, dict) else None
    managed_assignment = setup_runner.get("managed_assignment") if isinstance(setup_runner, dict) else None
    if (
        not isinstance(setup_agent, dict) or setup_agent.get("agent_profile") != managed[0]["agent_profile"]
        or not isinstance(managed_assignment, dict)
        or managed[0]["runner_template"] != {"template_id": managed_assignment.get("template_id"), "revision": managed_assignment.get("template_revision")}
    ):
        raise VerificationFailure("template_setup_exact_binding_mismatch")
    expected_instantiated = {
        "schema": "worldstream/studio-room-draft/v1", "draft_id": draft_name,
        "pack": template["pack"], "configuration": template["configuration"], "seats": template["seats"],
        "readiness": template["readiness"], "operator_view": template.get("operator_view", False), "last_valid_step": "readiness",
    }
    if instantiated != expected_instantiated:
        raise VerificationFailure("instantiated_draft_snapshot_invalid")
    source = [value for value in records(state_dir / "room-drafts", "source_draft_missing") if value.get("draft_id") == template.get("source_draft_id")]
    final = [value for value in records(state_dir / "room-drafts", "editable_draft_missing") if value.get("draft_id") == draft_name]
    expected_review = {key: template.get(key) for key in ("pack", "configuration", "seats", "readiness", "operator_view")}
    if (
        len(source) != 1 or len(final) != 1
        or {key: source[0].get(key) for key in expected_review} != expected_review
        or {key: final[0].get(key) for key in expected_review} != expected_review
        or source[0].get("last_valid_step") != "review"
        or final[0].get("last_valid_step") != "review"
    ):
        raise VerificationFailure("source_or_editable_draft_lineage_invalid")
    return True


def creation_and_ready_genesis(state_dir: Path, draft_name: str, setup: dict[str, Any], room_id: str) -> bool:
    operations = [value for value in records(state_dir / "room-creations", "room_creation_missing") if value.get("draft_id") == draft_name]
    if len(operations) != 1:
        raise VerificationFailure("room_creation_not_exactly_once")
    operation = operations[0]
    if operation.get("schema") != "worldstream/studio-room-creation-operation/v1" or operation.get("state") != "succeeded" or operation.get("room_id") != room_id:
        raise VerificationFailure("room_creation_setup_mismatch")
    if setup.get("state") != "ready" or setup.get("draft_id") != draft_name or setup.get("room_id") != room_id or setup.get("launch_applicability") != "active_at_genesis" or setup.get("launch") is not None:
        raise VerificationFailure("ready_at_genesis_not_retained")
    return True


def completion_receipts(state_dir: Path, room_id: str) -> tuple[tuple[str, str, bytes], ...]:
    database = state_dir.parent / "data" / "worldstream.sqlite3"
    try:
        metadata = database.lstat()
        if not stat.S_ISREG(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
            raise VerificationFailure("completion_database_invalid")
        connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
        try:
            connection.execute("PRAGMA query_only = ON")
            rows = connection.execute(
                "SELECT operation_id, activation_id, canonical_request_hash "
                "FROM activation_operation_receipts WHERE room_id = ? "
                "AND operation_kind = 'complete' AND result_code = 'completed' "
                "ORDER BY operation_id",
                (room_id,),
            ).fetchall()
        finally:
            connection.close()
    except (OSError, sqlite3.Error) as error:
        raise VerificationFailure("completion_receipts_unavailable") from error
    result = []
    for operation_id, activation_id, request_hash in rows:
        if (
            not isinstance(operation_id, str)
            or not isinstance(activation_id, str)
            or not ULID.fullmatch(activation_id)
            or not isinstance(request_hash, bytes)
            or len(request_hash) != 32
        ):
            raise VerificationFailure("completion_receipt_invalid")
        result.append((operation_id, activation_id, request_hash))
    if len(result) != 1:
        raise VerificationFailure("completion_receipt_count_invalid")
    return tuple(result)


def retained_invocation_context_values(
    state_dir: Path, room_id: str, activation_id: str
) -> set[bytes]:
    """Read retained claim contexts only to create local absence sentinels."""

    database = state_dir.parent / "data" / "worldstream.sqlite3"
    try:
        metadata = database.lstat()
        if not stat.S_ISREG(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
            raise VerificationFailure("invocation_context_database_invalid")
        connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
        try:
            connection.execute("PRAGMA query_only = ON")
            rows = connection.execute(
                "SELECT context_bytes FROM activation_operation_receipts "
                "WHERE room_id = ? AND activation_id = ? AND operation_kind = 'claim' "
                "AND result_code = 'granted' AND context_bytes IS NOT NULL",
                (room_id, activation_id),
            ).fetchall()
        finally:
            connection.close()
    except (OSError, sqlite3.Error) as error:
        raise VerificationFailure("invocation_context_unavailable") from error
    raw_values = {value for (value,) in rows if isinstance(value, bytes) and value}
    # The shared scanner accepts bounded 128-bit-or-larger sentinels. Keep an
    # exact small context when possible; otherwise retain its private leaves.
    values = {value for value in raw_values if 16 <= len(value) <= 512}
    for context in raw_values:
        values.update(
            value
            for value in private_context_leaf_values(context)
            if 16 <= len(value) <= 512
        )
    if not values or len(values) > 64:
        raise VerificationFailure("invocation_context_retained_value_invalid")
    return values


def private_context_leaf_values(context: bytes) -> set[bytes]:
    """Extract private-bearing context leaves, excluding shared offer metadata."""

    try:
        decoded = json.loads(context)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise VerificationFailure("invocation_context_retained_value_invalid") from error
    if not isinstance(decoded, dict):
        raise VerificationFailure("invocation_context_retained_value_invalid")
    leaves: set[bytes] = {
        candidate.encode("utf-8")
        for key in ("activation_id", "claim_id")
        if isinstance(candidate := decoded.get(key), str) and candidate
    }

    def collect_private(item: Any, private_scope: bool = False) -> None:
        if isinstance(item, str):
            if private_scope and len(item) >= 16 and item not in PUBLIC_CONTEXT_LEAVES:
                leaves.add(item.encode("utf-8"))
        elif isinstance(item, dict):
            for key, nested in item.items():
                if PUBLIC_CONTEXT_FIELD.search(key):
                    continue
                nested_private = private_scope or bool(PRIVATE_CONTEXT_FIELD.search(key))
                if nested_private or isinstance(nested, (dict, list)):
                    collect_private(nested, nested_private)
        elif isinstance(item, list):
            for nested in item:
                collect_private(nested, private_scope)

    def decode_private_bytes(item: Any) -> None:
        if not isinstance(item, list) or any(not isinstance(byte, int) or not 0 <= byte <= 255 for byte in item):
            return
        try:
            collect_private(json.loads(bytes(item)))
        except (UnicodeDecodeError, json.JSONDecodeError):
            return

    projection = decoded.get("projection_bytes")
    decode_private_bytes(projection)
    delivery = decoded.get("delivery")
    frames = delivery.get("frames") if isinstance(delivery, dict) else None
    if isinstance(frames, list):
        for frame in frames:
            if isinstance(frame, dict):
                decode_private_bytes(frame.get("payload_bytes"))
    return leaves


def bound_launch(state_dir: Path, assignment_id: str, room_id: str, runner_id: str) -> tuple[str, dict[str, Any], dict[str, Any]]:
    root = state_dir / "assignment-mcp-launches"
    active = safe_json(root / f"{assignment_id}.active.json", "active_launch_missing")
    reference = active.get("launch_reference")
    if active.get("schema") != "worldstream/assignment-mcp-active-launch/v1" or active.get("assignment_id") != assignment_id or not isinstance(reference, str) or not re.fullmatch(r"[0-9a-f]{64}", reference):
        raise VerificationFailure("active_launch_invalid")
    launch = safe_json(root / f"{reference}.json", "launch_record_missing")
    activation = safe_json(root / f"{reference}.activation.json", "activation_launch_record_missing")
    if (
        launch.get("schema") != "worldstream/assignment-mcp-launch/v1"
        or launch.get("assignment_id") != assignment_id or launch.get("room_id") != room_id
        or activation.get("schema") != "worldstream/assignment-mcp-activation-launch/v1"
        or activation.get("assignment_id") != assignment_id or activation.get("room_id") != room_id
        or activation.get("runner_id") != runner_id
    ):
        raise VerificationFailure("launch_binding_mismatch")
    return reference, launch, activation


def recovery_evidence(
    state_dir: Path,
    room_id: str,
    assignment_id: str,
    launch_reference: str,
    completion: tuple[str, str, bytes],
) -> bool:
    operation_root = state_dir / "managed-agent-hosts" / "operations"
    try:
        operations = [
            safe_json(path, "managed_host_operation_invalid")
            for path in operation_root.glob("*.json")
        ]
    except OSError as error:
        raise VerificationFailure("managed_host_operation_missing") from error
    matches = [
        operation
        for operation in operations
        if operation.get("schema") == "worldstream/managed-agent-host-operation/v1"
        and operation.get("assignment_id") == assignment_id
    ]
    if len(matches) != 1:
        raise VerificationFailure("managed_host_operation_missing")
    operation = matches[0]
    if not isinstance(operation.get("attempts"), int) or operation["attempts"] < 2:
        raise VerificationFailure("managed_host_restart_not_retained")
    root = state_dir / "assignment-mcp-activations" / launch_reference / "operations"
    try:
        records = [
            safe_json(path, "activation_ledger_invalid")
            for path in root.glob("*.json")
        ]
    except OSError as error:
        raise VerificationFailure("activation_ledger_invalid") from error
    expected_operation, expected_activation, _request_hash = completion
    matching = [
        record for record in records
        if record.get("schema") == "worldstream/studio-assignment-mcp-operation@1"
        and record.get("phase") == "complete"
        and isinstance(intent := record.get("intent"), dict)
        and isinstance(identity := intent.get("identity"), dict)
        and identity.get("assignment_id") == assignment_id
        and identity.get("operation_id") == expected_operation
        and isinstance(kind := intent.get("kind"), dict)
        and kind.get("kind") == "activation_completion"
        and kind.get("activation_id") == expected_activation
        and isinstance(kind.get("claim_id"), str)
        and isinstance(kind.get("acquisition_cursor"), int)
        and kind["acquisition_cursor"] > 0
        and isinstance(kind.get("lease_generation"), int)
        and kind["lease_generation"] > 0
        and isinstance(remote := record.get("remote_acceptance"), dict)
        and remote.get("outcome") == "accepted"
        and isinstance(remote_kind := remote.get("kind"), dict)
        and remote_kind.get("kind") == "activation_completion"
        and remote_kind.get("activation_id") == expected_activation
        and remote_kind.get("completion_id") == kind.get("completion_id")
    ]
    if len(matching) != 1:
        raise VerificationFailure("activation_lease_recovery_not_retained")
    kind = matching[0]["intent"]["kind"]
    retained_successful_acquisition(
        state_dir,
        room_id,
        expected_activation,
        kind["claim_id"],
        kind["lease_generation"],
    )
    return True


def retained_successful_acquisition(
    state_dir: Path,
    room_id: str,
    activation_id: str,
    claim_id: str,
    lease_generation: int,
) -> bool:
    """Bind a completion to the exact durable granted claim that supplied its lease."""

    database = state_dir.parent / "data" / "worldstream.sqlite3"
    try:
        metadata = database.lstat()
        if not stat.S_ISREG(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
            raise VerificationFailure("activation_acquisition_database_invalid")
        connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
        try:
            connection.execute("PRAGMA query_only = ON")
            rows = connection.execute(
                "SELECT result_bytes FROM activation_operation_receipts "
                "WHERE room_id = ? AND activation_id = ? "
                "AND operation_kind = 'claim' AND result_code = 'granted' "
                "ORDER BY operation_id",
                (room_id, activation_id),
            ).fetchall()
        finally:
            connection.close()
    except (OSError, sqlite3.Error) as error:
        raise VerificationFailure("activation_acquisition_unavailable") from error

    matches = []
    for (raw_result,) in rows:
        if not isinstance(raw_result, bytes):
            continue
        try:
            result = json.loads(raw_result)
        except (TypeError, UnicodeDecodeError, json.JSONDecodeError):
            continue
        context = result.get("context") if isinstance(result, dict) else None
        if (
            isinstance(context, dict)
            and result.get("activation_id") == activation_id
            and result.get("claim_id") == claim_id
            and result.get("code") == "granted"
            and result.get("state") == "leased"
            and result.get("lease_generation") == lease_generation
            and context.get("activation_id") == activation_id
            and context.get("claim_id") == claim_id
            and context.get("lease_generation") == lease_generation
        ):
            matches.append(result)
    if len(matches) != 1:
        raise VerificationFailure("activation_acquisition_not_retained")
    return True


def committed_increment_evidence(
    state_dir: Path, assignment_id: str, launch_reference: str, activation_id: str
) -> bool:
    """Bind the retained managed Action request to the completed Activation."""

    root = state_dir / "assignment-mcp-operations" / launch_reference
    matches: list[dict[str, Any]] = []
    try:
        paths = sorted(root.glob("*.json"))
    except OSError as error:
        raise VerificationFailure("increment_action_receipt_unavailable") from error
    for path in paths:
        record = safe_json(path, "increment_action_receipt_invalid")
        intent = record.get("intent")
        if not isinstance(intent, dict):
            continue
        identity, kind = intent.get("identity"), intent.get("kind")
        request = intent.get("canonical_request")
        if not isinstance(identity, dict) or not isinstance(kind, dict) or not isinstance(request, str):
            continue
        if identity.get("assignment_id") != assignment_id or identity.get("operation_id") != activation_id:
            continue
        if kind != {"kind": "action", "action_id": activation_id}:
            continue
        try:
            exact = json.loads(request)
        except json.JSONDecodeError as error:
            raise VerificationFailure("increment_action_receipt_invalid") from error
        if not isinstance(exact, dict):
            raise VerificationFailure("increment_action_receipt_invalid")
        if (
            exact.get("assignment_id") == assignment_id
            and exact.get("operation_id") == activation_id
            and exact.get("request_id") == activation_id
            and exact.get("action_id") == activation_id
            and exact.get("action_type") == "increment"
            and isinstance(exact.get("offer_id"), str)
            and exact["offer_id"]
            and record.get("phase") == "complete"
        ):
            matches.append(exact)
    if len(matches) != 1:
        raise VerificationFailure("increment_action_receipt_missing")
    return True


def walk_records(value: Any):
    if isinstance(value, dict):
        yield value
        for nested in value.values():
            yield from walk_records(nested)
    elif isinstance(value, list):
        for nested in value:
            yield from walk_records(nested)


def provider_counts(url: str) -> tuple[int, int]:
    value = public_get(url, "/fixture/status")
    received, accepted = value.get("received_requests"), value.get("accepted_requests")
    if not isinstance(received, int) or not isinstance(accepted, int) or received < 1 or accepted < 1:
        raise VerificationFailure("provider_stats_invalid")
    return received, accepted


def url_evidence(path: Path) -> bool:
    """Require both browser request channels and keep their private values local."""

    value = safe_json(path, "browser_url_evidence_invalid")
    if value.get("schema") != URL_EVIDENCE_SCHEMA:
        raise VerificationFailure("browser_url_evidence_invalid")
    channels = (value.get("studio_request_urls"), value.get("console_request_urls"))
    current = (value.get("studio_current_url"), value.get("console_current_url"))
    if not all(isinstance(channel, list) and channel for channel in channels):
        raise VerificationFailure("browser_url_evidence_incomplete")
    if not all(isinstance(url, str) and url for url in current):
        raise VerificationFailure("browser_url_evidence_incomplete")
    for channel in (*channels, current):
        for url in channel:
            if not isinstance(url, str) or len(url) > 8 * 1024:
                raise VerificationFailure("browser_url_evidence_invalid")
            if CONSOLE_FORBIDDEN.search(url) or STRUCTURAL_PRIVATE_FIELD.search(url):
                raise VerificationFailure("browser_url_disclosed_protected_material")
    return True


def public_room_state(supervisor: str, room_id: str) -> tuple[int, int, dict[str, str]]:
    inventory = public_get(supervisor, "/api/v1/rooms?limit=50")
    rooms = inventory.get("rooms")
    if inventory.get("schema") != "worldstream/studio-room-inventory/v1" or not isinstance(rooms, list) or len(rooms) != 1:
        raise VerificationFailure("room_inventory_not_exactly_one")
    room = rooms[0] if rooms else None
    if not isinstance(room, dict) or room.get("room_id") != room_id:
        raise VerificationFailure("room_inventory_setup_mismatch")
    head, pack = room.get("room_head"), room.get("pack")
    if not isinstance(head, dict) or not isinstance(pack, dict) or pack.get("id") != "worldstream.counter" or pack.get("version") != "4.0.0":
        raise VerificationFailure("room_inventory_not_counter_v4")
    value = public_get(supervisor, f"/api/v1/rooms/{room_id}/operator-view")
    counter = value.get("counter")
    operator_head = value.get("room_head")
    number = counter.get("value") if isinstance(counter, dict) else None
    sequence = head.get("room_seq")
    if number != 2 or sequence != 2 or not isinstance(operator_head, dict) or operator_head.get("room_seq") != sequence:
        raise VerificationFailure("public_counter_not_exactly_two")
    hashes = {key: head.get(key) for key in ("genesis_or_transition_hash", "authoritative_state_hash")}
    if not all(isinstance(value, str) and HASH.fullmatch(value) for value in hashes.values()):
        raise VerificationFailure("authoritative_head_hash_invalid")
    return number, sequence, hashes


def dom_evidence(studio_dom: Path, console_dom: Path, require_replay: bool, head: dict[str, str]) -> tuple[bool, bool]:
    studio, console = safe_bytes(studio_dom, "studio_dom_missing"), safe_bytes(console_dom, "console_dom_missing")
    studio_text, console_text = studio.decode("utf-8", "replace"), console.decode("utf-8", "replace")
    if STRUCTURAL_PRIVATE_FIELD.search(studio_text) or STRUCTURAL_PRIVATE_FIELD.search(console_text) or CONSOLE_FORBIDDEN.search(console_text):
        raise VerificationFailure("browser_dom_disclosed_protected_material")
    lineage = re.search(r"Lineage hash:\s*(blake3:[0-9a-f]{64})", console_text)
    authoritative = re.search(r"Authoritative state hash:\s*(blake3:[0-9a-f]{64})", console_text)
    replay = (
        "Verified Canonical History at sequence 2" in console_text
        and lineage is not None and authoritative is not None
        and lineage.group(1) == head["genesis_or_transition_hash"]
        and authoritative.group(1) == head["authoritative_state_hash"]
    )
    if require_replay and not replay:
        raise VerificationFailure("replay_dom_evidence_missing")
    return True, True


def retained_secret_canaries(
    state_dir: Path,
    setup: dict[str, Any],
    destination: Path,
    launch_reference: str,
    room_id: str,
    activation_id: str,
) -> dict[str, Path]:
    seats = setup.get("seats")
    if not isinstance(seats, list):
        raise VerificationFailure("secret_setup_invalid")
    values: dict[str, bytes] = {}
    references: dict[str, str] = {}
    for seat in seats:
        if not isinstance(seat, dict):
            continue
        kind = seat.get("principal_kind")
        capability = seat.get("member_capability")
        if kind not in {"human", "agent"} or not isinstance(capability, dict):
            continue
        reference = capability.get("secret_reference")
        if not isinstance(reference, str) or not re.fullmatch(r"[0-9a-f]{64}", reference):
            raise VerificationFailure("member_secret_reference_invalid")
        values[f"member_{kind}"] = safe_bytes(
            state_dir / "secrets" / f"membership-{reference}.secret", "member_secret_missing"
        )
        references[f"member_{kind}"] = reference
        if kind == "agent":
            runner = seat.get("runner")
            capability = runner.get("capability") if isinstance(runner, dict) else None
            reference = capability.get("secret_reference") if isinstance(capability, dict) else None
            if not isinstance(reference, str) or not re.fullmatch(r"[0-9a-f]{64}", reference):
                raise VerificationFailure("runner_secret_reference_invalid")
            values["runner"] = safe_bytes(
                state_dir / "secrets" / f"runner-{reference}.secret", "runner_secret_missing"
            )
            references["runner"] = reference
    host = safe_json(state_dir / "host-authority-reference.json", "host_reference_missing")
    host_reference = host.get("reference")
    model = safe_json(state_dir.parent / "model-provider-credentials" / "local-openai.json", "model_reference_missing")
    secret = model.get("secret")
    model_reference = secret.get("reference") if isinstance(secret, dict) else None
    for name, kind, reference in (
        ("host", "host", host_reference),
        ("model", "model-provider", model_reference),
    ):
        if not isinstance(reference, str) or not re.fullmatch(r"[0-9a-f]{64}", reference):
            raise VerificationFailure("owner_secret_reference_invalid")
        values[name] = safe_bytes(state_dir / "secrets" / f"{kind}-{reference}.secret", "owner_secret_missing")
        references[name] = reference
    context_hashes: set[str] = set()
    operations = state_dir / "assignment-mcp-activations" / launch_reference / "operations"
    try:
        operation_paths = sorted(operations.glob("*.json"))
    except OSError as error:
        raise VerificationFailure("private_context_ledger_unavailable") from error
    for operation_path in operation_paths:
        operation = safe_json(operation_path, "private_context_ledger_invalid")
        intent = operation.get("intent")
        request = intent.get("canonical_request") if isinstance(intent, dict) else None
        if not isinstance(request, str):
            continue
        try:
            retained = json.loads(request)
        except json.JSONDecodeError as error:
            raise VerificationFailure("private_context_ledger_invalid") from error
        context_hash = retained.get("context_hash") if isinstance(retained, dict) else None
        if isinstance(context_hash, str) and HASH.fullmatch(context_hash):
            context_hashes.add(context_hash)
    if len(context_hashes) != 1:
        raise VerificationFailure("private_context_canary_missing")
    context_hash = context_hashes.pop()
    values["private_context"] = context_hash.encode("ascii")
    references["private_context"] = context_hash
    for number, context in enumerate(
        sorted(retained_invocation_context_values(state_dir, room_id, activation_id))
    ):
        name = f"private_context_value_{number}"
        values[name] = context
        references[name] = context.hex()
    if not {"member_human", "member_agent", "runner", "host", "model", "private_context"}.issubset(values):
        raise VerificationFailure("secret_canary_bindings_incomplete")
    destination.mkdir(mode=0o700, parents=True, exist_ok=True)
    paths: dict[str, Path] = {}
    for name, value in values.items():
        for suffix, material in (("value", value), ("reference", references[name].encode("ascii"))):
            path = destination / f"{name}-{suffix}.canary"
            with open(path, "xb") as output:
                output.write(material)
            path.chmod(0o600)
            paths[f"{name}_{suffix}"] = path
    return paths


def scan_secret_absence(
    args: argparse.Namespace,
    setup: dict[str, Any],
    launch: dict[str, Any],
    activation_launch: dict[str, Any],
    launch_reference: str,
    room_id: str,
    activation_id: str,
) -> bool:
    args.witness.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    canary_root = Path(tempfile.mkdtemp(prefix=f".imo89-{args.mode}-", dir=args.witness.parent))
    try:
        canaries = retained_secret_canaries(
            args.state_dir, setup, canary_root, launch_reference, room_id, activation_id
        )
        for name, record in (("launch_member_reference", launch), ("launch_runner_reference", activation_launch)):
            reference = record.get("authority_reference")
            if not isinstance(reference, str) or not re.fullmatch(r"[0-9a-f]{64}", reference):
                raise VerificationFailure("launch_reference_invalid")
            path = canary_root / f"{name}.canary"
            with open(path, "xb") as output:
                output.write(reference.encode("ascii"))
            path.chmod(0o600)
            canaries[name] = path
        channels = [
            f"studio-dom={args.studio_dom}",
            f"console-dom={args.console_dom}",
            f"browser-urls={args.url_evidence}",
            *args.channel,
        ]
        for name, canary in canaries.items():
            output = canary_root / f"absence-{name}.json"
            run = subprocess.run(
                [
                    sys.executable,
                    str(REPOSITORY / "scripts/verify-secret-absence.py"),
                    "--sentinel-file", str(canary),
                    *chain.from_iterable(("--channel", channel) for channel in channels),
                    "--output", str(output),
                ],
                cwd=REPOSITORY,
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )
            if run.returncode != 0:
                raise VerificationFailure("secret_absence_scan_failed")
        return True
    except (OSError, subprocess.TimeoutExpired) as error:
        raise VerificationFailure("secret_absence_scan_unavailable") from error
    finally:
        shutil.rmtree(canary_root, ignore_errors=True)


def write_private(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=".imo89-witness-", dir=path.parent)
    candidate = Path(temporary)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump(value, output, sort_keys=True, separators=(",", ":"))
            output.flush()
            os.fsync(output.fileno())
        candidate.chmod(0o600)
        os.replace(candidate, path)
        path.chmod(0o600)
    finally:
        candidate.unlink(missing_ok=True)


def private_witness(path: Path) -> dict[str, Any]:
    value = safe_json(path, "private_witness_missing")
    try:
        if stat.S_IMODE(path.stat().st_mode) != 0o600:
            raise VerificationFailure("private_witness_mode_invalid")
    except OSError as error:
        raise VerificationFailure("private_witness_missing") from error
    return value


def bounded_report(mode: str, provider: tuple[int, int], unchanged: bool, secret_absence: bool) -> dict[str, Any]:
    return {
        "schema": SCHEMA,
        "status": "passed",
        "mode": mode,
        "ui_created": {"exact_template": True, "editable_draft": True, "one_room": True, "ready_at_genesis": True},
        "authorities": {"distinct_human_agent_memberships": True, "runner_bound_separately": True},
        "managed_recovery": {"cursor_and_lease_retained": True, "one_completion_receipt": True},
        "counter": {"value": 2, "room_seq": 2},
        "provider": {"received": provider[0], "accepted": provider[1]},
        "replay": {"visible_hash_parity": mode == "check", "no_effects_after_baseline": unchanged},
        "secret_absence": {"studio_dom": True, "console_dom": True, "retained_canaries": secret_absence},
    }


def verify(args: argparse.Namespace) -> dict[str, Any]:
    metadata = control_metadata(args.control_file)
    setup = setup_for_draft(args.state_dir, args.draft_name)
    room_id, human_member, agent_member, runner_id = setup_bindings(setup)
    exact_template_lineage(args.state_dir, args.draft_name, setup)
    creation_and_ready_genesis(args.state_dir, args.draft_name, setup, room_id)
    provider = provider_counts(loopback_url(args.provider_status_url, "provider_url_invalid"))
    counter_value, room_seq, head = public_room_state(metadata["supervisor"], room_id)
    receipts = completion_receipts(args.state_dir, room_id)
    launch_reference, launch, activation_launch = bound_launch(args.state_dir, agent_member, room_id, runner_id)
    recovery_evidence(args.state_dir, room_id, agent_member, launch_reference, receipts[0])
    committed_increment_evidence(args.state_dir, agent_member, launch_reference, receipts[0][1])
    dom_evidence(args.studio_dom, args.console_dom, args.mode == "check", head)
    url_evidence(args.url_evidence)
    secret_absence = scan_secret_absence(
        args, setup, launch, activation_launch, launch_reference, room_id, receipts[0][1]
    )
    witness = {
        "schema": SCHEMA,
        "room_id": room_id,
        "human_member_id": human_member,
        "agent_member_id": agent_member,
        "runner_id": runner_id,
        "completion_receipts": [[operation, activation, digest.hex()] for operation, activation, digest in receipts],
        "provider": list(provider),
        "room": [counter_value, room_seq, head["genesis_or_transition_hash"], head["authoritative_state_hash"]],
        "launch_reference": launch_reference,
    }
    unchanged = True
    if args.mode == "check":
        before = private_witness(args.before_witness)
        unchanged = (
            before.get("room_id") == room_id
            and before.get("completion_receipts") == witness["completion_receipts"]
            and before.get("provider") == witness["provider"]
            and before.get("room") == witness["room"]
        )
        if not unchanged:
            raise VerificationFailure("replay_or_restart_changed_retained_state")
    write_private(args.witness, witness)
    return bounded_report(args.mode, provider, unchanged, secret_absence)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("capture", "check"))
    parser.add_argument("--control-file", type=Path, required=True)
    parser.add_argument("--state-dir", type=Path, required=True)
    parser.add_argument("--draft-name", required=True)
    parser.add_argument("--provider-status-url", required=True)
    parser.add_argument("--studio-dom", type=Path, required=True)
    parser.add_argument("--console-dom", type=Path, required=True)
    parser.add_argument("--url-evidence", type=Path, required=True)
    parser.add_argument("--witness", type=Path, required=True)
    parser.add_argument("--before-witness", type=Path)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--channel", action="append", default=[])
    args = parser.parse_args()
    if args.mode == "check" and args.before_witness is None:
        parser.error("check requires --before-witness")
    try:
        report = verify(args)
        if args.report is not None:
            write_private(args.report, report)
        print(json.dumps(report, sort_keys=True, separators=(",", ":")))
        return 0
    except VerificationFailure as error:
        print(json.dumps({"schema": SCHEMA, "status": "blocked", "reason_code": str(error)}, sort_keys=True, separators=(",", ":")))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())

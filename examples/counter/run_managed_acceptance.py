#!/usr/bin/env python3
"""Run IMO-88's retained Controller-level Counter regression harness.

This internal harness predates the CLI-first cutover and calls lower-level
Controller HTTP operations directly. It is not an onboarding path or evidence
that the supported CLI flow passed. Retained ``studio`` schema, state-path,
report, and variable names are compatibility identifiers, not a web frontend.
The local OpenAI-compatible provider remains a deterministic development
fixture, not a production model-provider recommendation.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import re
import shutil
import signal
import socket
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from collections.abc import Callable
from itertools import chain
from pathlib import Path
from typing import Any

REPOSITORY = Path(__file__).resolve().parents[2]
INTERNAL_HANDOFF_REQUEST_ORIGIN = "http://127.0.0.1:5184"
sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(REPOSITORY / "sdk/python/src"))
from install_managed_counter_fixture import (
    COUNTER_PACK_DIGEST,
    COUNTER_PACK_ID,
    COUNTER_PACK_REVISION,
    MODEL_CREDENTIAL_ID,
    TEMPLATE_ID,
    TEMPLATE_REVISION,
    install_fixture,
)
from worldstream_sdk import Client, ProtocolError

SCHEMA = "worldstream/imo-88-managed-counter-acceptance/v1"
BEARER = re.compile(r"(?i)wsb1:[0-9a-f]{64}")
ULID = re.compile(r"(?<![0-9A-Z])[0-9A-HJKMNP-TV-Z]{26}(?![0-9A-Z])")
SECRET_REFERENCE = re.compile(r"^[0-9a-f]{64}$")
SENSITIVE = re.compile(
    r"(?i)(bearer|credential|secret|token|reference|context|prompt|response|memory|payload)"
)
PUBLIC_FORBIDDEN = {
    "bearer",
    "token_hash",
    "secret_reference",
    "secret_ref",
    "host_authority",
    "invocation_context",
    "prompt",
    "response",
    "memory",
}
HUMAN = "01ARZ3NDEKTSV4RRFFQ69G5FB0"
AGENT = "01ARZ3NDEKTSV4RRFFQ69G5FB1"
MAX_FAILURE_DIAGNOSTIC_COUNT = 256
MANAGED_HOST_STATES = {"stopped", "starting", "running", "needs_attention"}
ACTIVATION_STATES = {"idle", "waiting", "leased", "unavailable"}
ACTIVATION_DISPOSITIONS = {"handled", "declined", "failed"}


def stage(name: str) -> None:
    """Emit only bounded lifecycle progress; never URLs, IDs, or authority."""
    print(
        json.dumps({"schema": SCHEMA, "event": "stage", "stage": name}, sort_keys=True),
        flush=True,
    )


class AcceptanceFailure(RuntimeError):
    def __init__(self, code: str) -> None:
        self.code = code
        super().__init__(code)


def redact(value: str) -> str:
    return ULID.sub("<redacted-ulid>", BEARER.sub("wsb1:<redacted>", value))


def _port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def _write_secret(path: Path, value: bytes) -> None:
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as output:
        output.write(value)
        output.flush()
        os.fsync(output.fileno())
    path.chmod(0o600)


def _write_canary(directory: Path, name: str, value: bytes) -> Path:
    """Persist one scan-only canary in the protected run root."""
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    path = directory / name
    _write_secret(path, value)
    return path


def _write_report(path: Path, evidence: dict[str, Any]) -> None:
    path.write_text(
        json.dumps(evidence, sort_keys=True, indent=2) + "\n", encoding="utf-8"
    )
    path.chmod(0o600)


def _publish_report(candidate: Path, destination: Path | None) -> None:
    """Atomically publish only a scan-verified report outside the run root."""
    if destination is None or destination == candidate:
        return
    destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(
        prefix=".imo88-report-", dir=destination.parent
    )
    temporary_path = Path(temporary)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(candidate.read_bytes())
            output.flush()
            os.fsync(output.fileno())
        temporary_path.chmod(0o600)
        os.replace(temporary_path, destination)
    finally:
        temporary_path.unlink(missing_ok=True)


def _secret_reference(value: Any) -> str:
    if not isinstance(value, str) or not SECRET_REFERENCE.fullmatch(value):
        raise AcceptanceFailure("protected_authority_reference_invalid")
    return value


def _protected_task_setup(root: Path, draft_id: str) -> dict[str, Any]:
    """Read the owned durable setup record; browser-safe status omits secrets."""
    path = root / "studio" / "task-setups" / f"{draft_id}.json"
    try:
        metadata = path.lstat()
        if (
            not stat.S_ISREG(metadata.st_mode)
            or stat.S_ISLNK(metadata.st_mode)
            or metadata.st_mode & 0o077
            or metadata.st_size > 256 * 1024
        ):
            raise AcceptanceFailure("protected_task_setup_unsafe")
        value = json.loads(path.read_text("utf-8"))
    except AcceptanceFailure:
        raise
    except (OSError, ValueError, json.JSONDecodeError) as error:
        raise AcceptanceFailure("protected_task_setup_unavailable") from error
    if not isinstance(value, dict):
        raise AcceptanceFailure("protected_task_setup_invalid")
    return value


def _vault_secret(
    root: Path, kind: str, reference: str, expected: bytes | None
) -> bytes:
    path = root / "studio" / "secrets" / f"{kind}-{reference}.secret"
    try:
        metadata = path.lstat()
        value = path.read_bytes()
    except OSError as error:
        raise AcceptanceFailure("protected_authority_material_missing") from error
    if (
        not stat.S_ISREG(metadata.st_mode)
        or stat.S_ISLNK(metadata.st_mode)
        or metadata.st_mode & 0o077
        or (expected is not None and value != expected)
        or (expected is None and len(value) != 32)
    ):
        raise AcceptanceFailure("protected_authority_material_invalid")
    return value


def _retained_authorities(
    root: Path, setup: dict[str, Any], authority_file: Path, model_file: Path
) -> tuple[dict[str, bytes], dict[str, Path], dict[str, str]]:
    """Collect exact retained values and opaque references for absence scans.

    These never enter a status record, report, command line, or environment.
    """
    seats = setup.get("seats")
    if not isinstance(seats, list):
        raise AcceptanceFailure("protected_authority_setup_invalid")
    by_seat = {seat.get("seat_id"): seat for seat in seats if isinstance(seat, dict)}
    human, managed = by_seat.get("human-counter"), by_seat.get("managed-counter")
    if not isinstance(human, dict) or not isinstance(managed, dict):
        raise AcceptanceFailure("protected_authority_setup_invalid")
    human_capability = human.get("member_capability")
    managed_capability = managed.get("member_capability")
    runner = managed.get("runner")
    runner_capability = runner.get("capability") if isinstance(runner, dict) else None
    if not all(
        isinstance(item, dict)
        for item in (human_capability, managed_capability, runner_capability)
    ):
        raise AcceptanceFailure("protected_authority_setup_invalid")
    try:
        host_binding = json.loads(
            (root / "studio" / "host-authority-reference.json").read_text("utf-8")
        )
        model_binding = json.loads(
            (root / "model-provider-credentials" / "local-openai.json").read_text(
                "utf-8"
            )
        )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        raise AcceptanceFailure("protected_authority_binding_missing") from error
    model_secret = (
        model_binding.get("secret") if isinstance(model_binding, dict) else None
    )
    references = {
        "host": _secret_reference(host_binding.get("reference")),
        "member_human": _secret_reference(human_capability.get("secret_reference")),
        "member_managed": _secret_reference(managed_capability.get("secret_reference")),
        "runner": _secret_reference(runner_capability.get("secret_reference")),
        "model": _secret_reference(
            model_secret.get("reference") if isinstance(model_secret, dict) else None
        ),
    }
    try:
        expected_host = authority_file.read_bytes()
        expected_model = model_file.read_bytes()
    except OSError as error:
        raise AcceptanceFailure("protected_authority_material_missing") from error
    values = {
        "host": _vault_secret(root, "host", references["host"], expected_host),
        "member_human": _vault_secret(
            root, "membership", references["member_human"], None
        ),
        "member_managed": _vault_secret(
            root, "membership", references["member_managed"], None
        ),
        "runner": _vault_secret(root, "runner", references["runner"], None),
        "model": _vault_secret(
            root, "model-provider", references["model"], expected_model
        ),
    }
    identities = {
        "human_member_id": human.get("member_id"),
        "runner_id": runner.get("runner_id") if isinstance(runner, dict) else None,
    }
    if not all(
        isinstance(value, str) and ULID.fullmatch(value)
        for value in identities.values()
    ):
        raise AcceptanceFailure("protected_authority_identity_invalid")
    canary_dir = root / "scan-canaries"
    sentinels = {
        f"{name}_value": _write_canary(canary_dir, f"{name}-value", value)
        for name, value in values.items()
    }
    sentinels.update(
        {
            f"{name}_reference": _write_canary(
                canary_dir, f"{name}-reference", reference.encode("ascii")
            )
            for name, reference in references.items()
        }
    )
    return values, sentinels, identities


def _environment() -> dict[str, str]:
    env = dict(os.environ)
    for key in list(env):
        if key.startswith("WORLDSTREAM__AUTHORITY__BOOTSTRAP") or SENSITIVE.search(key):
            env.pop(key, None)
    env["RUST_LOG"] = "warn"
    return env


def request(
    base: str,
    path: str,
    *,
    method: str = "GET",
    body: dict[str, Any] | None = None,
    bearer: str | None = None,
    extra_headers: dict[str, str] | None = None,
    return_headers: bool = False,
) -> Any:
    headers = {"Accept": "application/json"}
    data = None
    if body is not None:
        data = json.dumps(body, sort_keys=True, separators=(",", ":")).encode()
        headers["Content-Type"] = "application/json"
    if bearer:
        headers["Authorization"] = f"Bearer {bearer}"
    if extra_headers:
        headers.update(extra_headers)
    try:
        with urllib.request.urlopen(
            urllib.request.Request(
                base + path, data=data, method=method, headers=headers
            ),
            timeout=5,
        ) as response:
            value = json.loads(response.read())
            response_headers = dict(response.headers.items())
    except urllib.error.HTTPError as error:
        try:
            payload = json.loads(error.read())
            error_body = payload.get("error", {}) if isinstance(payload, dict) else {}
            code = error_body.get("code") if isinstance(error_body, dict) else None
            if code is None and isinstance(payload, dict):
                # The retained Controller adapters use a flat closed error envelope.
                code = payload.get("code")
            if code == "room_draft_validation_failed" and isinstance(error_body, dict):
                fields = error_body.get("field_errors")
                if isinstance(fields, list) and fields and isinstance(fields[0], dict):
                    detail = fields[0].get("code")
                    if isinstance(detail, str):
                        code = f"{code}_{detail}"
        except (OSError, ValueError, json.JSONDecodeError):
            code = None
        suffix = (
            str(code)
            if isinstance(code, str) and code.replace("_", "").isalnum()
            else str(error.code)
        )
        raise AcceptanceFailure(f"bounded_http_{suffix}") from error
    except (OSError, TimeoutError, ValueError) as error:
        raise AcceptanceFailure("bounded_http_request_failed") from error
    if not isinstance(value, dict):
        raise AcceptanceFailure("bounded_http_response_invalid")
    return (value, response_headers) if return_headers else value


def wait_for(
    name: str, fn: Callable[[], dict[str, Any] | None], timeout: float
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = fn()
        if value is not None:
            return value
        time.sleep(0.1)
    raise AcceptanceFailure(f"{name}_timed_out")


def _stop(process: subprocess.Popen[bytes] | None) -> bool:
    if process is None or process.poll() is not None:
        return True
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=10)
    except (OSError, subprocess.TimeoutExpired):
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except OSError:
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            return process.poll() is not None
    return process.poll() is not None


def _profile(port: int) -> dict[str, Any]:
    return {
        "schema": "worldstream/studio-agent-profile-publish/v2",
        "profile_id": "counter-managed",
        "revision": "v1",
        "display_name": "Counter managed reference",
        "non_secret_configuration": {},
        "host_contract": {
            "kind": "managed_reference",
            "host_contract_revision": "v1",
            "runner_template": {
                "template_id": TEMPLATE_ID,
                "revision": TEMPLATE_REVISION,
            },
            "provider": "open_ai_compatible",
            "provider_address": f"127.0.0.1:{port}",
            "model_id": "counter-deterministic",
        },
        "managed_provider_credential_id": MODEL_CREDENTIAL_ID,
    }


def _draft() -> dict[str, Any]:
    seats = [
        {
            "seat_id": "human-counter",
            "role": "counter",
            "required": False,
            "display_name": "Human Counter",
            "principal_id": HUMAN,
            "principal_kind": "human",
        },
        {
            "seat_id": "managed-counter",
            "role": "counter",
            "required": False,
            "display_name": "Managed Counter",
            "principal_id": AGENT,
            "principal_kind": "agent",
            "agent_assignment": "managed",
            "agent_profile": {"profile_id": "counter-managed", "revision": "v1"},
            "runner_template": {
                "template_id": TEMPLATE_ID,
                "revision": TEMPLATE_REVISION,
            },
        },
    ]
    return {
        "schema": "worldstream/studio-room-draft/v1",
        "draft_id": "managed-counter",
        "pack": {
            "id": COUNTER_PACK_ID,
            "version": COUNTER_PACK_REVISION,
            "digest": COUNTER_PACK_DIGEST,
        },
        "configuration": {"initial_value": 0, "maximum_value": 3},
        "seats": seats,
        "readiness": [
            {"seat_id": seat["seat_id"], "role": "counter", "required": False}
            for seat in seats
        ],
        "operator_view": True,
        "last_valid_step": "review",
    }


def _assert_public(value: Any) -> None:
    if isinstance(value, dict):
        for key, item in value.items():
            if key.lower() in PUBLIC_FORBIDDEN:
                raise AcceptanceFailure("public_dto_contains_sensitive_field")
            _assert_public(item)
    elif isinstance(value, list):
        for item in value:
            _assert_public(item)
    elif isinstance(value, str) and BEARER.search(value):
        raise AcceptanceFailure("public_dto_contains_bearer")


def private_ack(base: str, canary_dir: Path) -> tuple[Path, Path]:
    """Exercise the retained browser handoff without minting another capability."""
    console_origin = "http://127.0.0.1:5173"
    try:
        issued = request(
            base,
            "/api/v1/participant-console/handoffs",
            method="POST",
            body={"draft_id": "managed-counter", "seat_id": "human-counter"},
            extra_headers={"Origin": INTERNAL_HANDOFF_REQUEST_ORIGIN},
        )
    except AcceptanceFailure as error:
        raise AcceptanceFailure(f"human_handoff_issue_{error.code}") from error
    url = issued.get("console_url")
    if not isinstance(url, str) or "#handoff=wsh1:" not in url:
        raise AcceptanceFailure("human_handoff_not_issued")
    handoff = url.split("#handoff=", 1)[1]
    try:
        _, headers = request(
            base,
            "/api/v1/participant-console/handoffs:redeem",
            method="POST",
            extra_headers={
                "Origin": console_origin,
                "x-worldstream-participant-handoff": handoff,
            },
            return_headers=True,
        )
    except AcceptanceFailure as error:
        raise AcceptanceFailure(f"human_handoff_redeem_{error.code}") from error
    cookie = next(
        (value for key, value in headers.items() if key.lower() == "set-cookie"), None
    )
    if not isinstance(cookie, str) or not cookie.startswith("ws_participant_session="):
        raise AcceptanceFailure("human_handoff_not_redeemed")
    try:
        observed = request(
            base,
            "/api/v1/participant-console/session:observe",
            method="POST",
            body={"after_frame_seq": None},
            extra_headers={"Origin": console_origin, "Cookie": cookie},
        )
    except AcceptanceFailure as error:
        raise AcceptanceFailure(f"human_observe_{error.code}") from error
    offer_id, schema_digest = _private_ack_offer(observed)
    try:
        action = request(
            base,
            "/api/v1/participant-console/session:act",
            method="POST",
            body={
                "action_id": "01ARZ3NDEKTSV4RRFFQ69G5FB2",
                "based_on_room_seq": observed.get("room_head", {}).get("room_seq"),
                "offer_id": offer_id,
                "schema_digest": schema_digest,
                "action_type": "private_ack",
                "payload": {},
            },
            extra_headers={"Origin": console_origin, "Cookie": cookie},
        )
    except AcceptanceFailure as error:
        raise AcceptanceFailure(f"human_private_ack_{error.code}") from error
    if action.get("room_head", {}).get("room_seq") != 1:
        raise AcceptanceFailure("human_private_ack_not_committed")
    handoff_file = _write_canary(canary_dir, "handoff-value", handoff.encode("ascii"))
    session_file = _write_canary(
        canary_dir, "member-session-value", cookie.encode("ascii")
    )
    try:
        pass
    except BaseException:
        handoff_file.unlink(missing_ok=True)
        session_file.unlink(missing_ok=True)
        raise
    return handoff_file, session_file


def _private_ack_offer(observed: dict[str, Any]) -> tuple[str, str]:
    head = observed.get("room_head", {})
    sequence = head.get("room_seq") if isinstance(head, dict) else None
    delivery = observed.get("delivery")
    if not isinstance(sequence, int) or not isinstance(delivery, list):
        raise AcceptanceFailure("human_observation_missing_offers")
    index = 0
    for item in delivery:
        if not isinstance(item, dict) or not isinstance(item.get("body"), dict):
            continue
        body = item["body"]
        root = (
            body.get("projection")
            if item.get("kind") == "projection_reset"
            else body.get("observation")
        )
        if not isinstance(root, dict):
            continue
        offers = root.get("action_offers")
        if not isinstance(offers, list):
            continue
        for offer in offers:
            if not isinstance(offer, dict):
                continue
            action_type, digest = (
                offer.get("action_type"),
                offer.get("payload_schema_digest"),
            )
            if action_type == "private_ack" and isinstance(digest, str):
                return f"{sequence}:private_ack:{index}", digest
            index += 1
    raise AcceptanceFailure("human_private_ack_offer_missing")


def _host_status(
    base: str, assignment: str, expected_state: str
) -> dict[str, Any] | None:
    value = request(base, f"/api/v1/managed-agent-hosts/{assignment}")
    return value if value.get("state") == expected_state else None


def _status(
    base: str, assignment: str, state: str | None = None, disposition: str | None = None
) -> dict[str, Any] | None:
    value = request(base, f"/api/v1/managed-agent-hosts/{assignment}")
    activation = value.get("activation", {})
    if state is not None and activation.get("state") != state:
        return None
    if (
        disposition is not None
        and activation.get("last_confirmed_disposition") != disposition
    ):
        return None
    return value


def _failure_diagnostic_from_public(
    host: Any, runner_attention: Any, provider: Any
) -> dict[str, Any]:
    """Project only fixed, safe aggregates from public operational responses.

    This is deliberately lossy: it must remain safe to emit if an upstream
    error response includes opaque identities, private context, or an
    unexpected provider field.
    """

    host = host if isinstance(host, dict) else {}
    activation = host.get("activation")
    activation = activation if isinstance(activation, dict) else {}
    runner_attention = runner_attention if isinstance(runner_attention, dict) else {}
    provider = provider if isinstance(provider, dict) else {}

    def state(value: Any, allowed: set[str]) -> str:
        return value if isinstance(value, str) and value in allowed else "unavailable"

    def disposition(value: Any) -> str:
        return (
            value
            if isinstance(value, str) and value in ACTIVATION_DISPOSITIONS
            else "none"
        )

    def count(value: Any) -> int:
        if isinstance(value, int) and not isinstance(value, bool):
            return value if 0 <= value <= MAX_FAILURE_DIAGNOSTIC_COUNT else 0
        return 0

    def collection_count(key: str) -> int:
        value = runner_attention.get(key)
        return (
            len(value)
            if isinstance(value, list) and len(value) <= MAX_FAILURE_DIAGNOSTIC_COUNT
            else 0
        )

    return {
        "schema": SCHEMA,
        "event": "failure_diagnostic",
        "managed_host": {
            "state": state(host.get("state"), MANAGED_HOST_STATES),
            "failure_present": isinstance(host.get("failure"), dict),
            "attempts": count(host.get("attempts")),
        },
        "activation": {
            "state": state(activation.get("state"), ACTIVATION_STATES),
            "last_confirmed_disposition": disposition(
                activation.get("last_confirmed_disposition")
            ),
        },
        "runner_attention": {
            "runners": collection_count("runners"),
            "managed_hosts": collection_count("managed_hosts"),
            "restart_attempts": collection_count("restart_attempts"),
        },
        "provider": {
            "received": count(provider.get("received_requests")),
            "accepted": count(provider.get("accepted_requests")),
        },
    }


def _failure_diagnostic(
    studio: str | None, assignment: str | None, provider_port: int | None
) -> dict[str, Any]:
    """Fetch optional public status without replacing the original failure."""

    host: Any = {}
    runner_attention: Any = {}
    provider: Any = {}
    if studio is not None:
        try:
            runner_attention = request(studio, "/api/v1/runner-attention")
        except AcceptanceFailure:
            pass
        if assignment is not None:
            try:
                host = request(studio, f"/api/v1/managed-agent-hosts/{assignment}")
            except AcceptanceFailure:
                pass
    if provider_port is not None:
        try:
            provider = request(f"http://127.0.0.1:{provider_port}", "/fixture/status")
        except AcceptanceFailure:
            pass
    return _failure_diagnostic_from_public(host, runner_attention, provider)


def _scan(root: Path, report: Path, sentinels: dict[str, Path]) -> list[dict[str, Any]]:
    channels = [
        f"supervisor-log={root / 'supervisor.log'}",
        f"provider-log={root / 'provider.log'}",
        f"public-api={root / 'public-api.json'}",
        f"report={report}",
    ]
    records = []
    for name, sentinel in sorted(sentinels.items()):
        output = root / f"absence-{name}.json"
        run = subprocess.run(
            [
                sys.executable,
                str(REPOSITORY / "scripts/verify-secret-absence.py"),
                "--sentinel-file",
                str(sentinel),
                *chain.from_iterable(("--channel", channel) for channel in channels),
                "--output",
                str(output),
            ],
            cwd=REPOSITORY,
            env=_environment(),
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
        if run.returncode:
            raise AcceptanceFailure("secret_absence_scan_failed")
        records.append({"canary": name, "status": "pass", "secrets_emitted": False})
    return records


def _activation_launch_reference_canary(
    root: Path, assignment_id: str, canary_name: str
) -> Path:
    """Retain the opaque launch reference solely as a secret-absence canary."""
    active_path = (
        root / "studio" / "assignment-mcp-launches" / f"{assignment_id}.active.json"
    )
    try:
        active = json.loads(active_path.read_text("utf-8"))
    except (OSError, ValueError, json.JSONDecodeError) as error:
        raise AcceptanceFailure("activation_launch_reference_missing") from error
    launch_reference = _secret_reference(
        active.get("launch_reference") if isinstance(active, dict) else None
    )
    return _write_canary(
        root / "scan-canaries",
        canary_name,
        launch_reference.encode("ascii"),
    )


def _daemon_operational_completion_receipts(
    root: Path, room_id: str
) -> tuple[tuple[str, str, bytes], ...]:
    """Read daemon completion receipts without exposing their opaque identities.

    The SQLite URI is opened read-only/query-only. Identities are compared in
    memory across a host restart and never written to logs, DTO captures, or
    the acceptance report.
    """
    database = root / "data" / "worldstream.sqlite3"
    try:
        metadata = database.lstat()
    except OSError as error:
        raise AcceptanceFailure("daemon_completion_receipt_unavailable") from error
    if (
        not stat.S_ISREG(metadata.st_mode)
        or stat.S_ISLNK(metadata.st_mode)
        or metadata.st_mode & 0o077
    ):
        raise AcceptanceFailure("daemon_completion_receipt_unsafe")
    try:
        connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
        try:
            connection.execute("PRAGMA query_only = ON")
            rows = connection.execute(
                "SELECT operation_id, activation_id, canonical_request_hash "
                "FROM activation_operation_receipts "
                "WHERE room_id = ? AND operation_kind = 'complete' "
                "AND result_code = 'completed' ORDER BY operation_id",
                (room_id,),
            ).fetchall()
        finally:
            connection.close()
    except (OSError, sqlite3.Error) as error:
        raise AcceptanceFailure("daemon_completion_receipt_unavailable") from error
    identities: list[tuple[str, str, bytes]] = []
    for operation_id, activation_id, request_hash in rows:
        if (
            not isinstance(operation_id, str)
            or not operation_id
            or len(operation_id) > 256
            or not isinstance(activation_id, str)
            or not ULID.fullmatch(activation_id)
            or not isinstance(request_hash, bytes)
            or len(request_hash) != 32
        ):
            raise AcceptanceFailure("daemon_completion_receipt_invalid")
        identities.append((operation_id, activation_id, request_hash))
    return tuple(identities)


def run_acceptance(args: argparse.Namespace) -> dict[str, Any]:
    binaries = (
        args.daemon_binary,
        args.supervisor_binary,
        args.host_binary,
        args.assignment_mcp_binary,
    )
    if any(not path.is_file() or not os.access(path, os.X_OK) for path in binaries):
        raise AcceptanceFailure("required_binary_missing")
    root = Path(tempfile.mkdtemp(prefix="worldstream-imo88-"))
    root.chmod(0o700)
    provider = supervisor = None
    handoff_file: Path | None = None
    session_file: Path | None = None
    scan_sentinels: dict[str, Path] = {}
    host_stop_confirmed = True
    daemon_stop_confirmed = False
    published_report = args.report.resolve() if args.report else None
    report = root / "report.json"
    studio: str | None = None
    assignment: str | None = None
    provider_port: int | None = None
    try:
        stage("fixture_install")
        daemon_port, supervisor_port, provider_port = _port(), _port(), _port()
        authority_file, model_file = root / "authority.secret", root / "model-token"
        _write_secret(authority_file, os.urandom(32))
        _write_secret(model_file, os.urandom(32).hex().encode())
        config = root / "daemon.toml"
        config.write_text(
            "\n".join(
                (
                    "config_version = 1",
                    "[server]",
                    f'bind = "127.0.0.1:{daemon_port}"',
                    "[storage]",
                    'profile = "sqlite-bundled"',
                    f"data_dir = {json.dumps(str(root / 'data'))}",
                    'deployment_lineage = "imo88/disposable"',
                    "storage_epoch = 1",
                    "[authority.bootstrap]",
                    f"secret_file = {json.dumps(str(authority_file))}",
                    "",
                )
            ),
            encoding="utf-8",
        )
        config.chmod(0o600)
        # The fixture imports the model secret before the Supervisor starts;
        # establish its parent as protected first so startup can retain Host
        # authority in the same vault without weakening file policy.
        (root / "studio").mkdir(mode=0o700)
        install_fixture(
            args.host_binary,
            root / "fixture",
            root / "runner-templates",
            provider_port,
            credential_file=model_file,
            supervisor_state_dir=root / "studio",
            model_provider_credentials_dir=root / "model-provider-credentials",
        )
        provider_log = (root / "provider.log").open("ab")
        provider = subprocess.Popen(
            [
                sys.executable,
                str(REPOSITORY / "examples/counter/deterministic_loopback_provider.py"),
                "--bind",
                "127.0.0.1",
                "--port",
                str(provider_port),
                "--credential-file",
                str(model_file),
                "--response-delay-ms",
                str(args.provider_delay_ms),
            ],
            cwd=REPOSITORY,
            env=_environment(),
            stdout=provider_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        supervisor_log = (root / "supervisor.log").open("ab")
        supervisor = subprocess.Popen(
            [
                str(args.supervisor_binary),
                "--bind",
                f"127.0.0.1:{supervisor_port}",
                "--daemon",
                f"127.0.0.1:{daemon_port}",
                "--daemon-executable",
                str(args.daemon_binary),
                "--daemon-config",
                str(config),
                "--state-dir",
                str(root / "studio"),
                # Retained compatibility flag for the internal browser-origin route.
                "--studio-origin",
                INTERNAL_HANDOFF_REQUEST_ORIGIN,
                "--runner-templates-dir",
                str(root / "runner-templates"),
                "--model-provider-credentials-dir",
                str(root / "model-provider-credentials"),
                "--assignment-mcp-executable",
                str(args.assignment_mcp_binary),
                "--probe-timeout-ms",
                "1500",
            ],
            cwd=REPOSITORY,
            env=_environment(),
            stdout=supervisor_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        studio, daemon = (
            f"http://127.0.0.1:{supervisor_port}",
            f"http://127.0.0.1:{daemon_port}",
        )
        stage("supervisor_ready")
        wait_for(
            "supervisor_ready",
            lambda: (
                {"ok": True}
                if supervisor.poll() is None
                and _available(studio + "/api/v1/daemon/status")
                else None
            ),
            args.startup_timeout,
        )
        request(studio, "/api/v1/daemon/start", method="POST")
        stage("daemon_ready")
        wait_for(
            "daemon_ready",
            lambda: {"ok": True} if _available(daemon + "/readyz") else None,
            args.startup_timeout,
        )
        stage("setup")
        credentials = request(studio, "/api/v1/model-provider-credentials")
        if (
            credentials.get("credentials", [{}])[0].get("credential_id")
            != MODEL_CREDENTIAL_ID
        ):
            raise AcceptanceFailure("named_model_credential_not_configured")
        profile = request(
            studio,
            "/api/v1/agent-profiles",
            method="POST",
            body=_profile(provider_port),
        )
        request(
            studio, "/api/v1/room-drafts/managed-counter", method="PUT", body=_draft()
        )
        creation = request(
            studio, "/api/v1/room-creations/managed-counter/start", method="POST"
        )
        if creation.get("state") != "succeeded":
            raise AcceptanceFailure("room_creation_not_succeeded")
        setup = request(
            studio, "/api/v1/task-setups/managed-counter/start", method="POST"
        )
        # The daemon boundary deliberately reports an interrupted provision as
        # ambiguous.  Reconcile that durable checkpoint through the production
        # retry route; do not issue a fresh authority request ourselves.
        if (
            setup.get("state") == "needs_attention"
            and setup.get("attention", {}).get("code") == "daemon_result_ambiguous"
        ):
            setup = request(
                studio, "/api/v1/task-setups/managed-counter/retry", method="POST"
            )
        if setup.get("state") != "ready":
            raise AcceptanceFailure("managed_task_setup_not_ready")
        setup_status = setup
        setup = _protected_task_setup(root, "managed-counter")
        if setup.get("state") != "ready" or setup.get("room_id") != setup_status.get(
            "room_id"
        ):
            raise AcceptanceFailure("protected_task_setup_status_mismatch")
        authority_values, scan_sentinels, authority_identities = _retained_authorities(
            root, setup, authority_file, model_file
        )
        room_id = setup["room_id"]
        human_seat = next(
            (s for s in setup["seats"] if s["seat_id"] == "human-counter"), None
        )
        if not isinstance(human_seat, dict) or not isinstance(
            human_seat.get("member_id"), str
        ):
            raise AcceptanceFailure("human_seat_not_provisioned")
        before = request(
            studio,
            f"/api/v1/rooms/{room_id}/operator-view",
            method="POST",
            body={"schema": "worldstream/studio-room-operator-view-enable/v1"},
        )
        if before.get("counter", {}).get("value") != 0:
            raise AcceptanceFailure("operator_view_not_bounded_at_genesis")
        stage("human_private_ack")
        handoff_file, session_file = private_ack(studio, root / "scan-canaries")
        scan_sentinels.update(
            {"handoff_value": handoff_file, "member_session_value": session_file}
        )
        stage("activation_pending_wait")
        pending = wait_for(
            "activation_pending",
            lambda: _attention(studio, room_id, "managed-counter"),
            args.activation_timeout,
        )
        stage("activation_pending_observed")
        stage("managed_host_start")
        started = request(
            studio,
            f"/api/v1/rooms/{room_id}/agent-seats/managed-counter/managed-host/start",
            method="POST",
            body={"schema": "worldstream/studio-managed-agent-host-action/v1"},
        )
        assignment = started.get("assignment_id")
        if not isinstance(assignment, str):
            raise AcceptanceFailure("managed_host_assignment_missing")
        host_stop_confirmed = False
        stage("activation_leased_wait")
        leased = wait_for(
            "activation_leased",
            lambda: _runner_status(studio, assignment, "leased"),
            args.activation_timeout,
        )
        stage("activation_leased_observed")
        stage("model_request_wait")
        model_request = wait_for(
            "managed_model_request",
            lambda: _provider_received(provider_port),
            args.activation_timeout,
        )
        stage("model_request_observed")
        stage("leased_host_stop")
        stopped = request(
            studio, f"/api/v1/managed-agent-hosts/{assignment}/stop", method="POST"
        )
        if stopped.get("state") != "stopped":
            raise AcceptanceFailure("leased_host_did_not_stop")
        request(
            studio,
            f"/api/v1/rooms/{room_id}/agent-seats/managed-counter/managed-host/retry",
            method="POST",
            body={"schema": "worldstream/studio-managed-agent-host-action/v1"},
        )
        host_stop_confirmed = False
        stage("completion_recovery_wait")
        completed = wait_for(
            "managed_turn_completion",
            lambda: _status(studio, assignment, disposition="handled"),
            args.recovery_timeout,
        )
        stage("completion_confirmed")
        after = wait_for(
            "operator_counter_increment",
            lambda: _counter(studio, room_id),
            args.recovery_timeout,
        )
        if (
            after.get("counter", {}).get("value") != 2
            or after.get("room_head", {}).get("room_seq") != 2
        ):
            raise AcceptanceFailure("managed_increment_not_exactly_once")
        provider_status = request(
            f"http://127.0.0.1:{provider_port}", "/fixture/status"
        )
        _provider_request_counts(provider_status)
        completion_receipts = _daemon_operational_completion_receipts(root, room_id)
        if len(completion_receipts) != 1:
            raise AcceptanceFailure("daemon_completion_receipt_count_invalid")
        scan_sentinels["activation_launch_reference_before_restart"] = (
            _activation_launch_reference_canary(
                root, assignment, "activation-launch-reference-before-restart"
            )
        )
        stage("completion_idempotency")
        stopped_after_completion = request(
            studio, f"/api/v1/managed-agent-hosts/{assignment}/stop", method="POST"
        )
        if stopped_after_completion.get("state") != "stopped":
            raise AcceptanceFailure("completion_retry_host_did_not_stop")
        host_stop_confirmed = True
        retry_started = request(
            studio,
            f"/api/v1/rooms/{room_id}/agent-seats/managed-counter/managed-host/retry",
            method="POST",
            body={"schema": "worldstream/studio-managed-agent-host-action/v1"},
        )
        if retry_started.get("assignment_id") != assignment:
            raise AcceptanceFailure("completion_retry_assignment_changed")
        host_stop_confirmed = False
        retry_running = wait_for(
            "completion_retry_running",
            lambda: _host_status(studio, assignment, "running"),
            args.recovery_timeout,
        )
        retry_idle = wait_for(
            "completion_retry_idle",
            lambda: _runner_status(studio, assignment, "idle"),
            args.recovery_timeout,
        )
        after_completion_retry = request(
            studio, f"/api/v1/rooms/{room_id}/operator-view"
        )
        provider_after_retry = request(
            f"http://127.0.0.1:{provider_port}", "/fixture/status"
        )
        final_provider_counts = _provider_request_counts(provider_after_retry)
        retried_completion_receipts = _daemon_operational_completion_receipts(
            root, room_id
        )
        scan_sentinels["activation_launch_reference_after_restart"] = (
            _activation_launch_reference_canary(
                root, assignment, "activation-launch-reference-after-restart"
            )
        )
        if (
            after_completion_retry.get("counter", {}).get("value") != 2
            or after_completion_retry.get("room_head", {}).get("room_seq") != 2
            or final_provider_counts != _provider_request_counts(provider_status)
            or retried_completion_receipts != completion_receipts
        ):
            raise AcceptanceFailure("completion_retry_duplicate_effect")
        scope_probes = _authority_scope_probes(
            daemon, room_id, authority_values, authority_identities
        )
        public = {
            "credentials": credentials,
            "profile": profile,
            "pending": pending,
            "leased": leased,
            "completed": completed,
            "completion_retry_started": retry_started,
            "completion_retry_running": retry_running,
            "completion_retry_idle": retry_idle,
            "operator_before": before,
            "operator_after": after,
            "operator_after_completion_retry": after_completion_retry,
            "provider_status": provider_status,
            "provider_after_retry": provider_after_retry,
        }
        _assert_public(public)
        (root / "public-api.json").write_text(
            json.dumps(public, sort_keys=True), encoding="utf-8"
        )
        exact_once = (
            after.get("room_head", {}).get("room_seq") == 2
            and after.get("counter", {}).get("value") == 2
        )
        evidence = {
            "schema": SCHEMA,
            "status": "completed",
            "secrets": "not_emitted",
            "criteria": {
                "fixture": {
                    "status": "passed",
                    "counter_revision": COUNTER_PACK_REVISION,
                    "template": f"{TEMPLATE_ID}@{TEMPLATE_REVISION}",
                },
                "managed_turn": {
                    "status": "passed",
                    "activation_disposition": completed["activation"][
                        "last_confirmed_disposition"
                    ],
                    "counter_value": 2,
                    "provider_requests_received": final_provider_counts["received"],
                    "provider_requests_accepted": final_provider_counts["accepted"],
                },
                "restart_recovery": {
                    "status": "passed",
                    "pending": _pending_activation_state(pending, "managed-counter"),
                    "leased": leased["activation"]["state"],
                    "no_duplicate_effect": exact_once,
                    "room_seq": after["room_head"]["room_seq"],
                },
                "completion_idempotency": {
                    "status": "passed",
                    "host_restart": {
                        "stopped": True,
                        "running": retry_running["state"] == "running",
                        "live_idle": retry_idle["activation"]["state"] == "idle",
                    },
                    "daemon_operational_completion_receipt_count": len(
                        completion_receipts
                    ),
                    "daemon_operational_completion_receipt_identity_unchanged": (
                        retried_completion_receipts == completion_receipts
                    ),
                    "provider_requests_unchanged": True,
                    "counter_value": 2,
                    "room_seq": 2,
                },
                "authority_isolation": {
                    "status": "passed",
                    **scope_probes,
                    "public_dtos_redacted": True,
                    "protected_state_excluded": True,
                },
                "operator_view": {
                    "status": "passed",
                    "counter_value": 2,
                    "room_seq": 2,
                },
            },
        }
        _write_report(report, evidence)
        evidence["secret_absence"] = _scan(root, report, scan_sentinels)
        _write_report(report, evidence)
        _publish_report(report, published_report)
        return evidence
    except AcceptanceFailure:
        stage("failure_diagnostic")
        print(
            json.dumps(
                _failure_diagnostic(studio, assignment, provider_port),
                sort_keys=True,
                separators=(",", ":"),
            ),
            flush=True,
        )
        raise
    finally:
        if supervisor is not None and supervisor.poll() is None:
            try:
                if "assignment" in locals() and not host_stop_confirmed:
                    host_stop = request(
                        studio,
                        f"/api/v1/managed-agent-hosts/{assignment}/stop",
                        method="POST",
                    )
                    host_stop_confirmed = host_stop.get("state") == "stopped"
                lifecycle = request(studio, "/api/v1/daemon/stop", method="POST")
                if lifecycle.get("state") == "stopped":
                    daemon_stop_confirmed = True
                else:
                    wait_for(
                        "daemon_cleanup",
                        lambda: _daemon_stopped(studio),
                        15,
                    )
                    daemon_stop_confirmed = True
            except AcceptanceFailure:
                pass
        supervisor_reaped = _stop(supervisor)
        provider_reaped = _stop(provider)
        if handoff_file is not None:
            handoff_file.unlink(missing_ok=True)
        if session_file is not None:
            session_file.unlink(missing_ok=True)
        # The managed-host launcher owns a further process boundary.  Retain
        # its owner-only state for diagnosis unless every process we started
        # has been reaped after the API-level host and daemon stops above.
        if (
            not args.keep_root
            and supervisor_reaped
            and provider_reaped
            and host_stop_confirmed
            and daemon_stop_confirmed
        ):
            shutil.rmtree(root, ignore_errors=True)


def _available(url: str) -> bool:
    try:
        with urllib.request.urlopen(url, timeout=0.5):
            return True
    except (OSError, urllib.error.HTTPError):
        return False


def _daemon_stopped(base: str) -> dict[str, Any] | None:
    lifecycle = request(base, "/api/v1/daemon/lifecycle")
    return lifecycle if lifecycle.get("state") == "stopped" else None


def _attention(base: str, room: str, expected_seat_id: str) -> dict[str, Any] | None:
    value = request(base, f"/api/v1/rooms/{room}/agent-attention")
    if _pending_activation_state(value, expected_seat_id) is None:
        return None
    return value


def _pending_activation_state(
    value: dict[str, Any], expected_seat_id: str
) -> str | None:
    seats = value.get("seats")
    if not isinstance(seats, list):
        return None
    for seat in seats:
        if not isinstance(seat, dict) or seat.get("seat_id") != expected_seat_id:
            continue
        activation = seat.get("activation")
        if not isinstance(activation, dict):
            return None
        state = activation.get("state")
        waiting, leased = activation.get("waiting"), activation.get("leased")
        # A managed-reference Runner is intentionally stopped, so a truthful
        # absent-presence aggregate is ``attention`` while its durable intent
        # remains waiting.  A connected Runner projects the same intent as
        # ``waiting``.  Both prove the pre-claim pending state.
        if (
            state in {"waiting", "attention"}
            and isinstance(waiting, int)
            and waiting > 0
            and leased == 0
        ):
            return state
    return None


def _provider_received(port: int) -> dict[str, Any] | None:
    value = request(f"http://127.0.0.1:{port}", "/fixture/status")
    received = value.get("received_requests")
    return value if isinstance(received, int) and received >= 1 else None


def _provider_request_counts(status: dict[str, Any]) -> dict[str, int]:
    """Return the received and accepted counts from one provider-status snapshot."""
    received, accepted = (
        status.get("received_requests"),
        status.get("accepted_requests"),
    )
    if (
        not isinstance(received, int)
        or received < 1
        or not isinstance(accepted, int)
        or accepted < 1
    ):
        raise AcceptanceFailure("managed_model_boundary_not_observed")
    return {"received": received, "accepted": accepted}


def _counter(base: str, room: str) -> dict[str, Any] | None:
    value = request(base, f"/api/v1/rooms/{room}/operator-view")
    return value if value.get("counter", {}).get("value") == 2 else None


def _runner_status(base: str, assignment: str, state: str) -> dict[str, Any] | None:
    value = request(base, "/api/v1/runner-attention")
    for host in value.get("managed_hosts", []):
        if (
            host.get("assignment_id") == assignment
            and host.get("activation", {}).get("state") == state
        ):
            return host
    return None


def _daemon_status_for_bearer(base: str, path: str, bearer: str) -> int:
    request_value = urllib.request.Request(
        base + path,
        headers={"Authorization": f"Bearer {bearer}", "Accept": "application/json"},
    )
    try:
        with urllib.request.urlopen(request_value, timeout=3) as response:
            return int(response.status)
    except urllib.error.HTTPError as error:
        return int(error.code)
    except OSError as error:
        raise AcceptanceFailure("authority_isolation_probe_unavailable") from error


def _member_is_denied_runner_stream(
    daemon: str, member_bearer: str, runner_id: str
) -> bool:
    async def probe() -> bool:
        runner = None
        try:
            runner = await asyncio.wait_for(
                Client(daemon, member_bearer).open_runner(
                    runner_id,
                    maximum_concurrent_activations=1,
                    supported_pack_ids=[COUNTER_PACK_ID],
                ),
                timeout=5,
            )
        except ProtocolError as error:
            return error.code in {"forbidden", "unauthenticated"}
        except (OSError, TimeoutError) as error:
            raise AcceptanceFailure("member_runner_scope_probe_unavailable") from error
        finally:
            if runner is not None:
                await runner.close()
        return False

    try:
        return asyncio.run(probe())
    except AcceptanceFailure:
        raise
    except Exception as error:
        raise AcceptanceFailure("member_runner_scope_probe_invalid") from error


def _authority_scope_probes(
    daemon: str,
    room_id: str,
    values: dict[str, bytes],
    identities: dict[str, str],
) -> dict[str, bool]:
    """Verify exact authority boundaries using the fixture's retained values."""
    try:
        host = "wsb1:" + values["host"].hex()
        agent_member = "wsb1:" + values["member_managed"].hex()
        runner = "wsb1:" + values["runner"].hex()
        model = values["model"].decode("ascii")
        runner_id = identities["runner_id"]
    except (KeyError, UnicodeDecodeError) as error:
        raise AcceptanceFailure("authority_isolation_probe_material_invalid") from error
    statuses = {
        "host_operator": _daemon_status_for_bearer(daemon, "/v1/operator/rooms", host),
        "member_operator": _daemon_status_for_bearer(
            daemon, "/v1/operator/rooms", agent_member
        ),
        "runner_operator": _daemon_status_for_bearer(
            daemon, "/v1/operator/rooms", runner
        ),
        "model_operator": _daemon_status_for_bearer(
            daemon, "/v1/operator/rooms", model
        ),
        "member_projection": _daemon_status_for_bearer(
            daemon, f"/v1/rooms/{room_id}/projection", agent_member
        ),
        "host_projection": _daemon_status_for_bearer(
            daemon, f"/v1/rooms/{room_id}/projection", host
        ),
        "runner_projection": _daemon_status_for_bearer(
            daemon, f"/v1/rooms/{room_id}/projection", runner
        ),
        "model_projection": _daemon_status_for_bearer(
            daemon, f"/v1/rooms/{room_id}/projection", model
        ),
    }
    probes = {
        "host_operator_authorized": statuses["host_operator"] == 200,
        "member_rejected_from_operator": statuses["member_operator"] in {401, 403},
        "runner_rejected_from_operator": statuses["runner_operator"] in {401, 403},
        "model_rejected_from_operator": statuses["model_operator"] in {401, 403},
        "member_projection_authorized": statuses["member_projection"] == 200,
        "host_rejected_from_member_projection": statuses["host_projection"]
        in {401, 403},
        "runner_rejected_from_member_projection": statuses["runner_projection"]
        in {401, 403},
        "model_rejected_from_member_projection": statuses["model_projection"]
        in {401, 403},
        "member_rejected_from_runner_stream": _member_is_denied_runner_stream(
            daemon, agent_member, runner_id
        ),
    }
    if not all(probes.values()):
        raise AcceptanceFailure("authority_scope_isolation_failed")
    return probes


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--daemon-binary", type=Path, default=REPOSITORY / "target/debug/worldstreamd"
    )
    parser.add_argument(
        "--supervisor-binary",
        type=Path,
        default=REPOSITORY / "target/debug/worldstream-studio-supervisor",
    )
    parser.add_argument(
        "--host-binary",
        type=Path,
        default=REPOSITORY / "target/debug/worldstream-managed-agent-host",
    )
    parser.add_argument(
        "--assignment-mcp-binary",
        type=Path,
        default=REPOSITORY / "target/debug/worldstream-assignment-mcp",
    )
    parser.add_argument("--report", type=Path)
    parser.add_argument("--startup-timeout", type=float, default=120)
    parser.add_argument("--activation-timeout", type=float, default=45)
    parser.add_argument("--recovery-timeout", type=float, default=90)
    parser.add_argument("--provider-delay-ms", type=int, default=3000)
    parser.add_argument("--keep-root", action="store_true")
    args = parser.parse_args()
    for key in (
        "daemon_binary",
        "supervisor_binary",
        "host_binary",
        "assignment_mcp_binary",
    ):
        setattr(args, key, getattr(args, key).resolve())
    try:
        print(json.dumps(run_acceptance(args), sort_keys=True, separators=(",", ":")))
        return 0
    except AcceptanceFailure as error:
        print(
            json.dumps(
                {
                    "schema": SCHEMA,
                    "status": "blocked",
                    "reason_code": error.code,
                    "secrets": "not_emitted",
                },
                sort_keys=True,
                separators=(",", ":"),
            )
        )
        return 2


if __name__ == "__main__":
    raise SystemExit(main())

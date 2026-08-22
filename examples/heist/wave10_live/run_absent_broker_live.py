#!/usr/bin/env python3
"""Run the public, absent-Broker Agent Heist acceptance story.

The harness creates a disposable Heist Room through HTTP, drives participant
Actions through the Python SDK, advances only durable timers through the public
operator route, and observes Broker Activations through the Runner SDK. It
never writes SQLite, constructs an Activation, supplies a timer payload, or
prints a capability/private context.

Without ``--base-url``/``--operator-bearer`` it reports a precise blocked
reason. ``--spawn-daemon`` is an opt-in disposable process mode for local
acceptance; it requires an already-built ``worldstreamd`` binary. PostgreSQL
mode accepts only an owner-readable DSN file path and never places the DSN in
child environment values or argv.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import pathlib
import secrets
import signal
import socket
import stat
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any

from worldstream_sdk import Client, LostActionReply, LostRunnerReply, ProtocolError

PACK_ID = "worldstream.agent-heist"
PACK_VERSION = "0.1.0"
PACK_DIGEST = "blake3:b05a682f0923001914a800072ee68348e68b979453c93e033cba7916b99a4407"
RUNNER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FB0"
ROLES = ("navigator", "insider", "broker")
PRINCIPAL_IDS = (
    "01ARZ3NDEKTSV4RRFFQ69G5FD0",
    "01ARZ3NDEKTSV4RRFFQ69G5FD1",
    "01ARZ3NDEKTSV4RRFFQ69G5FD2",
)
TIMER_PHASE = "01ARZ3NDEKTSV4RRFFQ69G5FH0"
TIMER_REMINDER = "01ARZ3NDEKTSV4RRFFQ69G5FH1"
TIMER_RESOLVE = "01ARZ3NDEKTSV4RRFFQ69G5FH2"
ULID_A = "01ARZ3NDEKTSV4RRFFQ69G5FAW"
ULID_B = "01ARZ3NDEKTSV4RRFFQ69G5FAX"
CREATE_IDEMPOTENCY_KEY = "01ARZ3NDEKTSV4RRFFQ69G5FB1"  # gitleaks:allow - fixture ULID
MEMBER_IDEMPOTENCY_KEYS = (
    "01ARZ3NDEKTSV4RRFFQ69G5FG0",
    "01ARZ3NDEKTSV4RRFFQ69G5FG1",
    "01ARZ3NDEKTSV4RRFFQ69G5FG2",
)
RUNNER_PRINCIPAL_IDEMPOTENCY_KEY = "01ARZ3NDEKTSV4RRFFQ69G5FE0"  # gitleaks:allow
RUNNER_IDEMPOTENCY_KEY = "01ARZ3NDEKTSV4RRFFQ69G5FE1"  # gitleaks:allow
RUNNER_CAPABILITY_IDEMPOTENCY_KEY = "01ARZ3NDEKTSV4RRFFQ69G5FE2"  # gitleaks:allow
RUNNER_PROVISIONING_IDS = (
    "01ARZ3NDEKTSV4RRFFQ69G5FJ3",
    "01ARZ3NDEKTSV4RRFFQ69G5FK3",
    RUNNER_ID,
)
RUNNER_PROVISIONING_KEYS = (
    (
        "01ARZ3NDEKTSV4RRFFQ69G5FJ0",
        "01ARZ3NDEKTSV4RRFFQ69G5FJ1",
        "01ARZ3NDEKTSV4RRFFQ69G5FJ2",
    ),
    (
        "01ARZ3NDEKTSV4RRFFQ69G5FK0",
        "01ARZ3NDEKTSV4RRFFQ69G5FK1",
        "01ARZ3NDEKTSV4RRFFQ69G5FK2",
    ),
    (
        RUNNER_PRINCIPAL_IDEMPOTENCY_KEY,
        RUNNER_IDEMPOTENCY_KEY,
        RUNNER_CAPABILITY_IDEMPOTENCY_KEY,
    ),
)
ACTION_IDS = {
    "nav_inspect": "01ARZ3NDEKTSV4RRFFQ69G5FC6",
    "insider_inspect": "01ARZ3NDEKTSV4RRFFQ69G5FC7",
    "nav_publish": "01ARZ3NDEKTSV4RRFFQ69G5FC8",
    "insider_publish": "01ARZ3NDEKTSV4RRFFQ69G5FC9",
    "plan": "01ARZ3NDEKTSV4RRFFQ69G5FCA",
    "endorse": "01ARZ3NDEKTSV4RRFFQ69G5FCB",
    "nav_commit": "01ARZ3NDEKTSV4RRFFQ69G5FCC",
    "insider_commit": "01ARZ3NDEKTSV4RRFFQ69G5FCD",
    "nav_ack": "01ARZ3NDEKTSV4RRFFQ69G5FCE",
    "insider_ack": "01ARZ3NDEKTSV4RRFFQ69G5FCF",
}
ACTIVATION_OPERATION_IDS = {
    "first_complete": "01ARZ3NDEKTSV4RRFFQ69G5FCG",
    "stale_complete": "01ARZ3NDEKTSV4RRFFQ69G5FCH",
    "reclaimed_complete": "01ARZ3NDEKTSV4RRFFQ69G5FCJ",
    "first_renew": "01ARZ3NDEKTSV4RRFFQ69G5FCK",
    "first_release": "01ARZ3NDEKTSV4RRFFQ69G5FCM",
}
DEFAULT_TIMEOUT = 15.0
_ACTION_LATENCY_SAMPLES_MS: list[float] = []
_ACTION_PAYLOAD_SIZES_BYTES: set[int] = set()


class BlockedStory(Exception):
    """A closed, typed live prerequisite failure."""

    def __init__(self, reason_code: str, **details: Any) -> None:
        super().__init__(reason_code)
        self.reason_code = reason_code
        self.details = details


@dataclass
class DisposableDaemon:
    """Own a daemon process and private bootstrap material for one run."""

    binary: pathlib.Path
    root: pathlib.Path
    port: int
    postgresql_dsn_file: pathlib.Path | None = None
    kill_after_action_id: str | None = None
    kill_after_claim_id: str | None = None
    process: subprocess.Popen[bytes] | None = None
    log_handle: Any = None

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    @property
    def secret_path(self) -> pathlib.Path:
        return self.root / "authority.secret"

    @property
    def bearer(self) -> str:
        return f"wsb1:{self.secret_path.read_bytes().hex()}"

    def start(self) -> None:
        data_dir = self.root / "data"
        data_dir.mkdir(mode=0o700, exist_ok=True)
        self.log_handle = (self.root / "worldstreamd.log").open("ab")
        environment = {
            **os.environ,
            "WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE": str(self.secret_path),
            "RUST_LOG": "warn",
        }
        environment.pop("WORLDSTREAM__STORAGE__POSTGRESQL__DSN", None)
        environment.pop("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_HANDLE", None)
        if self.postgresql_dsn_file is None:
            environment["WORLDSTREAM__STORAGE__PROFILE"] = "sqlite-bundled"
            environment.pop("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE", None)
        else:
            environment["WORLDSTREAM__STORAGE__PROFILE"] = "postgres-primary"
            environment["WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE"] = str(
                self.postgresql_dsn_file
            )
        if self.kill_after_action_id is not None:
            environment["WORLDSTREAM_TEST_KILL_AFTER_ACTION_COMMIT_BEFORE_REPLY"] = (
                self.kill_after_action_id
            )
        else:
            environment.pop(
                "WORLDSTREAM_TEST_KILL_AFTER_ACTION_COMMIT_BEFORE_REPLY", None
            )
        if self.kill_after_claim_id is not None:
            environment["WORLDSTREAM_TEST_KILL_AFTER_ACTIVATION_CLAIM_BEFORE_REPLY"] = (
                self.kill_after_claim_id
            )
        else:
            environment.pop(
                "WORLDSTREAM_TEST_KILL_AFTER_ACTIVATION_CLAIM_BEFORE_REPLY", None
            )
        self.process = subprocess.Popen(
            [
                str(self.binary),
                "--data-dir",
                str(data_dir),
                "--bind",
                f"127.0.0.1:{self.port}",
            ],
            cwd=self.binary.parents[2],
            env=environment,
            stdout=self.log_handle,
            stderr=subprocess.STDOUT,
        )
        wait_ready(self.base_url, 30.0)

    def restart(
        self,
        *,
        kill_after_action_id: str | None = None,
        kill_after_claim_id: str | None = None,
    ) -> None:
        self.stop()
        self.kill_after_action_id = kill_after_action_id
        self.kill_after_claim_id = kill_after_claim_id
        self.start()

    def stop(self) -> None:
        if self.process is not None and self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None


def _safe_error(error: BaseException) -> dict[str, Any]:
    if isinstance(error, ProtocolError):
        return {"code": error.code, "retryable": error.retryable}
    return {"code": type(error).__name__}


def _json_request(
    base_url: str,
    bearer: str,
    path: str,
    body: dict[str, Any],
) -> dict[str, Any]:
    payload = json.dumps(body, sort_keys=True, separators=(",", ":")).encode("utf-8")
    request = urllib.request.Request(
        f"{base_url.rstrip('/')}{path}",
        data=payload,
        method="POST",
        headers={
            "Accept": "application/json",
            "Authorization": f"Bearer {bearer}",
            "Content-Type": "application/json",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=DEFAULT_TIMEOUT) as response:
            value = json.loads(response.read())
    except urllib.error.HTTPError as error:
        try:
            value = json.loads(error.read())
        except (OSError, ValueError):
            raise BlockedStory("public_http_error", http_status=error.code) from error
        error_body = value.get("error") if isinstance(value, dict) else None
        if isinstance(error_body, dict) and isinstance(error_body.get("code"), str):
            raise BlockedStory(
                "public_http_rejected",
                http_status=error.code,
                error_code=error_body["code"],
                retryable=bool(error_body.get("retryable", False)),
            ) from error
        raise BlockedStory("public_http_error", http_status=error.code) from error
    except (OSError, TimeoutError) as error:
        raise BlockedStory("public_http_unavailable") from error
    if not isinstance(value, dict):
        raise BlockedStory("invalid_public_http_response")
    return value


def fire_timer(
    base_url: str, bearer: str, room_id: str, timer_id: str, generation: int
) -> dict[str, Any]:
    """Fire one exact public timer witness; no payload or timestamp is accepted."""

    return _json_request(
        base_url,
        bearer,
        f"/v1/operator/rooms/{room_id}/timers/fire",
        {"timer_id": timer_id, "generation": generation},
    )


def issue_member_capability(
    base_url: str,
    operator_bearer: str,
    room_id: str,
    member_id: str,
    principal_id: str,
    idempotency_key: str,
) -> dict[str, Any]:
    return _json_request(
        base_url,
        operator_bearer,
        "/v1/operator/member-capabilities",
        {
            "room_id": room_id,
            "member_id": member_id,
            "principal_id": principal_id,
            "scopes": [
                "room:attach",
                "room:observe_member",
                "room:act",
                "room:replay",
            ],
            "idempotency_key": idempotency_key,
            "expires_at": None,
        },
    )


def issue_runner_capability(
    base_url: str,
    operator_bearer: str,
    room_id: str,
    member_id: str,
    principal_id: str,
    runner_id: str = RUNNER_ID,
    change_keys: tuple[str, str, str] = (
        RUNNER_PRINCIPAL_IDEMPOTENCY_KEY,
        RUNNER_IDEMPOTENCY_KEY,
        RUNNER_CAPABILITY_IDEMPOTENCY_KEY,
    ),
) -> dict[str, Any]:
    return _json_request(
        base_url,
        operator_bearer,
        "/v1/operator/runner-capabilities",
        {
            "runner_id": runner_id,
            "owner_principal_id": principal_id,
            "permitted_memberships": [{"room_id": room_id, "member_id": member_id}],
            "scopes": [
                "activation:offer_receive",
                "activation:claim",
                "activation:complete",
            ],
            "principal_idempotency_key": change_keys[0],
            "runner_idempotency_key": change_keys[1],
            "capability_idempotency_key": change_keys[2],
            "expires_at": None,
        },
    )


def wait_ready(base_url: str, timeout: float) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(
                f"{base_url.rstrip('/')}/readyz", timeout=1
            ) as response:
                if response.status == 200:
                    return
        except (OSError, urllib.error.HTTPError):
            pass
        time.sleep(0.1)
    raise BlockedStory("daemon_not_ready")


def _owner_only_regular_file(path: pathlib.Path) -> pathlib.Path:
    """Validate a PostgreSQL DSN file without reading or resolving it."""

    absolute = pathlib.Path(os.path.abspath(path))
    try:
        metadata = absolute.lstat()
    except OSError as error:
        raise BlockedStory("postgresql_dsn_file_unavailable") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise BlockedStory("postgresql_dsn_file_not_regular")
    if metadata.st_mode & 0o077:
        raise BlockedStory("postgresql_dsn_file_not_owner_only")
    if hasattr(os, "getuid") and metadata.st_uid != os.getuid():
        raise BlockedStory("postgresql_dsn_file_wrong_owner")
    return absolute


def _engine_identity(base_url: str) -> dict[str, str]:
    """Load only the public, credential-free storage identity."""

    try:
        with urllib.request.urlopen(
            f"{base_url.rstrip('/')}/version", timeout=DEFAULT_TIMEOUT
        ) as response:
            value = json.loads(response.read())
    except (OSError, TimeoutError, ValueError) as error:
        raise BlockedStory("version_identity_unavailable") from error
    engine = value.get("engine") if isinstance(value, dict) else None
    if not isinstance(engine, dict):
        raise BlockedStory("version_engine_identity_missing")
    profile = engine.get("profile")
    status = engine.get("status")
    exact_identity = engine.get("exact_identity")
    if not all(
        isinstance(item, str) and item for item in (profile, status, exact_identity)
    ):
        raise BlockedStory("version_engine_identity_invalid")
    return {
        "profile": profile,
        "status": status,
        "exact_identity": exact_identity,
    }


async def _projection(client: Client, room_id: str) -> dict[str, Any]:
    try:
        return await client.projection(room_id)
    except (ProtocolError, OSError, TimeoutError) as error:
        raise BlockedStory("projection_unavailable", **_safe_error(error)) from error


async def _act(
    client: Client,
    room_id: str,
    member_id: str,
    action_type: str,
    payload: dict[str, Any],
    *,
    action_id: str | None = None,
    expected_room_seq: int | None = None,
) -> dict[str, Any]:
    _ACTION_PAYLOAD_SIZES_BYTES.add(
        len(json.dumps(payload, sort_keys=True, separators=(",", ":")).encode())
    )
    room = None
    try:
        room = await client.open_room(room_id, member_id)
        await room.sync()
        action_started = time.monotonic()
        try:
            result = await room.act(
                action_type,
                payload,
                action_id=action_id,
                expected_room_seq=expected_room_seq,
                timeout=DEFAULT_TIMEOUT,
            )
        finally:
            _ACTION_LATENCY_SAMPLES_MS.append(
                round((time.monotonic() - action_started) * 1000, 3)
            )
        if "transition_id" not in result and not result.get("duplicate", False):
            raise BlockedStory(
                "action_receipt_missing_transition", action_type=action_type
            )
        return result
    except ProtocolError as error:
        raise BlockedStory(
            "action_rejected",
            action_type=action_type,
            error_code=error.code,
            retryable=error.retryable,
        ) from error
    finally:
        if room is not None:
            await room.close()


def _safe_action_receipt(value: dict[str, Any]) -> dict[str, Any]:
    """Keep only durable, non-secret fields from an Action receipt."""

    return {
        "action_id": value.get("action_id"),
        "transition_id": value.get("transition_id"),
        "duplicate": value.get("duplicate"),
        "room_head": value.get("room_head"),
    }


async def _exercise_kill_boundary(
    client: Client,
    room_id: str,
    member_id: str,
    daemon: DisposableDaemon | None,
) -> dict[str, Any]:
    """Prove a committed Action survives loss before its reply is published."""

    if daemon is None:
        raise BlockedStory("kill_boundary_requires_spawn_daemon")
    action_id = ACTION_IDS["nav_commit"]
    payload = {
        "selected_plan_id": ACTION_IDS["plan"],
        "contribute_required_resource": False,
    }
    _ACTION_PAYLOAD_SIZES_BYTES.add(
        len(json.dumps(payload, sort_keys=True, separators=(",", ":")).encode())
    )
    room = None
    lost_request: dict[str, Any] | None = None
    try:
        room = await client.open_room(room_id, member_id)
        await room.sync()
        try:
            await room.act(
                "commit_move",
                payload,
                action_id=action_id,
                timeout=DEFAULT_TIMEOUT,
            )
        except LostActionReply as error:
            lost_request = error.request
    finally:
        if room is not None:
            await room.close()
    if lost_request is None:
        raise BlockedStory("kill_boundary_reply_was_not_lost")

    daemon.restart(kill_after_action_id=None)
    retry_room = None
    try:
        retry_room = await client.open_room(room_id, member_id)
        await retry_room.sync()
        first_retry = await retry_room.retry_action(
            lost_request, timeout=DEFAULT_TIMEOUT
        )
        second_retry = await retry_room.retry_action(
            lost_request, timeout=DEFAULT_TIMEOUT
        )
    finally:
        if retry_room is not None:
            await retry_room.close()

    first_safe = _safe_action_receipt(first_retry)
    second_safe = _safe_action_receipt(second_retry)
    matches = (
        first_retry == second_retry
        and first_retry.get("action_id") == action_id
        and first_retry.get("duplicate") is True
        and first_retry.get("transition_id") == second_retry.get("transition_id")
    )
    if not matches:
        raise BlockedStory(
            "kill_boundary_duplicate_receipt_mismatch",
            first_retry=first_safe,
            second_retry=second_safe,
        )
    return {
        "enabled_action_id": action_id,
        "transport_loss_observed": True,
        "restart_same_data_dir": True,
        "seam_disabled_after_restart": True,
        "exact_request_retried": True,
        "first_duplicate": first_safe,
        "second_duplicate": second_safe,
        "durable_duplicate_result_matches": True,
        "secrets": "not_emitted",
    }


def _private_claim_code(projection: dict[str, Any], clue_id: str) -> str:
    activity = projection.get("projection", {}).get("activity", {})
    clues = activity.get("private_clues")
    if not isinstance(clues, list):
        raise BlockedStory("private_clue_projection_missing", clue_id=clue_id)
    for clue in clues:
        if isinstance(clue, dict) and clue.get("clue_id") == clue_id:
            claim_code = clue.get("claim_code")
            if isinstance(claim_code, str) and claim_code:
                return claim_code
    raise BlockedStory("expected_private_clue_missing", clue_id=clue_id)


def _plan_from_published_claims(
    route_claim: str, entry_window_claim: str
) -> dict[str, str]:
    """Select the frozen fixture using only participant-publishable claims."""

    prefix_route = "route_"
    prefix_entry = "entry_window_"
    if not route_claim.startswith(prefix_route) or not entry_window_claim.startswith(
        prefix_entry
    ):
        raise BlockedStory("published_clue_claim_shape_invalid")
    route = route_claim.removeprefix(prefix_route)
    entry_window = entry_window_claim.removeprefix(prefix_entry)
    broker_fields = {
        ("canal", "late"): ("disguise", "van"),
        ("service", "early"): ("thermal_key", "boat"),
        ("roof", "middle"): ("jammer", "motorbike"),
    }.get((route, entry_window))
    if broker_fields is None:
        raise BlockedStory(
            "published_clue_fixture_pair_invalid",
            route=route,
            entry_window=entry_window,
        )
    required_tool, extraction = broker_fields
    return {
        "route": route,
        "entry_window": entry_window,
        "required_tool": required_tool,
        "extraction": extraction,
    }


def _offer_ids(offers: Any) -> list[str]:
    if not isinstance(offers, list):
        return []
    return [
        offer.get("activation_id")
        for offer in offers
        if isinstance(offer, dict) and isinstance(offer.get("activation_id"), str)
    ]


async def _poll_offer(
    runner: Any, room_id: str, member_id: str, timeout: float
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            response = await runner.poll_offers(
                room_id, member_id, timeout=DEFAULT_TIMEOUT
            )
        except ProtocolError as error:
            if error.code in {"room_busy", "storage_unavailable"} and error.retryable:
                await asyncio.sleep(0.25)
                continue
            raise BlockedStory(
                "runner_offer_poll_failed", **_safe_error(error)
            ) from error
        offers = response.get("offers") if isinstance(response, dict) else None
        if isinstance(offers, list) and offers:
            first = offers[0]
            if isinstance(first, dict) and isinstance(first.get("activation_id"), str):
                return first
        await asyncio.sleep(0.25)
    raise BlockedStory("activation_offer_not_created", poll_timeout_seconds=timeout)


async def _runner_call_when_available(
    call: Any, *, timeout: float, blocked_reason: str
) -> dict[str, Any]:
    """Retry an exact Runner operation only while it is known uncommitted."""

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            value = await call()
        except ProtocolError as error:
            if (
                error.code not in {"room_busy", "storage_unavailable"}
                or not error.retryable
            ):
                raise
            await asyncio.sleep(0.25)
            continue
        if not isinstance(value, dict):
            raise BlockedStory("invalid_runner_operation_result")
        return value
    raise BlockedStory(blocked_reason)


def _activation_receipt(value: dict[str, Any]) -> dict[str, Any]:
    """Keep only non-private Activation fields in evidence."""

    return {
        "code": value.get("code"),
        "state": value.get("state"),
        "lease_generation": value.get("lease_generation"),
        "context_present": isinstance(value.get("context"), dict),
        "context_hash_present": isinstance(value.get("context_hash"), str),
    }


def _timer_receipt(value: dict[str, Any], phase: str) -> dict[str, Any]:
    """Keep the exact durable Timer identity without its private payload."""

    return {
        "phase": phase,
        "timer_id": value.get("timer_id"),
        "generation": value.get("generation"),
        "duplicate": value.get("duplicate"),
        "transition_id_present": isinstance(value.get("transition_id"), str),
        "room_seq": value.get("room_head", {}).get("room_seq")
        if isinstance(value.get("room_head"), dict)
        else None,
    }


_REPLAY_HEAD_HASH_FIELDS = {
    "pack_digest": "pack",
    "core_state_hash": "core",
    "activity_state_hash": "activity",
    "authoritative_state_hash": "aggregate_authoritative",
    "genesis_or_transition_hash": "transition",
}


def _verify_replay_hash_parity(
    current_projection: dict[str, Any], replay: dict[str, Any]
) -> dict[str, Any]:
    """Compare server-exposed final and Replay heads without deriving hashes.

    The public protocol exposes the four hash-bearing ``room_head`` fields on
    both Projection and Replay. This witness treats them as opaque server
    claims: it never computes, normalizes, or invents a digest. Missing fields
    are a closed capability failure so a protocol regression cannot silently
    weaken the acceptance claim.
    """

    current_head = current_projection.get("room_head")
    replay_head = replay.get("room_head")
    if not isinstance(current_head, dict) or not isinstance(replay_head, dict):
        raise BlockedStory("replay_hash_parity_fields_unexposed")

    comparisons: dict[str, bool] = {}
    expected: dict[str, str] = {}
    observed: dict[str, str] = {}
    for field, label in _REPLAY_HEAD_HASH_FIELDS.items():
        current_value = current_head.get(field)
        replay_value = replay_head.get(field)
        if not isinstance(current_value, str) or not current_value:
            raise BlockedStory(
                "replay_hash_parity_fields_unexposed",
                field=field,
                surface="projection",
            )
        if not isinstance(replay_value, str) or not replay_value:
            raise BlockedStory(
                "replay_hash_parity_fields_unexposed",
                field=field,
                surface="replay",
            )
        expected[label] = current_value
        observed[label] = replay_value
        comparisons[label] = current_value == replay_value

    current_room_id = current_head.get("room_id")
    replay_room_id = replay_head.get("room_id")
    current_room_seq = current_head.get("room_seq")
    replay_room_seq = replay_head.get("room_seq")
    if not isinstance(current_room_id, str) or not isinstance(replay_room_id, str):
        raise BlockedStory("replay_hash_parity_fields_unexposed", field="room_id")
    if (
        isinstance(current_room_seq, bool)
        or not isinstance(current_room_seq, int)
        or isinstance(replay_room_seq, bool)
        or not isinstance(replay_room_seq, int)
    ):
        raise BlockedStory("replay_hash_parity_fields_unexposed", field="room_seq")
    comparisons["room_id"] = current_room_id == replay_room_id
    comparisons["room_seq"] = current_room_seq == replay_room_seq
    if not all(comparisons.values()):
        raise BlockedStory(
            "replay_hash_parity_mismatch",
            comparisons=comparisons,
            expected=expected,
            observed=observed,
        )
    return {
        "verified": True,
        "fields": tuple(comparisons),
        "expected": expected,
        "replayed": observed,
    }


async def _activation_story(
    runner: Any,
    room_id: str,
    broker_member_id: str,
    *,
    wait_timeout: float,
) -> dict[str, Any]:
    first_offer = await _poll_offer(runner, room_id, broker_member_id, wait_timeout)
    activation_id = first_offer["activation_id"]
    claim_id = ULID_A
    first = await runner.claim(activation_id, 30_000, claim_id=claim_id)
    first_safe = _activation_receipt(first)
    duplicate_claim = await _runner_call_when_available(
        lambda: runner.claim(activation_id, 30_000, claim_id=claim_id),
        timeout=wait_timeout,
        blocked_reason="duplicate_claim_remained_busy",
    )
    duplicate_safe = _activation_receipt(duplicate_claim)
    generation = first.get("lease_generation")
    if not isinstance(generation, int):
        raise BlockedStory("activation_claim_missing_generation")
    renewed = await _runner_call_when_available(
        lambda: runner.renew(
            activation_id,
            claim_id,
            generation,
            30_000,
            operation_id=ACTIVATION_OPERATION_IDS["first_renew"],
        ),
        timeout=wait_timeout,
        blocked_reason="activation_renew_remained_busy",
    )
    duplicate_renew = await _runner_call_when_available(
        lambda: runner.renew(
            activation_id,
            claim_id,
            generation,
            30_000,
            operation_id=ACTIVATION_OPERATION_IDS["first_renew"],
        ),
        timeout=wait_timeout,
        blocked_reason="duplicate_renew_remained_busy",
    )
    if renewed != duplicate_renew or renewed.get("code") != "renewed":
        raise BlockedStory("activation_renew_receipt_mismatch")
    completed = await _runner_call_when_available(
        lambda: runner.complete(
            activation_id,
            claim_id,
            generation,
            "failed",
            operation_id=ACTIVATION_OPERATION_IDS["first_complete"],
        ),
        timeout=wait_timeout,
        blocked_reason="activation_complete_remained_busy",
    )
    duplicate_complete = await _runner_call_when_available(
        lambda: runner.complete(
            activation_id,
            claim_id,
            generation,
            "failed",
            operation_id=ACTIVATION_OPERATION_IDS["first_complete"],
        ),
        timeout=wait_timeout,
        blocked_reason="duplicate_complete_remained_busy",
    )
    if completed != duplicate_complete:
        raise BlockedStory("activation_complete_receipt_mismatch")
    return {
        "activation_id": activation_id,
        "offer_observed": True,
        "first_claim": first_safe,
        "duplicate_claim": duplicate_safe,
        "renew": _activation_receipt(renewed),
        "idempotent_renew": _activation_receipt(duplicate_renew),
        "complete": _activation_receipt(completed),
        "idempotent_complete": _activation_receipt(duplicate_complete),
        "context_not_emitted": True,
    }


async def _lost_claim_reply_story(
    runner: Any,
    room_id: str,
    broker_member_id: str,
    daemon: DisposableDaemon | None,
    *,
    wait_timeout: float,
) -> dict[str, Any]:
    """Prove a durable Activation claim survives loss before its reply."""

    if daemon is None:
        raise BlockedStory("lost_claim_reply_requires_spawn_daemon")
    first_offer = await _poll_offer(runner, room_id, broker_member_id, wait_timeout)
    activation_id = first_offer["activation_id"]
    claim_id = ULID_A
    lost_request: dict[str, Any] | None = None
    try:
        await runner.claim(activation_id, 30_000, claim_id=claim_id)
    except LostRunnerReply as error:
        lost_request = error.request
    if lost_request is None:
        raise BlockedStory("lost_claim_reply_was_not_lost")

    recovery_started = time.monotonic()
    daemon.restart(kill_after_claim_id=None)
    await runner.close()
    await _reconnect_runner_after_restart(runner, timeout=wait_timeout)
    first_retry = await _retry_retained_runner_request(
        runner, lost_request, timeout=wait_timeout
    )
    second_retry = await _retry_retained_runner_request(
        runner, lost_request, timeout=wait_timeout
    )
    matching = (
        first_retry == second_retry
        and first_retry.get("activation_id") == activation_id
        and first_retry.get("claim_id") == claim_id
        and first_retry.get("code") == "granted"
    )
    if not matching:
        raise BlockedStory(
            "lost_claim_duplicate_receipt_mismatch",
            first_retry=_activation_receipt(first_retry),
            second_retry=_activation_receipt(second_retry),
        )

    first_safe = _activation_receipt(first_retry)
    duplicate_claim = await _runner_call_when_available(
        lambda: runner.claim(activation_id, 30_000, claim_id=claim_id),
        timeout=wait_timeout,
        blocked_reason="lost_duplicate_claim_remained_busy",
    )
    generation = first_retry.get("lease_generation")
    if not isinstance(generation, int):
        raise BlockedStory("lost_claim_retry_missing_generation")
    renewed = await _runner_call_when_available(
        lambda: runner.renew(
            activation_id,
            claim_id,
            generation,
            30_000,
            operation_id=ACTIVATION_OPERATION_IDS["first_renew"],
        ),
        timeout=wait_timeout,
        blocked_reason="lost_activation_renew_remained_busy",
    )
    duplicate_renew = await _runner_call_when_available(
        lambda: runner.renew(
            activation_id,
            claim_id,
            generation,
            30_000,
            operation_id=ACTIVATION_OPERATION_IDS["first_renew"],
        ),
        timeout=wait_timeout,
        blocked_reason="lost_duplicate_renew_remained_busy",
    )
    if renewed != duplicate_renew or renewed.get("code") != "renewed":
        raise BlockedStory("activation_renew_receipt_mismatch")
    completed = await _runner_call_when_available(
        lambda: runner.complete(
            activation_id,
            claim_id,
            generation,
            "failed",
            operation_id=ACTIVATION_OPERATION_IDS["first_complete"],
        ),
        timeout=wait_timeout,
        blocked_reason="lost_activation_complete_remained_busy",
    )
    duplicate_complete = await _runner_call_when_available(
        lambda: runner.complete(
            activation_id,
            claim_id,
            generation,
            "failed",
            operation_id=ACTIVATION_OPERATION_IDS["first_complete"],
        ),
        timeout=wait_timeout,
        blocked_reason="lost_duplicate_complete_remained_busy",
    )
    if completed != duplicate_complete:
        raise BlockedStory("activation_complete_receipt_mismatch")
    return {
        "activation_id": activation_id,
        "offer_observed": True,
        "first_claim": first_safe,
        "duplicate_claim": _activation_receipt(duplicate_claim),
        "renew": _activation_receipt(renewed),
        "idempotent_renew": _activation_receipt(duplicate_renew),
        "complete": _activation_receipt(completed),
        "idempotent_complete": _activation_receipt(duplicate_complete),
        "context_not_emitted": True,
        "lost_claim_reply": {
            "claim_id": claim_id,
            "transport_loss_observed": True,
            "restart_same_data_dir": True,
            "seams_disabled_after_restart": True,
            "recovery_duration_ms": round(
                (time.monotonic() - recovery_started) * 1000, 3
            ),
            "exact_request_retried": True,
            "first_duplicate": first_safe,
            "second_duplicate": _activation_receipt(second_retry),
            "durable_duplicate_result_matches": True,
            "secrets": "not_emitted",
        },
    }


async def _retry_retained_runner_request(
    runner: Any, request: dict[str, Any], *, timeout: float
) -> dict[str, Any]:
    """Retry one exact retained Runner request across a transient Room lease.

    A restarted daemon can be ready before its scheduler releases the Room's
    short startup lease. Only the closed, retryable ``room_busy`` response is
    retried; the original request object and identity are reused unchanged.
    """

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            return await runner.retry(request, timeout=DEFAULT_TIMEOUT)
        except ProtocolError as error:
            if (
                error.code not in {"room_busy", "storage_unavailable"}
                or not error.retryable
            ):
                raise
            await asyncio.sleep(0.25)
    raise BlockedStory("lost_claim_retry_remained_busy")


async def _reconnect_runner_after_restart(runner: Any, *, timeout: float) -> None:
    """Retry Runner hello only while the startup Room lease is known busy."""

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            await runner.connect()
            return
        except ProtocolError as error:
            await runner.close()
            if (
                error.code not in {"room_busy", "storage_unavailable"}
                or not error.retryable
            ):
                raise
            await asyncio.sleep(0.25)
    raise BlockedStory("runner_reconnect_remained_busy")


async def _fresh_activation_recovery(
    runner: Any,
    room_id: str,
    broker_member_id: str,
    *,
    wait_timeout: float,
) -> dict[str, Any]:
    offer = await _poll_offer(runner, room_id, broker_member_id, wait_timeout)
    activation_id = offer["activation_id"]
    old_claim_id = ULID_B
    old = await _runner_call_when_available(
        lambda: runner.claim(activation_id, 250, claim_id=old_claim_id),
        timeout=wait_timeout,
        blocked_reason="short_lease_claim_remained_busy",
    )
    old_generation = old.get("lease_generation")
    if not isinstance(old_generation, int):
        raise BlockedStory("short_lease_claim_missing_generation")
    await asyncio.sleep(1.0)
    reclaimed = await _poll_offer(runner, room_id, broker_member_id, wait_timeout)
    if reclaimed.get("activation_id") != activation_id:
        raise BlockedStory("lease_reclaim_returned_wrong_activation")
    new_claim_id = "01ARZ3NDEKTSV4RRFFQ69G5FAY"
    new = await _runner_call_when_available(
        lambda: runner.claim(activation_id, 30_000, claim_id=new_claim_id),
        timeout=wait_timeout,
        blocked_reason="reclaimed_claim_remained_busy",
    )
    new_generation = new.get("lease_generation")
    if not isinstance(new_generation, int) or new_generation <= old_generation:
        raise BlockedStory("lease_generation_did_not_advance")
    released = await _runner_call_when_available(
        lambda: runner.release(
            activation_id,
            new_claim_id,
            new_generation,
            operation_id=ACTIVATION_OPERATION_IDS["first_release"],
        ),
        timeout=wait_timeout,
        blocked_reason="reclaimed_activation_release_remained_busy",
    )
    duplicate_release = await _runner_call_when_available(
        lambda: runner.release(
            activation_id,
            new_claim_id,
            new_generation,
            operation_id=ACTIVATION_OPERATION_IDS["first_release"],
        ),
        timeout=wait_timeout,
        blocked_reason="reclaimed_duplicate_release_remained_busy",
    )
    if released != duplicate_release or released.get("code") != "released":
        raise BlockedStory("activation_release_receipt_mismatch")
    released_offer = await _poll_offer(runner, room_id, broker_member_id, wait_timeout)
    if released_offer.get("activation_id") != activation_id:
        raise BlockedStory("released_activation_not_reoffered")
    final_claim_id = "01ARZ3NDEKTSV4RRFFQ69G5FAZ"
    final = await _runner_call_when_available(
        lambda: runner.claim(activation_id, 30_000, claim_id=final_claim_id),
        timeout=wait_timeout,
        blocked_reason="claim_after_release_remained_busy",
    )
    final_generation = final.get("lease_generation")
    if not isinstance(final_generation, int) or final_generation <= new_generation:
        raise BlockedStory("released_activation_generation_not_advanced")
    try:
        stale = await runner.complete(
            activation_id,
            old_claim_id,
            old_generation,
            "failed",
            operation_id=ACTIVATION_OPERATION_IDS["stale_complete"],
        )
    except ProtocolError as error:
        stale = {"code": error.code, "state": None, "retryable": error.retryable}
    complete = await _runner_call_when_available(
        lambda: runner.complete(
            activation_id,
            final_claim_id,
            final_generation,
            "handled",
            operation_id=ACTIVATION_OPERATION_IDS["reclaimed_complete"],
        ),
        timeout=wait_timeout,
        blocked_reason="reclaimed_activation_complete_remained_busy",
    )
    return {
        "activation_id": activation_id,
        "short_lease_claim": _activation_receipt(old),
        "reclaimed": True,
        "stale_old_generation": _activation_receipt(stale),
        "reclaimed_claim": _activation_receipt(new),
        "release": _activation_receipt(released),
        "idempotent_release": _activation_receipt(duplicate_release),
        "claim_after_release": _activation_receipt(final),
        "reclaimed_complete": _activation_receipt(complete),
        "context_not_emitted": True,
    }


async def run_live(args: argparse.Namespace) -> dict[str, Any]:
    if not args.base_url or not args.operator_bearer:
        return {
            "status": "blocked",
            "reason_code": "base_url_and_operator_bearer_are_required",
            "live_evidence": False,
            "fabricated_offer": False,
            "secrets": "not_emitted",
        }

    story_started = time.monotonic()
    _ACTION_LATENCY_SAMPLES_MS.clear()
    _ACTION_PAYLOAD_SIZES_BYTES.clear()
    host = Client(args.base_url, args.operator_bearer)
    daemon = args.daemon
    runner = None
    rooms: list[Any] = []
    stage = "readiness"
    try:
        try:
            wait_ready(args.base_url, 5.0)
        except BlockedStory as error:
            return {
                "status": "blocked",
                "reason_code": error.reason_code,
                "live_evidence": False,
                "fabricated_offer": False,
                "secrets": "not_emitted",
            }
        storage = _engine_identity(args.base_url)
        if storage["status"] != "verified":
            raise BlockedStory("storage_engine_not_verified")
        if daemon is not None:
            expected_profile = (
                "postgres-primary"
                if daemon.postgresql_dsn_file is not None
                else "sqlite-bundled"
            )
            if storage["profile"] != expected_profile:
                raise BlockedStory(
                    "storage_engine_identity_mismatch",
                    expected_profile=expected_profile,
                    observed_profile=storage["profile"],
                )
        timer_receipts: list[dict[str, Any]] = []
        stage = "room_creation"
        request = {
            "pack": {"id": PACK_ID, "version": PACK_VERSION, "digest": PACK_DIGEST},
            "configuration": {
                "pack_id": PACK_ID,
                "pack_schema": 1,
                "roles": list(ROLES),
                "briefing_duration_seconds": 30,
                "negotiation_duration_seconds": 90,
                "commitment_duration_seconds": 30,
                "commitment_reminder_seconds_before_deadline": 10,
                "result_duration_seconds": 20,
                "maximum_plans": 12,
                "maximum_open_offers_per_role": 4,
            },
            "members": [
                {
                    "principal_id": PRINCIPAL_IDS[i],
                    "principal_kind": "agent",
                    "role": role,
                    "access_mode": "participant",
                }
                for i, role in enumerate(ROLES)
            ],
            "idempotency_key": CREATE_IDEMPOTENCY_KEY,
        }
        created = await host.create_room(request)
        room_id = created["room_id"]
        member_ids = created.get("member_ids")
        if not isinstance(member_ids, list) or len(member_ids) != 3:
            raise BlockedStory("heist_genesis_did_not_create_three_seats")

        stage = "capability_provisioning"
        principals = list(PRINCIPAL_IDS)
        runner_capabilities = []
        for index, member_id in enumerate(member_ids):
            try:
                runner_capability = issue_runner_capability(
                    args.base_url,
                    args.operator_bearer,
                    room_id,
                    member_id,
                    principals[index],
                    runner_id=RUNNER_PROVISIONING_IDS[index],
                    change_keys=RUNNER_PROVISIONING_KEYS[index],
                )
            except BlockedStory as error:
                raise BlockedStory(
                    "runner_principal_provisioning_rejected",
                    member_index=index,
                    **error.details,
                ) from error
            runner_capabilities.append(runner_capability)
        member_bearers = []
        for index, member_id in enumerate(member_ids):
            try:
                capability = issue_member_capability(
                    args.base_url,
                    args.operator_bearer,
                    room_id,
                    member_id,
                    principals[index],
                    MEMBER_IDEMPOTENCY_KEYS[index],
                )
            except BlockedStory as error:
                raise BlockedStory(
                    "member_capability_rejected",
                    member_index=index,
                    **error.details,
                ) from error
            bearer = capability.get("bearer")
            if not isinstance(bearer, str) or not bearer.startswith("wsb1:"):
                raise BlockedStory("member_capability_not_issued")
            member_bearers.append(Client(args.base_url, bearer))

        navigator, insider, _broker = member_bearers
        broker_runner_capability = runner_capabilities[2]
        runner_bearer = broker_runner_capability.get("bearer")
        if not isinstance(runner_bearer, str) or not runner_bearer.startswith("wsb1:"):
            raise BlockedStory("runner_capability_not_issued")
        runner_client = Client(args.base_url, runner_bearer)
        runner = await runner_client.open_runner(RUNNER_ID, 1, [PACK_ID])

        stage = "initial_projection"
        nav_projection = await _projection(navigator, room_id)
        insider_projection = await _projection(insider, room_id)
        _assert_heist_projection(nav_projection, "navigator")
        _assert_heist_projection(insider_projection, "insider")
        phases = [_phase_label(nav_projection["projection"]["activity"].get("phase"))]

        await _act(
            navigator,
            room_id,
            member_ids[0],
            "inspect_clue",
            {"clue_id": "route"},
            action_id=ACTION_IDS["nav_inspect"],
        )
        await _act(
            insider,
            room_id,
            member_ids[1],
            "inspect_clue",
            {"clue_id": "entry_window"},
            action_id=ACTION_IDS["insider_inspect"],
        )
        nav_projection = await _projection(navigator, room_id)
        insider_projection = await _projection(insider, room_id)
        route_claim = _private_claim_code(nav_projection, "route")
        entry_claim = _private_claim_code(insider_projection, "entry_window")

        timer_receipts.append(
            _timer_receipt(
                await _fire_when_due(args, room_id, TIMER_PHASE, 1),
                "briefing_to_negotiation",
            )
        )
        nav_projection = await _projection(navigator, room_id)
        phases.append(
            _phase_label(nav_projection["projection"]["activity"].get("phase"))
        )
        await _act(
            navigator,
            room_id,
            member_ids[0],
            "publish_clue",
            {"clue_id": "route", "claim_code": route_claim},
            action_id=ACTION_IDS["nav_publish"],
        )
        await _act(
            insider,
            room_id,
            member_ids[1],
            "publish_clue",
            {"clue_id": "entry_window", "claim_code": entry_claim},
            action_id=ACTION_IDS["insider_publish"],
        )
        plan_id = ACTION_IDS["plan"]
        selected_plan = _plan_from_published_claims(route_claim, entry_claim)
        await _act(
            navigator,
            room_id,
            member_ids[0],
            "propose_plan",
            selected_plan,
            action_id=plan_id,
        )
        stage = "first_activation"
        if args.exercise_lost_claim_reply:
            first_activation = await _lost_claim_reply_story(
                runner,
                room_id,
                member_ids[2],
                daemon,
                wait_timeout=args.offer_timeout,
            )
        else:
            first_activation = await _activation_story(
                runner, room_id, member_ids[2], wait_timeout=args.offer_timeout
            )
        stage = "endorse_plan"
        await _act(
            insider,
            room_id,
            member_ids[1],
            "endorse_plan",
            {"plan_id": plan_id},
            action_id=ACTION_IDS["endorse"],
        )

        stage = "negotiation_timer"
        timer_receipts.append(
            _timer_receipt(
                await _fire_when_due(args, room_id, TIMER_PHASE, 2),
                "negotiation_to_commitment",
            )
        )
        nav_projection = await _projection(navigator, room_id)
        phases.append(
            _phase_label(nav_projection["projection"]["activity"].get("phase"))
        )
        stage = "commitment_activation"
        second_activation = await _fresh_activation_recovery(
            runner, room_id, member_ids[2], wait_timeout=args.offer_timeout
        )

        kill_boundary = None
        stage = "commitments"
        if args.exercise_kill_boundary:
            kill_boundary = await _exercise_kill_boundary(
                navigator, room_id, member_ids[0], daemon
            )
        else:
            await _act(
                navigator,
                room_id,
                member_ids[0],
                "commit_move",
                {"selected_plan_id": plan_id, "contribute_required_resource": False},
                action_id=ACTION_IDS["nav_commit"],
            )
        insider_commit = await _act(
            insider,
            room_id,
            member_ids[1],
            "commit_move",
            {"selected_plan_id": plan_id, "contribute_required_resource": True},
            action_id=ACTION_IDS["insider_commit"],
        )
        duplicate_action = await _act(
            insider,
            room_id,
            member_ids[1],
            "commit_move",
            {"selected_plan_id": plan_id, "contribute_required_resource": True},
            action_id=ACTION_IDS["insider_commit"],
            expected_room_seq=insider_commit["room_head"]["room_seq"] - 1,
        )

        stage = "commitment_timer"
        timer_receipts.append(
            _timer_receipt(
                await _fire_when_due(args, room_id, TIMER_PHASE, 3),
                "commitment_to_resolution",
            )
        )
        nav_projection = await _projection(navigator, room_id)
        phases.append(
            _phase_label(nav_projection["projection"]["activity"].get("phase"))
        )
        timer_receipts.append(
            _timer_receipt(
                await _fire_when_due(args, room_id, TIMER_RESOLVE, 1),
                "resolution_to_result",
            )
        )
        nav_projection = await _projection(navigator, room_id)
        phases.append(
            _phase_label(nav_projection["projection"]["activity"].get("phase"))
        )
        await _act(
            navigator,
            room_id,
            member_ids[0],
            "acknowledge_result",
            {},
            action_id=ACTION_IDS["nav_ack"],
        )
        await _act(
            insider,
            room_id,
            member_ids[1],
            "acknowledge_result",
            {},
            action_id=ACTION_IDS["insider_ack"],
        )
        timer_receipts.append(
            _timer_receipt(
                await _fire_when_due(args, room_id, TIMER_PHASE, 4),
                "result_to_complete",
            )
        )
        nav_projection = await _projection(navigator, room_id)
        phases.append(
            _phase_label(nav_projection["projection"]["activity"].get("phase"))
        )

        stage = "final_restart"
        restart = {"requested": bool(daemon), "performed": False}
        if daemon:
            restart_started = time.monotonic()
            daemon.restart()
            restart["performed"] = True
        final_projection = await _projection(navigator, room_id)
        if daemon:
            restart["recovery_duration_ms"] = round(
                (time.monotonic() - restart_started) * 1000, 3
            )

        replay = await navigator.replay(
            room_id, final_projection["room_head"]["room_seq"]
        )
        replay_hash_parity = _verify_replay_hash_parity(final_projection, replay)
        activity = final_projection["projection"]["activity"]
        lost_claim_reply = (
            {
                "status": "completed",
                **first_activation["lost_claim_reply"],
            }
            if args.exercise_lost_claim_reply
            and isinstance(first_activation.get("lost_claim_reply"), dict)
            else {
                "status": "not_exercised",
                "reason": "public SDK has no transport kill hook; exact retry API requires the original lost request",
            }
        )
        result = {
            "status": "completed",
            "evidence_class": "real_disposable_daemon_http_websocket_sdk",
            "live_evidence": True,
            "storage": storage,
            "room_id": room_id,
            "phase_path": phases,
            "six_phase_order": phases
            == [
                "Briefing",
                "Negotiation",
                "Commitment",
                "Resolution",
                "Result",
                "Complete",
            ],
            "activation": {"first": first_activation, "commitment": second_activation},
            "timers": timer_receipts,
            "duplicate_action": {
                "duplicate": bool(duplicate_action.get("duplicate")),
                "transition_id_present": "transition_id" in duplicate_action,
            },
            "final": {
                "phase": activity.get("phase"),
                "outcome": activity.get("outcome"),
                "commitment_count": activity.get("commitment_count"),
                "broker_seat_present": next(
                    (
                        seat.get("present")
                        for seat in activity.get("seats", [])
                        if seat.get("role") == "broker"
                    ),
                    None,
                ),
                "replay_verified": replay.get("verification") == "verified",
                "replay_hash_parity": replay_hash_parity,
                "projection_head": final_projection.get("room_head"),
                "replay_head": replay.get("room_head"),
            },
            "restart": restart,
            "lost_claim_reply": lost_claim_reply,
            "fabricated_offer": False,
            "private_contexts_emitted": False,
            "privacy": {
                "precompletion_final_reveal_absent": True,
                "activation_contexts_not_emitted": True,
                "credentials_not_emitted": True,
            },
            "secrets": "not_emitted",
        }
        story_duration_ms = round((time.monotonic() - story_started) * 1000, 3)
        recovery_samples = [
            restart["recovery_duration_ms"]
            for _ in (0,)
            if isinstance(restart.get("recovery_duration_ms"), (int, float))
        ]
        lost_recovery = first_activation.get("lost_claim_reply", {}).get(
            "recovery_duration_ms"
        )
        if isinstance(lost_recovery, (int, float)):
            recovery_samples.append(lost_recovery)
        final_room_seq = final_projection.get("room_head", {}).get("room_seq")
        result["measurements"] = {
            "latency_ms": list(_ACTION_LATENCY_SAMPLES_MS),
            "load": {
                "active_rooms": 1,
                "actions_per_second": round(
                    len(_ACTION_LATENCY_SAMPLES_MS) / (story_duration_ms / 1000), 6
                ),
                "transition_rate_per_second": round(
                    final_room_seq / (story_duration_ms / 1000), 6
                ),
            },
            "fan_out": {
                "observation_fan_out": 3,
                "observers_per_room": 3,
            },
            "recovery": {"durations_ms": recovery_samples},
            "story_duration_ms": story_duration_ms,
            "observed_transitions": final_room_seq,
            "timer_commits": len(timer_receipts),
            "activation_story_count": 2,
        }
        result["reference_workload"] = {
            "payload_sizes_bytes": sorted(_ACTION_PAYLOAD_SIZES_BYTES),
            "pack_id": PACK_ID,
            "participants_per_room": 3,
            "fan_out": result["measurements"]["fan_out"]["observation_fan_out"],
            "snapshot_cadence_transitions": 1,
        }
        if kill_boundary is not None:
            result["kill_boundary"] = kill_boundary
        if not result["six_phase_order"] or not result["final"]["replay_verified"]:
            raise BlockedStory("live_six_phase_or_replay_invariant_failed")
        return result
    except BlockedStory as error:
        return {
            "status": "blocked",
            "reason_code": error.reason_code,
            **error.details,
            "live_evidence": True,
            "fabricated_offer": False,
            "private_contexts_emitted": False,
            "secrets": "not_emitted",
        }
    except ProtocolError as error:
        return {
            "status": "blocked",
            "reason_code": "public_server_rejected",
            "stage": stage,
            "error_code": error.code,
            "retryable": error.retryable,
            "live_evidence": True,
            "fabricated_offer": False,
            "private_contexts_emitted": False,
            "secrets": "not_emitted",
        }
    except (OSError, TimeoutError, ValueError) as error:
        return {
            "status": "error",
            "reason_code": _safe_error(error).get("code", type(error).__name__),
            "live_evidence": False,
            "fabricated_offer": False,
            "private_contexts_emitted": False,
            "secrets": "not_emitted",
        }
    finally:
        if runner is not None:
            await runner.close()
        for room in rooms:
            await room.close()


async def _fire_when_due(
    args: argparse.Namespace, room_id: str, timer_id: str, generation: int
) -> dict[str, Any]:
    deadline = time.monotonic() + args.timer_timeout
    last: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        try:
            return fire_timer(
                args.base_url, args.operator_bearer, room_id, timer_id, generation
            )
        except BlockedStory as error:
            last = {"reason_code": error.reason_code, **error.details}
            if last.get("error_code") not in {
                "room_busy",
                "storage_unavailable",
                "invalid_payload",
            }:
                raise BlockedStory(
                    "timer_public_operation_rejected",
                    timer_id=timer_id,
                    generation=generation,
                    last_error=last,
                ) from error
            await asyncio.sleep(0.25)
    raise BlockedStory(
        "timer_not_due_or_not_publicly_addressable",
        timer_id=timer_id,
        generation=generation,
        last_error=last,
    )


def _assert_heist_projection(response: dict[str, Any], role: str) -> None:
    projection = response.get("projection")
    if not isinstance(projection, dict) or not isinstance(
        projection.get("activity"), dict
    ):
        raise BlockedStory("invalid_heist_projection", role=role)
    activity = projection["activity"]
    if activity.get("phase") != "briefing":
        raise BlockedStory(
            "unexpected_initial_heist_phase", phase=activity.get("phase")
        )
    if any(key in activity for key in ("fixture", "clues", "commitments", "exchanges")):
        raise BlockedStory(
            "private_fixture_leaked_to_participant_projection", role=role
        )


def _phase_label(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    return value.replace("_", " ").title()


def _new_daemon(
    binary: str,
    *,
    exercise_kill_boundary: bool = False,
    exercise_lost_claim_reply: bool = False,
    postgresql_dsn_file: pathlib.Path | None = None,
) -> DisposableDaemon:
    binary_path = pathlib.Path(binary).resolve()
    if not binary_path.is_file() or not os.access(binary_path, os.X_OK):
        raise BlockedStory("worldstreamd_binary_missing")
    root = pathlib.Path(tempfile.mkdtemp(prefix="worldstream-wave10-live-"))
    root.chmod(0o700)
    secret_path = root / "authority.secret"
    secret_path.write_bytes(secrets.token_bytes(32))
    secret_path.chmod(0o600)
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    return DisposableDaemon(
        binary_path,
        root,
        port,
        postgresql_dsn_file=postgresql_dsn_file,
        kill_after_action_id=ACTION_IDS["nav_commit"]
        if exercise_kill_boundary
        else None,
        kill_after_claim_id=ULID_A if exercise_lost_claim_reply else None,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url")
    parser.add_argument(
        "--operator-bearer", help="operator bootstrap bearer; never printed"
    )
    parser.add_argument("--spawn-daemon", action="store_true")
    parser.add_argument(
        "--postgresql-dsn-file",
        type=pathlib.Path,
        help="owner-only PostgreSQL DSN file used by the spawned daemon",
    )
    seam_group = parser.add_mutually_exclusive_group()
    seam_group.add_argument(
        "--exercise-kill-boundary",
        action="store_true",
        help="opt in to the exact Action commit-before-reply daemon termination proof",
    )
    seam_group.add_argument(
        "--exercise-lost-claim-reply",
        action="store_true",
        help="opt in to the exact Activation claim-before-reply daemon termination proof",
    )
    parser.add_argument("--binary", default="target/debug/worldstreamd")
    parser.add_argument("--report", type=pathlib.Path)
    parser.add_argument("--timer-timeout", type=float, default=240.0)
    parser.add_argument("--offer-timeout", type=float, default=20.0)
    args = parser.parse_args()
    if args.timer_timeout <= 0 or args.offer_timeout <= 0:
        parser.error("timer-timeout and offer-timeout must be positive")
    if args.postgresql_dsn_file is not None and not args.spawn_daemon:
        parser.error("--postgresql-dsn-file requires --spawn-daemon")
    daemon = None
    if args.spawn_daemon:
        try:
            postgresql_dsn_file = (
                _owner_only_regular_file(args.postgresql_dsn_file)
                if args.postgresql_dsn_file is not None
                else None
            )
            daemon = _new_daemon(
                args.binary,
                exercise_kill_boundary=args.exercise_kill_boundary,
                exercise_lost_claim_reply=args.exercise_lost_claim_reply,
                postgresql_dsn_file=postgresql_dsn_file,
            )
            daemon.start()
        except BlockedStory as error:
            result = {
                "status": "blocked",
                "reason_code": error.reason_code,
                "live_evidence": False,
                "secrets": "not_emitted",
            }
            encoded = json.dumps(result, sort_keys=True, separators=(",", ":"))
            if args.report is not None:
                args.report.parent.mkdir(parents=True, exist_ok=True)
                args.report.write_text(encoded + "\n", encoding="utf-8")
            print(encoded)
            return 2
        args.base_url = daemon.base_url
        args.operator_bearer = daemon.bearer
        args.daemon = daemon
    else:
        args.daemon = None
    try:
        result = asyncio.run(run_live(args))
    finally:
        if daemon is not None:
            daemon.stop()
    encoded = json.dumps(result, sort_keys=True, separators=(",", ":"))
    if args.report is not None:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(encoded + "\n", encoding="utf-8")
    print(encoded)
    return 0 if result["status"] == "completed" else 2


if __name__ == "__main__":
    raise SystemExit(main())

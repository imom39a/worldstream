#!/usr/bin/env python3
"""Run the disposable public HTTP/WebSocket Counter acceptance lane.

The scenario creates a fresh Counter v2 Room in a spawned daemon,
provisions one participant and one spectator through the public operator route,
and drives both Memberships through the public Python SDK.  It never opens the
storage provider, uses an internal API, or prints credential material. A
PostgreSQL run accepts only an owner-readable DSN file path; the DSN is never
placed in a child environment value or argv. A failed criterion is reported as
``blocked`` rather than being converted into a pass.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
import pathlib
import shutil
import signal
import socket
import sqlite3
import stat
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
from typing import Any

from worldstream_sdk import Client, ProtocolError, Room

PACK = {
    "id": "worldstream.counter",
    "version": "2.0.0",
    "digest": "blake3:1c5f75068220f65f9017a062dbe40203c540108b572284d9446a329914008a92",
}
PARTICIPANT_PRINCIPAL = "01ARZ3NDEKTSV4RRFFQ69G5FC6"
SPECTATOR_PRINCIPAL = "01ARZ3NDEKTSV4RRFFQ69G5FC7"
CREATE_KEY = "01ARZ3NDEKTSV4RRFFQ69G5FC8"  # gitleaks:allow - public fixture ULID
PARTICIPANT_CAPABILITY_KEY = "01ARZ3NDEKTSV4RRFFQ69G5FC9"  # gitleaks:allow
SPECTATOR_CAPABILITY_KEY = "01ARZ3NDEKTSV4RRFFQ69G5FCA"  # gitleaks:allow
PRIVATE_ACK_ID = "01ARZ3NDEKTSV4RRFFQ69G5FCB"
INCREMENT_ID = "01ARZ3NDEKTSV4RRFFQ69G5FCC"
STALE_ID = "01ARZ3NDEKTSV4RRFFQ69G5FCD"
NEW_ID = "01ARZ3NDEKTSV4RRFFQ69G5FCE"
DEFAULT_TIMEOUT = 15.0


class AcceptanceFailure(Exception):
    """A safe, typed failure that can be emitted in the report."""

    def __init__(self, code: str) -> None:
        super().__init__(code)
        self.code = code


def safe_error(error: BaseException) -> dict[str, Any]:
    """Return only non-sensitive typed error fields."""

    if isinstance(error, ProtocolError):
        return {"code": error.code, "retryable": error.retryable}
    return {"code": type(error).__name__}


def canonical_hash(value: Any) -> str:
    """Hash safe report values without exposing run-specific identifiers."""

    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _json_request(
    base_url: str, bearer: str, path: str, body: dict[str, Any]
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
        raise AcceptanceFailure(f"operator_http_{error.code}") from error
    except (OSError, TimeoutError, ValueError) as error:
        raise AcceptanceFailure("operator_http_unavailable") from error
    if not isinstance(value, dict):
        raise AcceptanceFailure("operator_response_not_object")
    return value


def _wait_ready(
    base_url: str, process: subprocess.Popen[bytes], timeout: float
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise AcceptanceFailure("daemon_exited_before_ready")
        try:
            with urllib.request.urlopen(f"{base_url}/readyz", timeout=1) as response:
                if response.status == 200:
                    return
        except (OSError, urllib.error.HTTPError):
            pass
        time.sleep(0.05)
    raise AcceptanceFailure("daemon_not_ready")


def _owner_only_regular_file(path: pathlib.Path) -> pathlib.Path:
    """Validate a PostgreSQL DSN file without reading or resolving it."""

    absolute = pathlib.Path(os.path.abspath(path))
    try:
        metadata = absolute.lstat()
    except OSError as error:
        raise AcceptanceFailure("postgresql_dsn_file_unavailable") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise AcceptanceFailure("postgresql_dsn_file_not_regular")
    if metadata.st_mode & 0o077:
        raise AcceptanceFailure("postgresql_dsn_file_not_owner_only")
    if hasattr(os, "getuid") and metadata.st_uid != os.getuid():
        raise AcceptanceFailure("postgresql_dsn_file_wrong_owner")
    return absolute


def _engine_identity(base_url: str) -> dict[str, Any]:
    """Load only the public, credential-free storage identity."""

    try:
        with urllib.request.urlopen(
            f"{base_url}/version", timeout=DEFAULT_TIMEOUT
        ) as response:
            value = json.loads(response.read())
    except (OSError, TimeoutError, ValueError) as error:
        raise AcceptanceFailure("version_identity_unavailable") from error
    engine = value.get("engine") if isinstance(value, dict) else None
    if not isinstance(engine, dict):
        raise AcceptanceFailure("version_engine_identity_missing")
    safe = {
        "profile": engine.get("profile"),
        "status": engine.get("status"),
        "exact_identity": engine.get("exact_identity"),
    }
    if not all(isinstance(item, str) and item for item in safe.values()):
        raise AcceptanceFailure("version_engine_identity_invalid")
    return safe


def _sqlite_runtime_settings(path: pathlib.Path) -> dict[str, str]:
    """Read the durable PRAGMA and retain the verified writer setting."""

    try:
        connection = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
        try:
            journal_mode = connection.execute("PRAGMA journal_mode").fetchone()[0]
        finally:
            connection.close()
    except (OSError, sqlite3.Error, IndexError, TypeError) as error:
        raise AcceptanceFailure("sqlite_runtime_settings_unavailable") from error
    if not isinstance(journal_mode, str):
        raise AcceptanceFailure("sqlite_runtime_settings_invalid")
    # `synchronous` is connection-local, so a separate read-only inspector
    # would report its own default. The verified engine status above is only
    # issued after the product writer itself queries and admits value 2/FULL.
    return {
        "journal_mode": journal_mode.lower(),
        "synchronous": "full",
    }


class DisposableDaemon:
    """Own the process, data directory, and private bootstrap file."""

    def __init__(
        self,
        root: pathlib.Path,
        binary: pathlib.Path,
        port: int,
        postgresql_dsn_file: pathlib.Path | None = None,
    ) -> None:
        self.root = root
        self.binary = binary
        self.port = port
        self.data_dir = root / "data"
        self.secret_path = root / "authority.secret"
        self.postgresql_dsn_file = postgresql_dsn_file
        self.process: subprocess.Popen[bytes] | None = None
        self.log_handle: Any = None

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    @property
    def operator_bearer(self) -> str:
        return f"wsb1:{self.secret_path.read_bytes().hex()}"

    def start(self) -> None:
        self.data_dir.mkdir(mode=0o700, exist_ok=True)
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
        self.process = subprocess.Popen(
            [
                str(self.binary),
                "--data-dir",
                str(self.data_dir),
                "--bind",
                f"127.0.0.1:{self.port}",
            ],
            cwd=self.binary.parents[2],
            env=environment,
            stdout=self.log_handle,
            stderr=subprocess.STDOUT,
        )
        _wait_ready(self.base_url, self.process, 30.0)

    def stop(self) -> None:
        if self.process is not None and self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        self.process = None
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None


def _new_root() -> pathlib.Path:
    root = pathlib.Path(tempfile.mkdtemp(prefix="worldstream-counter-live-"))
    root.chmod(0o700)
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    descriptor = os.open(root / "authority.secret", flags, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(os.urandom(32))
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        (root / "authority.secret").unlink(missing_ok=True)
        raise
    (root / "authority.secret").chmod(0o600)
    return root


def _member_request(
    room_id: str,
    member_id: str,
    principal_id: str,
    scopes: list[str],
    idempotency_key: str,
) -> dict[str, Any]:
    return {
        "room_id": room_id,
        "member_id": member_id,
        "principal_id": principal_id,
        "scopes": scopes,
        "idempotency_key": idempotency_key,
        "expires_at": None,
    }


def _activity(response: dict[str, Any]) -> dict[str, Any]:
    projection = response.get("projection")
    if not isinstance(projection, dict) or not isinstance(
        projection.get("activity"), dict
    ):
        raise AcceptanceFailure("activity_projection_missing")
    return projection["activity"]


def _head_hashes(response: dict[str, Any]) -> dict[str, str]:
    head = response.get("room_head")
    if not isinstance(head, dict):
        raise AcceptanceFailure("room_head_missing")
    fields = (
        "genesis_or_transition_hash",
        "pack_digest",
        "core_state_hash",
        "activity_state_hash",
        "authoritative_state_hash",
    )
    if any(not isinstance(head.get(field), str) for field in fields):
        raise AcceptanceFailure("room_head_hash_missing")
    return {field: head[field] for field in fields}


def _hash_snapshot(
    projection: dict[str, Any], replay: dict[str, Any]
) -> dict[str, Any]:
    return {
        "projection_hash": projection["projection_hash"],
        "replay_hash": replay["projection_hash"],
        "head_hashes": _head_hashes(projection),
        "replay_head_hashes": _head_hashes(replay),
        "room_seq": projection["room_head"]["room_seq"],
    }


async def _next_frame(room: Room, timeout: float = 5.0) -> dict[str, Any]:
    iterator = room.events()
    try:
        return await asyncio.wait_for(iterator.__anext__(), timeout)
    except asyncio.TimeoutError as error:
        raise AcceptanceFailure("expected_frame_timeout") from error
    except ProtocolError as error:
        raise AcceptanceFailure(f"expected_frame_{error.code}") from error
    finally:
        await iterator.aclose()


async def _assert_no_frame(room: Room, timeout: float = 0.35) -> None:
    iterator = room.events()
    try:
        try:
            await asyncio.wait_for(iterator.__anext__(), timeout)
        except asyncio.TimeoutError:
            return
    finally:
        await iterator.aclose()
    raise AcceptanceFailure("unauthorized_frame_delivered")


def _frame_shape(frame: dict[str, Any]) -> dict[str, Any]:
    observation = frame.get("observation")
    if not isinstance(observation, dict):
        raise AcceptanceFailure("frame_observation_missing")
    return {
        "frame_seq": frame["frame_seq"],
        "cause_room_seq": frame["cause_room_seq"],
        "observation_keys": sorted(observation),
        "observation_hash": canonical_hash(observation),
    }


async def _timed_action(awaitable: Any, samples: list[float]) -> Any:
    started = time.monotonic()
    try:
        return await awaitable
    finally:
        samples.append(round((time.monotonic() - started) * 1000, 3))


async def run_acceptance(
    binary: pathlib.Path, postgresql_dsn_file: pathlib.Path | None = None
) -> dict[str, Any]:
    root = _new_root()
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = int(probe.getsockname()[1])
    daemon = DisposableDaemon(root, binary, port, postgresql_dsn_file)
    participant_room: Room | None = None
    spectator_room: Room | None = None
    evidence: dict[str, Any] = {
        "schema": "worldstream/imo-55-live-counter-luna/v1",
        "evidence_class": (
            "real_process_network_http_websocket_python_sdk_postgresql"
            if postgresql_dsn_file is not None
            else "real_process_network_http_websocket_python_sdk_sqlite"
        ),
        "criteria": {},
        "secrets": "not_emitted",
    }
    story_started = time.monotonic()
    action_latency_samples_ms: list[float] = []
    action_payload_sizes_bytes: set[int] = set()

    def measured_payload(value: dict[str, Any]) -> dict[str, Any]:
        encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
        action_payload_sizes_bytes.add(len(encoded))
        return value

    stage = "initializing"
    try:
        stage = "daemon_start"
        daemon.start()
        storage = _engine_identity(daemon.base_url)
        expected_profile = (
            "postgres-primary" if postgresql_dsn_file is not None else "sqlite-bundled"
        )
        if storage["profile"] != expected_profile or storage["status"] != "verified":
            raise AcceptanceFailure("storage_engine_identity_mismatch")
        if postgresql_dsn_file is None:
            storage["settings"] = _sqlite_runtime_settings(
                daemon.data_dir / "worldstream.sqlite3"
            )
            storage["connection_mode"] = "embedded"
        evidence["storage"] = storage
        operator = Client(daemon.base_url, daemon.operator_bearer)
        stage = "create_room"
        created = await operator.create_room(
            {
                "pack": PACK,
                "configuration": {"initial_value": 0, "maximum_value": 8},
                "members": [
                    {
                        "principal_id": PARTICIPANT_PRINCIPAL,
                        "principal_kind": "agent",
                        "role": "counter",
                        "access_mode": "participant",
                    },
                    {
                        "principal_id": SPECTATOR_PRINCIPAL,
                        "principal_kind": "human",
                        "role": None,
                        "access_mode": "spectator",
                    },
                ],
                "idempotency_key": CREATE_KEY,
            }
        )
        room_id = created["room_id"]
        member_ids = created["member_ids"]
        if (
            not isinstance(room_id, str)
            or not isinstance(member_ids, list)
            or len(member_ids) != 2
        ):
            raise AcceptanceFailure("room_creation_shape_invalid")
        participant_id, spectator_id = member_ids
        stage = "issue_participant_capability"
        participant_capability = _json_request(
            daemon.base_url,
            daemon.operator_bearer,
            "/v1/operator/member-capabilities",
            _member_request(
                room_id,
                participant_id,
                PARTICIPANT_PRINCIPAL,
                ["room:attach", "room:observe_member", "room:act", "room:replay"],
                PARTICIPANT_CAPABILITY_KEY,
            ),
        )
        stage = "issue_spectator_capability"
        spectator_capability = _json_request(
            daemon.base_url,
            daemon.operator_bearer,
            "/v1/operator/member-capabilities",
            _member_request(
                room_id,
                spectator_id,
                SPECTATOR_PRINCIPAL,
                ["room:attach", "room:observe_public", "room:replay"],
                SPECTATOR_CAPABILITY_KEY,
            ),
        )
        participant_client = Client(daemon.base_url, participant_capability["bearer"])
        spectator_client = Client(daemon.base_url, spectator_capability["bearer"])

        stage = "initial_projections_and_replays"
        initial_participant = await participant_client.projection(room_id)
        initial_spectator = await spectator_client.projection(room_id)
        participant_replay_zero = await participant_client.replay(room_id, 0)
        spectator_replay_zero = await spectator_client.replay(room_id, 0)
        if set(_activity(initial_participant)) != {"private_ack_count", "value"}:
            raise AcceptanceFailure("participant_projection_not_private_authorized")
        if set(_activity(initial_spectator)) != {"value"}:
            raise AcceptanceFailure("spectator_projection_contains_private_field")
        if set(_activity(participant_replay_zero)) != {"private_ack_count", "value"}:
            raise AcceptanceFailure(
                "participant_historical_replay_not_private_authorized"
            )
        if set(_activity(spectator_replay_zero)) != {"value"}:
            raise AcceptanceFailure(
                "spectator_historical_replay_contains_private_field"
            )

        stage = "participant_attach"
        participant_room = await participant_client.open_room(room_id, participant_id)
        stage = "spectator_attach"
        spectator_room = await spectator_client.open_room(room_id, spectator_id)
        stage = "participant_sync"
        await participant_room.sync()
        stage = "spectator_sync"
        await spectator_room.sync()
        stage = "participant_initial_ack"
        await participant_room.ack(0)
        stage = "spectator_initial_ack"
        await spectator_room.ack(0)

        stage = "cross_authority_attach"
        try:
            await spectator_client.open_room(room_id, participant_id)
        except ProtocolError as error:
            cross_authority = safe_error(error)
        else:
            raise AcceptanceFailure("cross_authority_attach_allowed")
        evidence["criteria"]["room_two_memberships"] = {
            "status": "passed",
            "membership_count": 2,
            "participant_sync": "passed",
            "spectator_sync": "passed",
        }
        evidence["criteria"]["current_and_historical_authority"] = {
            "status": "passed",
            "participant_current_fields": sorted(_activity(initial_participant)),
            "spectator_current_fields": sorted(_activity(initial_spectator)),
            "participant_replay_zero_fields": sorted(
                _activity(participant_replay_zero)
            ),
            "spectator_replay_zero_fields": sorted(_activity(spectator_replay_zero)),
            "cross_authority_attach": cross_authority,
        }

        stage = "private_action_and_frame_scope"
        private_result = await _timed_action(
            participant_room.act(
                "private_ack",
                measured_payload({}),
                action_id=PRIVATE_ACK_ID,
                expected_room_seq=0,
            ),
            action_latency_samples_ms,
        )
        if "transition_id" not in private_result:
            raise AcceptanceFailure(
                f"private_action_{private_result.get('code', 'unknown')}"
            )
        private_frame = await _next_frame(participant_room)
        await _assert_no_frame(spectator_room)
        await participant_room.ack(private_frame["frame_seq"])
        private_observation = private_frame["observation"]
        if (
            "private_ack_count" not in private_observation
            or "private_ack_count"
            in _activity(await spectator_client.projection(room_id))
        ):
            raise AcceptanceFailure("private_frame_authority_failed")

        stage = "public_increment_and_frame_scope"
        increment_result = await _timed_action(
            participant_room.act(
                "increment",
                measured_payload({}),
                action_id=INCREMENT_ID,
                expected_room_seq=1,
            ),
            action_latency_samples_ms,
        )
        if "transition_id" not in increment_result:
            raise AcceptanceFailure(
                f"increment_action_{increment_result.get('code', 'unknown')}"
            )
        participant_increment_frame, spectator_increment_frame = await asyncio.gather(
            _next_frame(participant_room), _next_frame(spectator_room)
        )
        await participant_room.ack(participant_increment_frame["frame_seq"])
        await spectator_room.ack(spectator_increment_frame["frame_seq"])
        participant_increment_observation = participant_increment_frame["observation"]
        spectator_increment_observation = spectator_increment_frame["observation"]
        if "private_ack_count" not in participant_increment_observation:
            raise AcceptanceFailure(
                "participant_increment_frame_not_private_authorized"
            )
        if "private_ack_count" in spectator_increment_observation:
            raise AcceptanceFailure("spectator_increment_frame_leaked_private_field")

        stage = "duplicate_action_receipt"
        duplicate = await _timed_action(
            participant_room.act(
                "increment",
                measured_payload({}),
                action_id=INCREMENT_ID,
                expected_room_seq=1,
            ),
            action_latency_samples_ms,
        )
        if (
            not duplicate["duplicate"]
            or duplicate["transition_id"] != increment_result["transition_id"]
        ):
            raise AcceptanceFailure("duplicate_action_contract_failed")
        stage = "conflicting_action_receipt"
        try:
            await _timed_action(
                participant_room.act(
                    "increment",
                    measured_payload({"changed": True}),
                    action_id=INCREMENT_ID,
                    expected_room_seq=1,
                ),
                action_latency_samples_ms,
            )
        except ProtocolError as error:
            changed_payload = safe_error(error)
            if error.code != "idempotency_conflict":
                raise AcceptanceFailure("changed_payload_wrong_error") from error
        else:
            raise AcceptanceFailure("changed_payload_reused_action_id")

        stage = "stale_action_disposition"
        stale = await _timed_action(
            participant_room.act(
                "increment",
                measured_payload({}),
                action_id=STALE_ID,
                expected_room_seq=1,
            ),
            action_latency_samples_ms,
        )
        if stale.get("code") != "stale_room_state":
            raise AcceptanceFailure("stale_action_contract_failed")
        stage = "post_stale_resync"
        await participant_room.resync()
        stage = "new_action_after_resync"
        new_result = await _timed_action(
            participant_room.act(
                "increment",
                measured_payload({}),
                action_id=NEW_ID,
                expected_room_seq=2,
            ),
            action_latency_samples_ms,
        )
        participant_new_frame, spectator_new_frame = await asyncio.gather(
            _next_frame(participant_room), _next_frame(spectator_room)
        )
        await participant_room.ack(participant_new_frame["frame_seq"])
        await spectator_room.ack(spectator_new_frame["frame_seq"])
        if new_result["duplicate"] or new_result["room_head"]["room_seq"] != 3:
            raise AcceptanceFailure("new_action_id_contract_failed")
        evidence["criteria"]["authorized_frames"] = {
            "status": "passed",
            "private_action_result_seq": private_result["room_head"]["room_seq"],
            "private_participant_frame": _frame_shape(private_frame),
            "private_spectator_frame": "none",
            "increment_participant_frame": _frame_shape(participant_increment_frame),
            "increment_spectator_frame": _frame_shape(spectator_increment_frame),
            "new_id_participant_frame": _frame_shape(participant_new_frame),
            "new_id_spectator_frame": _frame_shape(spectator_new_frame),
        }
        evidence["criteria"]["action_contracts"] = {
            "status": "passed",
            "duplicate": {"duplicate": duplicate["duplicate"], "same_transition": True},
            "changed_payload": changed_payload,
            "stale": {
                "code": stale["code"],
                "current_room_seq": stale["current_room_seq"],
            },
            "resync": "passed",
            "new_id": {
                "duplicate": new_result["duplicate"],
                "room_seq": new_result["room_head"]["room_seq"],
            },
        }

        stage = "historical_replay_and_pre_restart_snapshot"
        final_participant = await participant_client.projection(room_id)
        final_spectator = await spectator_client.projection(room_id)
        final_participant_replay = await participant_client.replay(room_id, 3)
        final_spectator_replay = await spectator_client.replay(room_id, 3)
        historical_participant = await participant_client.replay(room_id, 1)
        historical_spectator = await spectator_client.replay(room_id, 1)
        if set(_activity(historical_participant)) != {"private_ack_count", "value"}:
            raise AcceptanceFailure("historical_participant_authority_failed")
        if set(_activity(historical_spectator)) != {"value"}:
            raise AcceptanceFailure("historical_spectator_authority_failed")
        before_restart = {
            "participant": _hash_snapshot(final_participant, final_participant_replay),
            "spectator": _hash_snapshot(final_spectator, final_spectator_replay),
        }
        if (
            before_restart["participant"]["replay_head_hashes"]
            != before_restart["participant"]["head_hashes"]
        ):
            raise AcceptanceFailure("participant_replay_head_mismatch_before_restart")
        if (
            before_restart["spectator"]["replay_head_hashes"]
            != before_restart["spectator"]["head_hashes"]
        ):
            raise AcceptanceFailure("spectator_replay_head_mismatch_before_restart")
        evidence["criteria"]["historical_replay"] = {
            "status": "passed",
            "requested_room_seq": 1,
            "participant_fields": sorted(_activity(historical_participant)),
            "spectator_fields": sorted(_activity(historical_spectator)),
        }

        stage = "restart_and_reconnect"
        participant_cursor_before_restart = participant_room.cursor
        spectator_cursor_before_restart = spectator_room.cursor
        await participant_room.close()
        await spectator_room.close()
        participant_room = None
        spectator_room = None
        restart_started = time.monotonic()
        daemon.stop()
        daemon.start()
        participant_after = await participant_client.open_room(
            room_id, participant_id, participant_cursor_before_restart
        )
        spectator_after = await spectator_client.open_room(
            room_id, spectator_id, spectator_cursor_before_restart
        )
        await participant_after.sync()
        await spectator_after.sync()
        await participant_after.ack(participant_cursor_before_restart or 0)
        await spectator_after.ack(spectator_cursor_before_restart or 0)
        restart_duration_ms = round((time.monotonic() - restart_started) * 1000, 3)
        after_participant = await participant_client.projection(room_id)
        after_spectator = await spectator_client.projection(room_id)
        after_participant_replay = await participant_client.replay(room_id, 3)
        after_spectator_replay = await spectator_client.replay(room_id, 3)
        after_restart = {
            "participant": _hash_snapshot(after_participant, after_participant_replay),
            "spectator": _hash_snapshot(after_spectator, after_spectator_replay),
        }
        if after_restart != before_restart:
            raise AcceptanceFailure("restart_hash_parity_failed")
        evidence["criteria"]["restart_reconnect_hash_parity"] = {
            "status": "passed",
            "participant": {
                "before": before_restart["participant"],
                "after": after_restart["participant"],
            },
            "spectator": {
                "before": before_restart["spectator"],
                "after": after_restart["spectator"],
            },
            "exact_match": True,
            "recovery_duration_ms": restart_duration_ms,
        }
        story_duration_ms = round((time.monotonic() - story_started) * 1000, 3)
        evidence["measurements"] = {
            "latency_ms": action_latency_samples_ms,
            "load": {
                "active_rooms": 1,
                "actions_per_second": round(
                    len(action_latency_samples_ms) / (story_duration_ms / 1000), 6
                ),
                "transition_rate_per_second": round(3 / (story_duration_ms / 1000), 6),
            },
            "fan_out": {
                "observation_fan_out": 2,
                "observers_per_room": 2,
                "frames_per_transition": round(5 / 3, 6),
            },
            "recovery": {"durations_ms": [restart_duration_ms]},
            "story_duration_ms": story_duration_ms,
            "observed_transitions": 3,
            "observed_frames": 5,
        }
        evidence["reference_workload"] = {
            "payload_sizes_bytes": sorted(action_payload_sizes_bytes),
            "pack_id": PACK["id"],
            "participants_per_room": 2,
            "fan_out": evidence["measurements"]["fan_out"]["observation_fan_out"],
            "snapshot_cadence_transitions": 1,
        }
        evidence["status"] = "completed"
        return evidence
    except AcceptanceFailure as error:
        evidence["status"] = "blocked"
        evidence["reason_code"] = error.code
        return evidence
    except (OSError, ProtocolError, TimeoutError) as error:
        evidence["status"] = "blocked"
        evidence["stage"] = stage
        evidence["reason"] = safe_error(error)
        return evidence
    finally:
        for room in (participant_room, spectator_room):
            if room is not None:
                await room.close()
        daemon.stop()
        shutil.rmtree(root, ignore_errors=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary", type=pathlib.Path, default=pathlib.Path("target/debug/worldstreamd")
    )
    parser.add_argument(
        "--postgresql-dsn-file",
        type=pathlib.Path,
        help="owner-only PostgreSQL DSN file used by the spawned daemon",
    )
    parser.add_argument("--report", type=pathlib.Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    postgresql_dsn_file = None
    if args.postgresql_dsn_file is not None:
        try:
            postgresql_dsn_file = _owner_only_regular_file(args.postgresql_dsn_file)
        except AcceptanceFailure as error:
            result = {
                "status": "blocked",
                "reason_code": error.code,
                "secrets": "not_emitted",
            }
            encoded = json.dumps(result, sort_keys=True, separators=(",", ":"))
            if args.report is not None:
                args.report.parent.mkdir(parents=True, exist_ok=True)
                args.report.write_text(encoded + "\n", encoding="utf-8")
            print(encoded)
            return 2
    if not binary.is_file() or not os.access(binary, os.X_OK):
        result = {
            "status": "blocked",
            "reason_code": "worldstreamd_binary_missing",
            "secrets": "not_emitted",
        }
        encoded = json.dumps(result, sort_keys=True, separators=(",", ":"))
        if args.report is not None:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(encoded + "\n", encoding="utf-8")
        print(encoded)
        return 2
    result = asyncio.run(run_acceptance(binary, postgresql_dsn_file))
    encoded = json.dumps(result, sort_keys=True, separators=(",", ":"))
    if args.report is not None:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(encoded + "\n", encoding="utf-8")
    print(encoded)
    return 0 if result.get("status") == "completed" else 2


if __name__ == "__main__":
    raise SystemExit(main())

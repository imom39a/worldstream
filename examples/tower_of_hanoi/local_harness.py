"""Run a fresh local WorldStream Tower of Hanoi experiment with Luna agents.

The harness keeps three authority boundaries separate:

* WorldStream creates each scoped Membership credential and writes it directly
  to an owner-only file.
* Each solver owns one Membership credential and one separate Runner credential.
  A long-lived local supervisor claims Pack-issued Activations and starts bounded
  Codex invocations; the Participant itself snapshots and submits its own action.
* The harness only provisions, starts the bounded gameplay window, observes the
  public outcome, and retains redacted evidence. It never selects or submits a move.

Each invocation allocates a new `target/worldstream-hanoi-*` directory and
retains it, including sanitized JSONL evidence, for review.
"""

from __future__ import annotations

import argparse
import asyncio
import datetime as dt
import json
import os
import signal
import socket
import stat
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from worldstream_sdk import ProtocolError

from examples.cli_activity.credentials import (
    CredentialError,
    load_membership,
    load_runner,
    sdk_base_url,
    validate_pair,
)

from . import turn
from .protocol import (
    PACK_ID,
    PACK_VERSION,
    HanoiProtocolError,
    closed_code,
    redact_text,
)


class HarnessError(RuntimeError):
    """Closed failure whose text contains neither credentials nor child output."""


OBSERVER_SCOPES = (
    "room:attach",
    "room:observe_member",
)
REPLAY_VERIFIER_SCOPES = (
    "room:attach",
    "room:observe_member",
    "room:replay",
)
_ULID_ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
MAX_MOVE_LIMIT = 10_000


@dataclass(frozen=True)
class PackIdentity:
    pack_id: str
    version: str
    bundle_digest: str
    revision_digest: str


@dataclass(frozen=True)
class SeatCredential:
    seat: str
    path: Path
    room_id: str
    member_id: str
    role: str
    pack: dict[str, Any]


@dataclass(frozen=True)
class RunnerCredential:
    """One protected external Runner export paired with a solver Membership."""

    seat: str
    path: Path


@dataclass(frozen=True)
class ObserverCapabilitySpec:
    """One narrowly scoped derived observer credential purpose."""

    evidence_name: str
    filename_suffix: str
    scopes: tuple[str, ...]


LIVE_OBSERVER_CAPABILITY = ObserverCapabilitySpec(
    "observer-live",
    "observe",
    OBSERVER_SCOPES,
)
REPLAY_VERIFIER_CAPABILITY = ObserverCapabilitySpec(
    "replay-verifier",
    "replay-verifier",
    REPLAY_VERIFIER_SCOPES,
)


def _json_object(value: str, code: str) -> dict[str, Any]:
    try:
        parsed = json.loads(value)
    except json.JSONDecodeError as error:
        raise HarnessError(code) from error
    if not isinstance(parsed, dict):
        raise HarnessError(code)
    return parsed


def local_worldstream_environment(environment: dict[str, str]) -> dict[str, str]:
    """Keep ambient WorldStream configuration out of a fresh local experiment."""
    return {
        key: value
        for key, value in environment.items()
        if not key.startswith("WORLDSTREAM")
    }


def pack_identity_from_inspection(value: object) -> PackIdentity:
    """Require inspect output to bind separate bundle and semantic identities."""
    if not isinstance(value, dict):
        raise HarnessError("pack_inspection_invalid")
    fields = ("pack_id", "explanatory_version", "bundle_digest", "revision_digest")
    values = [value.get(field) for field in fields]
    if any(not isinstance(item, str) or not item for item in values):
        raise HarnessError("pack_inspection_invalid")
    if value.get("status") != "complete" or not all(
        item.startswith("blake3:") for item in values[2:]
    ):
        raise HarnessError("pack_inspection_invalid")
    return PackIdentity(values[0], values[1], values[2], values[3])


def inventory_contains(value: object, pack: PackIdentity) -> bool:
    """Find the exact installed physical Bundle and its semantic revision."""
    if not isinstance(value, dict) or value.get("status") != "complete":
        return False
    entries = value.get("entries")
    return isinstance(entries, list) and any(
        isinstance(entry, dict)
        and entry.get("pack_id") == pack.pack_id
        and entry.get("explanatory_version") == pack.version
        and entry.get("bundle_digest") == pack.bundle_digest
        and entry.get("revision_digest") == pack.revision_digest
        and entry.get("install_state") == "selectable"
        for entry in entries
    )


def installed_pack_paths(data_dir: Path, pack: PackIdentity) -> tuple[Path, ...]:
    """Name the exact retained metadata and bytes a local startup must consume."""
    component = pack.bundle_digest.removeprefix("blake3:")
    root = data_dir / "activity-packs"
    return (
        root / "inventory" / f"{component}.json",
        root / "objects" / "blake3" / component / "bundle.wspack",
        root / "restart-readiness-v1.json",
    )


def room_create_arguments(setup_path: Path, common: Sequence[str]) -> list[str]:
    """Require the reviewed acknowledgement that creation starts this activity."""
    return [
        "room",
        "create",
        "--file",
        str(setup_path),
        "--acknowledge-start",
        *common,
    ]


def manual_setup(
    pack: PackIdentity,
    solver_seats: Sequence[str],
    observer_seat: str,
    *,
    disks: int = 3,
    move_limit: int = MAX_MOVE_LIMIT,
    solver_role: str = "solver",
    observer_role: str = "observer",
) -> dict[str, object]:
    """Build a reviewed Room Setup v1 without relying on Pack-specific examples."""
    _validate_seats(solver_seats, observer_seat)
    if isinstance(disks, bool) or not isinstance(disks, int) or not 1 <= disks <= 10:
        raise HarnessError("disk_count_invalid")
    if (
        isinstance(move_limit, bool)
        or not isinstance(move_limit, int)
        or not 1 <= move_limit <= MAX_MOVE_LIMIT
    ):
        raise HarnessError("move_limit_invalid")
    seats: list[dict[str, object]] = []
    for label in solver_seats:
        seats.append(_external_agent_seat(label, solver_role))
    seats.append(_external_agent_seat(observer_seat, observer_role))
    return {
        "schema": "worldstream/room-setup/v1",
        "pack": {
            "id": pack.pack_id,
            "version": pack.version,
            "digest": pack.revision_digest,
        },
        "configuration": {"disks": disks, "move_limit": move_limit},
        "seats": seats,
        "operator_view": False,
    }


def _external_agent_seat(label: str, role: str) -> dict[str, object]:
    if not _reference(label) or not _reference(role):
        raise HarnessError("seat_reference_invalid")
    return {
        "label": label,
        "role": role,
        "required": True,
        "display_name": label,
        "principal": {"reference": label, "kind": "agent"},
        "assignment": {"mode": "external"},
    }


def _reference(value: object) -> bool:
    return (
        isinstance(value, str)
        and 1 <= len(value) <= 64
        and value[0].isalnum()
        and all(
            character.islower() or character.isdigit() or character == "-"
            for character in value
        )
    )


def _validate_seats(solver_seats: Sequence[str], observer_seat: str) -> None:
    if not 1 <= len(solver_seats) <= 16 or not all(
        _reference(seat) for seat in solver_seats
    ):
        raise HarnessError("solver_seats_invalid")
    if (
        not _reference(observer_seat)
        or len({*solver_seats, observer_seat}) != len(solver_seats) + 1
    ):
        raise HarnessError("seat_labels_not_distinct")


def _new_authority_change_id() -> str:
    """Create a canonical ULID-shaped idempotency key without retaining it."""
    value = int.from_bytes(os.urandom(16), "big")
    encoded = ["0"] * 26
    for index in range(25, -1, -1):
        encoded[index] = _ULID_ALPHABET[value & 0x1F]
        value >>= 5
    return "".join(encoded)


def member_capability_request(
    membership: dict[str, Any],
    capability: ObserverCapabilitySpec,
    idempotency_key: str,
) -> dict[str, object]:
    """Build one reviewed observer Capability request from public fields only."""
    required = ("room_id", "member_id", "principal_id")
    if (
        not isinstance(idempotency_key, str)
        or len(idempotency_key) != 26
        or any(character not in _ULID_ALPHABET for character in idempotency_key)
        or any(
            not isinstance(membership.get(key), str) or not membership[key]
            for key in required
        )
        or capability
        not in {
            LIVE_OBSERVER_CAPABILITY,
            REPLAY_VERIFIER_CAPABILITY,
        }
    ):
        raise HarnessError("observer_capability_request_invalid")
    return {
        "room_id": membership["room_id"],
        "member_id": membership["member_id"],
        "principal_id": membership["principal_id"],
        "scopes": list(capability.scopes),
        "idempotency_key": idempotency_key,
        "expires_at": None,
    }


def member_capability_document(
    membership: dict[str, Any],
    issued: dict[str, object],
    capability: ObserverCapabilitySpec,
) -> dict[str, object]:
    """Convert one operator response into a second, least-privilege export."""
    required = ("room_id", "member_id", "principal_id", "capability_id", "bearer")
    scopes = issued.get("scopes")
    if (
        any(not isinstance(issued.get(key), str) or not issued[key] for key in required)
        or any(
            issued.get(key) != membership.get(key)
            for key in ("room_id", "member_id", "principal_id")
        )
        or not isinstance(scopes, list)
        or any(not isinstance(scope, str) for scope in scopes)
        or set(scopes) != set(capability.scopes)
        or len(scopes) != len(capability.scopes)
    ):
        raise HarnessError("observer_capability_response_invalid")
    return {
        "schema": membership["schema"],
        "operation": membership["operation"],
        "seat": membership["seat"],
        "runtime_url": membership["runtime_url"],
        "room_id": membership["room_id"],
        "member_id": membership["member_id"],
        "principal_id": membership["principal_id"],
        "pack": membership["pack"],
        "role": membership["role"],
        "scopes": list(capability.scopes),
        "bearer": issued["bearer"],
    }


def redacted_member_capability_receipt(
    issued: dict[str, object],
    capability: ObserverCapabilitySpec,
) -> dict[str, object]:
    """Preserve only public operator result facts; never place its bearer in evidence."""
    return {
        "kind": "observer_capability_issued",
        "purpose": capability.evidence_name,
        "capability_id": issued.get("capability_id")
        if isinstance(issued.get("capability_id"), str)
        else None,
        "room_id": issued.get("room_id")
        if isinstance(issued.get("room_id"), str)
        else None,
        "member_id": issued.get("member_id")
        if isinstance(issued.get("member_id"), str)
        else None,
        "principal_id": issued.get("principal_id")
        if isinstance(issued.get("principal_id"), str)
        else None,
        "scopes": list(capability.scopes),
        "bearer_redacted": True,
    }


def credentials_from_paths(
    paths: Sequence[tuple[str, Path]], *, role: str, expected_pack: PackIdentity
) -> list[SeatCredential]:
    """Load distinct scoped files and reject cross-Room or cross-revision mixes."""
    loaded: list[SeatCredential] = []
    identities: set[tuple[str, str]] = set()
    for seat, path in paths:
        document = load_membership(path)
        if document["seat"] != seat or document["role"] != role:
            raise HarnessError("credential_seat_mismatch")
        if document["pack"] != {
            "id": expected_pack.pack_id,
            "version": expected_pack.version,
            "digest": expected_pack.revision_digest,
        }:
            raise HarnessError("credential_revision_mismatch")
        identity = (document["room_id"], document["member_id"])
        if identity in identities:
            raise HarnessError("credential_membership_duplicate")
        identities.add(identity)
        loaded.append(SeatCredential(seat, path, *identity, role, document["pack"]))
    if not loaded or len({item.room_id for item in loaded}) != 1:
        raise HarnessError("credential_room_mismatch")
    return loaded


def codex_command(
    codex: Path,
    workspace: Path,
    effort: str,
    prompt: str,
    *,
    repo: Path,
    python: Path,
    membership_file: Path,
) -> list[str]:
    """Return the fixed isolated-network JSONL invocation for one Luna seat."""
    if effort not in {"low", "medium"}:
        raise HarnessError("codex_effort_invalid")
    if "wsb1:" in prompt.lower():
        raise HarnessError("credential_secret_in_prompt")
    shell_values = {
        "PYTHONPATH": str(repo.resolve()),
        # This is intentionally absolute but not resolved: resolving the venv
        # launcher would discard its site-packages and use the base interpreter.
        "HANOI_PYTHON": str(python.absolute()),
        "HANOI_MEMBERSHIP_FILE": str(membership_file.resolve()),
    }
    if any("wsb1:" in value.lower() for value in shell_values.values()):
        raise HarnessError("credential_secret_in_shell_environment")
    shell_set = (
        "shell_environment_policy.set={"
        + ",".join(f"{key}={json.dumps(value)}" for key, value in shell_values.items())
        + "}"
    )
    return [
        str(codex),
        "-a",
        "never",
        "exec",
        "--ephemeral",
        "--json",
        "-C",
        str(workspace),
        "--skip-git-repo-check",
        "--ignore-user-config",
        "-s",
        "workspace-write",
        "-c",
        "sandbox_workspace_write.network_access=true",
        "-c",
        "sandbox_workspace_write.exclude_slash_tmp=true",
        "-c",
        "sandbox_workspace_write.exclude_tmpdir_env_var=true",
        "-c",
        'shell_environment_policy.inherit="none"',
        "-c",
        shell_set,
        "-m",
        "gpt-5.6-luna",
        "-c",
        f'model_reasoning_effort="{effort}"',
        prompt,
    ]


def _reserved_loopback_listener() -> socket.socket:
    """Reserve a loopback port until the detached managed server claims it."""
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.bind(("127.0.0.1", 0))
    return listener


def _write_new_json(path: Path, value: object) -> None:
    encoded = json.dumps(value, sort_keys=True, indent=2) + "\n"
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as output:
        output.write(encoded)
        output.flush()
        os.fsync(output.fileno())


def _write_private_text(path: Path, value: str) -> None:
    """Replace a bounded transcript while retaining owner-only file permissions."""
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as output:
        output.write(value)
        output.flush()
        os.fsync(output.fileno())
    if stat.S_IMODE(path.stat().st_mode) != 0o600:
        raise HarnessError("evidence_file_unprotected")


def _closed_exception_code(error: Exception) -> str:
    if isinstance(error, HanoiProtocolError):
        return str(error)
    return closed_code(getattr(error, "code", None), type(error).__name__)


def _owner_directory(path: Path) -> None:
    path.mkdir(mode=0o700, parents=True, exist_ok=False)
    if os.name != "nt" and stat.S_IMODE(path.stat().st_mode) != 0o700:
        raise HarnessError("run_directory_unprotected")


class LocalRun:
    """One fresh, retained local Runtime experiment."""

    def __init__(self, arguments: argparse.Namespace):
        self.arguments = arguments
        self.repo = arguments.repo.resolve()
        self.run_root = Path(
            tempfile.mkdtemp(prefix="worldstream-hanoi-", dir=self.repo / "target")
        )
        os.chmod(self.run_root, 0o700)
        self.evidence = self.run_root / "evidence"
        self.credentials_dir = self.run_root / "credentials"
        _owner_directory(self.evidence)
        _owner_directory(self.credentials_dir)
        self.runtime_reservation = (
            _reserved_loopback_listener() if arguments.runtime_port is None else None
        )
        self.controller_reservation = (
            _reserved_loopback_listener() if arguments.controller_port is None else None
        )
        self.canvas_reservation = (
            _reserved_loopback_listener()
            if arguments.canvas_demo and arguments.canvas_port is None
            else None
        )
        self.runtime_address = (
            f"127.0.0.1:{arguments.runtime_port}"
            if arguments.runtime_port is not None
            else f"127.0.0.1:{self.runtime_reservation.getsockname()[1]}"
        )
        self.controller_address = (
            f"127.0.0.1:{arguments.controller_port}"
            if arguments.controller_port is not None
            else f"127.0.0.1:{self.controller_reservation.getsockname()[1]}"
        )
        self.canvas_port = (
            arguments.canvas_port
            if arguments.canvas_port is not None
            else self.canvas_reservation.getsockname()[1]
            if self.canvas_reservation is not None
            else None
        )
        self.state_dir = self.run_root / ".worldstream" / "studio"
        self.data_dir = self.run_root / ".worldstream" / "data"
        self.config_path: Path | None = None
        self.started = False
        self.pack: PackIdentity | None = None
        self.canvas_process: subprocess.Popen[bytes] | None = None
        self.canvas_url: str | None = None
        self.canvas_sink: Path | None = None
        self.participant_processes: list[subprocess.Popen[bytes]] = []
        self.gameplay_started_at: float | None = None
        self.gameplay_deadline_at: float | None = None
        self.gameplay_deadline_epoch_ms: int | None = None
        self.setup_started_at = time.monotonic()

    @property
    def common(self) -> list[str]:
        return [
            *self.config_args,
            "--state-dir",
            str(self.state_dir),
            "--controller",
            self.controller_address,
            "--timeout-seconds",
            str(self.arguments.timeout_seconds),
            "--json",
        ]

    @property
    def config_args(self) -> list[str]:
        if self.config_path is None:
            raise HarnessError("isolated_config_missing")
        return ["--config", str(self.config_path)]

    @property
    def init_common(self) -> list[str]:
        return [
            "--state-dir",
            str(self.state_dir),
            "--timeout-seconds",
            str(self.arguments.timeout_seconds),
            "--json",
        ]

    def command(
        self, arguments: Sequence[str], *, allow_failure: bool = False
    ) -> dict[str, Any]:
        # Initialization deliberately records ambient WorldStream configuration when
        # present.  This retained experiment must instead own one explicit local
        # configuration, so no WorldStream variable reaches any child process.
        environment = local_worldstream_environment(dict(os.environ))
        completed = subprocess.run(
            [str(self.arguments.worldstreamctl), *arguments],
            cwd=self.run_root,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
            timeout=self.arguments.timeout_seconds + 30,
        )
        record = {
            "kind": "worldstreamctl",
            "arguments": [
                argument for argument in arguments if not argument.startswith("wsb1:")
            ],
            "returncode": completed.returncode,
            "stdout": redact_text(completed.stdout),
            "stderr": redact_text(completed.stderr),
        }
        self._append_evidence("operator.jsonl", record)
        if completed.returncode and not allow_failure:
            raise HarnessError("worldstreamctl_command_failed")
        return _json_object(completed.stdout, "worldstreamctl_json_invalid")

    def build_local_binaries(self) -> None:
        """Compile the controller and Runtime that must agree on Pack catalog APIs."""
        if self.arguments.skip_build:
            return
        commands = (
            [
                "cargo",
                "build",
                "--locked",
                "-p",
                "worldstream-server",
                "--bin",
                "worldstreamctl",
                "--bin",
                "worldstreamd",
            ],
            [
                "cargo",
                "build",
                "--locked",
                "-p",
                "worldstream-studio-supervisor",
                "--bin",
                "worldstream-studio-supervisor",
                "--bin",
                "worldstream-assignment-mcp",
            ],
        )
        for arguments in commands:
            completed = subprocess.run(
                arguments,
                cwd=self.repo,
                text=True,
                capture_output=True,
                check=False,
                timeout=self.arguments.build_timeout_seconds,
            )
            self._append_evidence(
                "build.jsonl",
                {
                    "kind": "cargo-build",
                    "arguments": arguments,
                    "returncode": completed.returncode,
                    "stdout": redact_text(completed.stdout),
                    "stderr": redact_text(completed.stderr),
                },
            )
            if completed.returncode:
                raise HarnessError("local_binary_build_failed")

    def _append_evidence(self, filename: str, value: object) -> None:
        encoded = json.dumps(value, sort_keys=True, separators=(",", ":"))
        if "wsb1:" in encoded.lower():
            encoded = redact_text(encoded)
        path = self.evidence / filename
        descriptor = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
        with os.fdopen(descriptor, "a", encoding="utf-8") as output:
            output.write(encoded + "\n")
            output.flush()
            os.fsync(output.fileno())
        if stat.S_IMODE(path.stat().st_mode) != 0o600:
            raise HarnessError("evidence_file_unprotected")

    def start_canvas(
        self, observer: SeatCredential, solvers: Sequence[SeatCredential]
    ) -> None:
        """Start the loopback canvas after the Room's replay preflight succeeds."""
        if not self.arguments.canvas_demo:
            return
        if self.canvas_port is None or self.canvas_process is not None:
            raise HarnessError("canvas_state_invalid")
        if self.canvas_reservation is not None:
            self.canvas_reservation.close()
            self.canvas_reservation = None
        labels_path = self.run_root / "canvas-solver-labels.json"
        ready_path = self.run_root / "canvas-ready.json"
        live_path = self.run_root / "canvas-live.json"
        self.canvas_sink = self.run_root / "canvas-receipts.jsonl"
        _write_new_json(
            labels_path,
            {
                "members": {solver.member_id: solver.seat for solver in solvers},
            },
        )
        _write_private_text(self.canvas_sink, "")
        command = [
            str(self.arguments.python),
            "-m",
            "examples.tower_of_hanoi.live_canvas",
            "--membership-file",
            str(observer.path),
            "--labels-file",
            str(labels_path),
            "--event-file",
            str(self.canvas_sink),
            "--ready-file",
            str(ready_path),
            "--live-file",
            str(live_path),
            "--port",
            str(self.canvas_port),
            "--batch-ms",
            str(self.arguments.canvas_batch_ms),
        ]
        self.canvas_process = subprocess.Popen(
            command,
            cwd=self.repo,
            env=local_worldstream_environment(dict(os.environ)),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        self.canvas_url = self._await_canvas_signal(ready_path, "ready", 10)
        self._await_canvas_signal(live_path, "live", 20)
        print(
            json.dumps(
                {
                    "status": "canvas_live",
                    "canvas_url": self.canvas_url,
                    "room_id": observer.room_id,
                    "run_root": str(self.run_root),
                },
                sort_keys=True,
                separators=(",", ":"),
            ),
            flush=True,
        )

    def _await_canvas_signal(
        self, path: Path, expected_status: str, seconds: float
    ) -> str:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if (
                self.canvas_process is not None
                and self.canvas_process.poll() is not None
            ):
                raise HarnessError("canvas_adapter_failed")
            if path.is_file():
                try:
                    value = _json_object(
                        path.read_text(encoding="utf-8"), "canvas_signal_invalid"
                    )
                except (OSError, UnicodeError, HarnessError):
                    raise HarnessError("canvas_signal_invalid") from None
                if value.get("status") != expected_status:
                    raise HarnessError("canvas_signal_invalid")
                url = value.get("url")
                if expected_status == "ready":
                    if (
                        not isinstance(url, str)
                        or url != f"http://127.0.0.1:{self.canvas_port}/"
                    ):
                        raise HarnessError("canvas_signal_invalid")
                    return url
                return ""
            time.sleep(0.05)
        raise HarnessError("canvas_adapter_timeout")

    def start_gameplay_window(self) -> None:
        """Fence model invocations to the post-setup Community Hanoi time budget."""
        if self.gameplay_started_at is not None:
            raise HarnessError("gameplay_window_already_started")
        self.gameplay_started_at = time.monotonic()
        self.gameplay_deadline_at = (
            self.gameplay_started_at + self.arguments.gameplay_seconds
        )
        self.gameplay_deadline_epoch_ms = int(time.time() * 1000) + (
            self.arguments.gameplay_seconds * 1000
        )
        self._publish_canvas_window("running")

    def _publish_canvas_window(self, status: str) -> None:
        if self.canvas_sink is None or self.gameplay_deadline_epoch_ms is None:
            return
        self._append_canvas_sink(
            {
                "schema": "worldstream/tower-of-hanoi-live-window/v1",
                "status": status,
                "deadline_at_ms": self.gameplay_deadline_epoch_ms,
            }
        )

    def _require_gameplay_time(self) -> None:
        if self.gameplay_deadline_at is None:
            return
        if time.monotonic() >= self.gameplay_deadline_at:
            self._publish_canvas_window("time_limit")
            raise HarnessError("demo_time_limit")

    def _canvas_remaining_seconds(self) -> float | None:
        if self.gameplay_deadline_at is None:
            return None
        self._require_gameplay_time()
        return max(0.01, self.gameplay_deadline_at - time.monotonic())

    def _append_canvas_sink(self, value: dict[str, object]) -> None:
        if self.canvas_sink is None:
            return
        encoded = json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n"
        if "wsb1:" in encoded.lower():
            raise HarnessError("canvas_sink_secret")
        try:
            descriptor = os.open(self.canvas_sink, os.O_WRONLY | os.O_APPEND)
            with os.fdopen(descriptor, "w", encoding="utf-8") as output:
                output.write(encoded)
                output.flush()
                os.fsync(output.fileno())
            if stat.S_IMODE(self.canvas_sink.stat().st_mode) != 0o600:
                raise HarnessError("canvas_sink_unprotected")
        except HarnessError:
            raise
        except OSError as error:
            raise HarnessError("canvas_sink_unavailable") from error

    def publish_canvas_receipt(
        self,
        status: str,
        seat: str,
        action_type: str,
        result: dict[str, object],
    ) -> None:
        """Append one minimal supervisor-observed receipt; browser never sees IDs/payloads."""
        if self.canvas_sink is None:
            return
        if (
            status not in {"accepted", "stale", "rejected"}
            or not _reference(seat)
            or action_type not in {"move_disk", "post_completion_claim", "assess_claim"}
        ):
            raise HarnessError("canvas_receipt_invalid")
        room_seq = (
            result.get("room_seq")
            if status == "accepted"
            else result.get("current_room_seq")
        )
        if isinstance(room_seq, bool) or not isinstance(room_seq, int) or room_seq < 0:
            raise HarnessError("canvas_receipt_invalid")
        event: dict[str, object] = {
            "schema": "worldstream/tower-of-hanoi-live-receipt/v2",
            "status": status,
            "actor": seat,
            "action_type": action_type,
            "room_seq": room_seq,
        }
        work_revision = result.get("work_revision")
        if (
            isinstance(work_revision, int)
            and not isinstance(work_revision, bool)
            and 0 <= work_revision <= MAX_MOVE_LIMIT
        ):
            event["work_revision"] = work_revision
        self._append_canvas_sink(event)

    def prepare_pack(self) -> PackIdentity:
        bundle = self.arguments.bundle.resolve()
        if not bundle.is_file():
            raise HarnessError("bundle_missing")
        inspected = pack_identity_from_inspection(
            self.command(
                ["pack", "inspect", "--bundle", str(bundle), *self.config_args]
            )
        )
        if inspected.pack_id != PACK_ID or inspected.version != PACK_VERSION:
            raise HarnessError("tower_pack_identity_mismatch")
        proof = self.command(
            ["pack", "prove", str(bundle), "--json", *self.config_args]
        )
        if (
            proof.get("status") != "passed"
            or proof.get("bundle_digest") != inspected.bundle_digest
            or proof.get("revision_digest") != inspected.revision_digest
        ):
            raise HarnessError("pack_proof_failed")
        stamp = (
            dt.datetime.now(dt.UTC)
            .replace(microsecond=0)
            .isoformat()
            .replace("+00:00", "Z")
        )
        self.command(
            [
                "pack",
                "approve",
                "--bundle",
                str(bundle),
                "--operator-id",
                "local-hanoi",
                "--decided-at",
                stamp,
                *self.config_args,
            ]
        )
        self.command(
            [
                "pack",
                "install",
                "--bundle",
                str(bundle),
                "--installed-at",
                stamp,
                *self.config_args,
            ]
        )
        self.command(
            [
                "pack",
                "set-selectable",
                "--bundle-digest",
                inspected.bundle_digest,
                "--selectable",
                "true",
                *self.config_args,
            ]
        )
        inventory = self.command(["pack", "inventory", *self.config_args])
        if not inventory_contains(inventory, inspected):
            raise HarnessError("pack_inventory_mismatch")
        self.command(["pack", "restart-readiness", *self.config_args])
        paths = installed_pack_paths(self.data_dir, inspected)
        self._append_evidence(
            "isolated-pack-files.jsonl",
            {
                "kind": "isolated_pack_files",
                "data_dir": str(self.data_dir),
                "paths": [str(path) for path in paths],
                "all_regular_files": all(path.is_file() for path in paths),
            },
        )
        if not all(path.is_file() for path in paths):
            raise HarnessError("isolated_pack_files_missing")
        return inspected

    def start_server(self) -> None:
        self.build_local_binaries()
        initialized = self.command(
            ["--bind", self.runtime_address, "init", *self.init_common]
        )
        initialization = initialized.get("initialization")
        configured = (
            initialization.get("config_path")
            if isinstance(initialization, dict)
            else None
        )
        expected = (self.run_root / ".worldstream" / "worldstream.toml").resolve()
        if not isinstance(configured, str):
            raise HarnessError("isolated_config_missing")
        try:
            actual = Path(configured).resolve(strict=True)
        except OSError as error:
            raise HarnessError("isolated_config_missing") from error
        if actual != expected:
            raise HarnessError("isolated_config_mismatch")
        self.config_path = actual
        # Pack lifecycle is deliberately offline.  Initialization creates the isolated data root;
        # stop remains implicit because init does not start services.
        self.pack = self.prepare_pack()
        for reservation in (self.runtime_reservation, self.controller_reservation):
            if reservation is not None:
                reservation.close()
        self.command(["server", "start", *self.common])
        self.started = True
        self.command(["server", "status", *self.common])
        self.command(["pack", "list", *self.common])
        self.command(["server", "logs", "--tail", "100", *self.common])

    def create_room(
        self, pack: PackIdentity
    ) -> tuple[str, str, list[SeatCredential], list[RunnerCredential], SeatCredential]:
        """Provision paired Membership and external Runner credentials for each solver."""
        setup = manual_setup(
            pack,
            self.arguments.solver_seat,
            self.arguments.observer_seat,
            disks=self.arguments.disks,
            move_limit=self.arguments.move_limit,
            solver_role=self.arguments.solver_role,
            observer_role=self.arguments.observer_role,
        )
        setup_path = self.run_root / "tower-room-setup.json"
        _write_new_json(setup_path, setup)
        self.command(["room", "validate", "--file", str(setup_path), *self.common])
        created = self.command(room_create_arguments(setup_path, self.common))
        operation_id = created.get("operation_id")
        room_id = created.get("room_id")
        if (
            not isinstance(operation_id, str)
            or not isinstance(room_id, str)
            or created.get("status") != "complete"
        ):
            raise HarnessError("room_setup_incomplete")
        membership_paths: list[tuple[str, Path]] = []
        runner_paths: list[tuple[str, Path]] = []
        for seat in self.arguments.solver_seat:
            membership_path = self.credentials_dir / f"{seat}.membership.json"
            runner_path = self.credentials_dir / f"{seat}.runner.json"
            self.command(
                [
                    "client",
                    "export-credentials",
                    "--operation",
                    operation_id,
                    "--seat",
                    seat,
                    "--output",
                    str(membership_path),
                    *self.common,
                ]
            )
            self.command(
                [
                    "runner",
                    "export-credentials",
                    "--operation",
                    operation_id,
                    "--seat",
                    seat,
                    "--output",
                    str(runner_path),
                    *self.common,
                ]
            )
            membership_paths.append((seat, membership_path))
            runner_paths.append((seat, runner_path))
        solvers = credentials_from_paths(
            membership_paths, role=self.arguments.solver_role, expected_pack=pack
        )
        solvers_by_seat = {solver.seat: solver for solver in solvers}
        runners: list[RunnerCredential] = []
        for seat, runner_path in runner_paths:
            if seat not in solvers_by_seat:
                raise HarnessError("runner_credential_seat_mismatch")
            runner_document = load_runner(runner_path)
            if runner_document.get("seat") != seat:
                raise HarnessError("runner_credential_seat_mismatch")
            validate_pair(load_membership(solvers_by_seat[seat].path), runner_document)
            runners.append(RunnerCredential(seat, runner_path))
        observer_path = self.credentials_dir / f"{self.arguments.observer_seat}.json"
        self.command(
            [
                "client",
                "export-credentials",
                "--operation",
                operation_id,
                "--seat",
                self.arguments.observer_seat,
                "--output",
                str(observer_path),
                *self.common,
            ]
        )
        observer = credentials_from_paths(
            [(self.arguments.observer_seat, observer_path)],
            role=self.arguments.observer_role,
            expected_pack=pack,
        )[0]
        if any(item.room_id != room_id for item in [*solvers, observer]):
            raise HarnessError("credential_room_mismatch")
        return operation_id, room_id, solvers, runners, observer

    def _operator_bearer(self) -> str:
        """Read the fresh Runtime bootstrap secret without retaining it in evidence."""
        path = self.run_root / ".worldstream" / "authority.secret"
        try:
            metadata = path.lstat()
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077:
                raise HarnessError("operator_authority_unprotected")
            raw = bytearray(path.read_bytes())
        except HarnessError:
            raise
        except OSError as error:
            raise HarnessError("operator_authority_unavailable") from error
        try:
            if len(raw) != 32:
                raise HarnessError("operator_authority_invalid")
            return "wsb1:" + bytes(raw).hex()
        finally:
            raw[:] = b"\0" * len(raw)

    def issue_observer_capability(
        self,
        bootstrap: SeatCredential,
        pack: PackIdentity,
        capability: ObserverCapabilitySpec,
    ) -> SeatCredential:
        """Issue one actionless observer, replay, or solver-read credential."""
        membership = load_membership(bootstrap.path)
        request_body = member_capability_request(
            membership, capability, _new_authority_change_id()
        )
        encoded = json.dumps(request_body, separators=(",", ":")).encode("utf-8")
        bearer = self._operator_bearer()
        try:
            request = urllib.request.Request(
                sdk_base_url(membership) + "/v1/operator/member-capabilities",
                data=encoded,
                headers={
                    "Accept": "application/json",
                    "Authorization": f"Bearer {bearer}",
                    "Content-Type": "application/json",
                },
                method="POST",
            )
            try:
                with urllib.request.urlopen(
                    request, timeout=self.arguments.timeout_seconds
                ) as response:
                    raw_response = response.read(65_537)
                    if response.status != 200 or len(raw_response) > 65_536:
                        raise HarnessError("observer_capability_operator_rejected")
            except urllib.error.HTTPError as error:
                raise HarnessError("observer_capability_operator_rejected") from error
            except (OSError, TimeoutError) as error:
                raise HarnessError(
                    "observer_capability_operator_unavailable"
                ) from error
        finally:
            # Do not retain the host bearer after the one operator request.
            bearer = ""
        try:
            issued = json.loads(raw_response)
        except (UnicodeError, json.JSONDecodeError) as error:
            raise HarnessError("observer_capability_response_invalid") from error
        if not isinstance(issued, dict):
            raise HarnessError("observer_capability_response_invalid")
        document = member_capability_document(membership, issued, capability)
        path = (
            self.credentials_dir / f"{bootstrap.seat}.{capability.filename_suffix}.json"
        )
        _write_new_json(path, document)
        derived = credentials_from_paths(
            [(bootstrap.seat, path)],
            role=bootstrap.role,
            expected_pack=pack,
        )[0]
        self._append_evidence(
            f"{capability.evidence_name}-capability.jsonl",
            redacted_member_capability_receipt(issued, capability),
        )
        return derived

    def remove_observer_bootstrap(self, bootstrap: SeatCredential) -> None:
        """Delete the default external-seat export before any observer process starts."""
        try:
            metadata = bootstrap.path.lstat()
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077:
                raise HarnessError("observer_bootstrap_unprotected")
            bootstrap.path.unlink()
        except HarnessError:
            raise
        except OSError as error:
            raise HarnessError("observer_bootstrap_remove_failed") from error
        if bootstrap.path.exists():
            raise HarnessError("observer_bootstrap_remove_failed")
        self._append_evidence(
            "observer-bootstrap.jsonl",
            {"kind": "observer_bootstrap_removed", "seat": bootstrap.seat},
        )

    def start_participant_supervisors(
        self, solvers: Sequence[SeatCredential], runners: Sequence[RunnerCredential]
    ) -> None:
        """Start one operational supervisor for each autonomous solver Participant."""
        if self.gameplay_deadline_epoch_ms is None or self.gameplay_deadline_at is None:
            raise HarnessError("gameplay_window_missing")
        if self.participant_processes:
            raise HarnessError("participant_supervisors_started")
        runner_by_seat = {runner.seat: runner for runner in runners}
        if len(runner_by_seat) != len(solvers) or set(runner_by_seat) != {
            solver.seat for solver in solvers
        }:
            raise HarnessError("runner_credentials_invalid")
        for solver in solvers:
            runner = runner_by_seat[solver.seat]
            # Codex transcripts use O_EXCL. Each supervisor owns a parent directory
            # so concurrent bootstrap invocations cannot race on bootstrap.jsonl.
            supervisor_evidence = self.evidence / f"supervisor-{solver.seat}"
            _owner_directory(supervisor_evidence)
            evidence = supervisor_evidence / "events.jsonl"
            stdout_path = supervisor_evidence / "stdout.jsonl"
            descriptor = os.open(
                stdout_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600
            )
            command = [
                str(self.arguments.python),
                "-m",
                "examples.tower_of_hanoi.participant_runner",
                "--membership-file",
                str(solver.path),
                "--runner-file",
                str(runner.path),
                "--repo",
                str(self.repo),
                "--python",
                str(self.arguments.python),
                "--codex",
                str(self.arguments.codex),
                "--solver-effort",
                self.arguments.solver_effort,
                "--codex-timeout-seconds",
                str(self.arguments.codex_timeout_seconds),
                "--deadline-at-ms",
                str(self.gameplay_deadline_epoch_ms),
                "--evidence-file",
                str(evidence),
            ]
            if self.canvas_sink is not None:
                command.extend(["--event-file", str(self.canvas_sink)])
            try:
                with os.fdopen(descriptor, "wb") as output:
                    process = subprocess.Popen(
                        command,
                        cwd=self.repo,
                        env=local_worldstream_environment(dict(os.environ)),
                        stdin=subprocess.DEVNULL,
                        stdout=output,
                        stderr=subprocess.STDOUT,
                        start_new_session=True,
                    )
            except OSError as error:
                raise HarnessError("participant_supervisor_start_failed") from error
            self.participant_processes.append(process)
            self._append_evidence(
                "participant-supervisors.jsonl",
                {
                    "kind": "participant_supervisor_started",
                    "seat": solver.seat,
                    "deadline_at_ms": self.gameplay_deadline_epoch_ms,
                    "model": "gpt-5.6-luna",
                    "reasoning_effort": self.arguments.solver_effort,
                },
            )

    def _stop_participant_supervisors(self) -> None:
        for process in self.participant_processes:
            if process.poll() is not None:
                continue
            try:
                if os.name == "posix":
                    os.killpg(process.pid, signal.SIGTERM)
                else:
                    process.terminate()
                process.wait(timeout=5)
            except (OSError, subprocess.TimeoutExpired):
                try:
                    if os.name == "posix":
                        os.killpg(process.pid, signal.SIGKILL)
                    else:
                        process.kill()
                    process.wait(timeout=5)
                except (OSError, subprocess.TimeoutExpired):
                    pass

    def monitor_participants(
        self,
        solvers: Sequence[SeatCredential],
        observer: SeatCredential,
        replay_verifier: SeatCredential,
    ) -> dict[str, object]:
        """Observe only Pack state until Participant quorum or the bounded deadline."""
        if self.gameplay_deadline_at is None:
            raise HarnessError("gameplay_window_missing")
        labels = {solver.member_id: solver.seat for solver in solvers}
        latest: dict[str, object] | None = None
        while time.monotonic() < self.gameplay_deadline_at:
            try:
                latest = asyncio.run(turn.snapshot(observer.path, observer.role))
            except (
                HanoiProtocolError,
                OSError,
                ProtocolError,
                TimeoutError,
                ValueError,
            ) as error:
                self._append_evidence(
                    "participant-monitor.jsonl",
                    {"kind": "observer_error", "code": _closed_exception_code(error)},
                )
                time.sleep(0.35)
                continue
            outcome = latest.get("outcome")
            if (
                isinstance(outcome, dict)
                and outcome.get("status") == "participant_accepted_completion"
            ):
                self._stop_participant_supervisors()
                terminal = asyncio.run(
                    turn.verify_terminal(replay_verifier.path, replay_verifier.role)
                )
                self._append_evidence("terminal-verification.jsonl", terminal)
                if terminal.get("status") != "verified":
                    raise HarnessError("terminal_replay_unverified")
                self._publish_canvas_window("complete")
                contributions = latest.get("contributions_by_member")
                contribution_by_seat = (
                    {
                        labels.get(member_id, "solver"): count
                        for member_id, count in contributions.items()
                        if isinstance(member_id, str) and isinstance(count, int)
                    }
                    if isinstance(contributions, dict)
                    else {}
                )
                return {
                    "status": "participant_accepted_completion",
                    "room_seq": latest.get("room_seq"),
                    "moves": outcome.get("moves"),
                    "model_contributions": contribution_by_seat,
                    "terminal": terminal,
                }
            if self.participant_processes and all(
                process.poll() is not None for process in self.participant_processes
            ):
                self._append_evidence(
                    "participant-monitor.jsonl",
                    {"kind": "participants_exited_unresolved"},
                )
                self._publish_canvas_window("time_limit")
                return {
                    "status": "unresolved",
                    "code": "participants_exited_unresolved",
                }
            time.sleep(0.35)
        self._stop_participant_supervisors()
        self._publish_canvas_window("time_limit")
        self._append_evidence("participant-monitor.jsonl", {"kind": "demo_time_limit"})
        return {
            "status": "unresolved",
            "code": "demo_time_limit",
            "last_room_seq": latest.get("room_seq")
            if isinstance(latest, dict)
            else None,
        }

    def stop(self) -> None:
        self._stop_participant_supervisors()
        if self.canvas_process is not None and self.canvas_process.poll() is None:
            self.canvas_process.terminate()
            try:
                self.canvas_process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.canvas_process.kill()
                self.canvas_process.wait(timeout=5)
        for reservation in (
            self.runtime_reservation,
            self.controller_reservation,
            self.canvas_reservation,
        ):
            if reservation is not None:
                reservation.close()
        if not self.started:
            return
        # Retain the run directory and evidence; only stop the processes this run started.
        try:
            self.command(["server", "stop", *self.common], allow_failure=True)
        finally:
            self.command(
                ["server", "controller-stop", *self.common], allow_failure=True
            )


def _arguments() -> argparse.Namespace:
    repo = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=repo)
    parser.add_argument(
        "--bundle",
        type=Path,
        default=repo
        / "packs/tower-of-hanoi/releases/0.1.0/worldstream-tower-of-hanoi-candidate.wspack",
    )
    parser.add_argument(
        "--worldstreamctl", type=Path, default=repo / "target/debug/worldstreamctl"
    )
    parser.add_argument("--codex", type=Path, default=Path("codex"))
    parser.add_argument(
        "--python", type=Path, default=repo / "sdk/python/.venv/bin/python"
    )
    parser.add_argument("--solver-seat", action="append", default=None)
    parser.add_argument("--observer-seat", default="observer")
    parser.add_argument("--solver-role", default="solver")
    parser.add_argument("--observer-role", default="observer")
    parser.add_argument("--runtime-port", type=int)
    parser.add_argument("--controller-port", type=int)
    parser.add_argument("--timeout-seconds", type=int, default=60)
    parser.add_argument("--codex-timeout-seconds", type=int, default=180)
    parser.add_argument("--disks", type=int)
    parser.add_argument("--move-limit", type=int, default=MAX_MOVE_LIMIT)
    parser.add_argument("--solver-effort", choices=("low", "medium"), default="medium")
    parser.add_argument("--canvas-demo", action="store_true")
    parser.add_argument("--community-demo", action="store_true")
    parser.add_argument("--canvas-port", type=int)
    parser.add_argument("--canvas-batch-ms", type=int, default=120)
    parser.add_argument("--canvas-demo-hold-seconds", type=int, default=0)
    parser.add_argument("--gameplay-seconds", type=int, default=300)
    parser.add_argument("--preflight-only", action="store_true")
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--build-timeout-seconds", type=int, default=900)
    arguments = parser.parse_args()
    if arguments.community_demo:
        arguments.canvas_demo = True
        if arguments.solver_seat is None:
            arguments.solver_seat = [
                "solver-a",
                "solver-b",
                "solver-c",
                "solver-d",
                "solver-e",
            ]
        arguments.gameplay_seconds = 300
    arguments.disks = (
        (4 if arguments.community_demo else 3)
        if arguments.disks is None
        else arguments.disks
    )
    if not 1 <= arguments.disks <= 10:
        parser.error("disks must be between 1 and 10")
    arguments.solver_seat = arguments.solver_seat or ["solver-a", "solver-b"]
    _validate_seats(arguments.solver_seat, arguments.observer_seat)
    if (
        not 1 <= arguments.timeout_seconds <= 300
        or not 1 <= arguments.codex_timeout_seconds <= 600
    ):
        parser.error("timeouts are outside the bounded range")
    if not 1 <= arguments.move_limit <= MAX_MOVE_LIMIT:
        parser.error("move-limit is outside the bounded range")
    if not 1 <= arguments.build_timeout_seconds <= 1_800:
        parser.error("build-timeout-seconds is outside the bounded range")
    if not 10 <= arguments.canvas_batch_ms <= 1_000:
        parser.error("canvas-batch-ms must be between 10 and 1000")
    if not 1 <= arguments.gameplay_seconds <= 3_600:
        parser.error("gameplay-seconds must be between 1 and 3600")
    if not 0 <= arguments.canvas_demo_hold_seconds <= 3_600:
        parser.error("canvas-demo-hold-seconds must be between 0 and 3600")
    for port in (
        arguments.runtime_port,
        arguments.controller_port,
        arguments.canvas_port,
    ):
        if port is not None and not 1 <= port <= 65535:
            parser.error("ports must be between 1 and 65535")
    return arguments


def _hold_canvas(arguments: argparse.Namespace) -> None:
    if not arguments.canvas_demo:
        return
    if arguments.canvas_demo_hold_seconds:
        deadline = time.monotonic() + arguments.canvas_demo_hold_seconds
        while time.monotonic() < deadline:
            time.sleep(min(1.0, deadline - time.monotonic()))
    else:
        while True:
            time.sleep(1)


def main() -> int:
    arguments = _arguments()
    run = LocalRun(arguments)
    try:
        run.start_server()
        if run.pack is None:
            raise HarnessError("pack_identity_missing")
        operation, room, solvers, runners, observer_bootstrap = run.create_room(
            run.pack
        )
        observer = run.issue_observer_capability(
            observer_bootstrap, run.pack, LIVE_OBSERVER_CAPABILITY
        )
        replay_verifier = run.issue_observer_capability(
            observer_bootstrap, run.pack, REPLAY_VERIFIER_CAPABILITY
        )
        run.remove_observer_bootstrap(observer_bootstrap)
        try:
            genesis_replay = asyncio.run(
                turn.verify_genesis_replay(replay_verifier.path, replay_verifier.role)
            )
        except (
            HanoiProtocolError,
            OSError,
            ProtocolError,
            TimeoutError,
            ValueError,
        ) as error:
            run._append_evidence(
                "genesis-replay-preflight.jsonl",
                {
                    "status": "error",
                    "exception_type": type(error).__name__,
                    "code": _closed_exception_code(error),
                },
            )
            raise
        run._append_evidence("genesis-replay-preflight.jsonl", genesis_replay)
        if genesis_replay.get("status") != "verified":
            raise HarnessError("genesis_replay_unverified")
        run.start_canvas(observer, solvers)
        if arguments.preflight_only:
            print(
                json.dumps(
                    {
                        "status": "preflight_verified",
                        "operation_id": operation,
                        "room_id": room,
                        "canvas_url": run.canvas_url,
                        "run_root": str(run.run_root),
                    },
                    sort_keys=True,
                    separators=(",", ":"),
                )
            )
            _hold_canvas(arguments)
            return 0
        # This clock intentionally starts only after Room creation, Genesis replay, and
        # the actionless canvas observer are ready. Pack build/proof remain setup work.
        run.start_gameplay_window()
        run.start_participant_supervisors(solvers, runners)
        completed = run.monitor_participants(solvers, observer, replay_verifier)
        run.command(["room", "inspect", room, *run.common])
        result = {
            "status": completed["status"],
            "code": completed.get("code"),
            "operation_id": operation,
            "room_id": room,
            "room_seq": completed.get("room_seq"),
            "moves": completed.get("moves"),
            "model_contributions": completed.get("model_contributions", {}),
            "observer_access": "participant_public_projection",
            "replay_equivalent": completed.get("terminal", {}).get("replay_equivalent")
            if isinstance(completed.get("terminal"), dict)
            else None,
            "setup_elapsed_ms": int(
                (run.gameplay_started_at - run.setup_started_at) * 1000
            )
            if run.gameplay_started_at is not None
            else None,
            "gameplay_elapsed_ms": int(
                (time.monotonic() - run.gameplay_started_at) * 1000
            )
            if run.gameplay_started_at is not None
            else None,
            "gameplay_limit_seconds": arguments.gameplay_seconds,
            "canvas_url": run.canvas_url,
            "run_root": str(run.run_root),
        }
        print(json.dumps(result, sort_keys=True, separators=(",", ":")), flush=True)
        _hold_canvas(arguments)
        return 0 if completed["status"] == "participant_accepted_completion" else 3
    except (
        CredentialError,
        HanoiProtocolError,
        HarnessError,
        OSError,
        ProtocolError,
        TimeoutError,
        ValueError,
    ) as error:
        code = str(error) if isinstance(error, HarnessError) else type(error).__name__
        print(
            json.dumps(
                {"status": "error", "code": code, "run_root": str(run.run_root)},
                sort_keys=True,
                separators=(",", ":"),
            ),
            flush=True,
        )
        return 3
    finally:
        run.stop()


if __name__ == "__main__":
    raise SystemExit(main())

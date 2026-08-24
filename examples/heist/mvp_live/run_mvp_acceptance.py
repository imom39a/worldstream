#!/usr/bin/env python3
"""Real-component MVP acceptance gate for one Studio-created Agent Heist Task.

The live mode owns disposable state, launches the release binaries as separate
processes, and crosses only their production HTTP, browser, WebSocket, and MCP
stdio boundaries.  The default mode validates a retained report so CI can run
the contract cheaply; ``--live`` is the release gate and fails closed when a
required built artifact or pinned browser adapter is unavailable.
"""

from __future__ import annotations

import argparse
import http.cookiejar
import json
import os
import pathlib
import re
import secrets
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Callable, Iterable, Mapping, Sequence
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[3]
PACK_ID = "worldstream.agent-heist"
PACK_VERSION = "0.2.0"
PACK_DIGEST = "blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820"
DRAFT_ID = "agent-heist-mvp"
PROFILE_ID = "external-heist-agent"
PROFILE_REVISION = "1"
HUMAN_PRINCIPAL = "01ARZ3NDEKTSV4RRFFQ69G5FD0"
AGENT_PRINCIPAL = "01ARZ3NDEKTSV4RRFFQ69G5FD1"
TIMER_PHASE = "01ARZ3NDEKTSV4RRFFQ69G5FH0"
TIMER_REMINDER = "01ARZ3NDEKTSV4RRFFQ69G5FH1"
TIMER_RESOLVE = "01ARZ3NDEKTSV4RRFFQ69G5FH2"
ACTION_IDS = {
    "human_inspect": "01ARZ3NDEKTSV4RRFFQ69G5FC6",
    "agent_inspect": "01ARZ3NDEKTSV4RRFFQ69G5FC7",
    "human_publish": "01ARZ3NDEKTSV4RRFFQ69G5FC8",
    "agent_publish": "01ARZ3NDEKTSV4RRFFQ69G5FC9",
    "plan": "01ARZ3NDEKTSV4RRFFQ69G5FCA",
    "endorse": "01ARZ3NDEKTSV4RRFFQ69G5FCB",
    "human_commit": "01ARZ3NDEKTSV4RRFFQ69G5FCC",
    "agent_commit": "01ARZ3NDEKTSV4RRFFQ69G5FCD",
    "human_ack": "01ARZ3NDEKTSV4RRFFQ69G5FCE",
    "agent_ack": "01ARZ3NDEKTSV4RRFFQ69G5FCF",
}
MAX_HTTP_BYTES = 8 * 1024 * 1024
MAX_MCP_LINE_BYTES = 1024 * 1024
MCP_TOOLS = frozenset(
    {
        "worldstream.list_assigned_tasks",
        "worldstream.observe",
        "worldstream.acknowledge",
        "worldstream.list_current_action_offers",
        "worldstream.submit_action",
        "worldstream.next_activation",
        "worldstream.complete_activation",
    }
)
ULID = re.compile(r"^[0-7][0-9A-HJKMNP-TV-Z]{25}$")
DIGEST = re.compile(r"^blake3:[0-9a-f]{64}$")
PRIVATE_TOKEN = re.compile(rb"(?:wsb1|wst1|wsh1|wss1|wsl1):[0-9a-f]{64}")
PRIVATE_FIELD = re.compile(
    rb'"(?:bearer|token|secret(?:_reference)?|activation_context|private_memory)"\s*:',
    re.IGNORECASE,
)


class AcceptanceFailure(RuntimeError):
    """A stable, credential-free acceptance failure."""


def _require(condition: bool, code: str) -> None:
    if not condition:
        raise AcceptanceFailure(code)


def _record(value: Any, code: str) -> dict[str, Any]:
    _require(isinstance(value, dict), code)
    return value


def validate_completed_report(value: Any) -> None:
    """Validate the bounded evidence report as the exact IMO-78 completion gate."""

    report = _record(value, "report_invalid")
    _require(
        report.get("schema") == "worldstream/agent-heist-mvp-acceptance/v1",
        "schema_invalid",
    )
    _require(report.get("status") == "completed", "story_not_completed")
    components = _record(report.get("components"), "component_evidence_missing")
    expected_components = {
        "daemon": "real_process_http_websocket",
        "supervisor": "real_process_http",
        "assignment_mcp": "real_process_stdio",
        "studio": "production_http_contract",
        "participant_console": "production_origin_http_contract",
    }
    _require(components == expected_components, "component_boundary_invalid")

    task = _record(report.get("task"), "task_evidence_missing")
    rooms = task.get("room_ids")
    _require(
        isinstance(rooms, list)
        and len(rooms) == 1
        and isinstance(rooms[0], str)
        and ULID.fullmatch(rooms[0]) is not None,
        "exactly_one_room",
    )
    _require(
        task.get("authoritative_room_count_before") == 0
        and task.get("authoritative_room_count_after") == 1,
        "authoritative_room_delta_invalid",
    )
    seats = task.get("seats")
    exact_seats = (
        isinstance(seats, list)
        and len(seats) == 2
        and sum(
            seat.get("principal_kind") == "human"
            for seat in seats
            if isinstance(seat, dict)
        )
        == 1
        and sum(
            seat.get("principal_kind") == "agent"
            and seat.get("agent_assignment") == "external"
            for seat in seats
            if isinstance(seat, dict)
        )
        == 1
    )
    _require(exact_seats, "exact_seat_shape")
    _require(
        task.get("reviewed") is True
        and task.get("lobby_observed") is True
        and task.get("readiness_observed") is True
        and task.get("explicit_launch") is True,
        "review_readiness_launch_evidence_missing",
    )

    human = _record(report.get("human"), "human_evidence_missing")
    _require(
        human.get("handoff_redeemed") is True
        and human.get("session_usable") is True
        and isinstance(human.get("actions_committed"), int)
        and human["actions_committed"] > 0,
        "human_story_incomplete",
    )
    agent = _record(report.get("agent"), "agent_evidence_missing")
    _require(
        agent.get("assignment_count") == 1
        and agent.get("task_count") == 1
        and agent.get("observation_acknowledged") is True
        and agent.get("listed_offers_only") is True
        and agent.get("activation_leased") is True
        and agent.get("helper_restarted") is True
        and agent.get("stable_launch_reference_reused") is True
        and agent.get("duplicate_actions") == 0
        and agent.get("duplicate_completions") == 0,
        "external_agent_story_incomplete",
    )

    outcome = _record(report.get("outcome"), "outcome_evidence_missing")
    replay = _record(report.get("replay"), "replay_evidence_missing")
    _require(
        outcome.get("phase") == "complete"
        and outcome.get("meaningful") is True
        and isinstance(outcome.get("committed_room_seq"), int)
        and outcome["committed_room_seq"] > 0,
        "meaningful_outcome_missing",
    )
    for source in (outcome, replay):
        _require(
            isinstance(source.get("authoritative_state_hash"), str)
            and DIGEST.fullmatch(source["authoritative_state_hash"]) is not None,
            "invalid_state_hash",
        )
        _require(
            isinstance(source.get("activity_state_hash"), str)
            and DIGEST.fullmatch(source["activity_state_hash"]) is not None,
            "invalid_activity_hash",
        )
        _require(
            isinstance(source.get("outcome_digest"), str)
            and DIGEST.fullmatch(source["outcome_digest"]) is not None,
            "invalid_outcome_digest",
        )
        _require(
            isinstance(source.get("transition_hash"), str)
            and DIGEST.fullmatch(source["transition_hash"]) is not None,
            "invalid_transition_hash",
        )
    _require(replay.get("verified") is True, "replay_not_verified")
    _require(
        replay.get("at_room_seq") == outcome.get("committed_room_seq"),
        "replay_sequence_mismatch",
    )
    _require(
        replay.get("authoritative_state_hash")
        == outcome.get("authoritative_state_hash"),
        "replay_hash_mismatch",
    )
    _require(
        replay.get("activity_state_hash") == outcome.get("activity_state_hash"),
        "replay_activity_mismatch",
    )
    _require(
        replay.get("outcome_digest") == outcome.get("outcome_digest"),
        "replay_outcome_mismatch",
    )
    _require(
        replay.get("transition_hash") == outcome.get("transition_hash"),
        "replay_transition_mismatch",
    )
    _require(
        replay.get("transition_ids_unique") is True, "duplicate_transition_detected"
    )

    privacy = _record(report.get("privacy"), "privacy_evidence_missing")
    checked = privacy.get("checked_surfaces")
    _require(
        isinstance(checked, list)
        and set(checked)
        == {
            "studio_http",
            "participant_console_http",
            "assignment_mcp_stdio",
            "process_logs",
        }
        and privacy.get("credential_disclosure") is False
        and privacy.get("private_activation_disclosure") is False,
        "privacy_evidence_invalid",
    )


def assert_public_evidence_safe(
    surfaces: Iterable[tuple[str, bytes]], private_values: Iterable[bytes]
) -> None:
    """Reject credentials/private Activation material in any retained surface."""

    secrets_to_find = [value for value in private_values if value]
    for label, body in surfaces:
        _require(len(body) <= MAX_HTTP_BYTES, "evidence_surface_oversized")
        private_field = PRIVATE_FIELD.search(body)
        if PRIVATE_TOKEN.search(body):
            raise AcceptanceFailure(f"credential_or_private_data:{label}:token")
        if private_field and not (
            label == "assignment_mcp_stdio"
            and b"activation_context" in private_field.group(0).lower()
        ):
            field = private_field.group(0).split(b'"', 2)[1].decode("ascii").lower()
            raise AcceptanceFailure(
                f"credential_or_private_data:{label}:field:{field}"
            )
        if label != "assignment_mcp_stdio" and any(
            value in body for value in secrets_to_find
        ):
            raise AcceptanceFailure(f"credential_or_private_data:{label}:private_value")


def retained_handoff_evidence(response: Mapping[str, Any]) -> dict[str, Any]:
    """Projects a handoff response without retaining its fragment secret."""

    retained = dict(response)
    url = retained.get("console_url")
    _require(isinstance(url, str) and "#handoff=" in url, "handoff_url_invalid")
    retained["console_url"] = url.split("#", 1)[0] + "#handoff=[REDACTED]"
    return retained


def mcp_request(identifier: int, tool: str, arguments: Mapping[str, Any]) -> str:
    """Create one bounded standard MCP tool request with a closed tool name."""

    _require(tool in MCP_TOOLS, "invalid_mcp_tool")
    value = {
        "jsonrpc": "2.0",
        "id": identifier,
        "method": "tools/call",
        "params": {"name": tool, "arguments": dict(arguments)},
    }
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"))
    _require(len(encoded.encode()) <= MAX_MCP_LINE_BYTES, "mcp_request_oversized")
    _require(
        PRIVATE_FIELD.search(encoded.encode()) is None, "mcp_request_private_field"
    )
    return encoded


class JsonHttp:
    """Bounded JSON client retaining only caller-selected public evidence."""

    def __init__(self, base_url: str) -> None:
        self.base_url = base_url.rstrip("/")
        self.cookies = http.cookiejar.CookieJar()
        self.opener = urllib.request.build_opener(
            urllib.request.HTTPCookieProcessor(self.cookies)
        )

    def request(
        self,
        method: str,
        path: str,
        body: Any = None,
        *,
        headers: Mapping[str, str] | None = None,
        expected: Sequence[int] = (200,),
    ) -> tuple[int, Any, Mapping[str, str]]:
        encoded = (
            None if body is None else json.dumps(body, separators=(",", ":")).encode()
        )
        request_headers = {"accept": "application/json"}
        if encoded is not None:
            request_headers["content-type"] = "application/json"
        if headers:
            request_headers.update(headers)
        request = urllib.request.Request(
            self.base_url + path, data=encoded, headers=request_headers, method=method
        )
        try:
            response = self.opener.open(request, timeout=10)
        except urllib.error.HTTPError as error:
            response = error
        payload = response.read(MAX_HTTP_BYTES + 1)
        _require(len(payload) <= MAX_HTTP_BYTES, "http_response_oversized")
        value = None if not payload else json.loads(payload)
        detail = ""
        if isinstance(value, dict) and isinstance(value.get("error"), dict):
            error = value["error"]
            code = error.get("code")
            paths = error.get("field_errors")
            safe_paths = (
                [
                    f"{item.get('path')}={item.get('code')}"
                    for item in paths
                    if isinstance(item, dict)
                    and isinstance(item.get("path"), str)
                    and isinstance(item.get("code"), str)
                ]
                if isinstance(paths, list)
                else []
            )
            if isinstance(code, str):
                detail = f":{code}:{','.join(safe_paths[:8])}"
        _require(
            response.status in expected,
            f"http_status_unexpected:{method}:{path}:{response.status}{detail}",
        )
        return response.status, value, dict(response.headers.items())


class OwnedProcess:
    """Disposable child whose argv/environment never contains bearer material."""

    def __init__(
        self,
        argv: Sequence[str],
        log: pathlib.Path,
        env: Mapping[str, str] | None = None,
    ) -> None:
        if any(PRIVATE_TOKEN.search(argument.encode()) for argument in argv):
            raise AcceptanceFailure("credential_in_process_argv")
        clean_env = dict(os.environ)
        if env:
            clean_env.update(env)
        if any(PRIVATE_TOKEN.search(value.encode()) for value in clean_env.values()):
            raise AcceptanceFailure("credential_in_process_environment")
        self._output = log.open("xb")
        self.process = subprocess.Popen(
            list(argv),
            stdin=subprocess.DEVNULL,
            stdout=self._output,
            stderr=subprocess.STDOUT,
            env=clean_env,
            start_new_session=True,
        )
        self._process_group = self.process.pid

    def close(self) -> None:
        if self.process.poll() is None:
            try:
                os.killpg(self._process_group, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(self._process_group, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                self.process.wait(timeout=5)
        else:
            try:
                os.killpg(self._process_group, signal.SIGTERM)
            except ProcessLookupError:
                pass
        self._output.close()


class McpHelper:
    """One real assignment helper process speaking line-delimited MCP stdio."""

    def __init__(
        self, executable: pathlib.Path, state_dir: pathlib.Path, launch_reference: str
    ) -> None:
        _require(
            re.fullmatch(r"[0-9a-f]{64}", launch_reference) is not None,
            "launch_reference_invalid",
        )
        self.process = subprocess.Popen(
            [
                str(executable),
                "--state-dir",
                str(state_dir),
                "--launch-reference",
                launch_reference,
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
            start_new_session=True,
        )
        self._process_group = self.process.pid
        self.identifier = 0
        self.transcript: list[bytes] = []
        self._raw(
            {
                "jsonrpc": "2.0",
                "id": self._id(),
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "worldstream-mvp-gate", "version": "1"},
                },
            }
        )
        self._write({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def _id(self) -> int:
        self.identifier += 1
        return self.identifier

    def _write(self, value: Mapping[str, Any]) -> None:
        assert self.process.stdin is not None
        line = json.dumps(value, sort_keys=True, separators=(",", ":"))
        _require(len(line.encode()) <= MAX_MCP_LINE_BYTES, "mcp_request_oversized")
        self.process.stdin.write(line + "\n")
        self.process.stdin.flush()

    def _raw(self, value: Mapping[str, Any]) -> dict[str, Any]:
        self._write(value)
        assert self.process.stdout is not None
        line = self.process.stdout.readline(MAX_MCP_LINE_BYTES + 1)
        _require(
            bool(line) and len(line.encode()) <= MAX_MCP_LINE_BYTES,
            "mcp_response_invalid",
        )
        self.transcript.append(line.encode())
        response = _record(json.loads(line), "mcp_response_invalid")
        _require("error" not in response, "mcp_tool_failed")
        return response

    def call(self, tool: str, arguments: Mapping[str, Any]) -> Any:
        identifier = self._id()
        response = self._raw(json.loads(mcp_request(identifier, tool, arguments)))
        result = _record(response.get("result"), "mcp_result_invalid")
        return result.get("structuredContent")

    def close(self) -> None:
        if self.process.stdin is not None:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(self._process_group, signal.SIGTERM)
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(self._process_group, signal.SIGKILL)
                self.process.wait(timeout=5)
        try:
            os.killpg(self._process_group, signal.SIGTERM)
        except ProcessLookupError:
            pass

    def crash(self) -> None:
        """Stops the exact helper process group without a graceful handoff."""
        try:
            os.killpg(self._process_group, signal.SIGKILL)
        except ProcessLookupError:
            pass
        self.process.wait(timeout=5)


def _private_write(path: pathlib.Path, value: bytes) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as output:
        output.write(value)
        output.flush()
        os.fsync(output.fileno())
    os.chmod(path, 0o600)


def _free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def _wait_http(url: str, process: OwnedProcess, timeout: float = 120.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.process.poll() is not None:
            raise AcceptanceFailure("component_exited_before_ready")
        try:
            with urllib.request.urlopen(url, timeout=0.5) as response:
                if response.status == 200:
                    return
        except (OSError, urllib.error.URLError):
            time.sleep(0.1)
    raise AcceptanceFailure("component_ready_timeout")


def _inventory_rooms(value: Any) -> list[dict[str, Any]]:
    record = _record(value, "room_inventory_invalid")
    rooms = record.get("rooms")
    _require(
        isinstance(rooms, list) and all(isinstance(room, dict) for room in rooms),
        "room_inventory_invalid",
    )
    return rooms


def _browser_python(environment: Mapping[str, str]) -> pathlib.Path:
    configured = environment.get("WORLDSTREAM_PYTHON")
    if configured:
        candidate = pathlib.Path(configured)
    else:
        candidate = ROOT / "sdk/python/.venv/bin/python"
    _require(candidate.is_file() and os.access(candidate, os.X_OK), "browser_python_missing")
    return candidate


def _browser_failure(stderr: bytes) -> AcceptanceFailure:
    matched = re.fullmatch(rb"blocked:([a-z0-9_]{1,80})\s*", stderr)
    if matched is None:
        return AcceptanceFailure("production_console_browser_failed")
    return AcceptanceFailure(
        "production_console_" + matched.group(1).decode("ascii")
    )


def reconcile_task_setup(
    initial: Mapping[str, Any],
    fetch: Callable[[], Mapping[str, Any]],
    retry: Callable[[], Mapping[str, Any]],
    *,
    maximum_attempts: int = 100,
    pause: Callable[[float], None] = time.sleep,
) -> Mapping[str, Any]:
    """Boundedly reconcile the documented Task Setup lifecycle."""

    status = initial
    for _ in range(maximum_attempts):
        state = status.get("state")
        if state == "ready":
            return status
        if state in {"waiting", "provisioning"}:
            pause(0.2)
            status = fetch()
            continue
        attention = status.get("attention")
        if (
            state == "needs_attention"
            and isinstance(attention, dict)
            and attention.get("code") == "daemon_result_ambiguous"
            and attention.get("retryable") is True
        ):
            pause(0.2)
            status = retry()
            continue
        raise AcceptanceFailure("task_setup_terminal_failure")
    state = status.get("state")
    safe_state = state if isinstance(state, str) and re.fullmatch(r"[a-z_]+", state) else "unknown"
    raise AcceptanceFailure(f"task_setup_reconciliation_timeout:{safe_state}")


class LiveGate:
    """Coordinates the exact live story without owning product internals."""

    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.root = pathlib.Path(tempfile.mkdtemp(prefix="worldstream-mvp-"))
        os.chmod(self.root, 0o700)
        self.processes: list[OwnedProcess] = []
        self.public_surfaces: list[tuple[str, bytes]] = []
        self.private_values: list[bytes] = []
        self.human_http: JsonHttp | None = None
        self.console_origin = ""

    def close(self) -> None:
        for process in reversed(self.processes):
            process.close()
        if self.args.keep_artifacts:
            print(f"evidence_root={self.root}", file=sys.stderr)
        else:
            import shutil

            shutil.rmtree(self.root)

    def run(self) -> dict[str, Any]:
        daemon = pathlib.Path(self.args.worldstreamd).resolve()
        supervisor = pathlib.Path(self.args.supervisor).resolve()
        helper_path = pathlib.Path(self.args.assignment_mcp).resolve()
        console_dist = pathlib.Path(self.args.console_dist).resolve()
        for executable in (daemon, supervisor, helper_path):
            _require(
                executable.is_file() and os.access(executable, os.X_OK),
                "built_binary_missing",
            )
        _require(
            (console_dist / "index.html").is_file(), "production_console_build_missing"
        )

        daemon_port = _free_port()
        # The production Console intentionally pins this local Supervisor origin.
        supervisor_port = 9420
        console_port = _free_port()
        studio_origin = "http://127.0.0.1:5173"
        console_origin = f"http://127.0.0.1:{console_port}"
        self.console_origin = console_origin
        authority = secrets.token_bytes(32)
        bearer = b"wsb1:" + authority.hex().encode()
        self.private_values.extend((authority, bearer))
        daemon_secret = self.root / "authority.secret"
        _private_write(daemon_secret, authority)
        data_dir = self.root / "daemon-data"
        data_dir.mkdir(mode=0o700)
        config = self.root / "worldstream.toml"
        config.write_text(
            "config_version = 1\n\n"
            "[server]\n"
            f'bind = "127.0.0.1:{daemon_port}"\n\n'
            + "[storage]\n"
            + 'profile = "sqlite-bundled"\n'
            + f'data_dir = "{data_dir}"\n'
            + 'deployment_lineage = "imo-78/mvp-live"\n'
            + "storage_epoch = 1\n\n"
            + "[authority.bootstrap]\n"
            + f'secret_file = "{daemon_secret}"\n',
            encoding="utf-8",
        )
        os.chmod(config, 0o600)
        daemon_process = OwnedProcess(
            [str(daemon), "--config", str(config)], self.root / "worldstreamd.log"
        )
        self.processes.append(daemon_process)
        daemon_url = f"http://127.0.0.1:{daemon_port}"
        _wait_http(daemon_url + "/readyz", daemon_process)

        # Runtime's canonical backup contract requires sibling data/Studio roots.
        supervisor_state = self.root / "studio"
        (self.root / "runner-templates").mkdir(mode=0o700)
        vault = supervisor_state / "secrets"
        reference = secrets.token_hex(32)
        _private_write(vault / (f"host-{reference}.secret"), authority)
        supervisor_process = OwnedProcess(
            [
                str(supervisor),
                "--bind",
                f"127.0.0.1:{supervisor_port}",
                "--daemon",
                f"127.0.0.1:{daemon_port}",
                "--daemon-executable",
                str(daemon),
                "--daemon-config",
                str(config),
                "--state-dir",
                str(supervisor_state),
                "--assignment-mcp-executable",
                str(helper_path),
                "--host-authority-reference",
                reference,
                "--runner-templates-dir",
                str(self.root / "runner-templates"),
                "--studio-origin",
                studio_origin,
                "--participant-console-origin",
                console_origin,
            ],
            self.root / "supervisor.log",
        )
        self.processes.append(supervisor_process)
        _wait_http(
            f"http://127.0.0.1:{supervisor_port}/api/v1/daemon/lifecycle",
            supervisor_process,
        )
        supervisor_http = JsonHttp(f"http://127.0.0.1:{supervisor_port}")
        daemon_http = JsonHttp(daemon_url)
        host_headers = {"authorization": "Bearer " + bearer.decode()}
        _, before_body, _ = daemon_http.request(
            "GET", "/v1/operator/rooms?limit=64", headers=host_headers
        )
        before_rooms = _inventory_rooms(before_body)
        _require(len(before_rooms) == 0, "preexisting_room_detected")

        self._publish_profile_and_reviewed_draft(supervisor_http)
        creation = self._post(
            supervisor_http, f"/api/v1/room-creations/{DRAFT_ID}/start"
        )
        _require(creation.get("state") == "succeeded", "room_creation_incomplete")
        room_id = creation.get("room_id")
        _require(
            isinstance(room_id, str) and ULID.fullmatch(room_id) is not None,
            "room_creation_invalid",
        )
        setup = reconcile_task_setup(
            self._post(supervisor_http, f"/api/v1/task-setups/{DRAFT_ID}/start"),
            lambda: self._get(supervisor_http, f"/api/v1/task-setups/{DRAFT_ID}"),
            lambda: self._post(
                supervisor_http, f"/api/v1/task-setups/{DRAFT_ID}/retry"
            ),
        )
        _require(setup.get("state") == "ready", "task_setup_incomplete")

        console_process = OwnedProcess(
            [
                sys.executable,
                "-m",
                "http.server",
                str(console_port),
                "--bind",
                "127.0.0.1",
                "--directory",
                str(console_dist),
            ],
            self.root / "console.log",
        )
        self.processes.append(console_process)
        _wait_http(console_origin + "/", console_process)
        handoff_url = self._issue_handoff(supervisor_http, studio_origin)
        self._exercise_real_console_page(handoff_url)
        self._open_automation_console_session(supervisor_http, studio_origin)

        assignments = self._get(supervisor_http, "/api/v1/agent-profile-assignments")
        assignment_rows = assignments.get("assignments")
        _require(
            isinstance(assignment_rows, list) and len(assignment_rows) == 1,
            "assignment_count_invalid",
        )
        assignment_id = assignment_rows[0].get("assignment_id")
        _require(
            isinstance(assignment_id, str)
            and ULID.fullmatch(assignment_id) is not None,
            "assignment_invalid",
        )
        launch = self._issue_assignment_launch(supervisor_http, assignment_id)
        launch_reference = launch.get("launch_reference")
        _require(isinstance(launch_reference, str), "launch_reference_missing")
        self.private_values.append(launch_reference.encode())

        helper = McpHelper(helper_path, supervisor_state, launch_reference)
        helper_crashed = False
        try:
            tasks = _record(
                helper.call("worldstream.list_assigned_tasks", {}),
                "assigned_tasks_invalid",
            )
            task_rows = tasks.get("tasks")
            _require(
                isinstance(task_rows, list) and len(task_rows) == 1,
                "assigned_task_count_invalid",
            )
            self._wait_ready_and_launch(supervisor_http, helper)
            story = self._drive_heist(daemon_http, host_headers, helper, room_id)
            helper_transcript = list(helper.transcript)
            # Exercise a real abrupt process restart while the lease is live;
            # the replacement gets only the same opaque launch reference and
            # owner-only durable state.
            helper.crash()
            helper_crashed = True
        finally:
            if not helper_crashed:
                helper.close()

        # Restart the real helper with the same opaque reference and durable state.
        restarted = McpHelper(helper_path, supervisor_state, launch_reference)
        try:
            resumed = restarted.call("worldstream.next_activation", {})
            restart_story = self._complete_resumed_activation(
                restarted, story["leased_activation"], resumed
            )
            self._finish_heist(daemon_http, host_headers, restarted, room_id, story)
            helper_transcript.extend(restarted.transcript)
        finally:
            restarted.close()

        _, after_body, _ = daemon_http.request(
            "GET", "/v1/operator/rooms?limit=64", headers=host_headers
        )
        after_rooms = _inventory_rooms(after_body)
        _require(
            len(after_rooms) == 1 and after_rooms[0].get("room_id") == room_id,
            "exactly_one_room",
        )
        outcome, replay = self._outcome_and_replay(
            daemon_http, host_headers, room_id, story
        )

        for label in ("worldstreamd.log", "supervisor.log", "console.log"):
            path = self.root / label
            self.public_surfaces.append(
                ("process_logs", path.read_bytes()[:MAX_HTTP_BYTES])
            )
        self.public_surfaces.extend(
            ("assignment_mcp_stdio", line) for line in helper_transcript
        )
        assert_public_evidence_safe(self.public_surfaces, self.private_values)
        report = {
            "schema": "worldstream/agent-heist-mvp-acceptance/v1",
            "status": "completed",
            "components": {
                "daemon": "real_process_http_websocket",
                "supervisor": "real_process_http",
                "assignment_mcp": "real_process_stdio",
                "studio": "production_http_contract",
                "participant_console": "production_origin_http_contract",
            },
            "task": {
                "draft_id": DRAFT_ID,
                "reviewed": True,
                "room_ids": [room_id],
                "seats": [
                    {"seat_id": "navigator-1", "principal_kind": "human"},
                    {
                        "seat_id": "insider-1",
                        "principal_kind": "agent",
                        "agent_assignment": "external",
                    },
                ],
                "lobby_observed": True,
                "readiness_observed": True,
                "explicit_launch": True,
                "authoritative_room_count_before": len(before_rooms),
                "authoritative_room_count_after": len(after_rooms),
            },
            "human": {
                "handoff_redeemed": True,
                "session_usable": True,
                "actions_committed": story["human_actions"],
            },
            "agent": {
                "assignment_count": 1,
                "task_count": 1,
                "observation_acknowledged": story["observation_acknowledged"],
                "listed_offers_only": True,
                "actions_committed": story["agent_actions"],
                "activation_leased": restart_story["activation_leased"],
                "helper_restarted": True,
                "stable_launch_reference_reused": True,
                "duplicate_actions": story["duplicate_actions"],
                "duplicate_completions": restart_story["duplicate_completions"],
            },
            "outcome": outcome,
            "replay": replay,
            "privacy": {
                "checked_surfaces": [
                    "studio_http",
                    "participant_console_http",
                    "assignment_mcp_stdio",
                    "process_logs",
                ],
                "credential_disclosure": False,
                "private_activation_disclosure": False,
            },
        }
        validate_completed_report(report)
        return report

    def _get(self, client: JsonHttp, path: str) -> dict[str, Any]:
        _, value, _ = client.request("GET", path)
        encoded = json.dumps(value, sort_keys=True).encode()
        self.public_surfaces.append(("studio_http", encoded))
        return _record(value, "studio_response_invalid")

    def _post(
        self,
        client: JsonHttp,
        path: str,
        body: Any = None,
        headers: Mapping[str, str] | None = None,
    ) -> dict[str, Any]:
        _, value, _ = client.request(
            "POST", path, body, headers=headers, expected=(200, 201)
        )
        encoded = json.dumps(value, sort_keys=True).encode()
        self.public_surfaces.append(("studio_http", encoded))
        return _record(value, "studio_response_invalid")

    @staticmethod
    def _issue_assignment_launch(
        client: JsonHttp, assignment_id: str
    ) -> dict[str, Any]:
        """Issue private launcher control without retaining its opaque reference as browser evidence."""
        _, value, _ = client.request(
            "POST",
            f"/v1/agent-profile-assignments/{assignment_id}/mcp-launches",
            expected=(200, 201),
        )
        return _record(value, "assignment_launch_response_invalid")

    def _publish_profile_and_reviewed_draft(self, client: JsonHttp) -> None:
        profile = {
            "schema": "worldstream/studio-agent-profile/v1",
            "profile_id": PROFILE_ID,
            "revision": PROFILE_REVISION,
            "display_name": "External Agent Heist participant",
            "non_secret_configuration": {"transport": "assignment_mcp"},
            "secret_settings": [],
            "host_contract": {"kind": "generic_mcp"},
        }
        self._post(client, "/api/v1/agent-profiles", profile)
        draft = {
            "schema": "worldstream/studio-room-draft/v1",
            "draft_id": DRAFT_ID,
            "pack": {"id": PACK_ID, "version": PACK_VERSION, "digest": PACK_DIGEST},
            "configuration": {
                "pack_id": PACK_ID,
                "pack_schema": 1,
                "roles": ["navigator", "insider", "broker"],
                "briefing_duration_seconds": 30,
                "negotiation_duration_seconds": 90,
                "commitment_duration_seconds": 30,
                "commitment_reminder_seconds_before_deadline": 10,
                "result_duration_seconds": 20,
                "maximum_plans": 12,
                "maximum_open_offers_per_role": 4,
            },
            "seats": [
                {
                    "seat_id": "navigator-1",
                    "role": "navigator",
                    "required": True,
                    "display_name": "Human Navigator",
                    "principal_id": HUMAN_PRINCIPAL,
                    "principal_kind": "human",
                },
                {
                    "seat_id": "insider-1",
                    "role": "insider",
                    "required": True,
                    "display_name": "External Agent Insider",
                    "principal_id": AGENT_PRINCIPAL,
                    "principal_kind": "agent",
                    "agent_assignment": "external",
                    "agent_profile": {
                        "profile_id": PROFILE_ID,
                        "revision": PROFILE_REVISION,
                    },
                },
            ],
            "readiness": [
                {"seat_id": "navigator-1", "role": "navigator", "required": True},
                {"seat_id": "insider-1", "role": "insider", "required": True},
            ],
            "last_valid_step": "review",
        }
        _, response, _ = client.request("PUT", f"/api/v1/room-drafts/{DRAFT_ID}", draft)
        self.public_surfaces.append(
            ("studio_http", json.dumps(response, sort_keys=True).encode())
        )
        _require(
            _record(response, "draft_response_invalid")
            .get("draft", {})
            .get("last_valid_step")
            == "review",
            "draft_not_reviewed",
        )

    def _issue_handoff(self, client: JsonHttp, studio_origin: str) -> str:
        _, response, _ = client.request(
            "POST",
            "/api/v1/participant-console/handoffs",
            {"draft_id": DRAFT_ID, "seat_id": "navigator-1"},
            headers={"origin": studio_origin},
            expected=(200, 201),
        )
        response = _record(response, "studio_response_invalid")
        self.public_surfaces.append(
            (
                "studio_http",
                json.dumps(retained_handoff_evidence(response), sort_keys=True).encode(),
            )
        )
        url = response.get("console_url")
        _require(
            isinstance(url, str) and "#handoff=wsh1:" in url, "handoff_url_invalid"
        )
        self.private_values.append(url.split("#handoff=", 1)[1].encode())
        return url

    def _exercise_real_console_page(self, handoff_url: str) -> None:
        """Open the served production Console and wait for its real live view."""

        adapter = ROOT / "scripts/cdp-browser.py"
        _require(adapter.is_file(), "pinned_browser_adapter_missing")
        environment = dict(os.environ)
        browser_state = self.root / "browser-state"
        browser_state.mkdir(mode=0o700)
        environment["WORLDSTREAM_CDP_STATE_DIR"] = str(browser_state)
        adapter_python = _browser_python(environment)

        def browser(arguments: Sequence[str], timeout: int = 45) -> bytes:
            completed = subprocess.run(
                [str(adapter_python), str(adapter), *arguments],
                capture_output=True,
                env=environment,
                timeout=timeout,
                check=False,
            )
            if completed.returncode != 0:
                raise _browser_failure(completed.stderr)
            _require(
                len(completed.stdout) <= MAX_HTTP_BYTES, "browser_evidence_oversized"
            )
            return completed.stdout.strip()

        opened = _record(
            json.loads(browser(["--json", "browser", "open", "about:blank"])),
            "browser_evidence_invalid",
        )
        surface = opened.get("surface_ref")
        _require(
            isinstance(surface, str) and surface.startswith("surface:"),
            "browser_surface_invalid",
        )
        try:
            browser(
                [
                    "browser",
                    surface,
                    "addinitscript",
                    "--script",
                    (
                        "(()=>{const original=window.fetch.bind(window);window.fetch=async(input,init)=>{"
                        "const url=new URL(typeof input==='string'?input:input.url,location.href);"
                        "const detail={kind:'fetch',path:url.pathname,method:String(init?.method||'GET')};"
                        "try{const response=await original(input,init);"
                        "window.__WORLDSTREAM_BROWSER_DIAGNOSTICS__.push({...detail,status:response.status,type:response.type});"
                        "return response;}catch(error){window.__WORLDSTREAM_BROWSER_DIAGNOSTICS__.push({...detail,error:String(error)});throw error;}};})();"
                    ),
                ]
            )
            browser(["browser", surface, "navigate", handoff_url])
            try:
                browser(
                    [
                        "browser",
                        surface,
                        "wait",
                        "--text",
                        "Participant session",
                        "--timeout-ms",
                        "30000",
                    ]
                )
            except AcceptanceFailure:
                body = browser(["browser", surface, "get", "text", "body"])
                diagnostics = browser(["browser", surface, "console", "list"])
                network_probe = browser(
                    [
                        "browser",
                        surface,
                        "eval",
                        "--script",
                        (
                            "(async()=>{for(const mode of ['cors','no-cors']){try{"
                            "const response=await fetch('http://127.0.0.1:9420/api/v1/participant-console/session',"
                            "{method:'GET',credentials:'include',cache:'no-store',mode});"
                            "window.__WORLDSTREAM_BROWSER_DIAGNOSTICS__.push({kind:'network-probe',mode,type:response.type,status:response.status});"
                            "}catch(error){window.__WORLDSTREAM_BROWSER_DIAGNOSTICS__.push({kind:'network-probe',mode,error:String(error)});}}"
                            "return JSON.stringify(window.__WORLDSTREAM_BROWSER_DIAGNOSTICS__);})()"
                        ),
                    ]
                )
                if self.args.keep_artifacts:
                    (self.root / "participant-console-browser.txt").write_bytes(
                        body[:MAX_HTTP_BYTES]
                        + b"\n"
                        + diagnostics[:MAX_HTTP_BYTES]
                        + b"\n"
                        + network_probe[:MAX_HTTP_BYTES]
                    )
                raise
            try:
                browser(
                    [
                        "browser",
                        surface,
                        "wait",
                        "--text",
                        "Authorized Room projection",
                        "--timeout-ms",
                        "30000",
                    ]
                )
            except AcceptanceFailure:
                body = browser(["browser", surface, "get", "text", "body"])
                diagnostics = browser(["browser", surface, "console", "list"])
                network_probe = browser(
                    [
                        "browser",
                        surface,
                        "eval",
                        "--script",
                        (
                            "(async()=>{for(const mode of ['cors','no-cors']){try{"
                            "const response=await fetch('http://127.0.0.1:9420/api/v1/participant-console/session',"
                            "{method:'GET',credentials:'include',cache:'no-store',mode});"
                            "window.__WORLDSTREAM_BROWSER_DIAGNOSTICS__.push({kind:'network-probe',mode,type:response.type,status:response.status});"
                            "}catch(error){window.__WORLDSTREAM_BROWSER_DIAGNOSTICS__.push({kind:'network-probe',mode,error:String(error)});}}"
                            "return JSON.stringify(window.__WORLDSTREAM_BROWSER_DIAGNOSTICS__);})()"
                        ),
                    ]
                )
                if self.args.keep_artifacts:
                    (self.root / "participant-console-browser.txt").write_bytes(
                        body[:MAX_HTTP_BYTES]
                        + b"\n"
                        + diagnostics[:MAX_HTTP_BYTES]
                        + b"\n"
                        + network_probe[:MAX_HTTP_BYTES]
                    )
                raise
            current_url = browser(["browser", surface, "get", "url"]).decode()
            _require("#handoff=" not in current_url, "handoff_fragment_not_scrubbed")
            body = browser(["browser", surface, "get", "text", "body"])
            safe = json.dumps(
                {
                    "handoff_fragment_scrubbed": True,
                    "session_live": b"Authorized Room projection" in body,
                },
                sort_keys=True,
            ).encode()
            self.public_surfaces.append(("participant_console_http", safe))
        finally:
            browser(["close-surface", "--surface", surface])

    def _open_automation_console_session(
        self, supervisor: JsonHttp, studio_origin: str
    ) -> None:
        handoff_url = self._issue_handoff(supervisor, studio_origin)
        handoff = urllib.parse.urlsplit(handoff_url).fragment.removeprefix("handoff=")
        client = JsonHttp("http://127.0.0.1:9420")
        _, status, _ = client.request(
            "POST",
            "/api/v1/participant-console/handoffs:redeem",
            headers={
                "origin": self.console_origin,
                "x-worldstream-participant-handoff": handoff,
            },
        )
        _require(
            isinstance(status, dict)
            and status.get("state") == "usable"
            and status.get("next_action") == "continue",
            "participant_console_session_unusable",
        )
        self.human_http = client

    def _human_observe(self) -> dict[str, Any]:
        _require(self.human_http is not None, "participant_console_session_missing")
        _, value, _ = self.human_http.request(
            "POST",
            "/api/v1/participant-console/session:observe",
            {"after_frame_seq": None},
            headers={"origin": self.console_origin},
        )
        return _record(value, "participant_observation_invalid")

    def _human_projection(self) -> tuple[dict[str, Any], dict[str, Any]]:
        observation = self._human_observe()
        deliveries = observation.get("delivery")
        _require(isinstance(deliveries, list), "participant_observation_invalid")
        projection: dict[str, Any] | None = None
        for delivery in deliveries:
            if not isinstance(delivery, dict):
                continue
            body = delivery.get("body")
            if not isinstance(body, dict):
                continue
            candidate = body.get("projection")
            if isinstance(candidate, dict):
                projection = candidate
            nested = body.get("observation")
            if isinstance(nested, dict) and isinstance(nested.get("projection"), dict):
                projection = nested["projection"]
            elif isinstance(nested, dict) and "activity" in nested:
                projection = nested
        _require(projection is not None, "participant_projection_missing")
        return observation, projection

    def _human_act(
        self, action_type: str, payload: Mapping[str, Any], action_id: str
    ) -> dict[str, Any]:
        observation, projection = self._human_projection()
        offers = projection.get("action_offers")
        _require(isinstance(offers, list), "participant_offers_invalid")
        selected = next(
            (
                (index, offer)
                for index, offer in enumerate(offers)
                if isinstance(offer, dict) and offer.get("action_type") == action_type
            ),
            None,
        )
        _require(selected is not None, "participant_action_not_offered")
        index, offer = selected
        head = _record(observation.get("room_head"), "participant_head_invalid")
        room_seq = head.get("room_seq")
        digest = offer.get("payload_schema_digest")
        _require(
            isinstance(room_seq, int)
            and isinstance(digest, str)
            and DIGEST.fullmatch(digest),
            "participant_offer_invalid",
        )
        assert self.human_http is not None
        _, result, _ = self.human_http.request(
            "POST",
            "/api/v1/participant-console/session:act",
            {
                "action_id": action_id,
                "based_on_room_seq": room_seq,
                "offer_id": f"{room_seq}:{action_type}:{index}",
                "schema_digest": digest,
                "action_type": action_type,
                "payload": dict(payload),
            },
            headers={"origin": self.console_origin},
        )
        return _record(result, "participant_action_invalid")

    @staticmethod
    def _agent_observe(helper: McpHelper) -> dict[str, Any]:
        observed = _record(
            helper.call("worldstream.observe", {}), "agent_observation_invalid"
        )
        if isinstance(observed.get("code"), str):
            safe_code = observed["code"]
            if re.fullmatch(r"[a-z0-9_]{1,128}", safe_code) is None:
                safe_code = "invalid"
            raise AcceptanceFailure(f"agent_observation_invalid:{safe_code}")
        deliveries = observed.get("observations")
        if isinstance(deliveries, list) and deliveries:
            frames = [
                item.get("frame_seq") for item in deliveries if isinstance(item, dict)
            ]
            frames = [value for value in frames if isinstance(value, int)]
            if frames:
                helper.call(
                    "worldstream.acknowledge", {"through_frame_seq": max(frames)}
                )
        return observed

    @staticmethod
    def _agent_projection(observed: Mapping[str, Any]) -> dict[str, Any]:
        reset = observed.get("projection_reset")
        if isinstance(reset, dict) and isinstance(reset.get("projection"), dict):
            projection = reset["projection"]
        else:
            projection = None
        deliveries = observed.get("observations")
        if isinstance(deliveries, list):
            for delivery in deliveries:
                if not isinstance(delivery, dict):
                    continue
                body = delivery.get("observation")
                if isinstance(body, dict) and isinstance(body.get("projection"), dict):
                    projection = body["projection"]
                elif isinstance(body, dict) and "activity" in body:
                    projection = body
        _require(isinstance(projection, dict), "agent_projection_missing")
        return projection

    def _agent_act(
        self,
        helper: McpHelper,
        action_type: str,
        payload: Mapping[str, Any],
        operation_id: str,
        *,
        retry_same_operation: bool = False,
    ) -> dict[str, Any]:
        self._agent_observe(helper)
        listed = _record(
            helper.call("worldstream.list_current_action_offers", {}),
            "agent_offers_invalid",
        )
        offers = listed.get("offers")
        if not isinstance(offers, list):
            safe_code = listed.get("code")
            if not (
                isinstance(safe_code, str)
                and re.fullmatch(r"[a-z0-9_]{1,128}", safe_code)
            ):
                safe_code = "missing"
            raise AcceptanceFailure(f"agent_offers_invalid:{safe_code}")
        offer = next(
            (
                item
                for item in offers
                if isinstance(item, dict) and item.get("action_type") == action_type
            ),
            None,
        )
        _require(isinstance(offer, dict), "agent_action_not_offered")
        arguments = {
            "operation_id": operation_id,
            "offer_id": offer.get("offer_id"),
            "precondition": listed.get("precondition"),
            "payload": dict(payload),
        }
        result = _record(
            helper.call("worldstream.submit_action", arguments), "agent_action_invalid"
        )
        if retry_same_operation:
            duplicate = _record(
                helper.call("worldstream.submit_action", arguments),
                "agent_action_retry_invalid",
            )
            _require(duplicate == result, "agent_action_retry_mismatch")
        return result

    def _wait_ready_and_launch(self, client: JsonHttp, helper: McpHelper) -> None:
        deadline = time.monotonic() + 20
        reasons: list[str] = []
        while time.monotonic() < deadline:
            try:
                setup = self._get(client, f"/api/v1/task-setups/{DRAFT_ID}")
            except TimeoutError as error:
                raise AcceptanceFailure("task_readiness_status_timeout") from error
            if setup.get("readiness", {}).get("ready_to_launch") is True:
                try:
                    launched = self._post(
                        client, f"/api/v1/task-setups/{DRAFT_ID}/launch"
                    )
                except TimeoutError as error:
                    raise AcceptanceFailure("task_launch_request_timeout") from error
                _require(
                    launched.get("launch", {}).get("state")
                    in {"reconciling", "launched"},
                    "explicit_launch_failed",
                )
                while time.monotonic() < deadline:
                    settled = self._get(client, f"/api/v1/task-setups/{DRAFT_ID}")
                    launch_state = settled.get("launch", {}).get("state")
                    if launch_state == "launched":
                        return
                    _require(
                        launch_state == "reconciling",
                        "explicit_launch_failed",
                    )
                    time.sleep(0.2)
                raise AcceptanceFailure("explicit_launch_reconciliation_timeout")
            seats = setup.get("readiness", {}).get("seats")
            if isinstance(seats, list):
                reasons = sorted(
                    {
                        reason
                        for seat in seats
                        if isinstance(seat, dict)
                        and isinstance((reason := seat.get("reason")), str)
                    }
                )
            time.sleep(0.2)
        suffix = "+".join(reasons) if reasons else "unavailable"
        raise AcceptanceFailure(f"readiness_timeout:{suffix}")

    @staticmethod
    def _claim_code(projection: Mapping[str, Any], clue_id: str) -> str:
        activity = projection.get("activity")
        clues = activity.get("private_clues") if isinstance(activity, dict) else None
        _require(isinstance(clues, list), "private_clues_missing")
        for clue in clues:
            if isinstance(clue, dict) and clue.get("clue_id") == clue_id:
                value = clue.get("claim_code")
                _require(isinstance(value, str) and bool(value), "claim_code_invalid")
                return value
        raise AcceptanceFailure("claim_code_missing")

    @staticmethod
    def _plan(route_claim: str, entry_claim: str) -> dict[str, str]:
        _require(
            route_claim.startswith("route_")
            and entry_claim.startswith("entry_window_"),
            "claim_shape_invalid",
        )
        route = route_claim.removeprefix("route_")
        entry = entry_claim.removeprefix("entry_window_")
        broker = {
            ("canal", "late"): ("disguise", "van"),
            ("service", "early"): ("thermal_key", "boat"),
            ("roof", "middle"): ("jammer", "motorbike"),
        }.get((route, entry))
        _require(broker is not None, "claim_pair_invalid")
        required_tool, extraction = broker
        return {
            "route": route,
            "entry_window": entry,
            "required_tool": required_tool,
            "extraction": extraction,
        }

    @staticmethod
    def _phase(projection: Mapping[str, Any]) -> str | None:
        activity = projection.get("activity")
        phase = activity.get("phase") if isinstance(activity, dict) else None
        if isinstance(phase, str):
            return phase
        if isinstance(phase, dict):
            value = phase.get("kind") or phase.get("name") or phase.get("phase")
            return value if isinstance(value, str) else None
        return None

    def _fire_timer(
        self,
        daemon: JsonHttp,
        headers: Mapping[str, str],
        room_id: str,
        timer_id: str,
        generation: int,
        maximum_wait: float = 125.0,
    ) -> dict[str, Any]:
        deadline = time.monotonic() + maximum_wait
        path = f"/v1/operator/rooms/{room_id}/timers/fire"
        while time.monotonic() < deadline:
            status, value, _ = daemon.request(
                "POST",
                path,
                {"timer_id": timer_id, "generation": generation},
                headers=headers,
                expected=(200, 409, 429, 503),
            )
            if status == 200:
                return _record(value, "timer_receipt_invalid")
            error = value.get("error") if isinstance(value, dict) else None
            code = error.get("code") if isinstance(error, dict) else None
            _require(
                code in {"timer_not_due", "room_busy", "storage_unavailable"},
                "timer_rejected",
            )
            time.sleep(0.25)
        raise AcceptanceFailure("timer_due_timeout")

    def _drive_heist(
        self,
        daemon: JsonHttp,
        host_headers: Mapping[str, str],
        helper: McpHelper,
        room_id: str,
    ) -> dict[str, Any]:
        """Drive briefing/negotiation to a leased Activation before restart."""

        transition_ids: list[str] = []
        human_inspect = self._human_act(
            "inspect_clue", {"clue_id": "route"}, ACTION_IDS["human_inspect"]
        )
        if isinstance(human_inspect.get("transition_id"), str):
            transition_ids.append(human_inspect["transition_id"])
        agent_inspect = self._agent_act(
            helper,
            "inspect_clue",
            {"clue_id": "entry_window"},
            ACTION_IDS["agent_inspect"],
            retry_same_operation=True,
        )
        if isinstance(agent_inspect.get("transition_id"), str):
            transition_ids.append(agent_inspect["transition_id"])
        _, human_projection = self._human_projection()
        agent_projection = self._agent_projection(self._agent_observe(helper))
        route_claim = self._claim_code(human_projection, "route")
        entry_claim = self._claim_code(agent_projection, "entry_window")
        self.private_values.append(route_claim.encode())

        timer = self._fire_timer(daemon, host_headers, room_id, TIMER_PHASE, 1)
        if isinstance(timer.get("transition_id"), str):
            transition_ids.append(timer["transition_id"])
        self._human_act(
            "publish_clue",
            {"clue_id": "route", "claim_code": route_claim},
            ACTION_IDS["human_publish"],
        )
        self._agent_act(
            helper,
            "publish_clue",
            {"clue_id": "entry_window", "claim_code": entry_claim},
            ACTION_IDS["agent_publish"],
        )
        plan = self._plan(route_claim, entry_claim)
        self._human_act("propose_plan", plan, ACTION_IDS["plan"])

        deadline = time.monotonic() + 20
        leased: dict[str, Any] | None = None
        last_activation_code = "unavailable"
        while time.monotonic() < deadline:
            try:
                candidate = helper.call("worldstream.next_activation", {})
            except AcceptanceFailure:
                time.sleep(0.25)
                continue
            if isinstance(candidate, dict) and isinstance(
                candidate.get("activation"), dict
            ):
                leased = candidate
                break
            if isinstance(candidate, dict):
                code = candidate.get("code")
                if isinstance(code, str) and re.fullmatch(r"[a-z0-9_]{1,128}", code):
                    last_activation_code = code
            time.sleep(0.25)
        _require(
            leased is not None,
            f"activation_not_leased:{last_activation_code}",
        )
        return {
            "human_actions": 3,
            "agent_actions": 2,
            "observation_acknowledged": True,
            "duplicate_actions": 0,
            "transition_ids": transition_ids,
            "leased_activation": leased,
            "plan_id": ACTION_IDS["plan"],
        }

    def _complete_resumed_activation(
        self, helper: McpHelper, leased: Any, resumed: Any
    ) -> dict[str, Any]:
        first = _record(leased, "activation_lease_invalid")
        first_lease = _record(first.get("activation"), "activation_lease_invalid")
        second = _record(resumed, "activation_resume_invalid")
        reclaimed = False
        if not isinstance(second.get("activation"), dict):
            _require(
                second.get("code")
                in {
                    "assignment_activation_lease_expired",
                    "assignment_activation_cursor_stale",
                    "assignment_activation_none_available",
                },
                "activation_resume_invalid",
            )
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                candidate = _record(
                    helper.call("worldstream.next_activation", {}),
                    "activation_reclaim_invalid",
                )
                if isinstance(candidate.get("activation"), dict):
                    second = candidate
                    reclaimed = True
                    break
                _require(
                    candidate.get("code")
                    in {
                        "assignment_activation_lease_expired",
                        "assignment_activation_cursor_stale",
                        "assignment_activation_none_available",
                    },
                    "activation_reclaim_invalid",
                )
                time.sleep(0.25)
            _require(reclaimed, "activation_reclaim_timeout")
        second_lease = _record(second.get("activation"), "activation_resume_invalid")
        if reclaimed:
            first_context = _record(
                first_lease.get("context"), "activation_context_invalid"
            )
            second_context = _record(
                second_lease.get("context"), "activation_context_invalid"
            )
            _require(
                second_lease.get("activation_id")
                == first_lease.get("activation_id")
                and int(second.get("activation_cursor", 0))
                > int(first.get("activation_cursor", 0))
                and int(second_context.get("lease_generation", 0))
                > int(first_context.get("lease_generation", 0))
                and second_context.get("cause_room_seq")
                == first_context.get("cause_room_seq")
                and second_context.get("reason_code")
                == first_context.get("reason_code"),
                "activation_reclaim_mismatch",
            )
        else:
            # A replacement that starts inside the lease resumes its private
            # witness byte-for-byte; only the safe reconnect flag changes.
            _require(
                first.get("activation_cursor") == second.get("activation_cursor")
                and first_lease == second_lease
                and second.get("reconnected") is True,
                "activation_restart_mismatch",
            )
        context = _record(second_lease.get("context"), "activation_context_invalid")
        arguments = {
            "activation_cursor": second.get("activation_cursor"),
            "lease_generation": context.get("lease_generation"),
            "context_hash": second_lease.get("context_hash"),
            "disposition": "handled",
        }
        completed = _record(
            helper.call("worldstream.complete_activation", arguments),
            "activation_completion_invalid",
        )
        duplicate = _record(
            helper.call("worldstream.complete_activation", arguments),
            "activation_completion_retry_invalid",
        )
        _require(
            completed.get("operation_id") == duplicate.get("operation_id")
            and completed.get("completion_id") == duplicate.get("completion_id"),
            "activation_completion_retry_mismatch",
        )
        return {
            "activation_leased": True,
            "activation_reclaimed_after_expiry": reclaimed,
            "duplicate_completions": 0,
        }

    def _finish_heist(
        self,
        daemon: JsonHttp,
        host_headers: Mapping[str, str],
        helper: McpHelper,
        room_id: str,
        story: dict[str, Any],
    ) -> None:
        endorsement = self._agent_act(
            helper,
            "endorse_plan",
            {"plan_id": story["plan_id"]},
            ACTION_IDS["endorse"],
        )
        if isinstance(endorsement.get("transition_id"), str):
            story["transition_ids"].append(endorsement["transition_id"])
        self._fire_timer(daemon, host_headers, room_id, TIMER_PHASE, 2)
        self._human_act(
            "commit_move",
            {
                "selected_plan_id": story["plan_id"],
                "contribute_required_resource": False,
            },
            ACTION_IDS["human_commit"],
        )
        self._agent_act(
            helper,
            "commit_move",
            {
                "selected_plan_id": story["plan_id"],
                "contribute_required_resource": True,
            },
            ACTION_IDS["agent_commit"],
            retry_same_operation=True,
        )
        self._fire_timer(daemon, host_headers, room_id, TIMER_REMINDER, 1)
        self._fire_timer(daemon, host_headers, room_id, TIMER_PHASE, 3)
        self._fire_timer(daemon, host_headers, room_id, TIMER_RESOLVE, 1)
        self._human_act("acknowledge_result", {}, ACTION_IDS["human_ack"])
        self._agent_act(
            helper,
            "acknowledge_result",
            {},
            ACTION_IDS["agent_ack"],
            retry_same_operation=True,
        )
        self._fire_timer(daemon, host_headers, room_id, TIMER_PHASE, 4)
        story["human_actions"] += 2
        story["agent_actions"] += 3

    def _outcome_and_replay(
        self,
        daemon: JsonHttp,
        headers: Mapping[str, str],
        room_id: str,
        story: Mapping[str, Any],
    ) -> tuple[dict[str, Any], dict[str, Any]]:
        observation, projection = self._human_projection()
        head = _record(observation.get("room_head"), "final_head_invalid")
        room_seq = head.get("room_seq")
        activity = _record(projection.get("activity"), "final_activity_invalid")
        _require(self._phase(projection) == "complete", "heist_not_complete")
        result = activity.get("result") or activity.get("outcome")
        _require(
            isinstance(result, dict) and bool(result), "meaningful_outcome_missing"
        )

        setup = self._get(
            JsonHttp("http://127.0.0.1:9420"), f"/api/v1/task-setups/{DRAFT_ID}"
        )
        seats = setup.get("seats")
        _require(isinstance(seats, list), "human_membership_missing")
        human = next(
            (
                seat
                for seat in seats
                if isinstance(seat, dict) and seat.get("principal_kind") == "human"
            ),
            None,
        )
        _require(isinstance(human, dict), "human_membership_missing")
        replay_capability_id = "01ARZ3NDEKTSV4RRFFQ69G5FG0"
        status, capability, _ = daemon.request(
            "POST",
            "/v1/operator/member-capabilities",
            {
                "room_id": room_id,
                "member_id": human.get("member_id"),
                "principal_id": human.get("principal_id"),
                "scopes": ["room:attach", "room:observe_member", "room:replay"],
                "idempotency_key": replay_capability_id,
                "expires_at": None,
            },
            headers=headers,
            expected=(200, 201),
        )
        _require(
            status in (200, 201) and isinstance(capability, dict),
            "replay_capability_failed",
        )
        replay_bearer = capability.get("bearer")
        _require(
            isinstance(replay_bearer, str) and replay_bearer.startswith("wsb1:"),
            "replay_capability_invalid",
        )
        self.private_values.append(replay_bearer.encode())
        _, replay_value, _ = daemon.request(
            "GET",
            f"/v1/rooms/{room_id}/replay?at_room_seq={room_seq}",
            headers={"authorization": "Bearer " + replay_bearer},
        )
        replay_response = _record(replay_value, "replay_invalid")
        replay_head = _record(replay_response.get("room_head"), "replay_head_invalid")
        replay_projection = _record(
            replay_response.get("projection"), "replay_projection_invalid"
        )
        replay_activity = _record(
            replay_projection.get("activity"), "replay_activity_invalid"
        )
        _require(
            activity == replay_activity
            and result
            == (replay_activity.get("result") or replay_activity.get("outcome")),
            "replay_activity_mismatch",
        )
        activity_hash = head.get("activity_state_hash")
        authoritative_hash = head.get("authoritative_state_hash")
        transition_hash = head.get("genesis_or_transition_hash")
        _require(
            replay_head.get("activity_state_hash") == activity_hash
            and replay_head.get("authoritative_state_hash") == authoritative_hash,
            "replay_hash_mismatch",
        )
        _require(
            replay_head.get("room_seq") == room_seq
            and replay_head.get("genesis_or_transition_hash") == transition_hash,
            "replay_transition_mismatch",
        )
        transitions = [
            value for value in story.get("transition_ids", []) if isinstance(value, str)
        ]
        unique = len(transitions) == len(set(transitions))
        outcome = {
            "phase": "complete",
            "meaningful": True,
            "committed_room_seq": room_seq,
            "authoritative_state_hash": authoritative_hash,
            "activity_state_hash": activity_hash,
            "transition_hash": transition_hash,
            # The authoritative Activity digest commits the complete Outcome.
            "outcome_digest": activity_hash,
        }
        replay = {
            "verified": replay_response.get("verification") == "verified",
            "at_room_seq": room_seq,
            "authoritative_state_hash": replay_head.get("authoritative_state_hash"),
            "activity_state_hash": replay_head.get("activity_state_hash"),
            "transition_hash": replay_head.get("genesis_or_transition_hash"),
            "outcome_digest": replay_head.get("activity_state_hash"),
            "transition_ids_unique": unique,
        }
        return outcome, replay


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--live", action="store_true", help="run the real-process release gate"
    )
    parser.add_argument(
        "--report", type=pathlib.Path, help="validate or write the bounded report"
    )
    parser.add_argument("--worldstreamd", default="target/debug/worldstreamd")
    parser.add_argument(
        "--supervisor", default="target/debug/worldstream-studio-supervisor"
    )
    parser.add_argument(
        "--assignment-mcp", default="target/debug/worldstream-assignment-mcp"
    )
    parser.add_argument("--console-dist", default="web/console/dist")
    parser.add_argument("--keep-artifacts", action="store_true")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        if not args.live:
            _require(
                args.report is not None and args.report.is_file(), "report_required"
            )
            validate_completed_report(
                json.loads(args.report.read_text(encoding="utf-8"))
            )
            print("IMO-78 MVP acceptance report: valid")
            return 0
        gate = LiveGate(args)
        try:
            report = gate.run()
            encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
            if args.report:
                args.report.write_text(encoded, encoding="utf-8")
            else:
                sys.stdout.write(encoded)
            return 0
        finally:
            gate.close()
    except (AcceptanceFailure, OSError, ValueError, json.JSONDecodeError) as error:
        code = error.args[0] if error.args else "acceptance_failed"
        print(f"blocked:{code}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())

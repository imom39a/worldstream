#!/usr/bin/env python3
"""Run the strongest currently available local daemon/SDK story.

The runner exercises the audited operator member-capability issuance route,
then verifies a real member attach, sync barrier, and observation ack. It also
checks that the durable scheduler starts and publishes readiness, while
keeping this Counter path separate from a complete Agent Heist execution
story.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import pathlib
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

from worldstream_sdk import Client, ProtocolError, Room

PACK_DIGEST = "blake3:1c5f75068220f65f9017a062dbe40203c540108b572284d9446a329914008a92"
PRINCIPAL_ID = "01ARZ3NDEKTSV4RRFFQ69G5FC2"
REPOSITORY = pathlib.Path(__file__).resolve().parents[3]


def digest(value: object) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def healthz(base_url: str) -> int:
    with urllib.request.urlopen(f"{base_url}/healthz", timeout=1) as response:
        return response.status


def readyz(base_url: str) -> int:
    try:
        with urllib.request.urlopen(f"{base_url}/readyz", timeout=1) as response:
            return response.status
    except urllib.error.HTTPError as error:
        return error.code


def version_response(base_url: str) -> dict:
    with urllib.request.urlopen(f"{base_url}/version", timeout=5) as response:
        return json.load(response)


def issue_member_capability(base_url: str, host_bearer: str, request: dict) -> dict:
    payload = json.dumps(request, sort_keys=True, separators=(",", ":")).encode()
    http_request = urllib.request.Request(
        f"{base_url}/v1/operator/member-capabilities",
        data=payload,
        method="POST",
        headers={
            "Accept": "application/json",
            "Authorization": f"Bearer {host_bearer}",
            "Content-Type": "application/json",
        },
    )
    with urllib.request.urlopen(http_request, timeout=5) as response:
        return json.load(response)


class Daemon:
    def __init__(self, root: pathlib.Path, data_dir: pathlib.Path, port: int) -> None:
        self.root = root
        self.data_dir = data_dir
        self.port = port
        self.base_url = f"http://127.0.0.1:{port}"
        self.binary = str((REPOSITORY / "target/debug/worldstreamd").resolve())
        self.secret = root / "authority.secret"
        self.log_path = root / "worldstreamd.log"
        self.process: subprocess.Popen[bytes] | None = None
        self.log_handle = None

    def start(self) -> None:
        self.log_handle = self.log_path.open("ab")
        environment = {
            **os.environ,
            "WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE": str(self.secret),
            "RUST_LOG": "warn",
        }
        self.process = subprocess.Popen(
            [
                self.binary,
                "--data-dir",
                str(self.data_dir),
                "--bind",
                f"127.0.0.1:{self.port}",
            ],
            cwd=REPOSITORY,
            env=environment,
            stdout=self.log_handle,
            stderr=subprocess.STDOUT,
        )
        for _ in range(100):
            try:
                if healthz(self.base_url) == 200:
                    return
            except OSError:
                pass
            if self.process.poll() is not None:
                raise RuntimeError("worldstreamd exited before healthz")
            time.sleep(0.05)
        raise RuntimeError("healthz did not become live")

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


def create_private_root() -> pathlib.Path:
    root = pathlib.Path(tempfile.mkdtemp(prefix="worldstream-wave6-live-"))
    root.chmod(0o700)
    secret = root / "authority.secret"
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    descriptor = os.open(secret, flags, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(os.urandom(32))
            handle.flush()
            os.fsync(handle.fileno())
    except BaseException:
        secret.unlink(missing_ok=True)
        raise
    secret.chmod(0o600)
    data_dir = root / "data"
    data_dir.mkdir(mode=0o700)
    return root


async def run() -> None:
    root = create_private_root()
    data_dir = root / "data"
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    daemon = Daemon(root, data_dir, port)
    client: Client | None = None
    try:
        daemon.start()
        ready_before = readyz(daemon.base_url)
        version_before = await asyncio.to_thread(version_response, daemon.base_url)
        bearer = f"wsb1:{(root / 'authority.secret').read_bytes().hex()}"
        client = Client(daemon.base_url, bearer)
        request = {
            "pack": {
                "id": "worldstream.counter",
                "version": "2.0.0",
                "digest": PACK_DIGEST,
            },
            "configuration": {"initial_value": 0, "maximum_value": 16},
            "members": [
                {
                    "principal_id": PRINCIPAL_ID,
                    "principal_kind": "human",
                    "role": "counter",
                    "access_mode": "participant",
                }
            ],
            "idempotency_key": "wave6-live-story-create-v2",
        }
        created = await client.create_room(request)
        capability = await asyncio.to_thread(
            issue_member_capability,
            daemon.base_url,
            bearer,
            {
                "room_id": created["room_id"],
                "member_id": created["member_ids"][0],
                "principal_id": PRINCIPAL_ID,
                "scopes": ["room:attach", "room:observe_member", "room:act"],
                "idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FD1",  # gitleaks:allow
                "expires_at": None,
            },
        )
        member_client = Client(daemon.base_url, capability["bearer"])
        first_room = Room(member_client, created["room_id"], created["member_ids"][0])
        attach_error = None
        observation_ack = False
        action_result = None
        action_error = None
        try:
            await first_room.connect()
            await first_room.sync()
            await first_room.ack(0)
            observation_ack = True
        except ProtocolError as error:
            attach_error = {
                "code": error.code,
                "retryable": error.retryable,
                "message": str(error),
            }
        if observation_ack:
            try:
                action_result = await first_room.act(
                    "increment", {}, expected_room_seq=0, timeout=5
                )
            except ProtocolError as error:
                action_error = {
                    "code": error.code,
                    "retryable": error.retryable,
                    "message": str(error),
                }
        welcome = first_room.welcome
        await first_room.close()
        daemon.stop()
        daemon.start()
        ready_after = readyz(daemon.base_url)
        version_after = await asyncio.to_thread(version_response, daemon.base_url)
        duplicate = await client.create_room(request)
        second_room = Room(member_client, created["room_id"], created["member_ids"][0])
        second_attach_error = None
        try:
            await second_room.connect()
            await second_room.sync()
            await second_room.ack(0)
        except ProtocolError as error:
            second_attach_error = {"code": error.code, "retryable": error.retryable}
        await second_room.close()
        print(
            json.dumps(
                {
                    "status": "partial",
                    "evidence_class": "real_process_network_http_and_websocket_sdk",
                    "healthz_before": 200,
                    "healthz_after_restart": healthz(daemon.base_url),
                    "readyz_before": ready_before,
                    "readyz_after_restart": ready_after,
                    "version_before": {
                        "engine_status": version_before["engine"]["status"],
                        "release_ready": version_before["manifest"]["release_ready"],
                    },
                    "version_after": {
                        "engine_status": version_after["engine"]["status"],
                        "release_ready": version_after["manifest"]["release_ready"],
                    },
                    "http_create": {
                        "committed": True,
                        "member_count": len(created["member_ids"]),
                        "room_head_seq": created["room_head"]["room_seq"],
                        "room_head_digest": digest(created["room_head"]),
                    },
                    "http_restart_idempotency": {
                        "same_room_id": duplicate["room_id"] == created["room_id"],
                        "same_member_ids": duplicate["member_ids"]
                        == created["member_ids"],
                        "same_head": duplicate["room_head"] == created["room_head"],
                    },
                    "sdk_websocket": {
                        "welcome_received": welcome is not None,
                        "selected_protocol": welcome["selected_protocol"]
                        if welcome
                        else None,
                        "attach_error": attach_error,
                        "attach_error_after_restart": second_attach_error,
                        "observation_ack": observation_ack,
                        "action_reply": {
                            "accepted": action_result is not None
                            and "transition_id" in action_result,
                            "action_id": action_result["action_id"]
                            if action_result is not None
                            else None,
                            "room_seq": action_result["room_head"]["room_seq"]
                            if action_result is not None
                            and "room_head" in action_result
                            else None,
                            "request_reply_correlated": action_result is not None,
                            "error": action_error,
                        },
                    },
                    "member_capability": {
                        "issued": capability["room_id"] == created["room_id"]
                        and capability["member_id"] == created["member_ids"][0],
                        "bearer": "not_emitted",
                    },
                    "boundary": "scheduler readiness is live, but this proves the Counter member path rather than the complete Agent Heist execution story",
                    "secrets": "not_emitted",
                },
                sort_keys=True,
                separators=(",", ":"),
            )
        )
    finally:
        daemon.stop()
        if client is not None:
            del client
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    asyncio.run(run())

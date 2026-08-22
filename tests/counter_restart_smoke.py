#!/usr/bin/env python3
"""Real-process Counter acceptance witness for restart and Action boundaries.

This runner uses only the public host HTTP routes and the Python SDK's HTTP /
WebSocket surfaces.  It deliberately opts into the daemon's test-only
post-commit termination seam for one Action so that a missing reply is
observable without inspecting SQLite or calling server internals.

The JSON result is intentionally limited to non-secret booleans, codes, and
server-provided hashes.  It is evidence for a disposable local run, never
release evidence.
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

from worldstream_sdk import Client, LostActionReply, ProtocolError

REPOSITORY = pathlib.Path(__file__).resolve().parents[1]
DAEMON = REPOSITORY / "target" / "debug" / "worldstreamd"
PACK_DIGEST = "blake3:1c5f75068220f65f9017a062dbe40203c540108b572284d9446a329914008a92"
PRINCIPAL_ID = "01ARZ3NDEKTSV4RRFFQ69G5FC2"
KILL_ENV = "WORLDSTREAM_TEST_KILL_AFTER_ACTION_COMMIT_BEFORE_REPLY"


def canonical_digest(value: object) -> str:
    raw = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return "sha256:" + hashlib.sha256(raw).hexdigest()


def status_code(url: str) -> int:
    try:
        with urllib.request.urlopen(url, timeout=1) as response:
            return response.status
    except urllib.error.HTTPError as error:
        return error.code
    except OSError:
        return 0


def issue_member_capability(
    base_url: str, host_bearer: str, room_id: str, member_id: str
) -> dict:
    request_body = {
        "room_id": room_id,
        "member_id": member_id,
        "principal_id": PRINCIPAL_ID,
        "scopes": ["room:attach", "room:observe_member", "room:act"],
        "idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FD1",  # gitleaks:allow
        "expires_at": None,
    }
    payload = json.dumps(request_body, sort_keys=True, separators=(",", ":")).encode()
    request = urllib.request.Request(
        f"{base_url}/v1/operator/member-capabilities",
        data=payload,
        method="POST",
        headers={
            "Accept": "application/json",
            "Authorization": f"Bearer {host_bearer}",
            "Content-Type": "application/json",
        },
    )
    with urllib.request.urlopen(request, timeout=5) as response:
        return json.load(response)


class Daemon:
    def __init__(self, root: pathlib.Path, port: int) -> None:
        self.root = root
        self.data_dir = root / "data"
        self.base_url = f"http://127.0.0.1:{port}"
        self.port = port
        self.process: subprocess.Popen[bytes] | None = None
        self.log_handle = None
        self.kill_action_id: str | None = None

    def start(self) -> None:
        if not DAEMON.is_file() or not os.access(DAEMON, os.X_OK):
            raise RuntimeError(f"worldstreamd is not executable: {DAEMON}")
        self.log_handle = (self.root / "worldstreamd.log").open("ab")
        environment = {
            **os.environ,
            "WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE": str(
                self.root / "authority.secret"
            ),
            "RUST_LOG": "warn",
        }
        if self.kill_action_id is not None:
            environment[KILL_ENV] = self.kill_action_id
        else:
            environment.pop(KILL_ENV, None)
        self.process = subprocess.Popen(
            [
                str(DAEMON),
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
        for _ in range(120):
            if status_code(f"{self.base_url}/healthz") == 200:
                return
            if self.process.poll() is not None:
                raise RuntimeError("worldstreamd exited before healthz")
            time.sleep(0.05)
        raise RuntimeError("healthz did not become live")

    def stop(self) -> int | None:
        process = self.process
        self.process = None
        if process is None:
            return None
        if process.poll() is None:
            process.send_signal(signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        exit_code = process.returncode
        if self.log_handle is not None:
            self.log_handle.close()
            self.log_handle = None
        return exit_code


def projection_fingerprint(projection: dict) -> dict:
    head = projection["room_head"]
    return {
        "room_seq": head["room_seq"],
        "projection_hash": projection["projection_hash"],
        "head_hashes": {
            key: head[key]
            for key in (
                "genesis_or_transition_hash",
                "core_state_hash",
                "activity_state_hash",
                "authoritative_state_hash",
            )
        },
        "projection_digest": canonical_digest(projection["projection"]),
    }


async def run() -> None:
    if os.name != "posix":
        raise RuntimeError("Counter restart smoke requires a POSIX process environment")
    root = pathlib.Path(tempfile.mkdtemp(prefix="worldstream-counter-restart-"))
    root.chmod(0o700)
    secret_path = root / "authority.secret"
    secret_path.write_bytes(os.urandom(32))
    secret_path.chmod(0o600)
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    daemon = Daemon(root, port)
    host_bearer = f"wsb1:{secret_path.read_bytes().hex()}"
    client: Client | None = None
    member_client: Client | None = None
    try:
        daemon.start()
        client = Client(daemon.base_url, host_bearer)
        created = await client.create_room(
            {
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
                "idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FD0",  # gitleaks:allow
            }
        )
        room_id = created["room_id"]
        member_id = created["member_ids"][0]
        capability = await asyncio.to_thread(
            issue_member_capability, daemon.base_url, host_bearer, room_id, member_id
        )
        member_client = Client(daemon.base_url, capability["bearer"])

        room = await member_client.open_room(room_id, member_id)
        await room.sync()
        await room.ack(0)
        warmup = await room.act(
            "increment", {}, action_id="01ARZ3NDEKTSV4RRFFQ69G5FD2", expected_room_seq=0
        )
        before_clean_restart = projection_fingerprint(
            await member_client.projection(room_id)
        )
        await room.close()
        clean_exit = daemon.stop()
        daemon.start()
        after_clean_restart = projection_fingerprint(
            await member_client.projection(room_id)
        )

        lost_action_id = "01ARZ3NDEKTSV4RRFFQ69G5FD3"
        daemon.kill_action_id = lost_action_id
        daemon.stop()
        daemon.start()
        lost_room = await member_client.open_room(room_id, member_id)
        await lost_room.sync()
        await lost_room.ack(1)
        try:
            await lost_room.act(
                "increment",
                {},
                action_id=lost_action_id,
                expected_room_seq=1,
                timeout=5,
            )
        except LostActionReply as error:
            lost_request = error.request
        else:
            raise AssertionError("post-commit kill did not produce a lost Action reply")
        killed_exit = daemon.stop()
        daemon.kill_action_id = None
        daemon.start()
        after_lost_restart = projection_fingerprint(
            await member_client.projection(room_id)
        )

        recovered = await member_client.open_room(room_id, member_id, after_frame_seq=1)
        await recovered.sync()
        duplicate = await recovered.retry_action(lost_request)
        after_duplicate = projection_fingerprint(
            await member_client.projection(room_id)
        )

        conflict = None
        conflict_room = await member_client.open_room(
            room_id, member_id, after_frame_seq=1
        )
        await conflict_room.sync()
        try:
            await conflict_room.act(
                "increment",
                {"changed": True},
                action_id=lost_action_id,
                expected_room_seq=2,
            )
        except ProtocolError as error:
            conflict = {"code": error.code, "retryable": error.retryable}
        if conflict is None or conflict["code"] != "idempotency_conflict":
            raise AssertionError(
                f"expected server idempotency conflict, got {conflict!r}"
            )

        stale = await recovered.act(
            "increment", {}, action_id="01ARZ3NDEKTSV4RRFFQ69G5FD4", expected_room_seq=0
        )
        if stale.get("code") != "stale_room_state":
            raise AssertionError(f"expected stale_room_state, got {stale!r}")
        resync_batch = await recovered.resync()
        await recovered.ack(2)
        after_resync = projection_fingerprint(await member_client.projection(room_id))
        await conflict_room.close()
        await recovered.close()

        if before_clean_restart != after_clean_restart:
            raise AssertionError(
                "clean restart changed the Counter projection/head fingerprint"
            )
        if after_lost_restart != after_duplicate:
            raise AssertionError(
                "duplicate retry changed the already-committed Counter state"
            )
        if after_resync != after_duplicate:
            raise AssertionError(
                "resync projection/head differs from the durable post-restart state"
            )
        print(
            json.dumps(
                {
                    "status": "passed",
                    "evidence_class": "real_process_network_http_websocket_python_sdk",
                    "release_evidence": False,
                    "secrets": "not_emitted",
                    "room": {
                        "created": True,
                        "warmup_room_seq": warmup["room_head"]["room_seq"],
                        "clean_restart_exit": clean_exit,
                        "clean_restart_hashes_equal": before_clean_restart
                        == after_clean_restart,
                        "lost_reply_observed": True,
                        "post_commit_kill_exit": killed_exit,
                        "lost_restart_room_seq": after_lost_restart["room_seq"],
                        "duplicate": duplicate["duplicate"],
                        "duplicate_preserved_post_restart_state": after_lost_restart
                        == after_duplicate,
                        "conflict": conflict,
                        "stale": {
                            "code": stale["code"],
                            "current_room_seq": stale["current_room_seq"],
                        },
                        "resync_batch_count": len(resync_batch),
                        "post_restart_hashes_equal_after_duplicate": after_lost_restart
                        == after_duplicate,
                        "post_restart_hashes_equal_after_resync": after_lost_restart
                        == after_resync,
                        "post_restart": after_lost_restart,
                    },
                },
                sort_keys=True,
                separators=(",", ":"),
            )
        )
    finally:
        daemon.kill_action_id = None
        daemon.stop()
        if client is not None:
            del client
        if member_client is not None:
            del member_client
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    asyncio.run(run())

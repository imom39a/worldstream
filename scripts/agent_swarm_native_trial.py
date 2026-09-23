"""Measured, disposable native Codex admission for a single local Swarm trial.

The release qualification matrix is unchanged. Receipts are collected from the
installed executable, not fixtures. Provider-managed login is copied into a
private temporary profile and removed when the owning trial exits.
"""

from __future__ import annotations

import hashlib
import http.server
import json
import os
import queue
import shutil
import signal
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from pathlib import Path

from agent_swarm_codex_probe import (
    API_ENV,
    READY,
    digest,
    guarded_turn,
    process_snapshot,
)

PROFILE_CONFIG = """model_provider = "openai"
forced_login_method = "chatgpt"
approval_policy = "never"
default_permissions = "worldstream_swarm_read"
web_search = "disabled"
project_doc_max_bytes = 0
[features]
apps = false
plugins = false
hooks = false
multi_agent = false
multi_agent_v2 = false
browser_use = false
computer_use = false
image_generation = false
shell_snapshot = false
shell_tool = false
unified_exec = false
skill_search = false
workspace_dependencies = false
view_image = false
[permissions.worldstream_swarm_read.filesystem]
":minimal" = "read"
":workspace_roots" = { "." = "read" }
[permissions.worldstream_swarm_read.network]
enabled = false
"""


def persist(path: Path, value: object) -> dict:
    data = (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(data)
    return {"path": path.name, "sha256": hashlib.sha256(data).hexdigest()}


class MetadataServer:
    def __init__(self, native: Path, work: Path, env: dict):
        self.process = subprocess.Popen(
            [str(native), "app-server", "--stdio", "--strict-config"],
            cwd=work,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            start_new_session=True,
        )
        self.messages = queue.Queue()
        self.serial = 0
        self.methods = set()

        def drain():
            for line in self.process.stdout:
                if len(line) > 1024 * 1024:
                    self.messages.put({"error": "oversized metadata frame"})
                    return
                self.messages.put(json.loads(line))
            self.messages.put({"error": "metadata server closed"})

        self.reader = threading.Thread(target=drain, daemon=True)
        self.reader.start()

    def rpc(self, method: str, params: dict) -> dict:
        self.serial += 1
        self.process.stdin.write(
            json.dumps({"id": self.serial, "method": method, "params": params}) + "\n"
        )
        self.process.stdin.flush()
        deadline = time.monotonic() + 20
        while True:
            response = self.messages.get(timeout=max(0.01, deadline - time.monotonic()))
            if "error" in response:
                raise RuntimeError("native metadata request failed: " + method)
            if response.get("id") == self.serial:
                return response["result"]
            if "method" in response:
                self.methods.add(response["method"])

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGTERM)
                self.process.wait(timeout=10)
        self.reader.join(timeout=2)


@dataclass
class NativeTrial:
    args: object
    root: Path
    work: Path

    def __post_init__(self):
        self.root.mkdir(mode=0o700)
        self.profile = self.root / "profile"
        self.profile.mkdir(mode=0o700)
        source = (
            Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex")))
            / "auth.json"
        )
        if not source.is_file():
            raise RuntimeError("normal provider-managed Codex sign-in is unavailable")
        shutil.copyfile(source, self.profile / "auth.json")
        (self.profile / "auth.json").chmod(0o600)
        config = self.profile / "config.toml"
        config.write_text(PROFILE_CONFIG, encoding="utf-8")
        config.chmod(0o600)
        self.env = {
            key: value for key, value in os.environ.items() if key not in API_ENV
        }
        self.env.update(CODEX_HOME=str(self.profile), NO_COLOR="1")
        inspected = subprocess.run(
            [
                str(self.args.swarm_app),
                "providers",
                "--codex-path",
                str(self.args.codex),
            ],
            capture_output=True,
            env=self.env,
            check=True,
            timeout=30,
        )
        capability = next(
            row["capabilities"]
            for row in json.loads(inspected.stdout)
            if row["provider"] == "codex"
            and row["capabilities"] is not None
            and Path(row["capabilities"]["executable"]) == self.args.codex
        )
        self.executable_digest = capability["executable_digest"]
        self.identities = {
            "executable_sha256": digest(self.args.codex),
            "guard_sha256": digest(self.args.guard),
            "configuration_sha256": digest(config),
        }

    def isolation(self) -> dict:
        inside = self.work / ".native-inside-canary"
        outside = self.root / ".native-outside-canary"
        inside.write_text("SYNTHETIC_INSIDE")
        outside.write_text("SYNTHETIC_OUTSIDE")
        server = MetadataServer(self.args.codex, self.work, self.env)

        class CanaryHandler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b"SYNTHETIC_NETWORK")

            def log_message(self, *_args):
                pass

        network = http.server.HTTPServer(("127.0.0.1", 0), CanaryHandler)
        threading.Thread(target=network.serve_forever, daemon=True).start()
        network_url = f"http://127.0.0.1:{network.server_port}"
        try:
            server.rpc(
                "initialize",
                {
                    "clientInfo": {"name": "worldstream_native_trial", "version": "1"},
                    "capabilities": {"experimentalApi": True},
                },
            )
            account = server.rpc("account/read", {"refreshToken": False})
            status = server.rpc("mcpServerStatus/list", {})
            settings = server.rpc(
                "thread/start",
                {
                    "model": self.args.model,
                    "modelProvider": "openai",
                    "cwd": str(self.work),
                    "permissions": "worldstream_swarm_read",
                    "approvalPolicy": "never",
                    "runtimeWorkspaceRoots": [str(self.work)],
                    "selectedCapabilityRoots": [],
                    "allowProviderModelFallback": False,
                    "config": {"model_reasoning_effort": self.args.effort},
                },
            )
            selected = {
                key: settings.get(key)
                for key in (
                    "model",
                    "reasoningEffort",
                    "activePermissionProfile",
                    "instructionSources",
                    "sandbox",
                    "runtimeWorkspaceRoots",
                )
            }
            if (
                selected["model"] != self.args.model
                or selected["reasoningEffort"] != self.args.effort
            ):
                raise RuntimeError("native metadata substituted model/effort")
            if (
                selected["instructionSources"] != []
                or status.get("data") != []
                or status.get("nextCursor") is not None
            ):
                raise RuntimeError(
                    "ambient instructions or external MCP tools remain enabled"
                )
            receipt = {
                **self.identities,
                "working_area": str(self.work),
                "account_type": account.get("account", {}).get("type"),
                "mcp_server_count": len(status["data"]),
                "thread_configuration": selected,
            }
            baseline = subprocess.run(
                [
                    "/usr/bin/curl",
                    "--silent",
                    "--show-error",
                    "--max-time",
                    "3",
                    network_url,
                ],
                env=self.env,
                capture_output=True,
                check=True,
            )
            if baseline.stdout != b"SYNTHETIC_NETWORK":
                raise RuntimeError("network canary server is unavailable")
            receipt["network_baseline"] = {
                "exit_code": baseline.returncode,
                "stdout": baseline.stdout.decode(),
            }
            commands = {
                "inside_read": ["/bin/cat", str(inside)],
                "outside_read": ["/bin/cat", str(outside)],
                "outside_write": ["/usr/bin/touch", str(self.root / "forbidden-write")],
                "inside_write": ["/usr/bin/touch", str(self.work / "forbidden-write")],
                "network": [
                    "/usr/bin/curl",
                    "--silent",
                    "--show-error",
                    "--connect-timeout",
                    "2",
                    network_url,
                ],
            }
            for label, command in commands.items():
                receipt[label] = server.rpc(
                    "command/exec",
                    {
                        "command": command,
                        "cwd": str(self.work),
                        "permissionProfile": "worldstream_swarm_read",
                        "timeoutMs": 4000,
                        "outputBytesCap": 3000,
                    },
                )
            return receipt
        finally:
            server.close()
            network.shutdown()
            network.server_close()
            inside.unlink(missing_ok=True)
            outside.unlink(missing_ok=True)

    def turn(
        self,
        label: str,
        payload: dict,
        session: str | None = None,
        schema: dict | None = None,
    ) -> dict:
        persist(self.root / f"{label}-prompt.json", payload)
        transcript, observation = guarded_turn(
            self.args,
            self.work,
            self.env,
            self.executable_digest,
            payload,
            session,
            permission_profile="worldstream_swarm_read",
            output_schema=schema,
        )
        receipt = {
            **self.identities,
            "transcript": transcript,
            "observation": observation,
        }
        persist(self.root / f"{label}.json", receipt)
        return receipt

    def cancellation(self, label: str, owner_loss: bool) -> dict:
        request = {
            "model": self.args.model,
            "effort": self.args.effort,
            "cwd": str(self.work),
            "resource_policy": "read_only",
            "session": {"mode": "fresh", "requested_id": None},
            "permission_profile": "worldstream_swarm_read",
            "prompt": 'Bounded native cancellation probe. Do not use tools. Return {"probe":"cancel"}.',
        }
        launch = {
            "program": str(self.args.codex),
            "expected_executable_digest": self.executable_digest,
            "arguments": ["app-server", "--stdio", "--strict-config"],
            "working_area": str(self.work),
            "stdin": json.dumps(request),
            "codex_app_server": True,
            "clear_environment": False,
            "environment": {},
        }
        reader, writer = os.pipe()
        owner = None
        if owner_loss:
            owner_code = "import os,sys; fd=int(sys.argv[1]); data=sys.stdin.buffer.readline(); os.write(fd,data); sys.stdin.buffer.readline(); os._exit(0)"
            owner = subprocess.Popen(
                [sys.executable, "-c", owner_code, str(writer)],
                stdin=subprocess.PIPE,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                pass_fds=(writer,),
                env=self.env,
            )
        guard = subprocess.Popen(
            [str(self.args.guard)],
            stdin=reader,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            cwd=self.work,
            env=self.env,
            start_new_session=True,
        )
        os.close(reader)
        before = {}
        try:
            encoded = (json.dumps(launch) + "\n").encode()
            if owner:
                os.close(writer)
                writer = None
                owner.stdin.write(encoded)
                owner.stdin.flush()
            else:
                os.write(writer, encoded)
            received = queue.Queue()

            def ready():
                received.put(guard.stdout.readline())

            threading.Thread(target=ready, daemon=True).start()
            try:
                ready_line = received.get(timeout=30)
            except queue.Empty as error:
                raise RuntimeError(
                    "native guard ownership handshake exceeded 30 seconds"
                ) from error
            if ready_line != READY:
                raise RuntimeError("native guard did not acknowledge ownership")
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                before = process_snapshot()
                children = {
                    pid for pid, (parent, _, _) in before.items() if parent == guard.pid
                }
                if children:
                    break
                time.sleep(0.01)
            if not children:
                raise RuntimeError(
                    "native provider was never observed before cancellation"
                )
            owned = {guard.pid}
            while True:
                expanded = owned | {
                    pid for pid, (parent, _, _) in before.items() if parent in owned
                }
                if expanded == owned:
                    break
                owned = expanded
            groups = {before[pid][1] for pid in owned if pid in before}
            if owner:
                owner.stdin.close()
                owner.wait(timeout=5)
            else:
                os.close(writer)
                writer = None
            stdout, stderr = guard.communicate(timeout=15)
            after = process_snapshot()
            remaining = {
                pid
                for pid, (_, group, status) in after.items()
                if (pid in owned or group in groups) and not status.startswith("Z")
            }
            receipt = {
                **self.identities,
                "mode": "owner_loss" if owner_loss else "cancellation",
                "guard_pid": guard.pid,
                "guard_ready": True,
                "running_provider_observed": bool(children),
                "exit_code": guard.returncode,
                "remaining_live_process_count": len(remaining),
                "observed_before": {
                    str(pid): before[pid] for pid in owned if pid in before
                },
                "owned_groups": sorted(groups),
                "observed_after": {
                    str(pid): row
                    for pid, row in after.items()
                    if pid in owned or row[1] in groups
                },
                "stdout_sha256": hashlib.sha256(stdout).hexdigest(),
                "stderr_sha256": hashlib.sha256(stderr).hexdigest(),
            }
            if remaining or guard.returncode == 0:
                raise RuntimeError("native cancellation was not established")
            persist(self.root / f"{label}.json", receipt)
            return receipt
        finally:
            if writer is not None:
                os.close(writer)
            if owner and owner.poll() is None:
                owner.stdin.close()
                owner.wait(timeout=5)
            if guard.poll() is None:
                guard.wait(timeout=15)

    def qualify(self) -> Path:
        isolation = self.isolation()
        refs = {"isolation": persist(self.root / "isolation.json", isolation)}
        schema = {
            "type": "object",
            "properties": {"probe": {"type": "string"}},
            "required": ["probe"],
            "additionalProperties": False,
        }
        fresh = self.turn(
            "fresh",
            {
                "instructions": 'Use no tools. Return {"probe":"fresh"}. This is a native subscription/configuration probe.'
            },
            schema=schema,
        )
        self.turn(
            "resumed",
            {"instructions": 'Use no tools. Return {"probe":"resumed"}.'},
            fresh["transcript"]["configuration"]["thread_id"],
            schema,
        )
        self.cancellation("cancellation", False)
        self.cancellation("owner-loss", True)
        for key, label in [
            ("fresh", "fresh"),
            ("resumed", "resumed"),
            ("cancellation", "cancellation"),
            ("owner_loss", "owner-loss"),
        ]:
            refs[key] = {
                "path": label + ".json",
                "sha256": digest(self.root / (label + ".json")),
            }
        evidence = {
            "schema": "worldstream/codex-native-local-admission@1",
            "executable": str(self.args.codex),
            "executable_sha256": self.identities["executable_sha256"],
            "guard": str(self.args.guard),
            "guard_sha256": self.identities["guard_sha256"],
            "model": self.args.model,
            "effort": self.args.effort,
            "profile": {
                "profile_root": str(self.profile),
                "config_sha256": self.identities["configuration_sha256"],
                "working_area": str(self.work),
                "expires_at": int(time.time()) + 3600,
                "guard_sha256": self.identities["guard_sha256"],
            },
            **refs,
        }
        path = self.root / "local-admission.json"
        persist(path, evidence)
        return path

    def close(self):
        shutil.rmtree(self.profile)

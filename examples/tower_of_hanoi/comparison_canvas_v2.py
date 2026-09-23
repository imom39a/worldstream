"""Serve the second two-room Hanoi comparison: a model judged by JEV.

Both Rooms run the exact same cheap, fast OpenRouter model. The left Room runs
the model alone; the right Room runs the same model plus a JEV verifier that
scores every proposal before submission and allows one bounded repair turn.
Both Rooms share one seed and one epoch barrier. This adapter reuses the
retained ``comparison_canvas`` prototype without changing it.
"""

from __future__ import annotations

import argparse
import http.client
import json
import os
import queue
import secrets
import subprocess
import tempfile
import threading
import time
import urllib.parse
import urllib.request
from http import HTTPStatus
from http.server import ThreadingHTTPServer
from pathlib import Path

from . import comparison_canvas as base

JUDGE_ENGINE = "jev"
JUDGE_MODEL = "jev-latest"
JUDGE_THRESHOLD = 0.5
DEFAULT_SOLVERS_V2 = 1
CHEAP_MODELS_LIMIT = 30
# Cheap/fast ceiling in USD per million tokens for the prompt side. Completion
# pricing is allowed to be a little higher because output tokens are shorter.
CHEAP_PROMPT_CEILING_USD_PER_MILLION = 1.0
CHEAP_COMPLETION_CEILING_USD_PER_MILLION = 3.0
# Known fast, instruction-following models at or below the prompt ceiling. They
# are surfaced first so the dropdown does not lead with the cheapest-but-slowest
# option; anything not present in the live catalog is simply skipped.
PREFERRED_MODELS = (
    "openai/gpt-oss-20b",
    "mistralai/mistral-small-24b-instruct-2501",
    "mistralai/mistral-nemo",
    "inclusionai/ling-3.0-flash",
    "~deepseek/deepseek-v4-flash-latest",
    "deepseek/deepseek-v4-flash-0731",
    "amazon/nova-micro-v1",
    "meta-llama/llama-3.1-8b-instruct",
    "qwen/qwen3-30b-a3b-instruct-2507",
    "qwen/qwen3-32b",
    "z-ai/glm-4.7-flash",
    "microsoft/phi-4",
    "nvidia/nemotron-3.5-lightning",
    "meta-llama/llama-3.3-70b-instruct",
    "qwen/qwen-2.5-7b-instruct",
    "qwen/qwen3-14b",
)
# Tried in order when the selected model keeps failing. Both Rooms use the same
# chain so the comparison stays fair.
MODEL_FALLBACK_CHAIN = (
    "mistralai/mistral-small-24b-instruct-2501",
    "inclusionai/ling-3.0-flash",
    "meta-llama/llama-3.1-8b-instruct",
)
CATALOG_CACHE_SECONDS = 300.0
FALLBACK_MODELS = (
    {"id": "openai/gpt-5.6-luna", "label": "Luna · fallback"},
    {"id": "openai/gpt-5.6-sol", "label": "Sol · fallback"},
)

_CATALOG_LOCK = threading.Lock()
_CATALOG_CACHE: dict[str, object] = {"at": 0.0, "models": []}


def _openrouter_key() -> str | None:
    for name in ("OPENROUTER_API_KEY", "OPENROUTER_KEY", "openouterkey"):
        value = os.environ.get(name)
        if value:
            return value
    return None


def _price_per_million(pricing: object, field: str) -> float | None:
    if not isinstance(pricing, dict):
        return None
    raw = pricing.get(field)
    if isinstance(raw, bool):
        return None
    try:
        value = float(raw)
    except (TypeError, ValueError):
        return None
    if value < 0:
        return None
    return value * 1_000_000


def _live_cheap_models() -> list[dict[str, str]]:
    """List the cheapest OpenRouter models, newest prices first."""
    key = _openrouter_key()
    request = urllib.request.Request(
        "https://openrouter.ai/api/v1/models",
        headers={
            "Authorization": f"Bearer {key}" if key else "",
            "X-Title": "WorldStream Community Hanoi",
        },
    )
    with urllib.request.urlopen(request, timeout=15) as response:
        document = json.loads(response.read())
    entries = document.get("data") if isinstance(document, dict) else None
    if not isinstance(entries, list):
        return []
    scored: list[tuple[float, str, str]] = []
    for entry in entries:
        if not isinstance(entry, dict):
            continue
        model_id = entry.get("id")
        if not isinstance(model_id, str) or not model_id:
            continue
        # Free endpoints are frequently excluded by account guardrails or
        # zero-data-retention policy, which surfaces as a 404 at request time.
        if ":free" in model_id:
            continue
        architecture = entry.get("architecture")
        if not isinstance(architecture, dict) or architecture.get("modality") != "text->text":
            continue
        prompt = _price_per_million(entry.get("pricing"), "prompt")
        completion = _price_per_million(entry.get("pricing"), "completion")
        if prompt is None or completion is None:
            continue
        if prompt > CHEAP_PROMPT_CEILING_USD_PER_MILLION:
            continue
        if completion > CHEAP_COMPLETION_CEILING_USD_PER_MILLION:
            continue
        name = entry.get("name") if isinstance(entry.get("name"), str) else model_id
        scored.append((prompt + completion, model_id, name))
    scored.sort(key=lambda item: (item[0], item[1]))
    by_id = {model_id: (total, name) for total, model_id, name in scored}
    ordered: list[tuple[float, str, str, bool]] = []
    for preferred in PREFERRED_MODELS:
        found = by_id.pop(preferred, None)
        if found is not None:
            ordered.append((found[0], preferred, found[1], True))
    for total, model_id, name in sorted(
        ((total, model_id, name) for model_id, (total, name) in by_id.items()),
        key=lambda item: (item[0], item[1]),
    ):
        ordered.append((total, model_id, name, False))
    return [
        {
            "id": model_id,
            "label": f"{'★ ' if preferred else ''}{name} · ${total:.2f}/M",
        }
        for total, model_id, name, preferred in ordered[:CHEAP_MODELS_LIMIT]
    ]


def cheap_models() -> list[dict[str, str]]:
    now = time.monotonic()
    with _CATALOG_LOCK:
        cached = _CATALOG_CACHE.get("models")
        if isinstance(cached, list) and cached and now - float(_CATALOG_CACHE["at"]) < CATALOG_CACHE_SECONDS:
            return [dict(item) for item in cached]
    try:
        models = _live_cheap_models()
    except (OSError, ValueError, TimeoutError):
        models = []
    if not models:
        models = [dict(item) for item in FALLBACK_MODELS]
    with _CATALOG_LOCK:
        _CATALOG_CACHE["at"] = now
        _CATALOG_CACHE["models"] = [dict(item) for item in models]
    return models


class ComparisonBroadcastV2(base.ComparisonBroadcast):
    """SSE fan-out that re-syncs a viewer whose cursor predates this process.

    A browser that was connected before a server restart reconnects with a
    ``Last-Event-ID`` from the previous process. This process has no history for
    it, so replaying deltas would send nothing and the UI would keep a stale
    "running" state with a disabled Start button. Sending the current state
    instead lets the viewer recover without a manual reload.
    """

    def subscribe(self, after: str | None) -> queue.Queue[tuple[int, str]] | None:
        with self.lock:
            if len(self.subscribers) >= base.MAX_CLIENTS:
                return None
            try:
                cursor = int(after) if after is not None else None
            except ValueError:
                cursor = None
            subscriber: queue.Queue[tuple[int, str]] = queue.Queue(maxsize=base.MAX_HISTORY)
            stale_cursor = (
                cursor is None
                or not self.history
                or cursor > self.cursor
                or cursor < self.history[0][0] - 1
            )
            replay = (
                [self._encoded(self.latest)]
                if stale_cursor
                else [item for item in self.history if item[0] > cursor]
            )
            for item in replay:
                subscriber.put_nowait(item)
            self.subscribers.add(subscriber)
            return subscriber


class ComparisonRunV2(base.ComparisonRun):
    def start(self, config: dict[str, object]) -> dict[str, object]:
        with self.lock:
            if self.processes:
                raise ValueError("comparison_already_running")
            self.generation += 1
            generation = self.generation
            disks = base._bounded_int(config.get("disks", 3), "disks", 1, 10)
            duration = base._bounded_int(
                config.get("duration_seconds", base.DEFAULT_DURATION_SECONDS),
                "duration_seconds",
                1,
                3600,
            )
            solver_count = base._bounded_int(
                config.get("solver_count", DEFAULT_SOLVERS_V2),
                "solver_count",
                1,
                base.MAX_SOLVERS,
            )
            model = base._safe(config.get("model", "openai/gpt-5.6-luna"), "model")
            threshold_value = config.get("judge_threshold", JUDGE_THRESHOLD)
            if isinstance(threshold_value, bool) or not isinstance(threshold_value, (int, float)):
                raise TypeError("judge_threshold_invalid")
            threshold = float(threshold_value)
            if not 0.0 < threshold <= 1.0:
                raise ValueError("judge_threshold_invalid")
            warmup = base._bounded_int(
                config.get("warmup_seconds", base.DEFAULT_WARMUP_SECONDS),
                "warmup_seconds",
                5,
                180,
            )
            randomize_board = config.get("randomize_board", False)
            if not isinstance(randomize_board, bool):
                raise TypeError("randomize_board_invalid")
            board_seed_value = config.get("board_seed")
            board_seed = (
                None
                if board_seed_value is None
                else base._bounded_int(board_seed_value, "board_seed", 0, 2**63 - 1)
            )
            if randomize_board and board_seed is None:
                board_seed = secrets.randbits(63)
            seat_names = base._participant_seat_names(solver_count * 2)
            left_seats = seat_names[:solver_count]
            right_seats = seat_names[solver_count : solver_count * 2]
            self.warmup_seconds = warmup
            self.config = {
                "disks": disks,
                "duration_seconds": duration,
                "model": model,
                "solver_count": solver_count,
                "judge_engine": JUDGE_ENGINE,
                "judge_model": JUDGE_MODEL,
                "judge_threshold": threshold,
                "randomize_board": randomize_board,
                "board_seed": board_seed,
            }
            self.started_at_ms = None
            self.deadline_at_ms = None
            self.completed_at_ms = {"left": None, "right": None}
            self.finished_at_ms = None
            target_dir = self.root / "target"
            target_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
            self.comparison_dir = Path(
                tempfile.mkdtemp(prefix="worldstream-hanoi-comparison-v2-", dir=target_dir)
            )
            self.start_file = self.comparison_dir / "release.json"
            self.sides = {
                "left": {
                    "status": "starting",
                    "solver_count": solver_count,
                    "model": model,
                    "judge_engine": "none",
                    "seats": list(left_seats),
                },
                "right": {
                    "status": "starting",
                    "solver_count": solver_count,
                    "model": model,
                    "judge_engine": JUDGE_ENGINE,
                    "judge_model": JUDGE_MODEL,
                    "seats": list(right_seats),
                },
            }
            self._reset_event_logs()
            base._load_dotenv(self.root)
            python = self.root / "sdk/python/.venv/bin/python"
            board_arguments = ["--randomize-board"]
            if board_seed is not None:
                board_arguments.extend(["--board-seed", str(board_seed)])
            if not randomize_board:
                board_arguments = []
            common = [
                str(python),
                "-m",
                "examples.tower_of_hanoi.local_harness",
                "--canvas-demo",
                "--canvas-demo-hold-seconds",
                "0",
                "--disks",
                str(disks),
                "--gameplay-seconds",
                str(duration),
                "--start-file",
                str(self.start_file),
                "--solver-effort",
                "low",
                "--context-mode",
                "stream",
                "--timeout-seconds",
                str(base.SETUP_TIMEOUT_SECONDS),
                "--fast-start",
                "--model-file",
                str(self.comparison_dir / "active-model.json"),
                *board_arguments,
            ]
            fallback_arguments: list[str] = []
            for fallback in MODEL_FALLBACK_CHAIN:
                if fallback != model:
                    fallback_arguments.extend(["--fallback-model", fallback])
            commands = {
                "left": common
                + [
                    "--solver-engine",
                    "openrouter",
                    "--solver-model",
                    model,
                    "--judge-engine",
                    "none",
                    *fallback_arguments,
                ],
                "right": common
                + [
                    "--solver-engine",
                    "openrouter",
                    "--solver-model",
                    model,
                    "--judge-engine",
                    JUDGE_ENGINE,
                    "--judge-model",
                    JUDGE_MODEL,
                    "--judge-threshold",
                    str(threshold),
                    *fallback_arguments,
                ],
            }
            for side, seats in (("left", left_seats), ("right", right_seats)):
                command = list(commands[side]) + ["--observer-seat", f"observer-{side}"]
                for seat in seats:
                    command.extend(["--solver-seat", seat])
                process = subprocess.Popen(
                    command,
                    cwd=self.root,
                    env=dict(os.environ),
                    text=True,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    start_new_session=True,
                    bufsize=1,
                )
                self.processes[side] = process
                thread = threading.Thread(
                    target=self._read_child, args=(side, process, generation), daemon=True
                )
                self.threads.append(thread)
                thread.start()
            self._publish("preparing")
            timer_thread = threading.Thread(target=self._timer_loop, daemon=True)
            self.threads.append(timer_thread)
            timer_thread.start()
            return {"status": "preparing"}


class ComparisonServerV2(base.ComparisonServer):
    def __init__(self, address: tuple[str, int], root: Path) -> None:
        ThreadingHTTPServer.__init__(self, address, ComparisonHandlerV2)
        self.root = root
        self.broadcast = ComparisonBroadcastV2()
        self.run = ComparisonRunV2(root, self.broadcast)
        # Host the retained v1 comparison in-process on an ephemeral port and
        # proxy it under /v1, so both experiments share one recording URL.
        self.v1 = base.ComparisonServer(("127.0.0.1", 0), root)
        threading.Thread(
            target=self.v1.serve_forever,
            kwargs={"poll_interval": 0.2},
            daemon=True,
            name="worldstream-hanoi-v1",
        ).start()


class ComparisonHandlerV2(base.ComparisonHandler):
    def _v1_target(self) -> str | None:
        """Return the v1 server path for a /v1 request, or None for the prefix."""
        if self.path.startswith("/v1/"):
            return self.path[3:]
        if self.path.startswith("/v1?"):
            return "/" + self.path[3:]
        return None

    def _proxy_v1(self, target: str, method: str) -> None:
        server: ComparisonServerV2 = self.server  # type: ignore[assignment]
        length = int(self.headers.get("Content-Length", "0") or "0")
        body = self.rfile.read(length) if method == "POST" and length > 0 else None
        headers = {
            key: value
            for key, value in self.headers.items()
            if key.lower() not in {"host", "connection"}
        }
        if body is not None:
            headers["Content-Length"] = str(len(body))
        connection = http.client.HTTPConnection(
            "127.0.0.1", server.v1.server_port, timeout=120
        )
        try:
            connection.request(method, target, body=body, headers=headers)
            response = connection.getresponse()
            self.send_response(response.status)
            for key, value in response.getheaders():
                if key.lower() in {"connection", "keep-alive", "transfer-encoding"}:
                    continue
                self.send_header(key, value)
            self.send_header("Connection", "close")
            self.end_headers()
            # read1 returns as soon as bytes are available, so an SSE stream is
            # forwarded live instead of waiting for a full buffer.
            read_chunk = getattr(response, "read1", response.read)
            while True:
                chunk = read_chunk(8192)
                if not chunk:
                    break
                self.wfile.write(chunk)
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError, OSError, http.client.HTTPException):
            return
        finally:
            connection.close()

    def do_GET(self) -> None:
        server: ComparisonServerV2 = self.server  # type: ignore[assignment]
        if self.path == "/v1":
            self.send_response(HTTPStatus.MOVED_PERMANENTLY)
            self.send_header("Location", "/v1/")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        target = self._v1_target()
        if target is not None:
            self._proxy_v1(target, "GET")
            return
        request = urllib.parse.urlsplit(self.path)
        path = request.path
        if path == "/":
            try:
                body = (
                    server.root / "web/demos/community-hanoi-comparison-v2.html"
                ).read_bytes()
            except OSError:
                self._send(HTTPStatus.SERVICE_UNAVAILABLE, b"demo unavailable\n", "text/plain")
                return
            self._send(HTTPStatus.OK, body, "text/html; charset=utf-8")
            return
        if path == "/config":
            body = json.dumps(
                {
                    "models": {"openrouter": cheap_models()},
                    "defaults": {
                        "solver_count": DEFAULT_SOLVERS_V2,
                        "duration_seconds": base.DEFAULT_DURATION_SECONDS,
                        "disks": 3,
                        "judge_model": JUDGE_MODEL,
                        "judge_threshold": JUDGE_THRESHOLD,
                    },
                },
                separators=(",", ":"),
            ).encode()
            self._send(HTTPStatus.OK, body, "application/json")
            return
        super().do_GET()

    def do_POST(self) -> None:
        target = self._v1_target()
        if target is not None:
            self._proxy_v1(target, "POST")
            return
        super().do_POST()


def _arguments() -> argparse.Namespace:
    root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=root)
    parser.add_argument("--port", type=int, default=5291)
    return parser.parse_args()


def main() -> int:
    arguments = _arguments()
    if not 1024 <= arguments.port <= 65535:
        raise SystemExit("port must be between 1024 and 65535")
    base._load_dotenv(arguments.root.resolve())
    server = ComparisonServerV2(("127.0.0.1", arguments.port), arguments.root.resolve())
    print(json.dumps({"status": "ready", "url": f"http://127.0.0.1:{server.server_port}/"}), flush=True)
    try:
        server.serve_forever(poll_interval=0.2)
    except KeyboardInterrupt:
        return 0
    finally:
        server.run.stop()
        server.shutdown()
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

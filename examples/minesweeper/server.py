"""Serve and orchestrate the standalone LLM-vs-JEV Minesweeper comparison.

Two independent Rooms play one shared Minesweeper board. The left Room is an
OpenRouter LLM; the right Room is a TypeSafe JEV participant. Each Room asks its
engine for one cell per round and applies it to its own copy of the same seeded
board. There is no WorldStream Runtime, Pack, or judge: JEV is a direct player,
which is fair here because a Minesweeper decision needs only the current board.

Run it with the repository Python environment:

    sdk/python/.venv/bin/python -m examples.minesweeper.server

Open http://127.0.0.1:5391/. Provider keys stay in this process.
"""

from __future__ import annotations

import argparse
import json
import os
import random
import threading
import time
import urllib.parse
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from .agents import (
    ProviderError,
    jev_choose,
    jev_key,
    live_cheap_models,
    llm_choose,
    openrouter_key,
)
from .game import MinesweeperGame, parse_cell_label

MAX_BODY_BYTES = 16_384
MAX_RETAINED_EVENTS = 20_000
FEED_EVENTS = 100
DEFAULT_PORT = 5391
DEFAULT_DURATION_SECONDS = 300
DEFAULT_WIDTH = 8
DEFAULT_HEIGHT = 8
DEFAULT_MINES = 10
MAX_DIMENSION = 500
MAX_MINES = 250_000
DEFAULT_CANDIDATE_LIMIT = 20
MAX_CANDIDATE_LIMIT = 48
MAX_ATTEMPTS = 3
RETRY_SECONDS = 1.5
RETRY_MAX_SECONDS = 8.0
# SSE heartbeat. The client computes the countdown locally, so frames are only
# needed when state changes; a long heartbeat keeps large boards cheap.
SSE_HEARTBEAT_SECONDS = 5.0
ENGINES = ("openrouter", "jev", "none")

JEV_CATALOG = [
    {"id": "jev-latest", "label": "JEV · latest alias"},
    {"id": "jev-1.13.0", "label": "JEV · pinned 1.13.0"},
]
FALLBACK_OPENROUTER_CATALOG = [
    {"id": "mistralai/mistral-small-24b-instruct-2501", "label": "Mistral Small 24B"},
    {"id": "inclusionai/ling-3.0-flash", "label": "Ling 3.0 Flash"},
    {"id": "meta-llama/llama-3.1-8b-instruct", "label": "Llama 3.1 8B"},
]
TERMINAL_SIDE_STATUSES = {"won", "lost", "time_limit", "ended", "error", "disabled"}
SIDE_KEYS = ("left", "right")


def now_ms() -> int:
    return int(time.time() * 1000)


def _load_dotenv(root: Path) -> None:
    """Make local development keys available without exposing them."""
    try:
        lines = (root / ".env").read_text(encoding="utf-8").splitlines()
    except OSError:
        return
    for line in lines:
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or "=" not in stripped:
            continue
        key, value = stripped.split("=", 1)
        key, value = key.strip(), value.strip().strip("'\"")
        if key and value and key not in os.environ:
            os.environ[key] = value
        if key == "openouterkey" and value and "OPENROUTER_API_KEY" not in os.environ:
            os.environ["OPENROUTER_API_KEY"] = value


def _safe(value: object, name: str) -> str:
    if not isinstance(value, str) or not value or any(c.isspace() for c in value):
        raise ValueError(f"{name}_invalid")
    return value


def _bounded_int(value: object, name: str, minimum: int, maximum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not minimum <= value <= maximum:
        raise ValueError(f"{name}_invalid")
    return value


class Side:
    """One Room: an engine, its own board, and its sanitized public feed."""

    def __init__(self, key: str, engine: str, model: str) -> None:
        self.key = key
        self.engine = engine
        self.model = model
        self.status = "idle"
        self.game: MinesweeperGame | None = None
        self.events: list[dict[str, object]] = []
        self.decisions: list[dict[str, object]] = []
        self.paused = False
        self.stopped = False
        self.completed_at_ms: int | None = None
        self.last_error: str | None = None

    def reset(self, engine: str, model: str, game: MinesweeperGame) -> None:
        self.engine = engine
        self.model = model
        self.status = "starting"
        self.game = game
        self.events = []
        self.decisions = []
        self.paused = False
        self.stopped = False
        self.completed_at_ms = None
        self.last_error = None

    def reset_disabled(self) -> None:
        self.engine = "none"
        self.model = ""
        self.status = "disabled"
        self.game = None
        self.events = []
        self.decisions = []
        self.paused = False
        self.stopped = True
        self.completed_at_ms = None
        self.last_error = None


class Comparison:
    """Owns the shared clock, both Rooms, and the SSE fan-out."""

    def __init__(self, html_path: Path, v2_html_path: Path | None = None) -> None:
        self.html_path = html_path
        self.v2_html_path = v2_html_path
        self.lock = threading.RLock()
        self.changed = threading.Condition(self.lock)
        self.version = 0
        self.config: dict[str, object] | None = None
        self.timer: dict[str, object] = {
            "status": "idle",
            "started_at_ms": None,
            "deadline_at_ms": None,
            "elapsed_ms": None,
            "frozen_elapsed_ms": None,
            "frozen_by": None,
        }
        self.sides = {key: Side(key, "openrouter", "") for key in SIDE_KEYS}
        self.stop_event = threading.Event()
        self.run_id = 0
        self.threads: list[threading.Thread] = []

    # -- lifecycle ----------------------------------------------------------

    def start(self, config: dict[str, object]) -> None:
        self.stop()
        run_id = self.run_id + 1
        seed = config["seed"]
        engines = {
            "left": (config["left_engine"], config["model"]),
            "right": (config["right_engine"], config["jev_model"]),
        }
        with self.lock:
            self.run_id = run_id
            self.config = config
            self.stop_event = threading.Event()
            for key, (engine, model) in engines.items():
                if engine == "none":
                    self.sides[key].reset_disabled()
                    continue
                game = MinesweeperGame(
                    int(config["width"]), int(config["height"]), int(config["mines"]), int(seed)
                )
                self.sides[key].reset(str(engine), str(model), game)
            started = now_ms()
            self.timer = {
                "status": "running",
                "started_at_ms": started,
                "deadline_at_ms": started + int(config["duration_seconds"]) * 1000,
                "elapsed_ms": None,
                "frozen_elapsed_ms": None,
                "frozen_by": None,
            }
            self._touch()
        self.threads = [
            threading.Thread(
                target=self._run_side,
                args=(self.sides[key], config, self.stop_event, run_id),
                name=f"minesweeper-{key}",
                daemon=True,
            )
            for key in SIDE_KEYS
            if self.sides[key].engine != "none"
        ]
        for thread in self.threads:
            thread.start()

    def stop(self) -> None:
        with self.lock:
            had_run = self.timer["status"] != "idle" or self.run_id > 0
            self.stop_event.set()
            for side in self.sides.values():
                side.stopped = True
                if side.status not in TERMINAL_SIDE_STATUSES:
                    side.status = "ended"
            if had_run:
                self.timer["status"] = "idle"
                self.timer["elapsed_ms"] = None
                self.timer["frozen_elapsed_ms"] = None
                self.timer["frozen_by"] = None
            self._touch()
        for thread in self.threads:
            thread.join(timeout=2.0)
        self.threads = []

    def pause(self, key: str, paused: bool) -> None:
        with self.lock:
            side = self.sides[key]
            if side.status in TERMINAL_SIDE_STATUSES:
                return
            side.paused = paused
            side.status = "paused" if paused else "live"
            self._touch()

    def end_side(self, key: str) -> None:
        with self.lock:
            side = self.sides[key]
            side.stopped = True
            if side.status not in TERMINAL_SIDE_STATUSES:
                side.status = "ended"
            self._touch()

    # -- workers ------------------------------------------------------------

    def _run_side(
        self, side: Side, config: dict[str, object], stop_event: threading.Event, run_id: int
    ) -> None:
        game = side.game
        if game is None:
            return
        rng = random.Random(int(config["seed"]) ^ (1 if side.key == "left" else 2))
        deadline_ms = int(self.timer["deadline_at_ms"])  # type: ignore[arg-type]
        candidate_limit = int(config.get("candidate_limit", DEFAULT_CANDIDATE_LIMIT))
        include_grid = config.get("include_grid")
        with self.lock:
            side.status = "live"
            self._touch()
        self._append_event(
            side,
            {
                "kind": "game_started",
                "actor": side.engine,
                "message": f"{side.engine} playing a {game.width}x{game.height} board with {game.mines} mines",
            },
            run_id,
        )
        round_number = 0
        while True:
            if stop_event.is_set() or side.stopped:
                self._finish(side, run_id, "ended", game)
                return
            if game.is_terminal:
                self._finish(side, run_id, game.phase, game)
                return
            if now_ms() >= deadline_ms:
                game.phase = "time_limit"
                self._append_event(side, {"kind": "time_limit", "actor": side.engine}, run_id)
                self._finish(side, run_id, "time_limit", game)
                return
            if side.paused:
                time.sleep(0.2)
                continue
            round_number += 1
            candidates = game.offered_candidates(candidate_limit)
            if not candidates:
                self._finish(side, run_id, game.phase, game)
                return
            candidate_labels = [str(candidate["cell"]) for candidate in candidates]
            cell, metadata, error = self._decide(
                side, game, config, round_number, candidate_limit, include_grid
            )
            fallback = False
            parsed = parse_cell_label(cell)
            if parsed is None or cell not in candidate_labels or not game.in_bounds(*parsed):
                fallback = True
                reason = error or "choice_not_offered"
                cell = rng.choice(candidate_labels)
                parsed = parse_cell_label(cell)
                self._append_event(
                    side,
                    {
                        "kind": "fallback",
                        "actor": side.engine,
                        "reason": reason,
                        "message": f"no usable choice ({reason}); revealed {cell} instead",
                    },
                    run_id,
                )
            if parsed is None:
                self._finish(side, run_id, game.phase, game)
                return
            row, col = parsed
            result = game.reveal(row, col)
            if result == "invalid":
                fallback = True
                cell = rng.choice(candidate_labels)
                parsed = parse_cell_label(cell)
                if parsed is None:
                    continue
                row, col = parsed
                result = game.reveal(row, col)
            self._record_decision(
                side,
                {
                    "kind": "participant_decision",
                    "actor": side.engine,
                    "round": round_number,
                    "selected": {
                        "cell": cell,
                        "row": row,
                        "col": col,
                        "label": cell,
                        "action_type": "reveal",
                    },
                    "engine": side.engine,
                    "model": side.model,
                    "confidence": metadata.get("confidence"),
                    "probabilities": metadata.get("probabilities"),
                    "reason": metadata.get("reason"),
                    "rationale": metadata.get("rationale"),
                    "provider_model": metadata.get("provider_model"),
                    "fallback": fallback,
                },
                run_id,
            )
            self._append_event(
                side,
                {
                    "kind": "cell_revealed",
                    "actor": side.engine,
                    "round": round_number,
                    "cell": cell,
                    "result": result,
                    "revealed": len(game.revealed),
                    "message": f"{cell} · {result} · {len(game.revealed)}/{game.safe_total} safe cells",
                },
                run_id,
            )
            if game.is_terminal:
                self._finish(side, run_id, game.phase, game)
                return

    def _decide(
        self,
        side: Side,
        game: MinesweeperGame,
        config: dict[str, object],
        round_number: int,
        candidate_limit: int = DEFAULT_CANDIDATE_LIMIT,
        include_grid: object = None,
    ) -> tuple[str | None, dict[str, object], str | None]:
        grid_flag = include_grid if isinstance(include_grid, bool) else None
        state = game.agent_state(round_number, candidate_limit, grid_flag)
        last_error: str | None = None
        for attempt in range(1, MAX_ATTEMPTS + 1):
            if side.stopped or self.stop_event.is_set():
                return None, {}, "stopped"
            try:
                if side.engine == "jev":
                    key = jev_key()
                    if not key:
                        raise ProviderError("jev_key_missing")
                    cell, metadata = jev_choose(
                        state, game.choice_criteria(candidate_limit), side.model, key
                    )
                else:
                    key = openrouter_key()
                    if not key:
                        raise ProviderError("openrouter_key_missing")
                    cell, metadata = llm_choose(state, side.model, key)
                return cell, metadata, None
            except ProviderError as error:
                last_error = str(error)
                self._append_event(
                    side,
                    {
                        "kind": "provider_error",
                        "actor": side.engine,
                        "attempt": attempt,
                        "code": last_error,
                        "message": f"attempt {attempt}/{MAX_ATTEMPTS} failed: {last_error}",
                    },
                    self.run_id,
                )
                if attempt < MAX_ATTEMPTS:
                    time.sleep(min(RETRY_MAX_SECONDS, RETRY_SECONDS * attempt))
        return None, {}, last_error

    def _finish(self, side: Side, run_id: int, phase: str, game: MinesweeperGame) -> None:
        status = phase if phase in TERMINAL_SIDE_STATUSES else "ended"
        with self.lock:
            if run_id != self.run_id:
                return
            if side.completed_at_ms is None:
                side.completed_at_ms = now_ms()
            side.status = status
            if status == "won":
                side.last_error = None
            # The shared clock records the first room to finish; the other room
            # keeps playing for its own result, but the official time is frozen.
            self._freeze_timer_locked(run_id, side.key)
            self._touch()
        self._append_event(
            side,
            {
                "kind": "game_over",
                "actor": side.engine,
                "result": status,
                "message": f"{status} · {game.moves} reveals · {len(game.revealed)}/{game.safe_total} safe cells",
            },
            run_id,
        )
        self._maybe_complete_timer(run_id)

    def _freeze_timer_locked(self, run_id: int, side_key: str) -> None:
        if run_id != self.run_id or self.timer.get("status") != "running":
            return
        if self.timer.get("frozen_elapsed_ms") is not None:
            return
        started = self.timer.get("started_at_ms")
        if not isinstance(started, int):
            return
        self.timer["frozen_elapsed_ms"] = now_ms() - started
        self.timer["frozen_by"] = side_key

    def _maybe_complete_timer(self, run_id: int) -> None:
        with self.lock:
            if run_id != self.run_id or self.timer["status"] != "running":
                return
            if not all(side.status in TERMINAL_SIDE_STATUSES for side in self.sides.values()):
                return
            started = self.timer.get("started_at_ms")
            frozen = self.timer.get("frozen_elapsed_ms")
            elapsed = (
                frozen
                if isinstance(frozen, int)
                else (now_ms() - int(started) if isinstance(started, int) else None)
            )
            self.timer = {
                "status": "complete",
                "started_at_ms": started,
                "deadline_at_ms": self.timer.get("deadline_at_ms"),
                "elapsed_ms": elapsed,
                "frozen_elapsed_ms": elapsed,
                "frozen_by": self.timer.get("frozen_by"),
            }
            self._touch()

    # -- snapshots ----------------------------------------------------------

    def snapshot(self) -> dict[str, object]:
        with self.lock:
            return {
                "type": "comparison",
                "config": self.config,
                "timer": dict(self.timer),
                "sides": {key: self._side_snapshot(side) for key, side in self.sides.items()},
            }

    def _side_snapshot(self, side: Side) -> dict[str, object]:
        elapsed_ms = None
        started = self.timer.get("started_at_ms")
        if isinstance(started, int):
            end = side.completed_at_ms if isinstance(side.completed_at_ms, int) else now_ms()
            elapsed_ms = max(0, end - started)
        return {
            "status": side.status,
            "engine": side.engine,
            "model": side.model,
            "elapsed_ms": elapsed_ms,
            "event_total": len(side.events),
            "projection": side.game.projection() if side.game else None,
            "events": side.events[-FEED_EVENTS:],
            "decisions": side.decisions[-FEED_EVENTS:],
            "seats": [side.engine],
        }

    def feed(self, key: str) -> dict[str, object]:
        with self.lock:
            side = self.sides[key]
            return {"side": key, "events": list(side.events), "decisions": list(side.decisions)}

    # -- event plumbing -----------------------------------------------------

    def _append_event(self, side: Side, event: dict[str, object], run_id: int) -> None:
        with self.lock:
            if run_id != self.run_id:
                return
            event = {**event, "room_seq": len(side.events) + 1, "at_ms": now_ms()}
            side.events.append(event)
            del side.events[:-MAX_RETAINED_EVENTS]
            self._touch()

    def _record_decision(self, side: Side, decision: dict[str, object], run_id: int) -> None:
        with self.lock:
            if run_id != self.run_id:
                return
            sequence = len(side.events) + 1
            timestamp = now_ms()
            decision = {**decision, "room_seq": sequence, "at_ms": timestamp}
            side.decisions.append(decision)
            del side.decisions[:-MAX_RETAINED_EVENTS]
            # Mirror every decision into the public event feed so the feed shows
            # what each engine chose, not only the resulting reveal.
            selected = decision.get("selected")
            cell = selected.get("cell") if isinstance(selected, dict) else None
            side.events.append(
                {
                    "kind": "decision",
                    "actor": decision.get("actor"),
                    "round": decision.get("round"),
                    "cell": cell,
                    "reason": decision.get("reason"),
                    "confidence": decision.get("confidence"),
                    "fallback": decision.get("fallback"),
                    "message": f"round {decision.get('round')} chose {cell}",
                    "room_seq": sequence,
                    "at_ms": timestamp,
                }
            )
            del side.events[:-MAX_RETAINED_EVENTS]
            self._touch()

    def _touch(self) -> None:
        self.version += 1
        self.changed.notify_all()


def _catalog() -> dict[str, list[dict[str, str]]]:
    live = live_cheap_models()
    return {
        "openrouter": live or list(FALLBACK_OPENROUTER_CATALOG),
        "jev": list(JEV_CATALOG),
    }


def _validate_start(body: object) -> dict[str, object]:
    if not isinstance(body, dict):
        raise TypeError("body_invalid")
    width = _bounded_int(body.get("width", DEFAULT_WIDTH), "width", 2, MAX_DIMENSION)
    height = _bounded_int(body.get("height", DEFAULT_HEIGHT), "height", 2, MAX_DIMENSION)
    max_mines = min(MAX_MINES, width * height - 1)
    mines = _bounded_int(body.get("mines", DEFAULT_MINES), "mines", 1, max_mines)
    duration = _bounded_int(
        body.get("duration_seconds", DEFAULT_DURATION_SECONDS),
        "duration_seconds",
        5,
        3600,
    )
    left_engine = body.get("left_engine", "openrouter")
    right_engine = body.get("right_engine", "jev")
    if left_engine not in ENGINES:
        raise ValueError("left_engine_invalid")
    if right_engine not in ENGINES:
        raise ValueError("right_engine_invalid")
    if left_engine == right_engine == "none":
        raise ValueError("no_engine_enabled")
    model = body.get("model", "openai/gpt-oss-20b")
    if left_engine == "openrouter":
        model = _safe(model, "model")
    elif not isinstance(model, str) or not model:
        model = "n/a"
    jev_model = _safe(body.get("jev_model", "jev-latest"), "jev_model")
    if right_engine == "jev" and jev_model not in {item["id"] for item in JEV_CATALOG}:
        raise ValueError("jev_model_invalid")
    candidate_limit = _bounded_int(
        body.get("candidate_limit", DEFAULT_CANDIDATE_LIMIT),
        "candidate_limit",
        4,
        MAX_CANDIDATE_LIMIT,
    )
    include_grid = body.get("include_grid")
    if not isinstance(include_grid, bool):
        include_grid = None
    seed = body.get("seed")
    if isinstance(seed, bool) or not isinstance(seed, int) or not 0 <= seed <= 2**53 - 1:
        seed = random.SystemRandom().randint(0, 2**53 - 1)
    return {
        "width": width,
        "height": height,
        "mines": mines,
        "duration_seconds": duration,
        "model": model,
        "jev_model": jev_model,
        "left_engine": left_engine,
        "right_engine": right_engine,
        "candidate_limit": candidate_limit,
        "include_grid": include_grid,
        "seed": seed,
    }


class Handler(BaseHTTPRequestHandler):
    comparison: Comparison
    server_version = "MinesweeperComparison/0.1"

    def log_message(self, format: str, *args: object) -> None:
        return

    # -- responses ----------------------------------------------------------

    def _send_json(self, status: HTTPStatus, payload: dict[str, object]) -> None:
        data = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def _read_body(self) -> object:
        length = int(self.headers.get("Content-Length", "0") or "0")
        if length <= 0:
            return {}
        if length > MAX_BODY_BYTES:
            raise ValueError("body_too_large")
        raw = self.rfile.read(length)
        try:
            return json.loads(raw)
        except json.JSONDecodeError as error:
            raise ValueError("body_invalid") from error

    def _serve_html(self, path: Path | None = None) -> None:
        try:
            data = (path or self.comparison.html_path).read_bytes()
        except OSError:
            self._send_json(HTTPStatus.NOT_FOUND, {"error": "html_not_found"})
            return
        self.send_response(HTTPStatus.OK)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def _serve_sse(self) -> None:
        self.send_response(HTTPStatus.OK)
        self.send_header("Content-Type", "text/event-stream; charset=utf-8")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Connection", "keep-alive")
        self.end_headers()
        last_version = -1
        try:
            while True:
                with self.comparison.changed:
                    if self.comparison.version == last_version:
                        self.comparison.changed.wait(timeout=SSE_HEARTBEAT_SECONDS)
                    last_version = self.comparison.version
                payload = self.comparison.snapshot()
                data = json.dumps(payload, separators=(",", ":"))
                self.wfile.write(f"event: comparison\ndata: {data}\n\n".encode())
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError, OSError):
            return
        finally:
            self.close_connection = True

    # -- routes -------------------------------------------------------------

    def do_GET(self) -> None:
        path = urllib.parse.urlparse(self.path).path
        if path in ("/", "/index.html"):
            self._serve_html()
            return
        if path in ("/v2", "/v2/", "/v2/index.html"):
            self._serve_html(self.comparison.v2_html_path)
            return
        if path == "/config":
            self._send_json(
                HTTPStatus.OK,
                {
                    "models": _catalog(),
                    "defaults": {
                        "width": DEFAULT_WIDTH,
                        "height": DEFAULT_HEIGHT,
                        "mines": DEFAULT_MINES,
                        "duration_seconds": DEFAULT_DURATION_SECONDS,
                        "candidate_limit": DEFAULT_CANDIDATE_LIMIT,
                        "max_dimension": MAX_DIMENSION,
                        "max_mines": MAX_MINES,
                    },
                    "keys": {
                        "openrouter": bool(openrouter_key()),
                        "jev": bool(jev_key()),
                    },
                },
            )
            return
        if path == "/events":
            self._serve_sse()
            return
        if path == "/feed":
            query = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
            side = (query.get("side") or ["left"])[0]
            if side not in SIDE_KEYS:
                self._send_json(HTTPStatus.BAD_REQUEST, {"error": "side_invalid"})
                return
            self._send_json(HTTPStatus.OK, self.comparison.feed(side))
            return
        self._send_json(HTTPStatus.NOT_FOUND, {"error": "not_found"})

    def do_POST(self) -> None:
        path = urllib.parse.urlparse(self.path).path
        try:
            body = self._read_body()
        except ValueError as error:
            self._send_json(HTTPStatus.BAD_REQUEST, {"error": str(error)})
            return
        if path == "/start":
            try:
                config = _validate_start(body)
            except (TypeError, ValueError) as error:
                self._send_json(HTTPStatus.BAD_REQUEST, {"error": str(error)})
                return
            self.comparison.start(config)
            self._send_json(HTTPStatus.OK, {"status": "started", "config": config})
            return
        if path == "/stop":
            self.comparison.stop()
            self._send_json(HTTPStatus.OK, {"status": "stopped"})
            return
        if path in ("/pause", "/resume"):
            side = body.get("side") if isinstance(body, dict) else None
            if side not in SIDE_KEYS:
                self._send_json(HTTPStatus.BAD_REQUEST, {"error": "side_invalid"})
                return
            self.comparison.pause(side, path == "/pause")
            self._send_json(HTTPStatus.OK, {"status": path[1:], "side": side})
            return
        if path == "/end":
            side = body.get("side") if isinstance(body, dict) else None
            if side not in SIDE_KEYS:
                self._send_json(HTTPStatus.BAD_REQUEST, {"error": "side_invalid"})
                return
            self.comparison.end_side(side)
            self._send_json(HTTPStatus.OK, {"status": "ended", "side": side})
            return
        self._send_json(HTTPStatus.NOT_FOUND, {"error": "not_found"})


def _default_html_path() -> Path:
    root = Path(__file__).resolve().parents[2]
    return root / "examples" / "minesweeper" / "web" / "community-minesweeper-comparison.html"


def _default_v2_html_path() -> Path:
    root = Path(__file__).resolve().parents[2]
    return root / "examples" / "minesweeper" / "web" / "community-minesweeper-jev.html"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    parser.add_argument("--html", type=Path, default=_default_html_path())
    parser.add_argument("--v2-html", type=Path, default=_default_v2_html_path())
    arguments = parser.parse_args(argv)
    _load_dotenv(Path(__file__).resolve().parents[2])
    comparison = Comparison(arguments.html, arguments.v2_html)
    handler = type("BoundHandler", (Handler,), {"comparison": comparison})
    server = ThreadingHTTPServer((arguments.host, arguments.port), handler)
    print(f"Minesweeper comparison on http://{arguments.host}:{arguments.port}/")
    print(f"JEV-only expert board on http://{arguments.host}:{arguments.port}/v2/")
    try:
        server.serve_forever(poll_interval=0.2)
    except KeyboardInterrupt:
        pass
    finally:
        comparison.stop()
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

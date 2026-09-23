"""Serve and orchestrate the local two-room Hanoi comparison prototype.

The adapter starts two independent ``local_harness`` processes with one shared
epoch barrier. The left process runs a selected Codex/OpenRouter solver and the
right process runs a JEV solver. Each process owns its own WorldStream Runtime,
Room, Memberships, Runner leases, and actionless canvas observer. This adapter
only combines their already-sanitized canvas DTOs for a local comparison view.
"""

from __future__ import annotations

import argparse
import json
import os
import queue
import random
import secrets
import signal
import subprocess
import tempfile
import threading
import time
import urllib.parse
import urllib.request
from collections import deque
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from .live_canvas import DECISION_SCHEMA, LiveCanvasError, _safe_decision_event

MAX_BODY_BYTES = 16_384
MAX_HISTORY = 128
MAX_CLIENTS = 16
MAX_SOLVERS = 16
MAX_ROOM_EVENTS = 100
MAX_DECISION_EVENTS = 100
# Full run history retained only for the explicit feed download.
MAX_FULL_EVENTS = 20_000
SETUP_TIMEOUT_SECONDS = 300
DEFAULT_SOLVERS = 5
DEFAULT_DURATION_SECONDS = 300
DEFAULT_WARMUP_SECONDS = 5
# Cool agent codenames replace opaque seat ids like ``left-solver-1``.
PARTICIPANT_NAMES = (
    "nova", "atlas", "cipher", "ember", "onyx", "viper", "zenith", "lumen",
    "quasar", "orbit", "raven", "sable", "cobalt", "indigo", "jade", "aster",
    "comet", "harbor", "delta", "echo", "flux", "gamma", "helix", "ion",
    "lynx", "nimbus", "opal", "pulse", "quartz", "rune", "storm", "tide",
    "umbra", "vertex", "wren", "zephyr", "aurora", "drift", "eclipse", "flare",
    "glacier", "haze", "iris", "jolt", "krypton", "mirage", "noctis", "orion",
    "prism", "quill", "rift", "solstice", "titan", "vellum", "wisp", "xenon",
    "yonder", "zodiac", "aether", "basilisk", "chronos", "dynamo",
)
MODEL_CATALOG = {
    "codex": [
        {"id": "gpt-5.6-luna", "label": "Luna · Codex"},
        {"id": "gpt-5.6-sol", "label": "Sol · Codex"},
        {"id": "gpt-5.6-terra", "label": "Terra · Codex"},
        {"id": "gpt-6-astra", "label": "Astra · Codex"},
    ],
    "openrouter": [
        {"id": "openai/gpt-5.6-luna", "label": "Luna · OpenRouter"},
        {"id": "openai/gpt-5.6-sol", "label": "Sol · OpenRouter"},
        {"id": "openai/gpt-5.6-terra", "label": "Terra · OpenRouter"},
        {"id": "openai/gpt-6-astra", "label": "Astra · OpenRouter"},
    ],
    "jev": [
        {"id": "jev-latest", "label": "JEV · latest alias"},
        {"id": "jev-1.13.0", "label": "JEV · pinned 1.13.0"},
    ],
}


def model_catalog() -> dict[str, list[dict[str, str]]]:
    """Cheap, fast OpenRouter models for the left Room's provider dropdown.

    The retained prototype originally shipped frontier aliases. Reuse the v2
    live catalog so both experiments offer the same cheap models; keep the
    retained aliases as a fallback when the live catalog is unavailable.
    """
    catalog = {key: list(value) for key, value in MODEL_CATALOG.items()}
    try:
        from .comparison_canvas_v2 import cheap_models as _cheap_models

        cheap = _cheap_models()
    except (ImportError, OSError, ValueError, TimeoutError):
        cheap = []
    if cheap:
        catalog["openrouter"] = cheap
    return catalog


def _load_dotenv(root: Path) -> None:
    """Make local development keys available to children without exposing them."""
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


def _participant_seat_names(count: int) -> list[str]:
    """Pick unique, reference-safe display names for one comparison run."""
    pool = list(PARTICIPANT_NAMES)
    random.SystemRandom().shuffle(pool)
    names = pool[:count]
    suffix = 1
    while len(names) < count:
        names.append(f"participant-{suffix}")
        suffix += 1
    return names


def _safe_comparison_event(value: object) -> dict[str, object] | None:
    """Copy only the small public event vocabulary rendered by the comparison UI."""
    if not isinstance(value, dict):
        return None
    if value.get("kind") == "participant_decision":
        try:
            return _safe_decision_event({"schema": DECISION_SCHEMA, **value})
        except LiveCanvasError:
            return None
    kind = value.get("kind")
    actor = value.get("actor")
    room_seq = value.get("room_seq")
    if (
        not isinstance(kind, str)
        or not kind
        or len(kind) > 64
        or any(character.isspace() for character in kind)
        or not isinstance(actor, str)
        or not actor
        or len(actor) > 64
        or any(character.isspace() for character in actor)
        or isinstance(room_seq, bool)
        or not isinstance(room_seq, int)
        or not 0 <= room_seq <= 9_007_199_254_740_991
    ):
        return None
    event: dict[str, object] = {
        "kind": kind,
        "actor": actor,
        "room_seq": room_seq,
    }
    for field in ("action_type", "status"):
        candidate = value.get(field)
        if candidate is not None:
            if (
                not isinstance(candidate, str)
                or not candidate
                or len(candidate) > 64
                or any(character.isspace() for character in candidate)
            ):
                return None
            event[field] = candidate
    numeric_fields = {
        "round": (1, 30_000),
        "work_revision": (0, 10_000),
        "claim_round": (1, 30_000),
        "quorum": (1, 16),
        "approval_count": (0, 16),
        "endorsement_count": (0, 16),
        "electorate_size": (1, 16),
    }
    for field, (minimum, maximum) in numeric_fields.items():
        candidate = value.get(field)
        if candidate is None:
            continue
        if (
            isinstance(candidate, bool)
            or not isinstance(candidate, int)
            or not minimum <= candidate <= maximum
        ):
            return None
        event[field] = candidate
    assessment = value.get("assessment")
    if assessment is not None:
        if assessment not in {"endorse", "challenge", "defer"}:
            return None
        event["assessment"] = assessment
    claim_open = value.get("claim_open")
    if claim_open is not None:
        if not isinstance(claim_open, bool):
            return None
        event["claim_open"] = claim_open
    move = value.get("move")
    if move is not None:
        if not isinstance(move, dict) or set(move) != {"from", "to", "disk"}:
            return None
        source, destination, disk = move["from"], move["to"], move["disk"]
        if (
            source not in {"A", "B", "C"}
            or destination not in {"A", "B", "C"}
            or source == destination
            or isinstance(disk, bool)
            or not isinstance(disk, int)
            or not 1 <= disk <= 10
        ):
            return None
        event["move"] = {"from": source, "to": destination, "disk": disk}
    return event


_DISPOSITION_BY_ACTION = {
    "move_disk": "act",
    "post_completion_claim": "claim",
    "assess_claim": "assess",
}
_ASSESSMENT_LABELS = {"endorse", "challenge", "defer"}


def _decision_from_invocation(
    value: object, actor: str
) -> dict[str, object] | None:
    """Project one provider invocation result onto a bounded decision trace.

    The comparison relay may observe provider invocation records that predate the
    canonical decision event. This synthesizes the same sanitized DTO shape so
    the browser receives one consistent participant decision vocabulary.
    """
    if not isinstance(value, dict):
        return None
    if not isinstance(actor, str) or not actor or len(actor) > 64:
        return None
    engine = value.get("engine")
    model = value.get("provider_model", value.get("model"))
    reason = value.get("reason")
    status = value.get("status")
    action_type = value.get("action_type")
    choice = value.get("choice")
    room_seq = value.get("room_seq", value.get("current_room_seq"))
    latency = value.get("latency_ms")
    if (
        not isinstance(engine, str)
        or not engine
        or len(engine) > 64
        or not isinstance(model, str)
        or not model
        or len(model) > 128
        or not isinstance(reason, str)
        or not reason
        or len(reason) > 128
        or isinstance(room_seq, bool)
        or not isinstance(room_seq, int)
        or not 0 <= room_seq <= 9_007_199_254_740_991
        or isinstance(latency, bool)
        or not isinstance(latency, int)
        or not 0 <= latency <= 600_000
    ):
        return None
    disposition = _DISPOSITION_BY_ACTION.get(action_type) if isinstance(action_type, str) else None
    selected: dict[str, object]
    if disposition is None:
        disposition = "wait"
        selected = {"label": "wait"}
    else:
        if not isinstance(choice, str) or not choice or len(choice) > 128:
            return None
        selected = {"label": choice, "action_type": action_type}
    result: dict[str, object] = {
        "kind": "participant_decision",
        "actor": actor,
        "engine": engine,
        "model": model,
        "observed_room_seq": room_seq,
        "activation_reason": reason,
        "disposition": disposition,
        "selected": selected,
        "latency_ms": latency,
        "recent_event_count": 0,
    }
    if status in {"accepted", "stale", "rejected", "error"}:
        result["status"] = status
    if value.get("reason") in {"progress", "break_cycle", "uncertain", "abandon"}:
        result["reason"] = value["reason"]
    confidence = value.get("confidence")
    if isinstance(confidence, (int, float)) and not isinstance(confidence, bool):
        ratio = float(confidence)
        if 0.0 <= ratio <= 1.0:
            result["confidence"] = ratio
    return result


class ComparisonBroadcast:
    """Bounded SSE fan-out of already-sanitized comparison state."""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.subscribers: set[queue.Queue[tuple[int, str]]] = set()
        self.history: deque[tuple[int, str]] = deque(maxlen=MAX_HISTORY)
        self.cursor = 0
        self.latest: dict[str, object] = {
            "type": "comparison",
            "status": "idle",
            "timer": {"status": "idle"},
            "sides": {
                "left": {"status": "idle", "decisions": []},
                "right": {"status": "idle", "decisions": []},
            },
        }

    def _encoded(self, value: dict[str, object]) -> tuple[int, str]:
        self.cursor += 1
        return self.cursor, json.dumps(
            {**value, "cursor": self.cursor}, separators=(",", ":"), sort_keys=True
        )

    def subscribe(self, after: str | None) -> queue.Queue[tuple[int, str]] | None:
        with self.lock:
            if len(self.subscribers) >= MAX_CLIENTS:
                return None
            try:
                cursor = int(after) if after is not None else None
            except ValueError:
                cursor = None
            subscriber: queue.Queue[tuple[int, str]] = queue.Queue(maxsize=MAX_HISTORY)
            # A viewer reconnecting after this process restarted carries a
            # Last-Event-ID from the previous process. Its cursor is outside our
            # history, so replaying deltas would send nothing and leave the UI
            # wedged on a stale "running" state. Send current state instead.
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

    def unsubscribe(self, subscriber: queue.Queue[tuple[int, str]]) -> None:
        with self.lock:
            self.subscribers.discard(subscriber)

    def publish(self, value: dict[str, object]) -> None:
        with self.lock:
            self.latest = value
            item = self._encoded(value)
            self.history.append(item)
            for subscriber in tuple(self.subscribers):
                try:
                    subscriber.put_nowait(item)
                except queue.Full:
                    try:
                        subscriber.get_nowait()
                        subscriber.put_nowait(item)
                    except queue.Empty:
                        pass


class ComparisonRun:
    def __init__(self, root: Path, broadcast: ComparisonBroadcast) -> None:
        self.root = root
        self.broadcast = broadcast
        self.lock = threading.RLock()
        self.processes: dict[str, subprocess.Popen[str]] = {}
        self.threads: list[threading.Thread] = []
        self.config: dict[str, object] | None = None
        self.started_at_ms: int | None = None
        self.deadline_at_ms: int | None = None
        self.generation = 0
        self.comparison_dir: Path | None = None
        self.start_file: Path | None = None
        self.warmup_seconds = DEFAULT_WARMUP_SECONDS
        self.sides: dict[str, dict[str, object]] = {
            "left": {"status": "idle"},
            "right": {"status": "idle"},
        }
        self.event_logs: dict[str, list[dict[str, object]]] = {
            "left": [],
            "right": [],
        }
        self.decision_logs: dict[str, list[dict[str, object]]] = {
            "left": [],
            "right": [],
        }
        self.full_event_logs: dict[str, list[dict[str, object]]] = {
            "left": [],
            "right": [],
        }
        self._event_keys: dict[str, set[str]] = {"left": set(), "right": set()}
        self._event_order: dict[str, deque[str]] = {
            "left": deque(),
            "right": deque(),
        }
        self._event_sequence: dict[str, dict[str, int]] = {
            "left": {},
            "right": {},
        }
        self._event_counter: dict[str, int] = {"left": 0, "right": 0}
        # Per-Room control so one misbehaving Room can be paused or ended alone.
        self.paused: dict[str, bool] = {"left": False, "right": False}
        self._resume_status: dict[str, str] = {"left": "idle", "right": "idle"}
        self.completed_at_ms: dict[str, int | None] = {"left": None, "right": None}
        self.finished_at_ms: int | None = None

    def _reset_event_logs(self) -> None:
        self.event_logs = {"left": [], "right": []}
        self.decision_logs = {"left": [], "right": []}
        self.full_event_logs = {"left": [], "right": []}
        self._event_keys = {"left": set(), "right": set()}
        self._event_order = {"left": deque(), "right": deque()}
        self._event_sequence = {"left": {}, "right": {}}
        self._event_counter = {"left": 0, "right": 0}

    def _record_side_events(self, side: str, payload: dict[str, object]) -> None:
        values = payload.get("events")
        if not isinstance(values, list):
            return
        for value in values:
            event = _safe_comparison_event(value)
            if event is None:
                continue
            key = json.dumps(event, separators=(",", ":"), sort_keys=True)
            if key in self._event_keys[side]:
                continue
            self._event_counter[side] += 1
            self._event_keys[side].add(key)
            self._event_order[side].append(key)
            self._event_sequence[side][key] = self._event_counter[side]
            self.event_logs[side].append(event)
            self.full_event_logs[side].append(event)
            if len(self.full_event_logs[side]) > MAX_FULL_EVENTS:
                del self.full_event_logs[side][: len(self.full_event_logs[side]) - MAX_FULL_EVENTS]
            if event.get("kind") == "participant_decision":
                self.decision_logs[side].append(dict(event))
        self.event_logs[side].sort(
            key=lambda event: (
                event.get("room_seq", event.get("observed_room_seq", 0)),
                self._event_sequence[side].get(
                    json.dumps(event, separators=(",", ":"), sort_keys=True), 0
                ),
            )
        )
        while len(self._event_order[side]) > MAX_ROOM_EVENTS:
            key = self._event_order[side].popleft()
            self._event_keys[side].discard(key)
            self._event_sequence[side].pop(key, None)
            self.decision_logs[side] = [
                event
                for event in self.decision_logs[side]
                if json.dumps(event, separators=(",", ":"), sort_keys=True) != key
            ]
            self.event_logs[side] = [
                event
                for event in self.event_logs[side]
                if json.dumps(event, separators=(",", ":"), sort_keys=True) != key
            ]

    def _apply_canvas_payload(
        self, side: str, payload: object, generation: int
    ) -> None:
        if side not in self.sides or not isinstance(payload, dict):
            return
        with self.lock:
            if generation != self.generation:
                return
            payload_type = payload.get("type")
            if payload_type == "hanoi":
                self._record_side_events(side, payload)
                projection = payload.get("projection")
                if isinstance(projection, dict):
                    self.sides[side] = {
                        **self.sides[side],
                        "feed": payload,
                        "feed_status": "live",
                    }
                    outcome = projection.get("outcome")
                    if (
                        isinstance(outcome, dict)
                        and outcome.get("status") == "participant_accepted_completion"
                        and self.completed_at_ms[side] is None
                        and self.started_at_ms is not None
                    ):
                        self.completed_at_ms[side] = int(time.time() * 1000)
                    self._maybe_finish_locked()
                elif self.sides[side].get("status") == "live":
                    self.sides[side] = {
                        **self.sides[side],
                        "feed_status": "live",
                    }
            elif payload_type == "status":
                stream_state = payload.get("state")
                if isinstance(stream_state, str) and stream_state:
                    self.sides[side] = {
                        **self.sides[side],
                        "feed_status": stream_state,
                    }
            else:
                return
        self._publish()

    def _completion_timing(self, side: str) -> dict[str, object]:
        """Report how long the Room took from the shared start to acceptance."""
        completed = self.completed_at_ms.get(side)
        started = self.started_at_ms
        if completed is None or started is None:
            return {}
        return {
            "completed_at_ms": completed,
            "elapsed_ms": max(0, completed - started),
        }

    def _maybe_finish_locked(self) -> bool:
        """End the run once both Rooms have accepted completion."""
        if self.finished_at_ms is not None:
            return False
        if not all(
            self.sides[side].get("status")
            in {"complete", "participant_accepted_completion"}
            for side in ("left", "right")
        ):
            return False
        self.finished_at_ms = int(time.time() * 1000)
        self._kill_children_locked()
        return True

    def _publish(self, status: str | None = None) -> None:
        timer: dict[str, object] = {"status": "idle"}
        if self.finished_at_ms is not None and self.started_at_ms is not None:
            # Both Rooms accepted: freeze the shared clock at the finish time.
            timer = {
                "status": "complete",
                "started_at_ms": self.started_at_ms,
                "deadline_at_ms": self.deadline_at_ms,
                "finished_at_ms": self.finished_at_ms,
                "elapsed_ms": max(0, self.finished_at_ms - self.started_at_ms),
            }
        elif self.config is not None and self.started_at_ms is None:
            timer = {"status": "preparing"}
        elif self.started_at_ms is not None and self.deadline_at_ms is not None:
            now = int(time.time() * 1000)
            timer = {
                "status": "arming" if now < self.started_at_ms else "running",
                "started_at_ms": self.started_at_ms,
                "deadline_at_ms": self.deadline_at_ms,
            }
            if now >= self.deadline_at_ms:
                timer["status"] = "time_limit"
        if status == "error":
            timer = {"status": "error"}
            if self.started_at_ms is not None and self.deadline_at_ms is not None:
                timer.update(
                    {
                        "started_at_ms": self.started_at_ms,
                        "deadline_at_ms": self.deadline_at_ms,
                    }
                )
        with self.lock:
            public_sides = {
                side: {
                    **info,
                    "events": [dict(event) for event in self.event_logs[side]],
                    "event_total": len(self.full_event_logs[side]),
                    **self._completion_timing(side),
                    "decisions": [
                        dict(event)
                        for event in self.decision_logs[side][-MAX_DECISION_EVENTS:]
                    ],
                }
                for side, info in self.sides.items()
            }
            config = {
                key: value
                for key, value in (self.config or {}).items()
                if key not in {"api_key", "secret"}
            }
            default_status = "idle"
            if self.finished_at_ms is not None:
                default_status = "complete"
            elif self.config is not None:
                default_status = "preparing" if self.started_at_ms is None else "running"
        self.broadcast.publish(
            {
                "type": "comparison",
                "status": status or default_status,
                "config": config,
                "timer": timer,
                "sides": public_sides,
            }
        )

    def start(self, config: dict[str, object]) -> dict[str, object]:
        with self.lock:
            if self.processes:
                raise ValueError("comparison_already_running")
            self.generation += 1
            generation = self.generation
            disks = _bounded_int(config.get("disks", 3), "disks", 1, 10)
            duration = _bounded_int(config.get("duration_seconds", DEFAULT_DURATION_SECONDS), "duration_seconds", 1, 3600)
            left_count = _bounded_int(config.get("left_solver_count", DEFAULT_SOLVERS), "left_solver_count", 1, MAX_SOLVERS)
            right_count = _bounded_int(config.get("right_solver_count", DEFAULT_SOLVERS), "right_solver_count", 1, MAX_SOLVERS)
            provider = _safe(config.get("left_provider", "openrouter"), "left_provider")
            if provider not in {"codex", "openrouter"}:
                raise ValueError("left_provider_invalid")
            left_model = _safe(config.get("left_model", "openai/gpt-5.6-luna"), "left_model")
            jev_model = _safe(config.get("jev_model", "jev-latest"), "jev_model")
            # The OpenRouter list is a live cheap catalog, so any safe model id
            # is accepted; Codex aliases remain a fixed set.
            if provider == "codex" and left_model not in {
                item["id"] for item in MODEL_CATALOG["codex"]
            }:
                raise ValueError("left_model_invalid")
            if jev_model not in {item["id"] for item in MODEL_CATALOG["jev"]}:
                raise ValueError("jev_model_invalid")
            warmup = _bounded_int(config.get("warmup_seconds", DEFAULT_WARMUP_SECONDS), "warmup_seconds", 5, 180)
            randomize_board = config.get("randomize_board", False)
            if not isinstance(randomize_board, bool):
                raise TypeError("randomize_board_invalid")
            board_seed_value = config.get("board_seed")
            board_seed = (
                None
                if board_seed_value is None
                else _bounded_int(board_seed_value, "board_seed", 0, 2**63 - 1)
            )
            if randomize_board and board_seed is None:
                # One shared seed keeps both Rooms on the identical mid-state board.
                board_seed = secrets.randbits(63)
            seat_names = _participant_seat_names(left_count + right_count)
            left_seats = seat_names[:left_count]
            right_seats = seat_names[left_count : left_count + right_count]
            self.warmup_seconds = warmup
            self.config = {
                "disks": disks,
                "duration_seconds": duration,
                "left_provider": provider,
                "left_model": left_model,
                "jev_model": jev_model,
                "left_solver_count": left_count,
                "right_solver_count": right_count,
                "randomize_board": randomize_board,
                "board_seed": board_seed,
            }
            self.started_at_ms = None
            self.deadline_at_ms = None
            self.completed_at_ms = {"left": None, "right": None}
            self.finished_at_ms = None
            target_dir = self.root / "target"
            target_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
            self.comparison_dir = Path(tempfile.mkdtemp(prefix="worldstream-hanoi-comparison-", dir=target_dir))
            self.start_file = self.comparison_dir / "release.json"
            self.sides = {
                "left": {"status": "starting", "solver_count": left_count, "provider": provider, "model": left_model, "seats": list(left_seats)},
                "right": {"status": "starting", "solver_count": right_count, "provider": "jev", "model": jev_model, "seats": list(right_seats)},
            }
            self._reset_event_logs()
            _load_dotenv(self.root)
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
                str(SETUP_TIMEOUT_SECONDS),
                "--fast-start",
                *board_arguments,
            ]
            commands = {
                "left": common + ["--solver-engine", provider, "--solver-model", left_model],
                "right": common + ["--solver-engine", "jev", "--solver-model", jev_model],
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
                thread = threading.Thread(target=self._read_child, args=(side, process, generation), daemon=True)
                self.threads.append(thread)
                thread.start()
            self._publish("preparing")
            timer_thread = threading.Thread(target=self._timer_loop, daemon=True)
            self.threads.append(timer_thread)
            timer_thread.start()
            return {"status": "preparing"}

    def _release_if_ready(self, generation: int) -> None:
        with self.lock:
            if generation != self.generation or self.started_at_ms is not None:
                return
            if not all(self.sides[side].get("status") == "live" for side in ("left", "right")):
                return
            self.started_at_ms = int(time.time() * 1000) + self.warmup_seconds * 1_000
            duration_seconds = self.config.get("duration_seconds") if self.config else None
            if not isinstance(duration_seconds, int):
                raise TypeError("comparison_duration_missing")
            self.deadline_at_ms = self.started_at_ms + duration_seconds * 1_000
            if self.start_file is None:
                raise RuntimeError("comparison_start_file_missing")
            encoded = json.dumps({"start_at_ms": self.started_at_ms}, separators=(",", ":")).encode("utf-8")
            descriptor = os.open(self.start_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(descriptor, "wb") as output:
                output.write(encoded)
                output.flush()
                os.fsync(output.fileno())
            self._publish("arming")

    def _abort_preparation_locked(
        self,
        side: str,
        code: str,
        *,
        exit_code: int | None = None,
    ) -> None:
        """Close a failed readiness barrier and retain the side-level evidence."""
        failed = {**self.sides[side], "status": "error", "code": code}
        if exit_code is not None:
            failed["exit_code"] = exit_code
        self.sides[side] = failed
        for peer, info in self.sides.items():
            if peer != side and info.get("status") not in {
                "complete",
                "participant_accepted_completion",
                "deadline",
                "error",
                "time_limit",
            }:
                self.sides[peer] = {
                    **info,
                    "status": "aborted",
                    "code": "peer_failed",
                }
        self.generation += 1
        self._kill_children_locked()
        self.config = None
        self.started_at_ms = None
        self.deadline_at_ms = None
        if self.start_file is not None:
            try:
                self.start_file.unlink()
            except OSError:
                pass
        if self.comparison_dir is not None:
            try:
                self.comparison_dir.rmdir()
            except OSError:
                pass
        self.start_file = None
        self.comparison_dir = None

    @staticmethod
    def _child_error_code(value: dict[str, object]) -> str:
        code = value.get("code")
        if (
            isinstance(code, str)
            and code
            and len(code) <= 64
            and not any(character.isspace() for character in code)
        ):
            return code
        return "child_error"

    def _read_child(self, side: str, process: subprocess.Popen[str], generation: int) -> None:
        if process.stdout is None:
            return
        for line in process.stdout:
            try:
                value = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(value, dict):
                continue
            if generation != self.generation:
                return
            status = value.get("status")
            if status == "canvas_live":
                self.sides[side] = {**self.sides[side], "status": "live"}
                self._release_if_ready(generation)
                url = value.get("canvas_url")
                if isinstance(url, str) and url.startswith("http://127.0.0.1:"):
                    thread = threading.Thread(target=self._follow_events, args=(side, url, generation), daemon=True)
                    self.threads.append(thread)
                    thread.start()
            elif status in {"complete", "participant_accepted_completion", "deadline"}:
                self.sides[side] = {**self.sides[side], "status": status}
                if (
                    status in {"complete", "participant_accepted_completion"}
                    and self.completed_at_ms[side] is None
                    and self.started_at_ms is not None
                ):
                    self.completed_at_ms[side] = int(time.time() * 1000)
                with self.lock:
                    self._maybe_finish_locked()
            elif status == "error":
                with self.lock:
                    if generation != self.generation:
                        return
                    preparing = self.started_at_ms is None
                    if preparing:
                        self._abort_preparation_locked(
                            side, self._child_error_code(value)
                        )
                    else:
                        self.sides[side] = {
                            **self.sides[side],
                            "status": "error",
                            "code": self._child_error_code(value),
                        }
                if preparing:
                    self._publish("error")
                    return
            self._publish()
        should_abort = False
        with self.lock:
            if generation != self.generation:
                return
            exit_code = process.wait()
            status = self.sides[side].get("status")
            preparing = self.started_at_ms is None
            if status not in {
                "complete",
                "participant_accepted_completion",
                "deadline",
                "error",
                "time_limit",
                "ended",
            }:
                if preparing:
                    self._abort_preparation_locked(
                        side, "child_exited", exit_code=exit_code
                    )
                else:
                    self.sides[side] = {
                        **self.sides[side],
                        "status": "error",
                        "code": "child_exited",
                        "exit_code": exit_code,
                    }
                should_abort = True
            else:
                self.sides[side] = {**self.sides[side], "exit_code": exit_code}
        self._publish("error" if should_abort else None)

    def _follow_events(self, side: str, url: str, generation: int) -> None:
        try:
            response = urllib.request.urlopen(url.rstrip("/") + "/events", timeout=15)
            current: list[str] = []
            for raw in response:
                line = raw.decode("utf-8", "replace").rstrip("\n")
                if line.startswith("data: "):
                    current.append(line[6:])
                elif not line and current:
                    try:
                        payload = json.loads("".join(current))
                    except json.JSONDecodeError:
                        payload = None
                    current = []
                    if isinstance(payload, dict) and generation == self.generation:
                        self._apply_canvas_payload(side, payload, generation)
        except (OSError, ValueError):
            with self.lock:
                if generation != self.generation:
                    return
                self.sides[side] = {
                    **self.sides[side],
                    "feed_status": "unavailable",
                }
            self._publish()

    def _timer_loop(self) -> None:
        generation = self.generation
        while generation == self.generation:
            with self.lock:
                self._maybe_finish_locked()
            if self.finished_at_ms is not None:
                # Both Rooms accepted; the frozen clock is already published.
                self._publish()
                return
            if self.deadline_at_ms is None:
                self._publish()
                time.sleep(1)
                continue
            if int(time.time() * 1000) >= self.deadline_at_ms:
                with self.lock:
                    if generation != self.generation:
                        return
                    self.generation += 1
                    self._kill_children_locked()
                    self.sides = {
                        side: {
                            **info,
                            "status": info.get("status")
                            if info.get("status") in {"complete", "participant_accepted_completion", "error", "ended"}
                            else "time_limit",
                        }
                        for side, info in self.sides.items()
                    }
                self._publish("time_limit")
                return
            self._publish()
            time.sleep(1)

    def _kill_child_locked(self, side: str) -> None:
        """Stop one child process group without touching the peer Room."""
        process = self.processes.get(side)
        if process is None:
            return
        if self.paused.get(side):
            # A stopped group ignores SIGTERM until it is resumed.
            try:
                os.killpg(process.pid, signal.SIGCONT)
            except OSError:
                pass
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except OSError:
                try:
                    process.terminate()
                except OSError:
                    pass
        if process.poll() is None:
            try:
                process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except OSError:
                    try:
                        process.kill()
                    except OSError:
                        pass
                try:
                    process.wait(timeout=5)
                except (OSError, subprocess.TimeoutExpired):
                    pass
        self.processes.pop(side, None)

    def _kill_children_locked(self) -> None:
        """Stop both child process groups while the run lock is held."""
        for side in tuple(self.processes):
            self._kill_child_locked(side)

    def pause_side(self, side: str, *, paused: bool) -> dict[str, object]:
        """Freeze or resume exactly one Room so the peer keeps running."""
        with self.lock:
            if side not in self.sides:
                raise ValueError("side_invalid")
            process = self.processes.get(side)
            if process is None or process.poll() is not None:
                raise ValueError("room_not_running")
            if paused:
                if os.name != "posix":
                    raise ValueError("room_pause_unsupported")
                try:
                    os.killpg(process.pid, signal.SIGSTOP)
                except OSError as error:
                    raise ValueError("room_pause_failed") from error
                if not self.paused[side]:
                    self._resume_status[side] = str(self.sides[side].get("status") or "live")
                self.paused[side] = True
                self.sides[side] = {**self.sides[side], "status": "paused"}
            else:
                if os.name != "posix":
                    raise ValueError("room_resume_unsupported")
                try:
                    os.killpg(process.pid, signal.SIGCONT)
                except OSError as error:
                    raise ValueError("room_resume_failed") from error
                self.paused[side] = False
                self.sides[side] = {**self.sides[side], "status": self._resume_status[side]}
        self._publish()
        return {"status": self.sides[side].get("status"), "side": side}

    def end_side(self, side: str) -> dict[str, object]:
        """End exactly one Room while the peer Room stays authoritative."""
        with self.lock:
            if side not in self.sides:
                raise ValueError("side_invalid")
            self._kill_child_locked(side)
            self.paused[side] = False
            self.sides[side] = {**self.sides[side], "status": "ended"}
        self._publish()
        return {"status": "ended", "side": side}

    def stop(self) -> None:
        with self.lock:
            self.generation += 1
            self._kill_children_locked()
            self.paused = {"left": False, "right": False}
            self._resume_status = {"left": "idle", "right": "idle"}
            self.completed_at_ms = {"left": None, "right": None}
            self.finished_at_ms = None
            self.config = None
            self.started_at_ms = None
            self.deadline_at_ms = None
            if self.start_file is not None:
                try:
                    self.start_file.unlink()
                except OSError:
                    pass
            if self.comparison_dir is not None:
                try:
                    self.comparison_dir.rmdir()
                except OSError:
                    pass
            self.start_file = None
            self.comparison_dir = None
            self.sides = {"left": {"status": "idle"}, "right": {"status": "idle"}}
            self._reset_event_logs()
            self._publish("idle")


class ComparisonServer(ThreadingHTTPServer):
    def __init__(self, address: tuple[str, int], root: Path) -> None:
        super().__init__(address, ComparisonHandler)
        self.root = root
        self.broadcast = ComparisonBroadcast()
        self.run = ComparisonRun(root, self.broadcast)


class ComparisonHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _headers(self, content_type: str, length: int | None = None) -> None:
        self.send_header("Content-Type", content_type)
        self.send_header("Cache-Control", "no-store, max-age=0")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header(
            "Content-Security-Policy",
            "default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'",
        )
        if length is not None:
            self.send_header("Content-Length", str(length))

    def _send(self, status: HTTPStatus, body: bytes, content_type: str) -> None:
        self.send_response(status)
        self._headers(content_type, len(body))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        server: ComparisonServer = self.server  # type: ignore[assignment]
        request = urllib.parse.urlsplit(self.path)
        path = request.path
        if path == "/":
            try:
                body = (server.root / "web/demos/community-hanoi-comparison.html").read_bytes()
            except OSError:
                self._send(HTTPStatus.SERVICE_UNAVAILABLE, b"demo unavailable\n", "text/plain")
                return
            self._send(HTTPStatus.OK, body, "text/html; charset=utf-8")
        elif path == "/config":
            self._send(HTTPStatus.OK, json.dumps({"models": model_catalog(), "defaults": {"solver_count": DEFAULT_SOLVERS, "duration_seconds": DEFAULT_DURATION_SECONDS}}).encode(), "application/json")
        elif path == "/events":
            self._events(server)
        elif path == "/feed":
            parameters = urllib.parse.parse_qs(request.query)
            side = (parameters.get("side") or [""])[0]
            if side not in {"left", "right"}:
                self._send(HTTPStatus.BAD_REQUEST, b'{"status":"error","code":"side_invalid"}\n', "application/json")
                return
            with server.run.lock:
                events = [dict(event) for event in server.run.full_event_logs[side]]
            body = json.dumps({"side": side, "count": len(events), "events": events}, separators=(",", ":")).encode()
            self.send_response(HTTPStatus.OK)
            self._headers("application/json", len(body))
            self.send_header("Content-Disposition", f'inline; filename="hanoi-{side}-feed.json"')
            self.end_headers()
            self.wfile.write(body)
        elif path == "/healthz":
            self._send(HTTPStatus.OK, b"ok\n", "text/plain")
        else:
            self._send(HTTPStatus.NOT_FOUND, b"not found\n", "text/plain")

    def do_POST(self) -> None:
        server: ComparisonServer = self.server  # type: ignore[assignment]
        if self.path not in {"/start", "/stop", "/pause", "/resume", "/end"}:
            self._send(HTTPStatus.NOT_FOUND, b"not found\n", "text/plain")
            return
        if self.path == "/stop":
            server.run.stop()
            self._send(HTTPStatus.OK, b'{"status":"idle"}\n', "application/json")
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if length < 0 or length > MAX_BODY_BYTES:
                raise ValueError("body_oversize")
            body = json.loads(self.rfile.read(length) or b"{}")
            if not isinstance(body, dict):
                raise TypeError("body_invalid")
            if self.path == "/start":
                result = server.run.start(body)
            else:
                side = body.get("side")
                if side not in {"left", "right"}:
                    raise ValueError("side_invalid")
                if self.path == "/pause":
                    result = server.run.pause_side(side, paused=True)
                elif self.path == "/resume":
                    result = server.run.pause_side(side, paused=False)
                else:
                    result = server.run.end_side(side)
        except (OSError, TypeError, ValueError) as error:
            self._send(HTTPStatus.BAD_REQUEST, json.dumps({"status": "error", "code": str(error)}).encode(), "application/json")
            return
        self._send(HTTPStatus.ACCEPTED, json.dumps(result).encode(), "application/json")

    def _events(self, server: ComparisonServer) -> None:
        subscriber = server.broadcast.subscribe(self.headers.get("Last-Event-ID"))
        if subscriber is None:
            self._send(HTTPStatus.SERVICE_UNAVAILABLE, b"viewer capacity reached\n", "text/plain")
            return
        try:
            self.send_response(HTTPStatus.OK)
            self._headers("text/event-stream; charset=utf-8")
            self.send_header("Connection", "keep-alive")
            self.end_headers()
            self.wfile.flush()
            while True:
                try:
                    cursor, payload = subscriber.get(timeout=10)
                    self.wfile.write(f"id: {cursor}\nevent: comparison\ndata: {payload}\n\n".encode())
                except queue.Empty:
                    self.wfile.write(b": keepalive\n\n")
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError, OSError):
            return
        finally:
            server.broadcast.unsubscribe(subscriber)

    def log_message(self, _format: str, *_arguments: object) -> None:
        return


def _arguments() -> argparse.Namespace:
    root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=root)
    parser.add_argument("--port", type=int, default=5191)
    return parser.parse_args()


def main() -> int:
    arguments = _arguments()
    if not 1024 <= arguments.port <= 65535:
        raise SystemExit("port must be between 1024 and 65535")
    _load_dotenv(arguments.root.resolve())
    server = ComparisonServer(("127.0.0.1", arguments.port), arguments.root.resolve())
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

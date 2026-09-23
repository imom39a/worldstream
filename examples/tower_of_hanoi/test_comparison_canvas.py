"""Focused tests for the two-room comparison adapter and provider candidates."""

from __future__ import annotations

import io
import signal
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import call, patch

from examples.tower_of_hanoi.comparison_canvas import (
    MAX_ROOM_EVENTS,
    MODEL_CATALOG,
    ComparisonBroadcast,
    ComparisonRun,
    _decision_from_invocation,
    _participant_seat_names,
)
from examples.tower_of_hanoi.live_canvas import _safe_sink_event
from examples.tower_of_hanoi.participant_runner import (
    _jev_choice,
    _legal_candidates,
)


class ComparisonCanvasTests(unittest.TestCase):
    def test_legal_candidates_are_enumerated_from_the_projection(self) -> None:
        view = {
            "action_offers": ["move_disk", "post_completion_claim"],
            "board": {"A": [3, 2], "B": [1], "C": []},
            "disks": 3,
            "work_revision": 4,
        }
        candidates = _legal_candidates(view)
        self.assertEqual(
            set(candidates),
            {"move:A>C:2", "move:B>A:1", "move:B>C:1", "post_completion_claim", "wait"},
        )
        self.assertEqual(
            candidates["post_completion_claim"]["payload"], {"work_revision": 4}
        )
        self.assertNotIn("move:A>B:1", candidates)

    def test_jev_choice_uses_closed_choice_criteria_and_returns_metadata(self) -> None:
        response = {
            "model": "jev-1.13.0",
            "answers": {
                "action": {
                    "type": "choice",
                    "choice": "move:A>C:1",
                    "confidence": 0.91,
                }
            },
            "usage": {"input_tokens": 120, "output_tokens": 8},
        }
        with patch(
            "examples.tower_of_hanoi.participant_runner._http_json",
            return_value=response,
        ) as request:
            choice, metadata = _jev_choice(
                {"legal_actions": {"move:A>C:1": "Move disk 1."}},
                {"move:A>C:1": "Move disk 1."},
                "jev-latest",
                "test-key",
                2,
            )
        self.assertEqual(choice, "move:A>C:1")
        self.assertEqual(metadata["provider_model"], "jev-1.13.0")
        body = request.call_args.args[1]
        self.assertEqual(body["model"], "jev-latest")
        self.assertEqual(body["questions"]["action"]["type"], "choice")
        self.assertEqual(body["questions"]["action"]["criteria"], {"move:A>C:1": "Move disk 1."})

    def test_model_catalog_keeps_codex_and_openrouter_ids_distinct(self) -> None:
        codex = {entry["id"] for entry in MODEL_CATALOG["codex"]}
        router = {entry["id"] for entry in MODEL_CATALOG["openrouter"]}
        self.assertIn("gpt-5.6-luna", codex)
        self.assertIn("openai/gpt-5.6-luna", router)
        self.assertNotEqual(codex, router)

    def test_both_rooms_completing_finishes_the_run_and_freezes_the_timer(self) -> None:
        comparison = ComparisonRun(Path("/tmp"), ComparisonBroadcast())
        comparison.generation = 1
        comparison.started_at_ms = int(time.time() * 1000) - 3_000
        comparison.deadline_at_ms = comparison.started_at_ms + 300_000
        comparison.sides = {"left": {"status": "complete"}, "right": {"status": "live"}}
        with comparison.lock:
            self.assertFalse(comparison._maybe_finish_locked())
        comparison.sides["right"] = {"status": "participant_accepted_completion"}
        with comparison.lock:
            self.assertTrue(comparison._maybe_finish_locked())
        comparison._publish()
        timer = comparison.broadcast.latest["timer"]
        self.assertEqual(timer["status"], "complete")
        self.assertGreaterEqual(timer["elapsed_ms"], 3_000)
        self.assertEqual(comparison.broadcast.latest["status"], "complete")

    def test_observed_acceptance_stamps_completion_timing(self) -> None:
        comparison = ComparisonRun(Path("/tmp"), ComparisonBroadcast())
        comparison.generation = 1
        comparison.sides["left"] = {"status": "live"}
        comparison.started_at_ms = int(time.time() * 1000) - 5_000
        comparison._apply_canvas_payload(
            "left",
            {
                "type": "hanoi",
                "projection": {
                    "room_seq": 9,
                    "outcome": {"moves": 9, "status": "participant_accepted_completion"},
                },
                "events": [],
            },
            1,
        )
        self.assertIsNotNone(comparison.completed_at_ms["left"])
        comparison._publish()
        self.assertGreaterEqual(
            comparison.broadcast.latest["sides"]["left"]["elapsed_ms"], 5_000
        )

    def test_completion_timing_is_published_only_for_the_finished_room(self) -> None:
        comparison = ComparisonRun(Path("/tmp"), ComparisonBroadcast())
        comparison.started_at_ms = 1_000_000
        comparison.sides = {"left": {"status": "complete"}, "right": {"status": "live"}}
        comparison.completed_at_ms = {"left": 1_065_000, "right": None}
        comparison._publish()
        left = comparison.broadcast.latest["sides"]["left"]
        self.assertEqual(left["completed_at_ms"], 1_065_000)
        self.assertEqual(left["elapsed_ms"], 65_000)
        self.assertNotIn("elapsed_ms", comparison.broadcast.latest["sides"]["right"])

    def test_full_event_log_outlives_the_display_window(self) -> None:
        comparison = ComparisonRun(Path("/tmp"), ComparisonBroadcast())
        comparison.generation = 1
        comparison.sides["left"] = {"status": "live"}
        for seq in range(1, MAX_ROOM_EVENTS + 40):
            comparison._apply_canvas_payload(
                "left",
                {
                    "type": "hanoi",
                    "events": [
                        {
                            "kind": "disk_moved",
                            "actor": "ada",
                            "action_type": "move_disk",
                            "status": "observed",
                            "room_seq": seq,
                            "move": {"from": "A", "to": "B", "disk": 1},
                        }
                    ],
                },
                1,
            )
        self.assertEqual(len(comparison.event_logs["left"]), MAX_ROOM_EVENTS)
        self.assertEqual(len(comparison.full_event_logs["left"]), MAX_ROOM_EVENTS + 39)
        comparison._publish()
        self.assertEqual(
            comparison.broadcast.latest["sides"]["left"]["event_total"],
            MAX_ROOM_EVENTS + 39,
        )

    def test_pause_and_resume_touch_only_the_requested_room(self) -> None:
        class RunningProcess:
            def __init__(self, pid: int) -> None:
                self.pid = pid
                self.returncode: int | None = None

            def poll(self) -> int | None:
                return self.returncode

            def wait(self, timeout: int | None = None) -> int:
                self.returncode = 0
                return 0

        comparison = ComparisonRun(Path("/tmp"), ComparisonBroadcast())
        comparison.generation = 1
        comparison.sides = {"left": {"status": "live"}, "right": {"status": "live"}}
        comparison.processes = {"left": RunningProcess(11), "right": RunningProcess(22)}
        with patch("examples.tower_of_hanoi.comparison_canvas.os.killpg") as killpg:
            result = comparison.pause_side("left", paused=True)
        self.assertEqual(result, {"status": "paused", "side": "left"})
        self.assertEqual(comparison.sides["left"]["status"], "paused")
        self.assertTrue(comparison.paused["left"])
        self.assertFalse(comparison.paused["right"])
        self.assertEqual(comparison.sides["right"]["status"], "live")
        self.assertEqual(killpg.call_args_list, [call(11, signal.SIGSTOP)])
        with patch("examples.tower_of_hanoi.comparison_canvas.os.killpg") as killpg:
            result = comparison.pause_side("left", paused=False)
        self.assertEqual(result, {"status": "live", "side": "left"})
        self.assertFalse(comparison.paused["left"])
        self.assertEqual(killpg.call_args_list, [call(11, signal.SIGCONT)])

    def test_end_side_stops_only_the_requested_room(self) -> None:
        class RunningProcess:
            def __init__(self, pid: int) -> None:
                self.pid = pid
                self.returncode: int | None = None

            def poll(self) -> int | None:
                return self.returncode

            def wait(self, timeout: int | None = None) -> int:
                self.returncode = 0
                return 0

        comparison = ComparisonRun(Path("/tmp"), ComparisonBroadcast())
        comparison.generation = 1
        comparison.sides = {
            "left": {"status": "live"},
            "right": {"status": "live", "feed": {"projection": {"room_seq": 9}}},
        }
        comparison.processes = {"left": RunningProcess(11), "right": RunningProcess(22)}
        with patch("examples.tower_of_hanoi.comparison_canvas.os.killpg") as killpg:
            result = comparison.end_side("left")
        self.assertEqual(result, {"status": "ended", "side": "left"})
        self.assertEqual(comparison.sides["left"]["status"], "ended")
        self.assertEqual(comparison.sides["right"]["status"], "live")
        self.assertEqual(comparison.sides["right"]["feed"]["projection"]["room_seq"], 9)
        self.assertNotIn("left", comparison.processes)
        self.assertIn("right", comparison.processes)
        self.assertEqual(killpg.call_args_list, [call(11, signal.SIGTERM)])

    def test_room_controls_reject_unknown_side_or_missing_process(self) -> None:
        comparison = ComparisonRun(Path("/tmp"), ComparisonBroadcast())
        comparison.sides = {"left": {"status": "live"}, "right": {"status": "live"}}
        with self.assertRaises(ValueError):
            comparison.end_side("middle")
        with self.assertRaises(ValueError):
            comparison.pause_side("left", paused=True)

    def test_participant_seat_names_are_unique_and_reference_safe(self) -> None:
        names = _participant_seat_names(40)
        self.assertEqual(len(names), 40)
        self.assertEqual(len(set(names)), 40)
        for name in names:
            self.assertRegex(name, r"[a-z0-9][a-z0-9-]*")

    def test_comparison_start_assigns_named_participants_and_shared_seed(self) -> None:
        class FakeProcess:
            stdout = None
            pid = 1

            def poll(self) -> int:
                return 0

            def wait(self) -> int:
                return 0

            def terminate(self) -> None:
                return None

        with tempfile.TemporaryDirectory() as directory:
            comparison = ComparisonRun(Path(directory), ComparisonBroadcast())
            with patch(
                "examples.tower_of_hanoi.comparison_canvas.subprocess.Popen",
                return_value=FakeProcess(),
            ) as popen:
                comparison.start(
                    {
                        "disks": 3,
                        "duration_seconds": 5,
                        "left_solver_count": 2,
                        "right_solver_count": 2,
                        "left_provider": "openrouter",
                        "left_model": "openai/gpt-5.6-luna",
                        "jev_model": "jev-1.13.0",
                        "warmup_seconds": 5,
                        "randomize_board": True,
                        "board_seed": 42,
                    }
                )
            commands = [call.args[0] for call in popen.call_args_list]
            seats: list[str] = []
            for command in commands:
                for index, argument in enumerate(command):
                    if argument == "--solver-seat":
                        seats.append(command[index + 1])
            self.assertEqual(len(seats), 4)
            self.assertEqual(len(set(seats)), 4)
            self.assertFalse(any("-solver-" in seat for seat in seats))
            self.assertEqual(comparison.sides["left"]["seats"], seats[:2])
            self.assertEqual(comparison.sides["right"]["seats"], seats[2:])
            for command in commands:
                self.assertIn("--randomize-board", command)
                seed_index = command.index("--board-seed")
                self.assertEqual(command[seed_index + 1], "42")
            comparison.stop()

    def test_comparison_start_launches_two_independent_harnesses_with_one_barrier(self) -> None:
        class FakeProcess:
            stdout = None
            pid = 1

            def poll(self) -> int:
                return 0

            def wait(self) -> int:
                return 0

            def terminate(self) -> None:
                return None

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            broadcast = ComparisonBroadcast()
            comparison = ComparisonRun(root, broadcast)
            with patch(
                "examples.tower_of_hanoi.comparison_canvas.subprocess.Popen",
                return_value=FakeProcess(),
            ) as popen:
                result = comparison.start(
                    {
                        "disks": 4,
                        "duration_seconds": 5,
                        "left_solver_count": 2,
                        "right_solver_count": 3,
                        "left_provider": "openrouter",
                        "left_model": "openai/gpt-5.6-luna",
                        "jev_model": "jev-1.13.0",
                        "warmup_seconds": 5,
                    }
                )
            self.assertEqual(result["status"], "preparing")
            self.assertEqual(popen.call_count, 2)
            commands = [call.args[0] for call in popen.call_args_list]
            self.assertTrue(all("--start-file" in command for command in commands))
            self.assertTrue(any("--solver-engine" in command and "openrouter" in command for command in commands))
            self.assertTrue(any("--solver-engine" in command and "jev" in command for command in commands))
            self.assertEqual(comparison.sides["left"]["solver_count"], 2)
            self.assertEqual(comparison.sides["right"]["solver_count"], 3)
            comparison.stop()

    def test_broadcast_replays_the_latest_bounded_comparison_event(self) -> None:
        broadcast = ComparisonBroadcast()
        broadcast.publish({"type": "comparison", "status": "running"})
        subscriber = broadcast.subscribe(None)
        self.assertIsNotNone(subscriber)
        assert subscriber is not None
        _cursor, payload = subscriber.get_nowait()
        self.assertIn('"status":"running"', payload)

    def test_relay_preserves_projection_and_orders_sanitized_room_events(self) -> None:
        broadcast = ComparisonBroadcast()
        with tempfile.TemporaryDirectory() as directory:
            comparison = ComparisonRun(Path(directory), broadcast)
            comparison.generation = 1
            comparison.sides["left"] = {"status": "live"}
            projection = {"room_seq": 12, "board": {"A": [1], "B": [], "C": []}}
            comparison._apply_canvas_payload(
                "left",
                {
                    "type": "hanoi",
                    "projection": projection,
                    "events": [
                        {
                            "kind": "disk_moved",
                            "actor": "solver-a",
                            "action_type": "move_disk",
                            "status": "observed",
                            "room_seq": 12,
                            "work_revision": 1,
                            "move": {"from": "A", "to": "C", "disk": 1},
                        }
                    ],
                },
                1,
            )
            comparison._apply_canvas_payload(
                "left", {"type": "status", "state": "reconnecting"}, 1
            )
            comparison._apply_canvas_payload(
                "left",
                {
                    "type": "hanoi",
                    "kind": "receipt",
                    "events": [
                        {
                            "kind": "move_disk_accepted",
                            "actor": "solver-a",
                            "action_type": "move_disk",
                            "status": "accepted",
                            "room_seq": 12,
                            "work_revision": 1,
                        }
                    ],
                },
                1,
            )
            self.assertEqual(comparison.sides["left"]["feed"]["projection"], projection)
            self.assertEqual(comparison.sides["left"]["feed_status"], "live")
            self.assertEqual(
                [(event["room_seq"], event["status"]) for event in comparison.event_logs["left"]],
                [(12, "observed"), (12, "accepted")],
            )
            public_side = broadcast.latest["sides"]["left"]
            self.assertEqual(len(public_side["events"]), 2)

    def test_relay_keeps_bounded_decision_trace_separate_from_event_feed(self) -> None:
        broadcast = ComparisonBroadcast()
        comparison = ComparisonRun(Path("/tmp"), broadcast)
        comparison.generation = 1
        comparison.sides["right"] = {"status": "live"}
        decision = _decision_from_invocation(
            {
                "engine": "jev",
                "model": "jev-latest",
                "provider_model": "jev-1.13.0",
                "reason": "board_changed",
                "status": "accepted",
                "action_type": "move_disk",
                "choice": "move:A>C:1",
                "confidence": 0.99,
                "latency_ms": 561,
                "room_seq": 7,
                "work_revision": 7,
            },
            "right-solver-1",
        )
        self.assertIsNotNone(decision)
        assert decision is not None
        self.assertEqual(decision["disposition"], "act")
        comparison._apply_canvas_payload(
            "right", {"type": "hanoi", "events": [decision]}, 1
        )
        comparison._apply_canvas_payload(
            "right", {"type": "hanoi", "events": [decision]}, 1
        )
        self.assertEqual(len(comparison.decision_logs["right"]), 1)
        self.assertEqual(comparison.decision_logs["right"][0]["selected"]["label"], "move:A>C:1")
        self.assertEqual(len(broadcast.latest["sides"]["right"]["decisions"]), 1)
        self.assertEqual(len(broadcast.latest["sides"]["right"]["events"]), 1)

    def test_relay_synthesizes_wait_trace_for_provider_failure_without_action(self) -> None:
        decision = _decision_from_invocation(
            {
                "engine": "openrouter",
                "model": "openai/gpt-5.6-luna",
                "reason": "bootstrap",
                "status": "error",
                "code": "provider_request_failed",
                "latency_ms": 234,
                "current_room_seq": 0,
            },
            "left-solver-1",
        )
        self.assertIsNotNone(decision)
        assert decision is not None
        self.assertEqual(decision["disposition"], "wait")
        self.assertEqual(decision["selected"], {"label": "wait"})
        self.assertEqual(decision["status"], "error")

    def test_backend_wait_trace_survives_sink_and_comparison_relay(self) -> None:
        event, deadline = _safe_sink_event(
            {
                "schema": "worldstream/tower-of-hanoi-decision/v1",
                "kind": "participant_decision",
                "actor": "right-solver-1",
                "engine": "jev",
                "model": "jev-1.13.0",
                "observed_room_seq": 32,
                "activation_reason": "claim_review_requested",
                "disposition": "wait",
                "selected": {"label": "wait"},
                "latency_ms": 44,
                "recent_event_count": 32,
                "cursor": {"from": 1, "to": 32},
            }
        )
        self.assertIsNone(deadline)
        self.assertIsNotNone(event)
        assert event is not None
        comparison = ComparisonRun(Path("/tmp"), ComparisonBroadcast())
        comparison.generation = 1
        comparison.sides["right"] = {"status": "live"}
        comparison._apply_canvas_payload(
            "right", {"type": "hanoi", "events": [event]}, 1
        )
        self.assertEqual(comparison.decision_logs["right"][0]["disposition"], "wait")
        self.assertEqual(comparison.decision_logs["right"][0]["selected"], {"label": "wait"})
        self.assertEqual(comparison.decision_logs["right"][0]["recent_event_count"], 32)

    def test_deadline_clears_infinite_hold_children_without_erasing_final_feed(self) -> None:
        class FakeProcess:
            pid = 12
            returncode = None
            wait_calls: list[int | None]

            def __init__(self) -> None:
                self.wait_calls = []

            def poll(self) -> int | None:
                return self.returncode

            def wait(self, timeout: int | None = None) -> int:
                self.wait_calls.append(timeout)
                self.returncode = 0
                return 0

        with tempfile.TemporaryDirectory() as directory:
            comparison = ComparisonRun(Path(directory), ComparisonBroadcast())
            comparison.generation = 4
            left = FakeProcess()
            right = FakeProcess()
            comparison.processes = {"left": left, "right": right}
            comparison.deadline_at_ms = int(time.time() * 1000) - 1
            comparison.sides = {
                "left": {"status": "live", "feed": {"projection": {"room_seq": 2}}},
                "right": {"status": "live", "feed": {"projection": {"room_seq": 3}}},
            }
            with patch("examples.tower_of_hanoi.comparison_canvas.os.killpg") as killpg:
                comparison._timer_loop()
            self.assertEqual(killpg.call_count, 2)
            self.assertEqual(left.wait_calls, [30])
            self.assertEqual(right.wait_calls, [30])
            self.assertEqual(comparison.processes, {})
            self.assertEqual(comparison.sides["left"]["status"], "time_limit")
            self.assertEqual(comparison.sides["right"]["feed"]["projection"]["room_seq"], 3)
            self.assertEqual(comparison.broadcast.latest["status"], "time_limit")

    def test_child_cleanup_sends_term_and_waits_before_escalating(self) -> None:
        class RunningProcess:
            def __init__(self, pid: int) -> None:
                self.pid = pid
                self.returncode: int | None = None
                self.wait_calls: list[int | None] = []

            def poll(self) -> int | None:
                return self.returncode

            def wait(self, timeout: int | None = None) -> int:
                self.wait_calls.append(timeout)
                self.returncode = 0
                return 0

        with tempfile.TemporaryDirectory() as directory:
            comparison = ComparisonRun(Path(directory), ComparisonBroadcast())
            left = RunningProcess(101)
            right = RunningProcess(202)
            comparison.processes = {"left": left, "right": right}
            with patch("examples.tower_of_hanoi.comparison_canvas.os.killpg") as killpg, comparison.lock:
                comparison._kill_children_locked()
            self.assertEqual(
                killpg.call_args_list,
                [
                    call(101, signal.SIGTERM),
                    call(202, signal.SIGTERM),
                ],
            )
            self.assertEqual(left.wait_calls, [30])
            self.assertEqual(right.wait_calls, [30])
            self.assertEqual(comparison.processes, {})

    def test_child_eof_before_terminal_status_publishes_closed_error(self) -> None:
        class EarlyExitProcess:
            stdout = io.StringIO("")

            def poll(self) -> int:
                return 1

            def wait(self) -> int:
                return 1

        with tempfile.TemporaryDirectory() as directory:
            broadcast = ComparisonBroadcast()
            comparison = ComparisonRun(Path(directory), broadcast)
            comparison.generation = 2
            comparison.sides["left"] = {"status": "starting"}

            comparison._read_child("left", EarlyExitProcess(), 2)

            self.assertEqual(comparison.sides["left"]["status"], "error")
            self.assertEqual(comparison.sides["left"]["code"], "child_exited")
            self.assertEqual(comparison.sides["left"]["exit_code"], 1)
            self.assertEqual(broadcast.latest["status"], "error")
            self.assertEqual(broadcast.latest["timer"]["status"], "error")

    def test_preparation_error_fails_fast_and_aborts_peer(self) -> None:
        class ErrorProcess:
            pid = 303
            stdout = io.StringIO('{"status":"error","code":"room_setup_incomplete"}\n')

            def __init__(self) -> None:
                self.returncode: int | None = None

            def poll(self) -> int | None:
                return self.returncode

            def wait(self, timeout: int | None = None) -> int:
                self.returncode = 3
                return 3

        class PeerProcess(ErrorProcess):
            pid = 404
            stdout = io.StringIO("")

        with tempfile.TemporaryDirectory() as directory:
            broadcast = ComparisonBroadcast()
            comparison = ComparisonRun(Path(directory), broadcast)
            comparison.generation = 7
            comparison.config = {"disks": 3}
            comparison.sides = {
                "left": {"status": "starting"},
                "right": {"status": "live", "feed": {"projection": {"room_seq": 4}}},
            }
            comparison.processes = {"left": ErrorProcess(), "right": PeerProcess()}

            with patch("examples.tower_of_hanoi.comparison_canvas.os.killpg") as killpg:
                comparison._read_child("left", comparison.processes["left"], 7)

            self.assertEqual(comparison.sides["left"]["status"], "error")
            self.assertEqual(comparison.sides["left"]["code"], "room_setup_incomplete")
            self.assertEqual(comparison.sides["right"]["status"], "aborted")
            self.assertEqual(comparison.sides["right"]["code"], "peer_failed")
            self.assertEqual(comparison.sides["right"]["feed"]["projection"]["room_seq"], 4)
            self.assertEqual(comparison.processes, {})
            self.assertIsNone(comparison.config)
            self.assertEqual(broadcast.latest["status"], "error")
            self.assertEqual(broadcast.latest["timer"]["status"], "error")
            self.assertEqual(killpg.call_count, 2)


if __name__ == "__main__":
    unittest.main()

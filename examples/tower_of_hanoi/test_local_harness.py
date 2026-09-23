"""Focused control-boundary tests for autonomous local Hanoi supervisors."""

from __future__ import annotations

import argparse
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from examples.tower_of_hanoi.local_harness import (
    LIVE_OBSERVER_CAPABILITY,
    REPLAY_VERIFIER_CAPABILITY,
    HarnessError,
    LocalRun,
    PackIdentity,
    RunnerCredential,
    SeatCredential,
    _arguments,
    _validate_seats,
    codex_command,
    local_worldstream_environment,
    manual_setup,
    member_capability_request,
    normalize_initial_board,
    random_initial_board,
    room_create_arguments,
)
from examples.tower_of_hanoi.participant_runner import participant_prompt
from examples.tower_of_hanoi.protocol import HanoiProtocolError, safe_activity

PACK = PackIdentity(
    "worldstream.tower-of-hanoi", "0.1.0", "blake3:" + "a" * 64, "blake3:" + "b" * 64
)


class HarnessParsingTests(unittest.TestCase):
    def test_community_demo_parser_sets_five_seats_canvas_four_disks_and_300_seconds(
        self,
    ) -> None:
        with patch("sys.argv", ["local_harness", "--community-demo"]):
            arguments = _arguments()
        self.assertTrue(arguments.canvas_demo)
        self.assertEqual(
            arguments.solver_seat,
            ["solver-a", "solver-b", "solver-c", "solver-d", "solver-e"],
        )
        self.assertEqual(arguments.disks, 4)
        self.assertEqual(arguments.gameplay_seconds, 300)
        self.assertEqual(arguments.move_limit, 10_000)

    def test_explicit_disk_count_overrides_community_default_and_parser_is_smoke_safe(
        self,
    ) -> None:
        with patch(
            "sys.argv",
            ["local_harness", "--community-demo", "--disks", "10", "--codex", "codex"],
        ):
            arguments = _arguments()
        self.assertEqual(arguments.disks, 10)
        self.assertEqual(str(arguments.codex), "codex")

    def test_random_board_is_reachable_and_seed_deterministic(self) -> None:
        board = random_initial_board(4, seed=11)
        self.assertEqual(set(board), {"A", "B", "C"})
        self.assertEqual(
            sorted(disk for stack in board.values() for disk in stack), [1, 2, 3, 4]
        )
        for stack in board.values():
            self.assertEqual(stack, sorted(stack, reverse=True))
        self.assertEqual(random_initial_board(4, seed=11), board)
        self.assertEqual(
            normalize_initial_board(board, 4),
            board,
        )

    def test_setup_carries_a_reviewed_initial_board(self) -> None:
        setup = manual_setup(
            PACK,
            ["solver-a"],
            "observer",
            disks=3,
            initial_board={"A": [3], "B": [2, 1], "C": []},
        )
        self.assertEqual(
            setup["configuration"],
            {"disks": 3, "move_limit": 10_000, "initial_board": {"A": [3], "B": [2, 1], "C": []}},
        )
        with self.assertRaisesRegex(HarnessError, "initial_board_invalid"):
            normalize_initial_board({"A": [3, 3], "B": [], "C": []}, 3)
        with self.assertRaisesRegex(HarnessError, "initial_board_invalid"):
            normalize_initial_board({"A": [2, 3], "B": [1], "C": []}, 3)

    def test_parser_randomizes_or_reads_an_initial_board(self) -> None:
        with patch(
            "sys.argv",
            ["local_harness", "--disks", "3", "--randomize-board", "--board-seed", "5"],
        ):
            randomized = _arguments()
        self.assertIsNotNone(randomized.initial_board)
        self.assertEqual(randomized.initial_board, random_initial_board(3, 5))
        with patch(
            "sys.argv",
            ["local_harness", "--disks", "3", "--initial-board", '{"A":[3],"B":[2,1],"C":[]}'],
        ):
            explicit = _arguments()
        self.assertEqual(explicit.initial_board, {"A": [3], "B": [2, 1], "C": []})

    def test_pack_and_setup_allow_one_to_sixteen_solver_participants(self) -> None:
        setup = manual_setup(PACK, ["solver-a"], "observer", disks=10)
        self.assertEqual(setup["configuration"], {"disks": 10, "move_limit": 10_000})
        _validate_seats([f"solver-{index}" for index in range(16)], "observer")
        with self.assertRaisesRegex(HarnessError, "solver_seats_invalid"):
            _validate_seats([f"solver-{index}" for index in range(17)], "observer")

    def test_runner_prompt_states_objective_but_never_injects_a_solution_or_bearer(
        self,
    ) -> None:
        prompt = participant_prompt("bootstrap")
        self.assertIn("full tower from A to C", prompt)
        self.assertIn("post_completion_claim", prompt)
        self.assertIn("assess_claim", prompt)
        self.assertNotIn("wsb1:", prompt.lower())
        self.assertNotIn("optimal", prompt.lower())
        self.assertNotIn("next move", prompt.lower())

    def test_codex_command_only_exposes_a_credential_path_to_an_inherit_none_shell(
        self,
    ) -> None:
        command = codex_command(
            Path("codex"),
            Path("/seat-workspace"),
            "low",
            participant_prompt("bootstrap"),
            repo=Path("/repo"),
            python=Path("/sdk/python/.venv/bin/python"),
            membership_file=Path("/private/solver-a.json"),
        )
        self.assertIn('shell_environment_policy.inherit="none"', command)
        shell_set = next(
            item for item in command if item.startswith("shell_environment_policy.set=")
        )
        self.assertIn("HANOI_MEMBERSHIP_FILE", shell_set)
        self.assertNotIn("wsb1:", "\n".join(command).lower())

    def test_local_environment_keeps_codex_login_context_but_drops_worldstream_configuration(
        self,
    ) -> None:
        environment = local_worldstream_environment(
            {
                "PATH": "/bin",
                "HOME": "/home/demo",
                "WORLDSTREAM_CONFIG": "/shared/config",
            }
        )
        self.assertEqual(environment, {"PATH": "/bin", "HOME": "/home/demo"})

    def test_capability_requests_remain_actionless_and_identity_bound(self) -> None:
        membership = {
            "room_id": "room-a",
            "member_id": "member-a",
            "principal_id": "principal-a",
        }
        for capability in (LIVE_OBSERVER_CAPABILITY, REPLAY_VERIFIER_CAPABILITY):
            request = member_capability_request(
                membership, capability, "01ARZ3NDEKTSV4RRFFQ69G5FC8"
            )
            self.assertNotIn("room:act", request["scopes"])
            self.assertEqual(request["member_id"], "member-a")

    def test_room_create_explicitly_acknowledges_activity_start(self) -> None:
        command = room_create_arguments(
            Path("/private/setup.json"), ["--config", "/private/run.toml"]
        )
        self.assertEqual(
            command[:5],
            ["room", "create", "--file", "/private/setup.json", "--acknowledge-start"],
        )

    def test_supervisor_command_has_paired_credentials_and_no_controller_action_flags(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            evidence = root / "evidence"
            evidence.mkdir(mode=0o700)
            canvas = root / "canvas.jsonl"
            canvas.write_text("", encoding="utf-8")
            canvas.chmod(0o600)
            run = object.__new__(LocalRun)
            run.arguments = argparse.Namespace(
                python=Path("/python"),
                codex=Path("/codex"),
                solver_effort="low",
                codex_timeout_seconds=30,
            )
            run.repo = Path("/repo")
            run.evidence = evidence
            run.canvas_sink = canvas
            run.gameplay_deadline_epoch_ms = 12345
            run.gameplay_deadline_at = 1.0
            run.participant_processes = []
            records: list[dict[str, object]] = []
            run._append_evidence = lambda _name, value: records.append(value)  # type: ignore[method-assign]
            solver = SeatCredential(
                "solver-a", root / "solver.json", "room-a", "member-a", "solver", {}
            )
            runner = RunnerCredential("solver-a", root / "runner.json")
            with patch(
                "examples.tower_of_hanoi.local_harness.subprocess.Popen"
            ) as popen:
                process = popen.return_value
                process.poll.return_value = None
                run.start_participant_supervisors([solver], [runner])
            command = popen.call_args.args[0]
            self.assertIn("examples.tower_of_hanoi.participant_runner", command)
            self.assertIn("--membership-file", command)
            self.assertIn("--runner-file", command)
            self.assertIn("--event-file", command)
            self.assertNotIn("--action-type", command)
            self.assertNotIn("move_disk", command)
            supervisor_evidence = evidence / "supervisor-solver-a"
            self.assertEqual(oct(supervisor_evidence.stat().st_mode & 0o777), "0o700")
            self.assertEqual(
                oct((supervisor_evidence / "stdout.jsonl").stat().st_mode & 0o777),
                "0o600",
            )
            self.assertEqual(records[0]["kind"], "participant_supervisor_started")

    def test_stop_cleans_managed_server_when_start_was_interrupted(self) -> None:
        run = object.__new__(LocalRun)
        run.arguments = argparse.Namespace(timeout_seconds=1)
        run.config_path = Path("/run/.worldstream/worldstream.toml")
        run.state_dir = Path("/run/.worldstream/studio")
        run.controller_address = "127.0.0.1:12345"
        run.participant_processes = []
        run.canvas_process = None
        run.runtime_reservation = None
        run.controller_reservation = None
        run.canvas_reservation = None
        run.started = False
        run.server_start_attempted = True
        run.command = Mock(return_value={})

        run.stop()

        commands = [entry.args[0][:2] for entry in run.command.call_args_list]
        self.assertEqual(commands, [["server", "stop"], ["server", "controller-stop"]])

    def test_partial_room_setup_resumes_same_operation_until_complete(self) -> None:
        operation = "op-01partial"
        run = object.__new__(LocalRun)
        run.arguments = argparse.Namespace(timeout_seconds=1)
        run.config_path = Path("/run/.worldstream/worldstream.toml")
        run.state_dir = Path("/run/.worldstream/studio")
        run.controller_address = "127.0.0.1:12345"
        run.command = Mock(
            side_effect=[
                {"status": "partial", "operation_id": operation},
                {"status": "partial", "operation_id": operation},
                {
                    "status": "complete",
                    "operation_id": operation,
                    "room_id": "01ROOMCOMPLETE",
                },
            ]
        )

        result = run._resume_room_setup(
            {
                "status": "partial",
                "code": "setup_incomplete",
                "operation_id": operation,
                "room_id": "01ROOMPARTIAL",
            }
        )

        self.assertEqual(result["status"], "complete")
        commands = [entry.args[0][:4] for entry in run.command.call_args_list]
        self.assertEqual(
            commands,
            [
                ["room", "setup", "status", operation],
                ["room", "setup", "resume", operation],
                ["room", "setup", "status", operation],
            ],
        )

    def test_safe_activity_requires_public_objective(self) -> None:
        activity = {
            "board": {"A": [1], "B": [], "C": []},
            "disks": 1,
            "objective": {
                "source_rod": "A",
                "target_rod": "C",
                "description": "Participants may aim to move the full tower from A to C.",
            },
            "outcome": {"moves": 0, "status": "in_progress"},
            "phase": "solving",
            "round": 1,
            "work_revision": 0,
            "completion": {
                "claim_open": False,
                "assessments_by_member": {},
                "endorsement_count": 0,
                "approval_count": 0,
                "quorum": 1,
            },
        }
        self.assertEqual(safe_activity(activity)["objective"]["target_rod"], "C")
        activity.pop("objective")
        with self.assertRaisesRegex(HanoiProtocolError, "objective_invalid"):
            safe_activity(activity)


if __name__ == "__main__":
    unittest.main()

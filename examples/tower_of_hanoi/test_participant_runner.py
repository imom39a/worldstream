"""Focused operational tests for the autonomous Hanoi Runner supervisor."""

from __future__ import annotations

import os
import subprocess
import tempfile
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from unittest.mock import patch

from examples.tower_of_hanoi.participant_runner import _invoke, participant_prompt


class _SuccessfulProcess:
    """A completed Codex subprocess for concurrent evidence-path regression tests."""

    pid = 4343
    returncode = 0

    def poll(self) -> int:
        return self.returncode

    def communicate(self, timeout: float | None = None) -> tuple[str, str]:
        return '{"status":"complete"}\n', ""


class _TimedOutProcess:
    pid = 4242

    def __init__(self) -> None:
        self.returncode: int | None = None

    def poll(self) -> int | None:
        return self.returncode

    def communicate(self, timeout: float | None = None) -> tuple[str, str]:
        if self.returncode is None:
            raise subprocess.TimeoutExpired("codex", timeout)
        return "", ""

    def wait(self, timeout: float | None = None) -> int:
        self.returncode = -15
        return self.returncode


class ParticipantRunnerTests(unittest.TestCase):
    def test_prompt_is_bounded_to_one_participant_and_rejects_unknown_reason(
        self,
    ) -> None:
        prompt = participant_prompt("claim_review_requested")
        self.assertIn("at most two action attempts", prompt)
        self.assertIn("full tower from A to C", prompt)
        with self.assertRaisesRegex(Exception, "activation_reason_invalid"):
            participant_prompt("untrusted")

    def test_timeout_kills_the_codex_process_group_and_keeps_login_environment(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            evidence = root / "supervisor.events.jsonl"
            timed_out = _TimedOutProcess()
            with (
                patch.dict(
                    os.environ,
                    {
                        "PATH": "/bin",
                        "HOME": "/home/demo",
                        "WORLDSTREAM_CONFIG": "/leak",
                    },
                    clear=True,
                ),
                patch(
                    "examples.tower_of_hanoi.participant_runner.subprocess.Popen",
                    return_value=timed_out,
                ) as popen,
                patch("examples.tower_of_hanoi.participant_runner.os.killpg") as killpg,
            ):
                result = _invoke(
                    codex=Path("codex"),
                    python=Path("/python"),
                    repo=Path("/repo"),
                    membership_file=root / "solver.json",
                    effort="low",
                    timeout_seconds=0.01,
                    reason="bootstrap",
                    evidence=evidence,
                    invocation_name="timeout",
                    seat="solver-a",
                )
            self.assertFalse(result)
            self.assertTrue(killpg.called)
            kwargs = popen.call_args.kwargs
            self.assertTrue(kwargs["start_new_session"])
            self.assertEqual(kwargs["env"]["HOME"], "/home/demo")
            self.assertNotIn("WORLDSTREAM_CONFIG", kwargs["env"])
            self.assertEqual(
                oct((root / "timeout.jsonl").stat().st_mode & 0o777), "0o600"
            )
            self.assertIn('"timed_out":true', evidence.read_text(encoding="utf-8"))

    def test_concurrent_isolated_bootstraps_create_owner_only_transcripts_without_collision(
        self,
    ) -> None:
        """Five supervisors can all name their first invocation bootstrap safely."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            seats = ("solver-a", "solver-b", "solver-c", "solver-d", "solver-e")

            def bootstrap(seat: str) -> tuple[str, bool]:
                parent = root / f"supervisor-{seat}"
                parent.mkdir(mode=0o700)
                return (
                    seat,
                    _invoke(
                        codex=Path("codex"),
                        python=Path("/python"),
                        repo=Path("/repo"),
                        membership_file=root / f"{seat}.json",
                        effort="low",
                        timeout_seconds=1,
                        reason="bootstrap",
                        evidence=parent / "events.jsonl",
                        invocation_name="bootstrap",
                        seat=seat,
                    ),
                )

            with (
                patch(
                    "examples.tower_of_hanoi.participant_runner.subprocess.Popen",
                    side_effect=lambda *_args, **_kwargs: _SuccessfulProcess(),
                ),
                ThreadPoolExecutor(max_workers=len(seats)) as executor,
            ):
                results = dict(executor.map(bootstrap, seats))
            self.assertEqual(results, {seat: True for seat in seats})
            self.assertFalse((root / "bootstrap.jsonl").exists())
            for seat in seats:
                parent = root / f"supervisor-{seat}"
                self.assertEqual(oct(parent.stat().st_mode & 0o777), "0o700")
                transcript = parent / "bootstrap.jsonl"
                self.assertTrue(transcript.is_file())
                self.assertEqual(oct(transcript.stat().st_mode & 0o777), "0o600")
                self.assertIn(
                    '"name":"bootstrap"',
                    (parent / "events.jsonl").read_text(encoding="utf-8"),
                )

    def test_cancelled_invocation_uses_the_same_process_group_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            evidence = root / "supervisor.events.jsonl"
            cancelled = threading.Event()
            cancelled.set()
            process = _TimedOutProcess()
            with (
                patch(
                    "examples.tower_of_hanoi.participant_runner.subprocess.Popen",
                    return_value=process,
                ),
                patch("examples.tower_of_hanoi.participant_runner.os.killpg") as killpg,
            ):
                self.assertFalse(
                    _invoke(
                        codex=Path("codex"),
                        python=Path("/python"),
                        repo=Path("/repo"),
                        membership_file=root / "solver.json",
                        effort="low",
                        timeout_seconds=10,
                        reason="bootstrap",
                        evidence=evidence,
                        invocation_name="cancelled",
                        seat="solver-a",
                        cancelled=cancelled,
                    )
                )
            self.assertTrue(killpg.called)
            self.assertIn('"cancelled":true', evidence.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()

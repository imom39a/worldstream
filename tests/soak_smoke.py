#!/usr/bin/env python3
"""Boundary tests for scripts/soak-smoke.sh."""

from __future__ import annotations

import json
import os
import stat
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "soak-smoke.sh"

TEST_NAMES = [
    "tests::action_advance_is_one_atomic_bundle_and_unknown_commit_resolves_original",
    "tests::actual_read_only_driver_error_rolls_back_and_the_identical_plan_retries",
    "tests::authority_current_anchor_rejects_tampered_or_misaddressed_seq_two_predecessor",
    "tests::corrupt_paired_snapshot_is_disposable_and_lineage_rebuild_is_exact",
    "tests::create_duplicate_conflict_and_guarded_resolve_use_the_durable_receipt",
    "tests::every_create_write_failpoint_rolls_back_the_entire_bundle",
    "tests::faulted_replay_rejects_and_quarantines_an_impossible_same_generation_append",
    "tests::migration_failure_is_atomic_and_restart_retries_the_same_forward_prefix",
    "tests::observation_attach_ack_prune_and_suffix_recovery_preserve_delivery_positions",
    "tests::property_based_replay_fuzz_probe",
    "tests::postcommit_snapshot_binds_the_complete_head_and_all_state_hashes",
    "tests::recovery_reproduces_join_reset_and_following_visibility_loss",
    "tests::replay_verification_rejects_each_corrupt_or_missing_durable_history_component",
    "tests::canonical_hash_parity_round_trip_is_exact",
    "tests::sealed_native_envelope_constructs_and_verifies_real_online_restore",
    "tests::timer_advance_unknown_commit_duplicate_and_failpoint_rollbacks_are_exact",
    "tests::writer_lock_rejects_a_second_owner_and_releases_for_restart",
]


def write_fake_cargo(
    directory: Path,
    *,
    mode: str = "normal",
    include_corruption: bool = True,
    include_fuzz: bool = True,
) -> Path:
    cargo = directory / "cargo"
    selected_names = [
        name
        for name in TEST_NAMES
        if include_corruption
        or not any(
            token in name for token in ("corrupt", "quarantine", "missing_durable")
        )
        if include_fuzz or "fuzz" not in name
    ]
    names = "\n".join(f"{name}: test" for name in selected_names)
    script = textwrap.dedent(
        f"""
        #!/usr/bin/env python3
        import sys
        import time
        args = sys.argv[1:]
        if {mode!r} == "timeout":
            time.sleep(3)
        if "--list" in args:
            print({names!r})
            raise SystemExit(0)
        print("test result: ok. {len(selected_names)} passed; 0 failed; 0 ignored")
        raise SystemExit(0)
        """
    )
    cargo.write_text(script.lstrip(), encoding="utf-8")
    cargo.chmod(cargo.stat().st_mode | stat.S_IXUSR)
    return cargo


class SoakSmokeBoundaryTests(unittest.TestCase):
    def parse_report(self, completed: subprocess.CompletedProcess[str]) -> dict:
        lines = completed.stdout.strip().splitlines()
        self.assertTrue(
            lines,
            "soak harness produced no machine-readable report; "
            f"returncode={completed.returncode}, stderr={completed.stderr!r}",
        )
        try:
            report = json.loads(lines[-1])
        except json.JSONDecodeError as error:
            self.fail(
                "soak harness final line was not valid JSON: "
                f"{lines[-1]!r}; stderr={completed.stderr!r}; error={error}"
            )
        self.assertIsInstance(report, dict)
        return report

    def run_harness(self, *, mode: str = "normal", extra: list[str] | None = None):
        with tempfile.TemporaryDirectory(prefix="worldstream-soak-test-") as temp:
            temp_path = Path(temp)
            write_fake_cargo(temp_path, mode=mode)
            env = os.environ.copy()
            env["PATH"] = f"{temp_path}{os.pathsep}{env.get('PATH', '')}"
            command = [
                str(SCRIPT),
                "--iterations",
                "1",
                "--command-timeout-seconds",
                "1",
                "--max-total-seconds",
                "10",
                "--max-output-bytes",
                "4096",
            ]
            if extra:
                command.extend(extra)
            completed = subprocess.run(
                command,
                cwd=ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            report = self.parse_report(completed)
            return completed, report

    def test_missing_python_launcher_reports_a_bounded_failure(self):
        with tempfile.TemporaryDirectory(prefix="worldstream-soak-no-python-") as temp:
            temp_path = Path(temp)
            write_fake_cargo(temp_path)
            completed = subprocess.run(
                [
                    "/bin/bash",
                    str(SCRIPT),
                    "--iterations",
                    "1",
                    "--command-timeout-seconds",
                    "1",
                    "--max-total-seconds",
                    "10",
                ],
                cwd=ROOT,
                env={"PATH": str(temp_path)},
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertNotEqual(completed.returncode, 0)
            self.assertEqual(completed.stdout, "")
            self.assertIn("python3", completed.stderr)

    def test_default_boundary_is_fast_and_machine_readable(self):
        completed, report = self.run_harness()
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(report["schema"], "worldstream/soak-evidence/v1")
        self.assertEqual(report["status"], "pass")
        self.assertFalse(report["release_evidence"])
        self.assertEqual(
            report["named_gate_evidence"]["manifest_evidence_id"],
            "failure-fuzz-resource-and-one-hour-sqlite-soak",
        )
        self.assertFalse(report["named_gate_evidence"]["release_evidence"])
        self.assertTrue(report["evidence_scope"]["fixture_only"])
        self.assertFalse(report["evidence_scope"]["process_level"])
        self.assertFalse(report["evidence_scope"]["database_workload_bound"])
        self.assertEqual(
            report["configuration"]["database_workload_binding"],
            "observer_only_not_bound_to_cargo_test_databases",
        )
        self.assertEqual(report["statistics"]["matrix_run_count"], 1)
        self.assertEqual(
            report["configuration"]["packages"],
            ["worldstream-sqlite", "worldstream-backup"],
        )
        self.assertEqual(
            report["configuration"]["matrix_command"],
            [
                "cargo",
                "test",
                "-p",
                "worldstream-sqlite",
                "-p",
                "worldstream-backup",
                "--locked",
            ],
        )
        self.assertEqual(
            report["preflight"]["test_list_parse"]["coverage_groups"]["fuzz"]["status"],
            "covered",
        )
        self.assertEqual(
            report["preflight"]["test_list_parse"]["coverage_groups"]["concurrency"][
                "status"
            ],
            "covered",
        )
        durations = report["statistics"]["command_duration_ms"]
        self.assertIsNotNone(durations["p50"])
        self.assertIsNotNone(durations["p95"])
        self.assertIsNotNone(durations["p99"])
        self.assertEqual(report["kill_points"]["status"], "not_exposed")
        self.assertFalse(report["kill_points"]["process_kill_claim"])
        self.assertNotIn(str(ROOT), completed.stdout)

    def test_output_file_is_redacted_and_database_is_optional(self):
        with tempfile.TemporaryDirectory(prefix="worldstream-soak-output-") as temp:
            output = Path(temp) / "evidence.json"
            completed, report = self.run_harness(extra=["--output", str(output)])
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertTrue(output.is_file())
            self.assertEqual(json.loads(output.read_text(encoding="utf-8")), report)
            self.assertEqual(report["database"]["status"], "not_configured")
            self.assertNotIn(str(output), output.read_text(encoding="utf-8"))

    def test_database_and_sidecar_sizes_are_reported_without_path_leak(self):
        with tempfile.TemporaryDirectory(prefix="worldstream-soak-database-") as temp:
            database = Path(temp) / "worldstream.sqlite3"
            database.write_bytes(b"sqlite-fixture")
            Path(f"{database}-wal").write_bytes(b"wal-fixture")
            completed, report = self.run_harness(extra=["--database", str(database)])
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertEqual(report["database"]["status"], "measured")
            self.assertEqual(report["database"]["growth_bytes"], 0)
            self.assertEqual(
                report["database"]["workload_binding"],
                "observer_only_not_bound_to_cargo_test_databases",
            )
            self.assertFalse(report["evidence_scope"]["database_workload_bound"])
            self.assertNotIn(str(database), completed.stdout)

    def test_missing_semantic_fixture_fails_closed(self):
        with tempfile.TemporaryDirectory(prefix="worldstream-soak-gap-") as temp:
            temp_path = Path(temp)
            write_fake_cargo(temp_path, include_corruption=False)
            env = os.environ.copy()
            env["PATH"] = f"{temp_path}{os.pathsep}{env.get('PATH', '')}"
            completed = subprocess.run(
                [
                    str(SCRIPT),
                    "--iterations",
                    "1",
                    "--command-timeout-seconds",
                    "1",
                    "--max-total-seconds",
                    "10",
                    "--max-output-bytes",
                    "4096",
                ],
                cwd=ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            report = self.parse_report(completed)
            self.assertNotEqual(completed.returncode, 0)
            self.assertEqual(report["status"], "error")
            self.assertIn(
                "corruption_or_quarantine",
                report["preflight"]["test_list_parse"]["missing_groups"],
            )

    def test_missing_fuzz_fixture_fails_closed(self):
        with tempfile.TemporaryDirectory(prefix="worldstream-soak-fuzz-gap-") as temp:
            temp_path = Path(temp)
            write_fake_cargo(temp_path, include_fuzz=False)
            env = os.environ.copy()
            env["PATH"] = f"{temp_path}{os.pathsep}{env.get('PATH', '')}"
            completed = subprocess.run(
                [
                    str(SCRIPT),
                    "--iterations",
                    "1",
                    "--command-timeout-seconds",
                    "1",
                    "--max-total-seconds",
                    "10",
                    "--max-output-bytes",
                    "4096",
                ],
                cwd=ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            report = self.parse_report(completed)
            self.assertNotEqual(completed.returncode, 0)
            self.assertEqual(report["status"], "error")
            self.assertIn(
                "fuzz", report["preflight"]["test_list_parse"]["missing_groups"]
            )
            self.assertIn("named evidence gaps", report["error"])

    def test_command_timeout_is_reported_without_claiming_success(self):
        completed, report = self.run_harness(mode="timeout")
        self.assertNotEqual(completed.returncode, 0)
        self.assertEqual(report["status"], "error")
        self.assertEqual(report["preflight"]["command"]["status"], "timeout")
        self.assertEqual(report["preflight"]["command"]["failure_class"], "timeout")
        self.assertFalse(report.get("kill_points", {}).get("process_kill_claim", True))

    def test_one_hour_mode_respects_a_lower_test_bound_without_promoting_evidence(self):
        completed, report = self.run_harness(
            extra=["--one-hour", "--max-total-seconds", "2", "--max-iterations", "1000"]
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(report["status"], "pass")
        self.assertLessEqual(report["elapsed_seconds"], 2.1)
        self.assertFalse(report["one_hour_window_completed"])
        self.assertFalse(report["release_evidence"])
        self.assertEqual(
            report["named_gate_evidence"]["gate_cell"],
            "failure-fuzz-resource-and-one-hour-sqlite-soak",
        )

    def test_total_bound_over_one_hour_fails_closed(self):
        completed, report = self.run_harness(extra=["--max-total-seconds", "3601"])
        self.assertNotEqual(completed.returncode, 0)
        self.assertEqual(report["status"], "error")
        self.assertIn("at most one hour", report["error"])
        self.assertFalse(report["release_evidence"])


if __name__ == "__main__":
    unittest.main()

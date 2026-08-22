#!/usr/bin/env python3
"""Fail-closed boundary tests for the disposable PostgreSQL evidence lane."""

from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "postgres-live-evidence.sh"
SCHEMA = "worldstream/postgresql-live-evidence/v1"


class PostgreSQLLiveEvidenceBoundaryTests(unittest.TestCase):
    def run_script(
        self, *args: str, **env_updates: str
    ) -> tuple[subprocess.CompletedProcess[str], dict[str, object]]:
        env = os.environ.copy()
        env.update(env_updates)
        completed = subprocess.run(
            [str(SCRIPT), *args],
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
        lines = [line for line in completed.stdout.splitlines() if line.strip()]
        self.assertTrue(lines, completed.stderr)
        value = json.loads(lines[-1])
        return completed, value

    def test_missing_sqlite_source_is_reported_without_docker_or_secret(self) -> None:
        completed, value = self.run_script(
            WORLDSTREAM_PG_LIVE_DOCKER=str(ROOT / "does-not-exist-docker"),
            WORLDSTREAM_PG_LIVE_PSQL=str(ROOT / "does-not-exist-psql"),
            WORLDSTREAM_PG_LIVE_CARGO=str(ROOT / "does-not-exist-cargo"),
        )
        self.assertEqual(completed.returncode, 10)
        self.assertEqual(value["schema"], SCHEMA)
        self.assertEqual(value["status"], "unavailable")
        self.assertEqual(value["reason"], "docker_unavailable")
        self.assertFalse(value["release_evidence"])
        self.assertFalse(value["secrets_emitted"])
        self.assertNotIn("password=", completed.stdout + completed.stderr)

    def test_non_regular_sqlite_source_fails_before_provider_start(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-pg-live-test-") as name:
            directory = Path(name)
            target = directory / "source.sqlite"
            target.write_text("not used", encoding="utf-8")
            link = directory / "source-link.sqlite"
            link.symlink_to(target)
            completed, value = self.run_script(
                "--sqlite",
                str(link),
                WORLDSTREAM_PG_LIVE_DOCKER=str(ROOT / "does-not-exist-docker"),
                WORLDSTREAM_PG_LIVE_PSQL=str(ROOT / "does-not-exist-psql"),
                WORLDSTREAM_PG_LIVE_CARGO=str(ROOT / "does-not-exist-cargo"),
            )
        self.assertEqual(completed.returncode, 13)
        self.assertEqual(value["status"], "incomplete")
        self.assertEqual(value["reason"], "sqlite_source_not_a_regular_file")
        self.assertIn("sqlite_source_not_a_regular_file", value["errors"])
        self.assertFalse(value["release_evidence"])

    def test_missing_docker_is_unavailable_not_a_provider_pass(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-pg-live-test-") as name:
            source = Path(name) / "source.sqlite"
            source.write_bytes(b"placeholder")
            completed, value = self.run_script(
                "--sqlite",
                str(source),
                WORLDSTREAM_PG_LIVE_DOCKER=str(ROOT / "does-not-exist-docker"),
                WORLDSTREAM_PG_LIVE_PSQL=sys.executable,
                WORLDSTREAM_PG_LIVE_CARGO=sys.executable,
            )
        self.assertEqual(completed.returncode, 10)
        self.assertEqual(value["reason"], "docker_unavailable")
        self.assertEqual(value["profiles"]["transaction_pooler"], "not_checked")
        self.assertEqual(value["transfer_parity_epoch"]["status"], "not_run")
        self.assertFalse(value["release_evidence"])

    def test_script_is_executable_and_help_is_side_effect_free(self) -> None:
        self.assertTrue(SCRIPT.stat().st_mode & stat.S_IXUSR)
        completed = subprocess.run(
            [str(SCRIPT), "--help"],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(completed.returncode, 0)
        self.assertIn("pinned disposable PostgreSQL 17.11", completed.stdout)

    def test_provider_pass_requires_both_shared_conformance_paths(self) -> None:
        script = SCRIPT.read_text(encoding="utf-8")
        acceptance = script[script.rindex('if [[ "$live_adapter_status" == "pass"') :]
        first_condition = acceptance.split("then", maxsplit=1)[0]
        self.assertIn('"$gateway_status" == "pass"', first_condition)
        self.assertIn('"$imo50_shared_direct_status" == "pass"', first_condition)
        self.assertIn('"$imo50_shared_pooler_status" == "pass"', first_condition)
        self.assertNotIn(
            'overall_status="pass"',
            first_condition,
            "shared conformance must be part of the same fail-closed pass predicate",
        )

    def test_production_gateway_is_an_executable_required_acceptance_vector(
        self,
    ) -> None:
        script = SCRIPT.read_text(encoding="utf-8")
        self.assertIn(
            "live_postgres_gateway_counter_workflow_uses_production_backend",
            script,
        )
        self.assertIn(
            "LIVE_POSTGRES_GATEWAY=PASS "
            "create+duplicate+conflict+projection+replay+attach+sync+action+live+ack+restart",
            script,
        )
        self.assertIn('gateway_status="failed"', script)
        self.assertIn('add_error "production_gateway_workflow_failed"', script)


if __name__ == "__main__":
    unittest.main()

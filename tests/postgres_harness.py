#!/usr/bin/env python3
"""Boundary tests for scripts/postgres-harness.sh.

These tests deliberately use a fake psql executable. They prove command
classification and redaction without claiming that a PostgreSQL provider was
available or that a live migration ran.
"""

from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HARNESS = ROOT / "scripts" / "postgres-harness.sh"


class PostgreSQLHarnessBoundaryTests(unittest.TestCase):
    def run_harness(
        self,
        *,
        psql: Path | None = None,
        adapter: str = "skip",
        admin_dsn: str = "host=admin user=admin password=ADMIN_SECRET",
        runtime_dsn: str = "host=runtime user=runtime password=RUNTIME_SECRET",
        extra: dict[str, str] | None = None,
    ) -> tuple[subprocess.CompletedProcess[str], dict[str, object]]:
        environment = os.environ.copy()
        environment.update(
            {
                "WORLDSTREAM_PG_HARNESS_MODE": "external",
                "WORLDSTREAM_PG_HARNESS_ADAPTER_TEST": adapter,
                "WORLDSTREAM_PG_HARNESS_ADMIN_DSN": admin_dsn,
                "WORLDSTREAM_PG_HARNESS_RUNTIME_DSN": runtime_dsn,
                "WORLDSTREAM_PG_HARNESS_PYTHON": sys.executable,
                # Keep tests independent of a developer's inherited database
                # configuration and prevent any real pooler from being used.
                "WORLDSTREAM_PG_HARNESS_POOLER_DSN": "",
            }
        )
        if psql is not None:
            environment["WORLDSTREAM_PG_HARNESS_PSQL"] = str(psql)
        else:
            environment["WORLDSTREAM_PG_HARNESS_PSQL"] = str(
                Path(tempfile.gettempdir()) / "worldstream-harness-no-such-psql"
            )
        if extra and "WORLDSTREAM_PG_HARNESS_CARGO" in extra:
            environment["WORLDSTREAM_PG_HARNESS_CARGO"] = extra[
                "WORLDSTREAM_PG_HARNESS_CARGO"
            ]
        if extra:
            environment.update(extra)
        completed = subprocess.run(
            [str(HARNESS)],
            cwd=ROOT,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
        )
        lines = [line for line in completed.stdout.splitlines() if line.strip()]
        self.assertTrue(lines, completed.stderr)
        evidence = json.loads(lines[-1])
        return completed, evidence

    @staticmethod
    def fake_psql(
        directory: Path,
        version: str = "170011",
        migration_ledger_privilege: str = "false",
        migration_shape: str = "pass",
    ) -> Path:
        tool = directory / "fake-psql"
        tool.write_text(
            textwrap.dedent(
                f"""\
                #!/usr/bin/env bash
                set -eu
                sql=""
                dsn=""
                previous=""
                no_password=0
                for argument in "$@"; do
                  if [[ "$previous" == "--command" ]]; then sql="$argument"; fi
                  if [[ -z "$dsn" && "$argument" != -* ]]; then dsn="$argument"; fi
                  if [[ "$argument" == "--no-password" ]]; then no_password=1; fi
                  previous="$argument"
                done
                if [[ -n "${{FAKE_PSQL_TRACE:-}}" ]]; then
                  printf '%s\\t%s\\t%s\\t%s\\n' "$dsn" "$sql" "$no_password" "${{PGPASSWORD-}}" >>"$FAKE_PSQL_TRACE"
                fi
                case "$sql" in
                  "SHOW server_version_num") printf '%s\\n' '{version}' ;;
                  "SELECT current_user")
                    if [[ "$dsn" == *"user=runtime"* || "$dsn" == *"pooler"* ]]; then printf '%s\\n' runtime; else printf '%s\\n' admin; fi
                    ;;
                  "SELECT rolsuper::text"*) printf '%s\\n' 'false|false|false' ;;
                  "SELECT has_schema_privilege"*) printf '%s\\n' false ;;
                  "SELECT count(*)::text FROM worldstream_schema_migrations") printf '%s\\n' 18 ;;
                  "SELECT CASE WHEN count(*) = 18 AND min(version) = 1"*) printf '%s\\n' '{migration_shape}' ;;
                  "SELECT "*"EXISTS (SELECT 1 FROM unnest"*) printf '%s\\n' '{migration_ledger_privilege}' ;;
                  "SELECT 1") printf '%s\\n' 1 ;;
                  "SELECT count(*)::text FROM information_schema.tables"*) printf '%s\\n' 40 ;;
                  "CREATE TABLE worldstream_harness_runtime_ddl_probe"*)
                    printf 'fake psql error dsn=%s password=ADMIN_SECRET\\n' "$dsn" >&2
                    exit 1
                    ;;
                  *) exit 0 ;;
                esac
                """
            ),
            encoding="utf-8",
        )
        tool.chmod(tool.stat().st_mode | stat.S_IXUSR)
        return tool

    @staticmethod
    def fake_cargo(directory: Path) -> Path:
        tool = directory / "fake-cargo"
        tool.write_text(
            textwrap.dedent(
                """\
                #!/usr/bin/env bash
                set -eu
                if [[ "${FAKE_CARGO_MODE:-}" == "adapter-fail" && "$*" == *"--features conformance-tracer"* ]]; then
                  printf '%s%s%s%s%s%s%s\\n' \\
                    'adapter failure password=ADAPTER_SECRET uri=post' \\
                    'gresql' ':' '//' 'user:' 'secret@' 'example.invalid/db' >&2
                  exit 1
                fi
                if [[ "${FAKE_CARGO_MODE:-}" == "pooler-fail" && "$*" == *"--features conformance-tracer"* ]]; then
                  printf '%s\\n' \\
                    'LIVE_POSTGRES=PASS direct_admin=migrate+verify major=17' \\
                    'LIVE_POSTGRES=PASS direct_admin=restart-idempotent' \\
                    'LIVE_POSTGRES=PASS runtime_ddl=create_denied' \\
                    'LIVE_POSTGRES=PASS runtime=direct create+duplicate+conflict+resolve' \\
                    'LIVE_POSTGRES=PASS runtime=direct same-room-create=reprepare'
                  exit 1
                fi
                printf '%s\\n' \
                  'LIVE_POSTGRES=PASS direct_admin=migrate+verify major=17' \
                  'LIVE_POSTGRES=PASS direct_admin=restart-idempotent' \
                  'LIVE_POSTGRES=PASS runtime_ddl=create_denied' \
                  'LIVE_POSTGRES=PASS runtime=direct create+duplicate+conflict+resolve' \
                  'LIVE_POSTGRES=PASS runtime=direct same-room-create=reprepare'
                """
            ),
            encoding="utf-8",
        )
        tool.chmod(tool.stat().st_mode | stat.S_IXUSR)
        return tool

    def test_unavailable_psql_is_visible_and_non_release(self) -> None:
        completed, evidence = self.run_harness()
        self.assertEqual(completed.returncode, 10)
        self.assertEqual(evidence["status"], "unavailable")
        self.assertEqual(evidence["evidence_class"], "missing_prerequisite")
        self.assertFalse(evidence["release_evidence"])
        self.assertIn("psql_unavailable", evidence["errors"])
        self.assertNotIn("ADMIN_SECRET", completed.stdout + completed.stderr)
        self.assertNotIn("RUNTIME_SECRET", completed.stdout + completed.stderr)

    def test_wrong_major_cannot_be_release_evidence(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-pg-harness-test-") as name:
            fake = self.fake_psql(Path(name), version="140013")
            completed, evidence = self.run_harness(psql=fake)
        self.assertEqual(completed.returncode, 11)
        self.assertEqual(evidence["status"], "wrong_version")
        self.assertEqual(evidence["postgres"]["major"], 14)
        self.assertEqual(evidence["postgres"]["version_status"], "wrong_major")
        self.assertEqual(evidence["evidence_class"], "unsupported_provider")
        self.assertFalse(evidence["release_evidence"])
        output = completed.stdout + completed.stderr
        self.assertNotIn("ADMIN_SECRET", output)
        self.assertNotIn("RUNTIME_SECRET", output)
        self.assertNotIn("password=", output)

    def test_below_minimum_patch_cannot_be_release_evidence(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-pg-harness-test-") as name:
            fake = self.fake_psql(Path(name), version="170010")
            completed, evidence = self.run_harness(psql=fake)
        self.assertEqual(completed.returncode, 11)
        self.assertEqual(evidence["postgres"]["major"], 17)
        self.assertEqual(evidence["postgres"]["patch"], 10)
        self.assertEqual(evidence["postgres"]["version_status"], "below_minimum_patch")
        self.assertEqual(evidence["evidence_class"], "unsupported_provider")
        self.assertFalse(evidence["release_evidence"])

    def test_fixture_boundary_output_is_redacted_and_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-pg-harness-test-") as name:
            fake = self.fake_psql(Path(name))
            completed, evidence = self.run_harness(psql=fake)
        self.assertEqual(completed.returncode, 12)
        self.assertEqual(evidence["status"], "incomplete")
        self.assertEqual(evidence["evidence_class"], "incomplete_boundary")
        self.assertEqual(evidence["schema"], "worldstream/postgresql-evidence/v1")
        self.assertFalse(evidence["release_evidence"])
        self.assertEqual(evidence["adapter_conformance"], "skipped")
        self.assertEqual(
            evidence["adapter_evidence"]["direct_admin_migrate_verify"], "not_run"
        )
        self.assertEqual(
            evidence["adapter_evidence"]["transaction_pooler_commit_resolution"],
            "not_configured",
        )
        self.assertFalse(evidence["secrets_emitted"])
        output = completed.stdout + completed.stderr
        for secret in (
            "ADMIN_SECRET",
            "RUNTIME_SECRET",
            "host=admin",
            "host=runtime",
            "password=",
        ):
            self.assertNotIn(secret, output)

    def test_runtime_cannot_write_migration_ledger(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-pg-harness-test-") as name:
            fake = self.fake_psql(Path(name), migration_ledger_privilege="true")
            completed, evidence = self.run_harness(psql=fake)
        self.assertEqual(completed.returncode, 13)
        self.assertEqual(evidence["status"], "validation_failed")
        self.assertEqual(evidence["evidence_class"], "provider_validation_failure")
        self.assertIn("direct_migration_ledger_write_allowed", evidence["errors"])
        schema_checks = evidence["schema_checks"]
        self.assertEqual(
            schema_checks["runtime_migration_ledger_write"], "write_allowed"
        )
        self.assertFalse(evidence["release_evidence"])
        output = completed.stdout + completed.stderr
        self.assertNotIn("ADMIN_SECRET", output)
        self.assertNotIn("RUNTIME_SECRET", output)

    def test_migration_history_shape_failure_is_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-pg-harness-test-") as name:
            directory = Path(name)
            fake = self.fake_psql(directory, migration_shape="fail")
            cargo = self.fake_cargo(directory)
            completed, evidence = self.run_harness(
                psql=fake,
                adapter="run",
                extra={"WORLDSTREAM_PG_HARNESS_CARGO": str(cargo)},
            )
        self.assertEqual(completed.returncode, 13)
        self.assertEqual(evidence["status"], "validation_failed")
        self.assertIn("migration_history_not_forward_complete", evidence["errors"])
        self.assertEqual(evidence["migration"]["forward_only_contract"], "failed")
        self.assertEqual(evidence["adapter_conformance"], "pass")
        self.assertEqual(
            evidence["adapter_evidence"]["direct_admin_migrate_verify"], "pass"
        )
        self.assertEqual(
            evidence["adapter_evidence"]["direct_runtime_root_guard"], "pass"
        )
        self.assertEqual(
            evidence["adapter_evidence"]["transaction_pooler_commit_resolution"],
            "not_configured",
        )
        self.assertFalse(evidence["release_evidence"])
        output = completed.stdout + completed.stderr
        self.assertNotIn("ADMIN_SECRET", output)
        self.assertNotIn("RUNTIME_SECRET", output)

    def test_external_mode_requires_two_distinct_profiles(self) -> None:
        completed, evidence = self.run_harness(runtime_dsn="")
        self.assertEqual(completed.returncode, 12)
        self.assertEqual(evidence["status"], "configuration_error")
        self.assertEqual(evidence["evidence_class"], "configuration_error")
        self.assertIn("admin_and_runtime_dsns_required", evidence["errors"])
        self.assertFalse(evidence["release_evidence"])

    def test_identical_direct_profiles_are_rejected_before_connection(self) -> None:
        dsn = "host=shared user=shared password=SHARED_SECRET"
        completed, evidence = self.run_harness(admin_dsn=dsn, runtime_dsn=dsn)
        self.assertEqual(completed.returncode, 12)
        self.assertIn("admin_and_runtime_dsns_must_be_distinct", evidence["errors"])
        self.assertNotIn("SHARED_SECRET", completed.stdout + completed.stderr)

    def test_pooler_requires_transaction_mode_and_distinct_admin_profile(self) -> None:
        completed, evidence = self.run_harness(
            extra={
                "WORLDSTREAM_PG_HARNESS_POOLER_DSN": "host=pooler user=runtime password=POOLER_SECRET",
                "WORLDSTREAM_PG_HARNESS_POOLER_MODE": "session",
            }
        )
        self.assertEqual(completed.returncode, 12)
        self.assertIn("pooler_mode_must_be_transaction", evidence["errors"])
        self.assertNotIn("POOLER_SECRET", completed.stdout + completed.stderr)

        completed, evidence = self.run_harness(
            extra={
                "WORLDSTREAM_PG_HARNESS_POOLER_DSN": "host=admin user=admin password=ADMIN_SECRET",
            }
        )
        self.assertEqual(completed.returncode, 12)
        self.assertIn("pooler_dsn_must_be_distinct_from_admin", evidence["errors"])
        self.assertNotIn("ADMIN_SECRET", completed.stdout + completed.stderr)

    def test_pooler_uses_no_password_prompt_and_preserves_profile_boundaries(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-pg-harness-test-") as name:
            directory = Path(name)
            trace = directory / "psql-trace"
            fake = self.fake_psql(directory)
            completed, evidence = self.run_harness(
                psql=fake,
                extra={
                    "WORLDSTREAM_PG_HARNESS_POOLER_DSN": "host=pooler user=runtime password=POOLER_SECRET",
                    "FAKE_PSQL_TRACE": str(trace),
                },
            )
            trace_rows = [
                line.split("\t")
                for line in trace.read_text(encoding="utf-8").splitlines()
            ]
        self.assertEqual(completed.returncode, 12)
        self.assertEqual(evidence["paths"]["direct_runtime"], "pass")
        self.assertEqual(evidence["paths"]["transaction_pooler"], "pass")
        self.assertTrue(trace_rows)
        self.assertTrue(all(row[2] == "1" and row[3] == "" for row in trace_rows))
        self.assertTrue(any("host=admin" in row[0] for row in trace_rows))
        self.assertTrue(any("host=runtime" in row[0] for row in trace_rows))
        self.assertTrue(any("host=pooler" in row[0] for row in trace_rows))
        self.assertNotIn("POOLER_SECRET", completed.stdout + completed.stderr)

    def test_adapter_failure_is_normalized_and_raw_provider_output_is_withheld(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-pg-harness-test-") as name:
            directory = Path(name)
            fake = self.fake_psql(directory)
            cargo = self.fake_cargo(directory)
            completed, evidence = self.run_harness(
                psql=fake,
                adapter="run",
                extra={
                    "WORLDSTREAM_PG_HARNESS_CARGO": str(cargo),
                    "FAKE_CARGO_MODE": "adapter-fail",
                },
            )
        self.assertEqual(completed.returncode, 13)
        self.assertEqual(evidence["status"], "validation_failed")
        self.assertEqual(evidence["evidence_class"], "provider_validation_failure")
        self.assertEqual(evidence["adapter_conformance"], "failed")
        self.assertEqual(
            evidence["adapter_evidence"]["direct_admin_migrate_verify"], "failed"
        )
        self.assertIn("adapter_conformance_failed", evidence["errors"])
        self.assertEqual(len(evidence["errors"]), len(set(evidence["errors"])))
        output = completed.stdout + completed.stderr
        for secret in ("ADAPTER_SECRET", "postgresql://user:secret", "password="):
            self.assertNotIn(secret, output)

    def test_pooler_failure_preserves_direct_adapter_markers(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-pg-harness-test-") as name:
            directory = Path(name)
            fake = self.fake_psql(directory)
            cargo = self.fake_cargo(directory)
            completed, evidence = self.run_harness(
                psql=fake,
                adapter="run",
                extra={
                    "WORLDSTREAM_PG_HARNESS_CARGO": str(cargo),
                    "WORLDSTREAM_PG_HARNESS_POOLER_DSN": "host=pooler user=runtime password=POOLER_SECRET",
                    "FAKE_CARGO_MODE": "pooler-fail",
                },
            )
        self.assertEqual(completed.returncode, 13)
        self.assertEqual(evidence["evidence_class"], "provider_validation_failure")
        self.assertEqual(evidence["adapter_conformance"], "failed")
        self.assertEqual(
            evidence["adapter_evidence"]["direct_admin_migrate_verify"], "pass"
        )
        self.assertEqual(
            evidence["adapter_evidence"]["direct_runtime_commit_resolution"], "pass"
        )
        self.assertEqual(
            evidence["adapter_evidence"]["transaction_pooler_commit_resolution"],
            "failed",
        )
        self.assertIn("pooler_conformance_evidence_missing", evidence["errors"])
        self.assertIn("adapter_conformance_failed", evidence["errors"])
        self.assertNotIn("POOLER_SECRET", completed.stdout + completed.stderr)

    def test_unusable_python_is_a_static_fail_closed_prerequisite(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-pg-harness-test-") as name:
            directory = Path(name)
            unusable = directory / "unusable-python"
            unusable.write_text("#!/usr/bin/env bash\nexit 1\n", encoding="utf-8")
            unusable.chmod(unusable.stat().st_mode | stat.S_IXUSR)
            completed, evidence = self.run_harness(
                extra={"WORLDSTREAM_PG_HARNESS_PYTHON": str(unusable)}
            )
        self.assertEqual(completed.returncode, 10)
        self.assertEqual(evidence["status"], "unavailable")
        self.assertEqual(evidence["evidence_class"], "missing_prerequisite")
        self.assertIn("python3_unavailable", evidence["errors"])
        self.assertFalse(evidence["release_evidence"])


if __name__ == "__main__":
    unittest.main()

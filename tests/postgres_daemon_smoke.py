"""Focused boundary tests for the PostgreSQL worldstreamd smoke."""

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
SCRIPT = ROOT / "scripts" / "postgres-daemon-smoke.sh"
SCHEMA = "worldstream/postgresql-daemon-smoke/v1"
RUNTIME_ROLE_SQL_FRAGMENTS = (
    "role.rolsuper::text",
    "role.rolcreaterole::text",
    "role.rolcreatedb::text",
    "role.rolreplication::text",
    "role.rolbypassrls::text",
    "has_database_privilege(current_user, current_database(), 'CREATE')",
    "pg_catalog.pg_auth_members",
    "has_schema_privilege(current_user, 'public', 'CREATE')",
    "pg_catalog.pg_namespace",
    "pg_catalog.pg_class",
    "pg_catalog.pg_proc",
    "pg_catalog.pg_type",
    "'public.worldstream_schema_migrations', 'INSERT'",
    "'public.worldstream_schema_migrations', 'UPDATE'",
    "'public.worldstream_schema_migrations', 'DELETE'",
    "'public.worldstream_schema_migrations', 'TRUNCATE'",
    "protected_table.table_name, 'INSERT'",
    "protected_table.table_name, 'UPDATE'",
    "protected_table.table_name, 'DELETE'",
    "protected_table.table_name, 'TRUNCATE'",
    "'public.worldstream_frames', 'DELETE'",
)


def assert_runtime_role_contract(source: str) -> None:
    start = source.index("runtime_role_admission_sql() {")
    end = source.index("\n}\n", start)
    query = source[start:end]
    expected = "false|" * 17 + "false"
    assert f'RUNTIME_ROLE_ADMISSION_EXPECTED="{expected}"' in source
    assert query.count("|| '|' ||") == 17
    for fragment in RUNTIME_ROLE_SQL_FRAGMENTS:
        assert fragment in query
    assert "unnest(ARRAY['INSERT'" not in query


class PostgreSQLDaemonSmokeTests(unittest.TestCase):
    def run_script(
        self, *args: str, **env_updates: str
    ) -> tuple[subprocess.CompletedProcess[str], dict[str, object]]:
        environment = os.environ.copy()
        environment.update(env_updates)
        environment.setdefault("WORLDSTREAM_POSTGRES_DAEMON_PYTHON", sys.executable)
        completed = subprocess.run(
            [str(SCRIPT), *args],
            cwd=ROOT,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
        )
        lines = [line for line in completed.stdout.splitlines() if line.strip()]
        self.assertTrue(lines, completed.stderr)
        return completed, json.loads(lines[-1])

    def test_help_is_side_effect_free_and_script_is_executable(self) -> None:
        self.assertTrue(SCRIPT.stat().st_mode & stat.S_IXUSR)
        completed = subprocess.run(
            [str(SCRIPT), "--help"],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(completed.returncode, 0)
        self.assertIn("postgres:17.11-alpine", completed.stdout)
        self.assertNotIn("postgresql://", completed.stdout + completed.stderr)

    def test_missing_docker_is_unavailable_and_redacted(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-pg-daemon-test-") as name:
            report = Path(name) / "report.json"
            completed, value = self.run_script(
                "--report",
                str(report),
                WORLDSTREAM_POSTGRES_DAEMON_DOCKER=str(Path(name) / "missing-docker"),
                WORLDSTREAM_POSTGRES_DAEMON_PSQL=str(Path(name) / "missing-psql"),
                WORLDSTREAM_POSTGRES_DAEMON_CTL=str(
                    Path(name) / "missing-worldstreamctl"
                ),
                WORLDSTREAM_POSTGRES_DAEMON_CURL=str(Path(name) / "missing-curl"),
                WORLDSTREAM_POSTGRES_DAEMON_BIN=str(
                    Path(name) / "missing-worldstreamd"
                ),
            )
            self.assertEqual(completed.returncode, 10)
            self.assertEqual(value["schema"], SCHEMA)
            self.assertEqual(value["status"], "unavailable")
            self.assertEqual(value["reason"], "docker_unavailable")
            self.assertFalse(value["release_evidence"])
            self.assertFalse(value["secrets_emitted"])
            self.assertEqual(json.loads(report.read_text(encoding="utf-8")), value)
            output = completed.stdout + completed.stderr
            for forbidden in (
                "postgresql://",
                "password=",
                "ADMIN_SECRET",
                "RUNTIME_SECRET",
            ):
                self.assertNotIn(forbidden, output)

    def test_source_keeps_pinned_identity_migration_and_secret_fences(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        self.assertIn("postgres:17.11-alpine@sha256:", source)
        self.assertIn("EXPECTED_POSTGRES_DIGEST", source)
        self.assertIn("WORLDSTREAM_POSTGRES_DAEMON_CTL", source)
        self.assertIn(
            'ctl_bin="${WORLDSTREAM_POSTGRES_DAEMON_CTL:-$workspace_dir/target/debug/worldstreamctl}"',
            source,
        )
        self.assertIn('"$ctl_bin" postgres migrate --dsn-file', source)
        self.assertIn('"$ctl_bin" postgres verify --dsn-file', source)
        self.assertIn('admin_dsn_file="$temp_root/postgresql-admin-dsn"', source)
        self.assertNotIn("conformance", source.lower())
        self.assertIn('profile = "postgres-primary"', source)
        self.assertIn("dsn_file", source)
        self.assertIn("secret_file", source)
        self.assertIn("chmod 600", source)
        self.assertIn("server_version_num", source)
        self.assertIn("postgresql/17.11; server_version_num=170011", source)
        self.assertIn('"degraded"', source)
        self.assertIn('"fallback"', source)
        self.assertIn("secrets_emitted", source)
        self.assertIn('"$python_bin" - "$report_file"', source)
        self.assertNotIn('REPORT_SECRETS="$secrets_emitted" python3 -', source)
        self.assertNotIn("KEEP_TEMP", source)

    def test_source_keeps_postgres_startup_off_the_daemon_runtime(self) -> None:
        source = (
            ROOT / "crates" / "worldstream-server" / "src" / "bin" / "worldstreamd.rs"
        ).read_text(encoding="utf-8")
        self.assertIn("run_on_blocking_worker", source)
        self.assertIn("store.verify_schema()", source)
        self.assertIn("store.engine_identity()", source)
        self.assertIn("bootstrap_authority(&store", source)
        self.assertIn(".with_scheduler()", source)
        self.assertIn("tokio::task::spawn_blocking(work)", source)

    def test_runtime_role_witness_is_exact_and_mutation_closed(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        assert_runtime_role_contract(source)
        self.assertIn("NOREPLICATION NOBYPASSRLS", source)
        for fragment in RUNTIME_ROLE_SQL_FRAGMENTS:
            with self.subTest(fragment=fragment), self.assertRaises(AssertionError):
                assert_runtime_role_contract(source.replace(fragment, "mutated"))

    def test_shell_syntax_is_valid(self) -> None:
        completed = subprocess.run(
            ["bash", "-n", str(SCRIPT)],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)


if __name__ == "__main__":
    unittest.main()

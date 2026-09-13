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
)


def assert_runtime_role_contract(source: str) -> None:
    start = source.index("runtime_role_admission_sql() {")
    end = source.index("\n}\n", start)
    query = source[start:end]
    expected = "false|" * 16 + "false"
    assert f'RUNTIME_ROLE_ADMISSION_EXPECTED="{expected}"' in source
    assert query.count("|| '|' ||") == 16
    for fragment in RUNTIME_ROLE_SQL_FRAGMENTS:
        assert fragment in query
    assert "unnest(ARRAY['INSERT'" not in query


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
            "create+duplicate+conflict+projection+replay+attach+sync+resync+action+stale+live+ack+restart",
            script,
        )
        self.assertIn('gateway_status="failed"', script)
        self.assertIn('add_error "production_gateway_workflow_failed"', script)

    def test_runtime_grants_follow_migration_and_exclude_control_tables(self) -> None:
        script = SCRIPT.read_text(encoding="utf-8")
        pre_migration = script.index(
            'postgres migrate --dsn-file "$database_admin_dsn_file"'
        )
        runtime_grants = script.index(
            "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public "
            "TO runtime"
        )
        adapter = script.index('adapter_log="$temp_root/live-adapter.log"')

        self.assertLess(pre_migration, runtime_grants)
        self.assertLess(runtime_grants, adapter)
        self.assertNotIn("ALTER DEFAULT PRIVILEGES", script)
        self.assertIn(
            "REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON TABLE "
            "public.worldstream_schema_migrations, "
            "public.worldstream_transfer_imports, "
            "public.worldstream_transfer_chunks, "
            "public.worldstream_transfer_target_fence, "
            "public.worldstream_transfer_stream_imports_v2, "
            "public.worldstream_transfer_stream_chunks_v2, "
            "public.worldstream_transfer_stream_records_v2 FROM runtime",
            script,
        )
        self.assertIn(
            "table_name NOT IN ('worldstream_schema_migrations', "
            "'worldstream_authority_state')",
            script,
        )
        self.assertIn("NOREPLICATION NOBYPASSRLS", script)
        self.assertIn("CREATE DATABASE worldstream_transfer_abort OWNER admin", script)
        self.assertIn(
            'WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN_FILE="$transfer_abort_admin_dsn_file"',
            script,
        )

    def test_live_runner_resolves_and_propagates_the_pinned_python(self) -> None:
        script = SCRIPT.read_text(encoding="utf-8")
        self.assertIn('python_bin="$(resolve_python "$python_bin")"', script)
        self.assertIn("sys.version_info[:3] != (3, 14, 7)", script)
        self.assertIn("uv run --python 3.14.7 --no-project python", script)
        self.assertIn(
            'WORLDSTREAM_PG_TRANSFER_PYTHON="$python_bin"',
            script,
        )
        self.assertIn(
            'WORLDSTREAM_PG_HARNESS_PYTHON="$python_bin"',
            script,
        )
        self.assertNotIn("python3 -", script)

    def test_runtime_role_witness_is_exact_and_mutation_closed(self) -> None:
        script = SCRIPT.read_text(encoding="utf-8")
        assert_runtime_role_contract(script)
        self.assertIn('user=runtime"', script)
        self.assertIn("runtime_role_admission_query_failed", script)
        self.assertIn("runtime_role_not_least_privileged", script)
        for fragment in RUNTIME_ROLE_SQL_FRAGMENTS:
            with self.subTest(fragment=fragment), self.assertRaises(AssertionError):
                assert_runtime_role_contract(script.replace(fragment, "mutated"))

    def test_redacted_harness_starts_from_fresh_logical_state(self) -> None:
        script = SCRIPT.read_text(encoding="utf-8")
        comparison = script.index(
            'imo50_shared_comparison_file="${evidence_file}.imo-50-shared-comparison.json"'
        )
        harness = script.index('harness_log="$temp_root/harness.log"')
        reset = script.index('run_db_admin_sql "$truncate_sql"', harness)

        self.assertGreater(reset, comparison)
        self.assertLess(reset, script.index("scripts/postgres-harness.sh", harness))
        self.assertIn(
            'add_error "redacted_postgres_harness_state_reset_failed"',
            script[harness : script.index("transfer_evidence=", harness)],
        )
        self.assertIn('POSTGRES_LIVE_HARNESS_SUMMARY="$harness_summary_json"', script)
        self.assertIn('"redacted_harness_evidence": harness_summary', script)
        self.assertIn(
            'harness_summary_json="$("$python_bin" - "$harness_evidence"', script
        )


if __name__ == "__main__":
    unittest.main()

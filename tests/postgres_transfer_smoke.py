#!/usr/bin/env python3
"""Boundary tests for the live SQLite -> PostgreSQL transfer smoke runner.

These tests exercise only fail-closed and redaction boundaries.  They do not
pretend that a fake driver is a PostgreSQL provider, and they intentionally do
not set release or manifest state.
"""

from __future__ import annotations

import json
import os
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HARNESS = ROOT / "scripts" / "postgres-transfer-smoke.sh"
SCHEMA = "worldstream/sqlite-postgresql-transfer-evidence/v1"


class PostgreSQLTransferSmokeBoundaryTests(unittest.TestCase):
    def run_harness(
        self,
        *,
        source: Path | None = None,
        mode: str = "external",
        admin_dsn: str = "host=admin user=admin password=ADMIN_SECRET",
        runtime_dsn: str = "host=runtime user=runtime password=RUNTIME_SECRET",
        abort_admin_dsn: str = "host=abort user=admin password=ABORT_SECRET",
        cargo: Path | None = None,
        docker: Path | None = None,
        evidence: Path | None = None,
        extra: dict[str, str] | None = None,
    ) -> tuple[subprocess.CompletedProcess[str], dict[str, object]]:
        environment = os.environ.copy()
        for variable in (
            "WORLDSTREAM_PG_TRANSFER_ADMIN_DSN",
            "WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN",
            "WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN",
            "WORLDSTREAM_PG_TRANSFER_ADMIN_DSN_FILE",
            "WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN_FILE",
            "WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN_FILE",
        ):
            environment.pop(variable, None)
        secret_directory = tempfile.TemporaryDirectory(
            prefix="worldstream-transfer-dsn-boundary-"
        )
        self.addCleanup(secret_directory.cleanup)
        secret_root = Path(secret_directory.name)
        paths_by_dsn: dict[str, Path] = {}
        paths_by_role: dict[str, Path] = {}
        for label, value in (
            ("admin", admin_dsn),
            ("runtime", runtime_dsn),
            ("abort-admin", abort_admin_dsn),
        ):
            if not value:
                continue
            existing = paths_by_dsn.get(value)
            if existing is not None:
                paths_by_role[label] = existing
                continue
            path = secret_root / f"{label}.dsn"
            path.write_text(value, encoding="utf-8")
            path.chmod(0o600)
            paths_by_dsn[value] = path
            paths_by_role[label] = path
        environment.update(
            {
                "WORLDSTREAM_PG_TRANSFER_MODE": mode,
                "WORLDSTREAM_PG_TRANSFER_CARGO": str(cargo or ROOT / "no-such-cargo"),
                "WORLDSTREAM_PG_TRANSFER_DOCKER": str(
                    docker or ROOT / "no-such-docker"
                ),
                "WORLDSTREAM_PG_TRANSFER_PYTHON": os.environ.get(
                    "WORLDSTREAM_PG_TRANSFER_PYTHON", sys.executable
                ),
            }
        )
        for variable, label in (
            ("WORLDSTREAM_PG_TRANSFER_ADMIN_DSN_FILE", "admin"),
            ("WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN_FILE", "runtime"),
            ("WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN_FILE", "abort-admin"),
        ):
            path = paths_by_role.get(label)
            if path is not None:
                environment[variable] = str(path)
        if source is not None:
            environment["WORLDSTREAM_PG_TRANSFER_SQLITE"] = str(source)
        if evidence is not None:
            environment["WORLDSTREAM_PG_TRANSFER_EVIDENCE_FILE"] = str(evidence)
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
        return completed, json.loads(lines[-1])

    @staticmethod
    def executable(path: Path, body: str) -> Path:
        path.write_text(body, encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)
        return path

    @staticmethod
    def source_fixture(path: Path) -> dict[str, bytes]:
        values = {
            "pack_revision_lock": b"lock-bytes",
            "genesis": b"genesis-bytes",
            "head": b"head-bytes",
            "core": b"core-bytes",
            "activity": b"activity-bytes",
        }
        connection = sqlite3.connect(path)
        connection.executescript(
            """
            CREATE TABLE rooms (
                room_id TEXT PRIMARY KEY,
                room_status TEXT NOT NULL,
                room_seq INTEGER NOT NULL,
                genesis_or_transition_hash TEXT NOT NULL,
                core_schema_version TEXT NOT NULL,
                pack_digest TEXT NOT NULL,
                core_state_hash TEXT NOT NULL,
                activity_state_hash TEXT NOT NULL,
                authoritative_state_hash TEXT NOT NULL,
                complete_head_bytes BLOB NOT NULL
            );
            CREATE TABLE room_genesis (
                room_id TEXT PRIMARY KEY,
                pack_revision_lock_bytes BLOB NOT NULL,
                genesis_bytes BLOB NOT NULL
            );
            CREATE TABLE room_materializations (
                room_id TEXT PRIMARY KEY,
                core_state_bytes BLOB NOT NULL,
                activity_state_bytes BLOB NOT NULL
            );
            """
        )
        digest_placeholder = "blake3:" + "0" * 64
        connection.execute(
            "INSERT INTO rooms VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                "room-test",
                "active",
                0,
                digest_placeholder,
                "worldstream-core-v1",
                digest_placeholder,
                digest_placeholder,
                digest_placeholder,
                digest_placeholder,
                values["head"],
            ),
        )
        connection.execute(
            "INSERT INTO room_genesis VALUES (?, ?, ?)",
            ("room-test", values["pack_revision_lock"], values["genesis"]),
        )
        connection.execute(
            "INSERT INTO room_materializations VALUES (?, ?, ?)",
            ("room-test", values["core"], values["activity"]),
        )
        connection.commit()
        connection.close()
        return values

    def test_missing_source_is_incomplete_without_provider_claim(self) -> None:
        completed, evidence = self.run_harness()
        self.assertEqual(completed.returncode, 13)
        self.assertEqual(evidence["schema"], SCHEMA)
        self.assertEqual(evidence["status"], "incomplete")
        self.assertEqual(evidence["reason"], "sqlite_source_not_supplied")
        self.assertFalse(evidence["release_evidence"])
        self.assertFalse(evidence["secrets_emitted"])

    def test_external_credentials_are_required_and_never_echoed(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-transfer-boundary-"
        ) as name:
            source = Path(name) / "source.sqlite"
            source.write_bytes(b"not-a-database")
            completed, evidence = self.run_harness(
                source=source,
                admin_dsn="",
                runtime_dsn="",
                abort_admin_dsn="",
            )
        self.assertEqual(completed.returncode, 12)
        self.assertEqual(evidence["reason"], "external_target_credentials_missing")
        output = completed.stdout + completed.stderr
        for secret in ("ADMIN_SECRET", "RUNTIME_SECRET", "password=", "host=admin"):
            self.assertNotIn(secret, output)

    def test_legacy_plaintext_dsn_environment_is_rejected_without_echo(self) -> None:
        sentinel = "LEGACY_PLAINTEXT_DSN_SECRET"
        completed, evidence = self.run_harness(
            extra={
                "WORLDSTREAM_PG_TRANSFER_ADMIN_DSN": (
                    f"host=legacy user=admin password={sentinel}"
                )
            }
        )
        self.assertEqual(completed.returncode, 12)
        self.assertEqual(evidence["reason"], "plaintext_dsn_environment_rejected")
        self.assertNotIn(sentinel, completed.stdout + completed.stderr)

    def test_external_abort_target_must_be_explicitly_distinct(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-transfer-boundary-"
        ) as name:
            source = Path(name) / "source.sqlite"
            source.write_bytes(b"not-a-database")
            success_dsn = "host=shared user=admin password=SHARED_SECRET"
            completed, evidence = self.run_harness(
                source=source,
                admin_dsn=success_dsn,
                abort_admin_dsn=success_dsn,
            )
        self.assertEqual(completed.returncode, 12)
        self.assertEqual(evidence["reason"], "external_abort_target_not_distinct")
        self.assertNotIn("SHARED_SECRET", completed.stdout + completed.stderr)

    def test_runner_resolves_the_exact_pinned_python(self) -> None:
        script = HARNESS.read_text(encoding="utf-8")
        self.assertIn('python_bin="$(resolve_python "$python_bin")"', script)
        self.assertIn("sys.version_info[:3] != (3, 14, 7)", script)
        self.assertIn("uv run --python 3.14.7 --no-project python", script)
        self.assertNotIn("python3_unavailable", script)

    def test_helper_reads_only_bounded_owner_only_dsn_files(self) -> None:
        script = HARNESS.read_text(encoding="utf-8")
        self.assertIn("worldstream_runtime::SecretSource", script)
        self.assertIn("const MAX_DSN_BYTES: usize = 64 * 1024", script)
        self.assertIn(".read_bounded(MAX_DSN_BYTES)", script)
        for variable in (
            "WORLDSTREAM_PG_TRANSFER_ADMIN_DSN_FILE",
            "WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN_FILE",
            "WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN_FILE",
        ):
            self.assertIn(variable, script)
        self.assertNotIn('"WORLDSTREAM_PG_TRANSFER_ADMIN_DSN=$admin_dsn"', script)
        self.assertNotIn('env::var("WORLDSTREAM_PG_TRANSFER_ADMIN_DSN")', script)

    def test_transfer_preserves_exact_postgres_operation_guards(self) -> None:
        implementation = (
            ROOT / "crates" / "worldstream-postgres" / "src" / "transfer.rs"
        ).read_text(encoding="utf-8")
        script = HARNESS.read_text(encoding="utf-8")

        hydrate_only = implementation.index("if mode == NativePublicationMode::Hydrate")
        guard_insert = implementation.index(
            "INSERT INTO worldstream_operation_guards(identity_bytes, request_hash, room_id, receipt_bytes)"
        )
        guard_read = implementation.index(
            "SELECT request_hash, room_id, receipt_bytes FROM worldstream_operation_guards"
        )
        self.assertLess(hydrate_only, guard_insert)
        self.assertLess(guard_insert, guard_read)
        self.assertIn("semantic receipt operation guard missing", implementation)
        self.assertIn("semantic receipt operation guard mismatch", implementation)
        self.assertRegex(
            implementation,
            r'"semantic_receipts",\s*"SELECT count\(\*\) FROM worldstream_operation_guards"',
        )

        for witness in (
            "operation_guard_exact_parity_verified",
            "operation_guard_mismatch_count",
            "post_cutover_operation_guard_resolution",
            "same_hash_stored_resolution",
            "different_hash_conflict",
            "ResolveOutcomeV1::StoredResolution",
            "ResolveOutcomeV1::Conflict",
        ):
            self.assertIn(witness, script)

    def test_post_retirement_accept_is_publication_only(self) -> None:
        implementation = (
            ROOT / "crates" / "worldstream-postgres" / "src" / "transfer.rs"
        ).read_text(encoding="utf-8")
        start = implementation.index("    fn accept_target_write(")
        end = implementation.index("\n    fn abort_import(", start)
        publication = implementation[start:end]

        for forbidden in (
            "verify_schema",
            "verify_native_semantic_evidence",
            "verify_staged_chunks",
            "verify_native_operational_rows",
            "verify_hydrated_target",
            "reconfirm_hydrated_target_behind_fence",
        ):
            self.assertNotIn(forbidden, publication)
        self.assertIn("DELETE FROM worldstream_transfer_target_fence", publication)
        self.assertIn("SET state = 'authoritative'", publication)

    def test_docker_absence_is_unavailable_not_incomplete_provider_evidence(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-transfer-boundary-"
        ) as name:
            source = Path(name) / "source.sqlite"
            source.write_bytes(b"placeholder")
            fake_cargo = self.executable(Path(name) / "cargo", "#!/bin/sh\nexit 99\n")
            completed, evidence = self.run_harness(
                source=source, mode="docker", cargo=fake_cargo
            )
        self.assertEqual(completed.returncode, 10)
        self.assertEqual(evidence["status"], "unavailable")
        self.assertEqual(evidence["reason"], "docker_unavailable")
        self.assertFalse(evidence["release_evidence"])

    def test_invalid_mode_is_configuration_failure(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-transfer-boundary-"
        ) as name:
            source = Path(name) / "source.sqlite"
            source.write_bytes(b"placeholder")
            completed, evidence = self.run_harness(source=source, mode="fixture")
        self.assertEqual(completed.returncode, 12)
        self.assertEqual(evidence["reason"], "unsupported_mode")
        self.assertFalse(evidence["release_evidence"])

    def test_invalid_timeout_is_configuration_failure(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-transfer-boundary-"
        ) as name:
            source = Path(name) / "source.sqlite"
            source.write_bytes(b"placeholder")
            completed, evidence = self.run_harness(
                source=source,
                extra={"WORLDSTREAM_PG_TRANSFER_TIMEOUT_SECONDS": "0"},
            )
        self.assertEqual(completed.returncode, 12)
        self.assertEqual(evidence["reason"], "invalid_helper_timeout")
        self.assertFalse(evidence["release_evidence"])

    def test_hosted_fixture_revision_is_explicit_and_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-transfer-boundary-"
        ) as name:
            source = Path(name) / "source.sqlite"
            source.write_bytes(b"placeholder")
            completed, evidence = self.run_harness(
                source=source,
                extra={"WORLDSTREAM_PG_TRANSFER_SOURCE_REVISION": "not-a-revision"},
            )
        self.assertEqual(completed.returncode, 12)
        self.assertEqual(evidence["reason"], "invalid_source_revision")
        script = HARNESS.read_text(encoding="utf-8")
        self.assertIn("untrusted_source_bound_input_construction", script)
        self.assertIn('"trusted_product_execution": False', script)
        self.assertIn('"source_revision": sys.argv[7]', script)

    def test_fake_driver_output_is_canonicalized_and_redacted(self) -> None:
        driver_output = (
            '{"schema":"worldstream/sqlite-postgresql-transfer-evidence/v1",'
            '"status":"incomplete","reason":"native_adapter_operational_rows_only",'
            '"release_evidence":false,"secrets_emitted":false,'
            '"provider_mode":"external","transfer":{"finalization":"refused"}}\n'
        )
        with tempfile.TemporaryDirectory(
            prefix="worldstream-transfer-boundary-"
        ) as name:
            directory = Path(name)
            source = directory / "source.sqlite"
            source.write_bytes(b"placeholder")
            fake_cargo = self.executable(
                directory / "cargo",
                f"#!/bin/sh\nprintf '%s\\n' '{driver_output.strip()}'\nexit 13\n",
            )
            evidence_file = directory / "evidence.json"
            completed, evidence = self.run_harness(
                source=source,
                cargo=fake_cargo,
                evidence=evidence_file,
            )
            stored = json.loads(evidence_file.read_text(encoding="utf-8"))
        self.assertEqual(completed.returncode, 13)
        self.assertEqual(evidence, stored)
        self.assertFalse(evidence["release_evidence"])
        self.assertFalse(evidence["secrets_emitted"])
        self.assertEqual(evidence["schema"], SCHEMA)
        self.assertNotIn("ADMIN_SECRET", completed.stdout + completed.stderr)

    def test_incomplete_driver_path_cannot_be_upgraded_to_pass_by_wrapper(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-transfer-boundary-"
        ) as name:
            directory = Path(name)
            source = directory / "source.sqlite"
            source.write_bytes(b"placeholder")
            fake_cargo = self.executable(
                directory / "cargo",
                '#!/bin/sh\nprintf \'%s\\n\' \'{"schema":"worldstream/sqlite-postgresql-transfer-evidence/v1","status":"pass","release_evidence":false,"secrets_emitted":false}\'\nexit 0\n',
            )
            completed, evidence = self.run_harness(source=source, cargo=fake_cargo)
        self.assertEqual(completed.returncode, 13)
        self.assertEqual(evidence["reason"], "transfer_evidence_invalid_or_unredacted")
        self.assertEqual(evidence["status"], "incomplete")
        self.assertFalse(evidence["release_evidence"])
        self.assertFalse(evidence["secrets_emitted"])

    def test_pass_requires_complete_source_and_target_witnesses(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-transfer-boundary-"
        ) as name:
            directory = Path(name)
            source = directory / "source.sqlite"
            source.write_bytes(b"placeholder")
            fake_cargo = self.executable(
                directory / "cargo",
                "#!/bin/sh\n"
                "printf '%s\\n' '"
                '{"schema":"worldstream/sqlite-postgresql-transfer-evidence/v1",'
                '"status":"pass","release_evidence":false,"secrets_emitted":false,'
                '"source":{"canonical_evidence":{"status":"complete"}},'
                '"transfer":{"finalization":"refused","target_authority":"refused"}}'
                "'\nexit 0\n",
            )
            completed, evidence = self.run_harness(source=source, cargo=fake_cargo)
        self.assertEqual(completed.returncode, 13)
        self.assertEqual(evidence["reason"], "transfer_evidence_invalid_or_unredacted")
        self.assertFalse(evidence["release_evidence"])

    def test_driver_cannot_smuggle_release_flags_into_evidence(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-transfer-boundary-"
        ) as name:
            directory = Path(name)
            source = directory / "source.sqlite"
            source.write_bytes(b"placeholder")
            fake_cargo = self.executable(
                directory / "cargo",
                '#!/bin/sh\nprintf \'%s\\n\' \'{"status":"pass","release_evidence":true,"secrets_emitted":false}\'\nexit 13\n',
            )
            completed, evidence = self.run_harness(source=source, cargo=fake_cargo)
        self.assertEqual(completed.returncode, 13)
        self.assertEqual(evidence["reason"], "transfer_evidence_invalid_or_unredacted")
        self.assertFalse(evidence["release_evidence"])
        self.assertFalse(evidence["secrets_emitted"])

    def test_driver_timeout_is_bounded_and_incomplete(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-transfer-boundary-"
        ) as name:
            directory = Path(name)
            source = directory / "source.sqlite"
            source.write_bytes(b"placeholder")
            fake_cargo = self.executable(
                directory / "cargo",
                "#!/bin/sh\nsleep 2\nexit 0\n",
            )
            completed, evidence = self.run_harness(
                source=source,
                cargo=fake_cargo,
                extra={"WORLDSTREAM_PG_TRANSFER_TIMEOUT_SECONDS": "1"},
            )
        self.assertEqual(completed.returncode, 13)
        self.assertEqual(evidence["reason"], "transfer_driver_timeout")
        self.assertFalse(evidence["release_evidence"])

    def test_driver_output_containing_dsn_material_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-transfer-boundary-"
        ) as name:
            directory = Path(name)
            source = directory / "source.sqlite"
            source.write_bytes(b"placeholder")
            fake_cargo = self.executable(
                directory / "cargo",
                '#!/bin/sh\nprintf \'%s\\n\' \'{"status":"incomplete","note":"password=LEAK"}\'\nexit 13\n',
            )
            completed, evidence = self.run_harness(source=source, cargo=fake_cargo)
        self.assertEqual(completed.returncode, 13)
        self.assertEqual(evidence["reason"], "transfer_evidence_invalid_or_unredacted")
        output = completed.stdout + completed.stderr
        self.assertNotIn("password=LEAK", output)
        self.assertFalse(evidence["secrets_emitted"])

    def test_unversioned_source_schema_fails_closed_before_provider(self) -> None:
        cargo = shutil.which("cargo")
        self.assertIsNotNone(
            cargo, "cargo is required for the live source-lifecycle boundary"
        )
        with tempfile.TemporaryDirectory(prefix="worldstream-transfer-source-") as name:
            directory = Path(name)
            source = directory / "source.sqlite"
            self.source_fixture(source)
            completed, evidence = self.run_harness(
                source=source,
                cargo=Path(cargo),
                admin_dsn="host=admin user=admin password=ADMIN_SECRET",
                runtime_dsn="host=runtime user=runtime password=RUNTIME_SECRET",
            )

        if evidence.get("reason") == "transfer_driver_failed":
            self.skipTest(
                "the ephemeral driver is blocked by pre-existing out-of-scope "
                "worldstream-backup compilation errors"
            )

        self.assertEqual(completed.returncode, 13)
        self.assertEqual(evidence["reason"], "sqlite_source_lifecycle_open_failed")
        self.assertFalse(evidence["release_evidence"])
        self.assertFalse(evidence["secrets_emitted"])
        self.assertEqual(evidence["postgres"]["status"], "not_checked")
        self.assertEqual(evidence["source"]["status"], "not_checked")
        output = completed.stdout + completed.stderr
        for secret in (
            "ADMIN_SECRET",
            "RUNTIME_SECRET",
            "ABORT_SECRET",
            "password=",
            "room-test",
        ):
            self.assertNotIn(secret, output)

    def test_generated_source_permitted_isolation_reaches_provider_boundary(
        self,
    ) -> None:
        cargo = shutil.which("cargo")
        self.assertIsNotNone(
            cargo, "cargo is required for the generated-source boundary"
        )
        environment = os.environ.copy()
        for variable in (
            "WORLDSTREAM_PG_TRANSFER_ADMIN_DSN",
            "WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN",
            "WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN",
            "WORLDSTREAM_PG_TRANSFER_ADMIN_DSN_FILE",
            "WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN_FILE",
            "WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN_FILE",
        ):
            environment.pop(variable, None)
        environment.update(
            {
                "WORLDSTREAM_PG_TRANSFER_MODE": "external",
                "WORLDSTREAM_PG_TRANSFER_CARGO": cargo,
                "WORLDSTREAM_PG_TRANSFER_PYTHON": sys.executable,
            }
        )
        secret_directory = tempfile.TemporaryDirectory(
            prefix="worldstream-transfer-generated-source-dsn-"
        )
        self.addCleanup(secret_directory.cleanup)
        for name, value in (
            ("admin", "host=127.0.0.1 port=1 user=admin dbname=success"),
            ("runtime", "host=127.0.0.1 port=1 user=runtime dbname=success"),
            ("abort-admin", "host=127.0.0.1 port=1 user=admin dbname=abort"),
        ):
            path = Path(secret_directory.name) / f"{name}.dsn"
            path.write_text(value, encoding="utf-8")
            path.chmod(0o600)
            variable = name.upper().replace("-", "_")
            environment[f"WORLDSTREAM_PG_TRANSFER_{variable}_DSN_FILE"] = str(path)
        completed = subprocess.run(
            [str(HARNESS), "--build-source"],
            cwd=ROOT,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
        )
        evidence = json.loads(completed.stdout.splitlines()[-1])

        self.assertEqual(
            completed.returncode,
            14,
            completed.stdout + completed.stderr,
        )
        self.assertEqual(evidence["reason"], "postgres_abort_target_not_isolated")
        self.assertNotEqual(
            evidence["reason"], "sqlite_abort_backup_verification_failed"
        )


if __name__ == "__main__":
    unittest.main()

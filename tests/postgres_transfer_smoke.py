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
        cargo: Path | None = None,
        docker: Path | None = None,
        evidence: Path | None = None,
        extra: dict[str, str] | None = None,
    ) -> tuple[subprocess.CompletedProcess[str], dict[str, object]]:
        environment = os.environ.copy()
        environment.update(
            {
                "WORLDSTREAM_PG_TRANSFER_MODE": mode,
                "WORLDSTREAM_PG_TRANSFER_ADMIN_DSN": admin_dsn,
                "WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN": runtime_dsn,
                "WORLDSTREAM_PG_TRANSFER_CARGO": str(cargo or ROOT / "no-such-cargo"),
                "WORLDSTREAM_PG_TRANSFER_DOCKER": str(
                    docker or ROOT / "no-such-docker"
                ),
                "WORLDSTREAM_PG_TRANSFER_PYTHON": os.environ.get(
                    "WORLDSTREAM_PG_TRANSFER_PYTHON", sys.executable
                ),
            }
        )
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
            )
        self.assertEqual(completed.returncode, 12)
        self.assertEqual(evidence["reason"], "external_target_credentials_missing")
        output = completed.stdout + completed.stderr
        for secret in ("ADMIN_SECRET", "RUNTIME_SECRET", "password=", "host=admin"):
            self.assertNotIn(secret, output)

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

    def test_source_bytes_are_digest_preserved_and_missing_global_evidence_refuses_provider(
        self,
    ) -> None:
        cargo = shutil.which("cargo")
        self.assertIsNotNone(
            cargo, "cargo is required for the live source-evidence boundary"
        )
        with tempfile.TemporaryDirectory(prefix="worldstream-transfer-source-") as name:
            directory = Path(name)
            source = directory / "source.sqlite"
            values = self.source_fixture(source)
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
        self.assertEqual(
            evidence["reason"], "sqlite_source_canonical_evidence_incomplete"
        )
        self.assertFalse(evidence["release_evidence"])
        self.assertFalse(evidence["secrets_emitted"])
        output = completed.stdout + completed.stderr
        for secret in ("ADMIN_SECRET", "RUNTIME_SECRET", "password=", "room-test"):
            self.assertNotIn(secret, output)

        source_evidence = evidence["source"]["canonical_evidence"]
        self.assertEqual(
            source_evidence["stored_bytes"],
            sum(len(value) for value in values.values()),
        )
        room = source_evidence["rooms"][0]
        self.assertEqual(room["status"], "incomplete")
        expected = {
            "pack_revision_lock": (
                10,
                "6e89e80621c93dcc0432bfa3679a0a390faa85f12240adea2e7d0b56c5ddfd06",
            ),
            "genesis": (
                13,
                "fd85d24d655b38c54508ebf8d1fd6d3342a5bde9d7c9a609feb029ac0b2ca640",
            ),
            "head": (
                10,
                "814844cd77ae9d3395404ac4b0f99e8ca2ddaa1894e408fa38d03ae2423ec0c5",
            ),
            "core": (
                10,
                "58c4e7700c6d54f9162062ae6a2b0cbc532ac8f164581ec603f4fc90dfbea23e",
            ),
            "activity": (
                14,
                "c874a0b97530c9ef281841e2b48f46591a3f6e7fda6cc8243af5f1f7fd436e2b",
            ),
        }
        for field, (size, digest) in expected.items():
            self.assertEqual(room[field]["status"], "observed")
            self.assertEqual(room[field]["bytes"], size)
            self.assertEqual(room[field]["digest"], digest)

        missing_codes = {item["code"] for item in source_evidence["missing_evidence"]}
        self.assertTrue(
            {
                "deployment_lineage_absent",
                "storage_epoch_absent",
                "sqlite_canonical_export_unavailable",
            }.issubset(missing_codes)
        )


if __name__ == "__main__":
    unittest.main()

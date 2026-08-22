"""Boundary tests for the PostgreSQL-native restore runner."""

from __future__ import annotations

import json
import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RUNNER = ROOT / "scripts" / "postgres-native-restore-smoke.sh"


class NativePostgresRestoreSmokeTests(unittest.TestCase):
    def test_help_is_provider_independent(self) -> None:
        result = subprocess.run(
            [str(RUNNER), "--help"],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("digest-pinned PostgreSQL 17.11", result.stdout)

    def test_missing_tools_are_unavailable_and_redacted(self) -> None:
        environment = os.environ.copy()
        environment.update(
            {
                "WORLDSTREAM_NATIVE_PG_CARGO": str(ROOT / "missing-cargo"),
                "WORLDSTREAM_NATIVE_PG_DUMP": str(ROOT / "missing-pg-dump"),
                "WORLDSTREAM_NATIVE_PG_RESTORE": str(ROOT / "missing-pg-restore"),
                "WORLDSTREAM_NATIVE_PG_PSQL": str(ROOT / "missing-psql"),
            }
        )
        result = subprocess.run(
            [str(RUNNER)],
            cwd=ROOT,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 10)
        evidence = json.loads(result.stdout.splitlines()[-1])
        self.assertEqual(evidence["status"], "unavailable")
        self.assertFalse(evidence["release_evidence"])
        self.assertFalse(evidence["secrets_emitted"])

    def test_external_missing_driver_fails_closed_without_target_publication(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-native-pg-test-") as name:
            passfile = Path(name) / "pgpass"
            passfile.write_text(
                "source:5432:worldstream:postgres:SOURCE_SECRET\n", encoding="utf-8"
            )
            passfile.chmod(stat.S_IRUSR | stat.S_IWUSR)
            environment = os.environ.copy()
            environment.update(
                {
                    "WORLDSTREAM_NATIVE_PG_SOURCE_HOST": "source",
                    "WORLDSTREAM_NATIVE_PG_SOURCE_PORT": "5432",
                    "WORLDSTREAM_NATIVE_PG_TARGET_HOST": "target",
                    "WORLDSTREAM_NATIVE_PG_TARGET_PORT": "5433",
                    "WORLDSTREAM_NATIVE_PG_PGPASSFILE": str(passfile),
                    "WORLDSTREAM_NATIVE_PG_CARGO": "/usr/bin/true",
                }
            )
            result = subprocess.run(
                [str(RUNNER)],
                cwd=ROOT,
                env=environment,
                text=True,
                capture_output=True,
                check=False,
            )
        self.assertEqual(result.returncode, 13)
        evidence = json.loads(result.stdout.splitlines()[-1])
        self.assertFalse(evidence["release_evidence"])
        self.assertFalse(evidence["target_published"])
        self.assertNotIn("SOURCE_SECRET", result.stdout + result.stderr)
        self.assertNotIn("postgresql://", result.stdout + result.stderr)

    def test_fake_cargo_captures_no_password_or_uri_in_argv_output_or_error(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-native-pg-test-") as name:
            directory = Path(name)
            passfile = directory / "pgpass"
            passfile.write_text(
                "source:5432:worldstream:postgres:TOP_SECRET\n", encoding="utf-8"
            )
            passfile.chmod(stat.S_IRUSR | stat.S_IWUSR)
            trace = directory / "trace"
            fake_cargo = directory / "fake-cargo"
            fake_cargo.write_text(
                "#!/bin/sh\n"
                "set -eu\n"
                ': "${TRACE:?}"\n'
                'printf \'%s\\n\' "$@" >>"$TRACE"\n'
                'printf \'%s\\n\' "${PGPASSFILE-}" >>"$TRACE"\n'
                'printf \'%s\\n\' \'{"status":"incomplete","release_evidence":false,"target_published":false}\'\n'
                "printf '%s\\n' 'fake stderr' >&2\n"
                "exit 13\n",
                encoding="utf-8",
            )
            fake_cargo.chmod(fake_cargo.stat().st_mode | stat.S_IXUSR)
            fake_tool = directory / "fake-tool"
            fake_tool.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            fake_tool.chmod(fake_tool.stat().st_mode | stat.S_IXUSR)
            environment = os.environ.copy()
            environment.update(
                {
                    "TRACE": str(trace),
                    "WORLDSTREAM_NATIVE_PG_SOURCE_HOST": "source-host",
                    "WORLDSTREAM_NATIVE_PG_SOURCE_PORT": "5432",
                    "WORLDSTREAM_NATIVE_PG_TARGET_HOST": "target-host",
                    "WORLDSTREAM_NATIVE_PG_TARGET_PORT": "5433",
                    "WORLDSTREAM_NATIVE_PG_PGPASSFILE": str(passfile),
                    "WORLDSTREAM_NATIVE_PG_CARGO": str(fake_cargo),
                    "WORLDSTREAM_NATIVE_PG_DUMP": str(fake_tool),
                    "WORLDSTREAM_NATIVE_PG_RESTORE": str(fake_tool),
                    "WORLDSTREAM_NATIVE_PG_PSQL": str(fake_tool),
                }
            )
            result = subprocess.run(
                [str(RUNNER)],
                cwd=ROOT,
                env=environment,
                text=True,
                capture_output=True,
                check=False,
            )
            captured = trace.read_text(encoding="utf-8") + result.stdout + result.stderr
        self.assertEqual(result.returncode, 13)
        self.assertIn("source-host", captured)
        self.assertIn("target-host", captured)
        self.assertNotIn("TOP_SECRET", captured)
        self.assertNotIn("postgresql://", captured)
        self.assertNotIn("password=", captured)


if __name__ == "__main__":
    unittest.main()

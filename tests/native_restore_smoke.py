#!/usr/bin/env python3
"""Fail-closed boundary tests for the bundled native SQLite restore runner."""

from __future__ import annotations

import json
import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HARNESS = ROOT / "scripts" / "native-restore-smoke.sh"
SCHEMA = "worldstream/native-sqlite-restore-evidence/v1"


class NativeRestoreSmokeTests(unittest.TestCase):
    def run_harness(
        self,
        *,
        source: Path | None = None,
        cargo: Path | None = None,
        metadata: Path | None = None,
        envelope: Path | None = None,
    ):
        env = os.environ.copy()
        env["WORLDSTREAM_NATIVE_RESTORE_CARGO"] = str(cargo or ROOT / "missing-cargo")
        if source is not None:
            env["WORLDSTREAM_NATIVE_RESTORE_SOURCE"] = str(source)
        if metadata is not None:
            env["WORLDSTREAM_NATIVE_RESTORE_METADATA_FILE"] = str(metadata)
        if envelope is not None:
            env["WORLDSTREAM_NATIVE_RESTORE_ENVELOPE_FILE"] = str(envelope)
        return subprocess.run(
            [str(HARNESS)],
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )

    @staticmethod
    def executable(path: Path, body: str) -> Path:
        path.write_text(body, encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)
        return path

    def test_missing_source_is_incomplete(self) -> None:
        completed = self.run_harness()
        self.assertEqual(completed.returncode, 13)
        evidence = json.loads(completed.stdout.splitlines()[-1])
        self.assertEqual(evidence["schema"], SCHEMA)
        self.assertEqual(evidence["reason"], "source_not_supplied")
        self.assertFalse(evidence["release_evidence"])

    def test_corrupt_source_is_fail_closed_and_redacted(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-native-restore-") as name:
            source = Path(name) / "corrupt.sqlite"
            source.write_bytes(b"not-a-sqlite-database")
            envelope = Path(name) / "envelope.json"
            envelope.write_text("{}", encoding="utf-8")
            completed = self.run_harness(
                source=source, cargo=Path("/bin/false"), envelope=envelope
            )
        self.assertIn(completed.returncode, (10, 14))
        output = completed.stdout + completed.stderr
        self.assertNotIn(str(source), output)
        evidence = json.loads(completed.stdout.splitlines()[-1])
        self.assertFalse(evidence["release_evidence"])
        self.assertFalse(evidence["secrets_emitted"])

    def test_missing_cargo_is_unavailable_not_success(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-native-restore-") as name:
            source = Path(name) / "source.sqlite"
            source.write_bytes(b"placeholder")
            envelope = Path(name) / "envelope.json"
            envelope.write_text("{}", encoding="utf-8")
            completed = self.run_harness(source=source, envelope=envelope)
        self.assertEqual(completed.returncode, 10)
        evidence = json.loads(completed.stdout.splitlines()[-1])
        self.assertEqual(evidence["reason"], "cargo_unavailable")
        self.assertFalse(evidence["release_evidence"])

    def test_help_is_available_without_provider(self) -> None:
        completed = subprocess.run(
            [str(HARNESS), "--help"],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(completed.returncode, 0)
        self.assertIn("isolated temporary target", completed.stdout)
        self.assertIn("--metadata PATH", completed.stdout)

    def test_missing_explicit_metadata_sidecar_is_incomplete(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-native-restore-") as name:
            source = Path(name) / "source.sqlite"
            source.write_bytes(b"placeholder")
            metadata = Path(name) / "missing-metadata.json"
            envelope = Path(name) / "envelope.json"
            envelope.write_text("{}", encoding="utf-8")
            completed = self.run_harness(
                source=source,
                cargo=Path("/usr/bin/true"),
                metadata=metadata,
                envelope=envelope,
            )
        self.assertEqual(completed.returncode, 13)
        evidence = json.loads(completed.stdout.splitlines()[-1])
        self.assertEqual(evidence["reason"], "metadata_adapter_not_a_regular_file")
        self.assertFalse(evidence["release_evidence"])


if __name__ == "__main__":
    unittest.main()

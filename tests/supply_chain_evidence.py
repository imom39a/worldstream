#!/usr/bin/env python3
"""Bounded tests for the non-release supply-chain audit lane."""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/supply-chain-evidence.sh"


class SupplyChainEvidenceTests(unittest.TestCase):
    def run_audit(self, *args: str, env: dict[str, str] | None = None):
        return subprocess.run(
            [str(SCRIPT), *args],
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_audit_is_structural_and_covers_exact_subject_hashes(self):
        with tempfile.TemporaryDirectory() as temporary:
            first = Path(temporary) / "first.bin"
            second = Path(temporary) / "second.bin"
            first.write_bytes(b"first subject\n")
            second.write_bytes(b"second subject\n")
            result = self.run_audit("--subject", str(first), "--subject", str(second))

            self.assertEqual(result.returncode, 0, result.stderr)
            report = json.loads(result.stdout)
            self.assertEqual(
                report["schema"], "worldstream/non-release-supply-chain-audit/v1"
            )
            self.assertFalse(report["release_evidence"])
            self.assertFalse(report["identity_backed"])
            self.assertTrue(report["subject_coverage"]["exact"])
            self.assertEqual(report["subject_coverage"]["subject_count"], 2)
            expected = {
                os.path.relpath(path, ROOT): hashlib.sha256(
                    path.read_bytes()
                ).hexdigest()
                for path in (first, second)
            }
            observed = {
                row["path"]: row["sha256"]
                for row in report["subject_coverage"]["subjects"]
            }
            self.assertEqual(observed, expected)
            self.assertEqual(report["manifests_read"], [])
            self.assertEqual(report["manifests_written"], [])

    def test_keyless_fails_closed_without_oidc_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            subject = Path(temporary) / "subject.bin"
            subject.write_bytes(b"keyless must not start\n")
            env = os.environ.copy()
            for name in (
                "SIGSTORE_ID_TOKEN",
                "COSIGN_IDENTITY_TOKEN",
                "ACTIONS_ID_TOKEN_REQUEST_URL",
                "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
            ):
                env.pop(name, None)
            result = self.run_audit(
                "--mode", "keyless", "--subject", str(subject), env=env
            )

            self.assertEqual(result.returncode, 2)
            report = json.loads(result.stdout)
            self.assertEqual(report["status"], "blocked")
            self.assertEqual(report["reason"], "oidc_identity_missing")
            self.assertFalse(report["release_evidence"])

    def test_keyless_fails_closed_when_cosign_is_unavailable(self):
        with tempfile.TemporaryDirectory() as temporary:
            subject = Path(temporary) / "subject.bin"
            subject.write_bytes(b"cosign is required\n")
            result = self.run_audit(
                "--mode",
                "keyless",
                "--subject",
                str(subject),
                env={
                    **os.environ,
                    "COSIGN_BIN": str(Path(temporary) / "missing-cosign"),
                },
            )

            self.assertEqual(result.returncode, 2)
            report = json.loads(result.stdout)
            self.assertEqual(report["status"], "blocked")
            self.assertEqual(report["reason"], "cosign_missing")
            self.assertFalse(report["release_evidence"])

    def test_local_mode_requires_an_explicit_key_and_is_not_release_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            subject = Path(temporary) / "subject.bin"
            subject.write_bytes(b"local key is intentionally bounded\n")
            result = self.run_audit(
                "--mode",
                "local",
                "--subject",
                str(subject),
                "--local-key",
                str(Path(temporary) / "missing-key"),
            )

            self.assertEqual(result.returncode, 2)
            report = json.loads(result.stdout)
            self.assertEqual(report["status"], "blocked")
            self.assertEqual(report["reason"], "local_key_missing")
            self.assertFalse(report["release_evidence"])


if __name__ == "__main__":
    unittest.main()

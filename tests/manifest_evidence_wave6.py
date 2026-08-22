"""Tests for the read-only Wave 6 manifest identity audit."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/manifest-evidence-wave6.py"


def load_module():
    spec = importlib.util.spec_from_file_location("manifest_evidence_wave6", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load Wave 6 audit module")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ManifestEvidenceWave6Tests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.module = load_module()
        cls.report = cls.module.build_report()

    def test_blake3_known_vectors(self):
        self.assertEqual(
            self.module.blake3(b"").hex(),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
        )

    def test_report_is_implementation_consistent_and_fail_closed(self):
        failures = self.module._validate(self.report)
        self.assertEqual(failures, [])
        self.assertTrue(self.report["manifest"]["state"]["release_ready"])
        self.assertEqual(self.report["manifest"]["state"]["manifest_kind"], "release")
        self.assertTrue(self.report["sqlite"]["identity_consistent"])
        self.assertEqual(
            self.report["packs"]["agent_heist"]["manifest_identity_status"],
            "resolved",
        )
        self.assertEqual(
            self.report["packs"]["agent_heist"]["manifest_row"][
                "executor_artifact_digest"
            ],
            self.report["packs"]["agent_heist"]["executor_artifact_input"]["blake3"],
        )
        self.assertFalse(self.report["claim_policy"]["writes_manifest"])
        self.assertFalse(self.report["claim_policy"]["writes_release_directory"])

    def test_exact_source_digests_and_migration_counts_are_reported(self):
        sqlite = self.report["sqlite"]
        self.assertEqual(sqlite["source_constants"]["SQLITE_VERSION"], "3.53.4")
        self.assertEqual(
            sqlite["source_constants"]["RUSQLITE_BUNDLE_REVISION"],
            "229140734a4a60cc9fa34507fe79cb2277142f49",
        )
        self.assertEqual(self.report["migrations"]["sqlite"]["source_count"], 9)
        self.assertEqual(self.report["migrations"]["postgresql"]["source_count"], 10)
        self.assertEqual(
            self.report["migrations"]["postgresql"]["source_ids_not_in_manifest"],
            [],
        )
        manifest = self.module.tomllib.loads(
            (ROOT / "compatibility.toml").read_text(encoding="utf-8")
        )
        self.assertEqual(
            self.report["migrations"]["postgresql"]["source_records"][0]["blake3"],
            manifest["migrations"]["entries"][0]["postgresql_checksum"],
        )
        for record in self.report["migrations"]["sqlite"]["source_records"]:
            self.assertRegex(record["sha256"], r"^sha256:[0-9a-f]{64}$")
            self.assertRegex(record["blake3"], r"^blake3:[0-9a-f]{64}$")

    def test_cli_json_is_parseable_and_does_not_write(self):
        before = subprocess.run(
            [sys.executable, str(SCRIPT), "--json"],
            cwd=ROOT,
            capture_output=True,
            check=True,
            text=True,
        )
        report = json.loads(before.stdout)
        self.assertEqual(report["schema"], "worldstream/manifest-evidence-wave6/v1")
        self.assertFalse(report["release_inventory"]["release_directory_created"])
        self.assertFalse(report["claim_policy"]["manufactures_external_evidence"])


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Fixture-only tests for the IMO-61 reference evidence aggregator."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "reference-evidence.py"


def digest(label: str) -> str:
    return "sha256:" + hashlib.sha256(label.encode()).hexdigest()


def reference_workload(kind: str) -> dict:
    index = ("counter", "heist", "sqlite", "postgres", "soak").index(kind) + 1
    return {
        "payload_sizes_bytes": [128 * index, 1024 * index],
        "pack_id": {
            "counter": "worldstream.counter",
            "heist": "worldstream.agent-heist",
            "sqlite": "worldstream.sqlite-reference",
            "postgres": "worldstream.postgresql-reference",
            "soak": "worldstream.counter",
        }[kind],
        "participants_per_room": index + 1,
        "fan_out": index,
        "snapshot_cadence_transitions": index,
    }


def report(kind: str, *, complete: bool = True) -> dict:
    schemas = {
        "counter": "worldstream/imo-55-live-counter-luna/v1",
        "heist": "worldstream/imo-53-57-live-wave11/v1",
        "sqlite": "worldstream/soak-evidence/v1",
        "postgres": "worldstream/postgresql-evidence/v1",
        "soak": "worldstream/soak-evidence/v1",
    }
    value: dict = {
        "schema": schemas[kind],
        "status": "completed" if kind in {"counter", "heist"} else "pass",
        "release_evidence": False,
        "performance_class": "reference_non_release",
        "identity": {
            "product": "worldstream",
            "profile": "linux-reference",
            "version": "fixture-1",
            "artifact_sha256": digest(kind),
            "packaged_acceptance_sha256": digest("packaged-acceptance"),
        },
        "reference_environment": {
            "platform": {
                "system": "Linux",
                "distribution": "Ubuntu",
                "distribution_version": "24.04",
                "machine": "x86_64",
            },
            "hardware": {
                "cpu_model": "fixture cpu",
                "logical_cpu_count": 4,
                "memory_bytes": 8589934592,
            },
            "filesystem": {
                "type": "ext4",
                "mount_options": ["rw"],
                "storage_class": "local_ssd_or_nvme",
            },
            "engines": {
                "sqlite": {
                    "version": "3.51.3",
                    "settings": {"journal_mode": "wal"},
                    "connection_mode": "embedded",
                },
                "postgresql": {
                    "version": "17.11",
                    "settings": {"synchronous_commit": "on"},
                    "connection_mode": "direct",
                },
            },
        },
        "reference_workload": reference_workload(kind),
        "measurements": {
            "latency_ms": [10, 20, 30, 40, 50, 60, 70, 80, 90, 100],
            "load": {
                "connections": 1000,
                "active_rooms": 100,
                "actions_per_second": 100,
            },
            "fan_out": {"observation_fan_out": 10},
            "memory": {"status": "measured", "peak_rss_bytes": 123456},
            "database_growth": {"status": "measured", "growth_bytes": 4096},
            "recovery": {"durations_ms": [100, 200, 300]},
        },
    }
    if kind == "soak":
        value["one_hour_window_completed"] = complete
        value["database"] = {"status": "measured", "growth_bytes": 4096}
    return value


class ReferenceEvidenceTests(unittest.TestCase):
    def run_aggregator(self, values: dict[str, dict], *extra: str):
        with tempfile.TemporaryDirectory(
            prefix="worldstream-reference-evidence-"
        ) as directory:
            root = Path(directory)
            paths = {}
            for kind, value in values.items():
                path = root / f"{kind}.json"
                path.write_text(json.dumps(value), encoding="utf-8")
                paths[kind] = path
            command = [sys.executable, str(SCRIPT)]
            for kind in ("counter", "heist", "sqlite", "postgres", "soak"):
                command.extend((f"--{kind}-report", str(paths[kind])))
            command.extend(extra)
            return subprocess.run(
                command, cwd=ROOT, text=True, capture_output=True, check=False
            )

    def test_complete_fixture_is_deterministic_and_recomputes_percentiles(self):
        values = {
            kind: report(kind)
            for kind in ("counter", "heist", "sqlite", "postgres", "soak")
        }
        first = self.run_aggregator(values)
        second = self.run_aggregator(values)
        self.assertEqual(first.returncode, 0, first.stderr + first.stdout)
        self.assertEqual(first.stdout, second.stdout)
        result = json.loads(first.stdout)
        self.assertEqual(result["schema"], "worldstream/imo-61-reference-evidence/v1")
        self.assertEqual(result["status"], "pass")
        self.assertFalse(result["release_evidence"])
        percentiles = result["measurements"]["latency_ms"]["percentiles"]["counter"]
        self.assertEqual(percentiles["p50_ms"], 50.0)
        self.assertEqual(percentiles["p95_ms"], 100.0)
        self.assertEqual(percentiles["p99_ms"], 100.0)
        self.assertEqual(len(result["input_sha256_inventory"]["items"]), 5)
        self.assertEqual(
            result["workloads"]["counter"]["pack_id"], "worldstream.counter"
        )
        self.assertEqual(
            result["workloads"]["heist"]["pack_id"], "worldstream.agent-heist"
        )
        self.assertEqual(
            result["identity"]["packaged_acceptance_sha256"],
            digest("packaged-acceptance"),
        )
        self.assertNotIn("signature_bundle", result)
        self.assertNotIn("signature_bundle_digest", result)

    def test_soak_report_accepts_existing_nearest_rank_statistics(self):
        values = {
            kind: report(kind)
            for kind in ("counter", "heist", "sqlite", "postgres", "soak")
        }
        values["soak"].pop("measurements")
        values["soak"]["statistics"] = {
            "matrix_run_count": 507,
            "command_duration_ms": {
                "definition": "nearest-rank percentile",
                "p50": 7112,
                "p95": 7527,
                "p99": 8028,
            },
            "memory": {
                "status": "measured",
                "peak_rss_bytes_per_run": [130000000, 131022848],
            },
        }
        values["soak"]["database"] = {"status": "measured", "growth_bytes": 0}
        completed = self.run_aggregator(values)
        self.assertEqual(completed.returncode, 0, completed.stderr + completed.stdout)
        result = json.loads(completed.stdout)
        self.assertEqual(
            result["measurements"]["latency_ms"]["percentiles"]["soak"]["p99_ms"],
            8028.0,
        )
        self.assertEqual(
            result["measurements"]["memory"]["sources"]["soak"]["peak_rss_bytes"],
            131022848,
        )

    def test_true_release_label_is_blocked(self):
        values = {
            kind: report(kind)
            for kind in ("counter", "heist", "sqlite", "postgres", "soak")
        }
        values["counter"]["release_evidence"] = True
        completed = self.run_aggregator(values)
        self.assertEqual(completed.returncode, 2)
        result = json.loads(completed.stdout)
        self.assertEqual(result["status"], "blocked")
        self.assertIn("counter:release_evidence_must_be_false", result["errors"])

    def test_common_identity_version_mismatch_is_blocked(self):
        values = {
            kind: report(kind)
            for kind in ("counter", "heist", "sqlite", "postgres", "soak")
        }
        values["postgres"]["identity"]["version"] = "fixture-2"
        completed = self.run_aggregator(values)
        self.assertEqual(completed.returncode, 2)
        result = json.loads(completed.stdout)
        self.assertEqual(result["status"], "blocked")
        self.assertIn("postgres:identity_version_mismatch", result["errors"])

    def test_non_linux_reference_profile_is_blocked(self):
        values = {
            kind: report(kind)
            for kind in ("counter", "heist", "sqlite", "postgres", "soak")
        }
        values["sqlite"]["identity"]["profile"] = "windows-reference"
        completed = self.run_aggregator(values)
        self.assertEqual(completed.returncode, 2)
        result = json.loads(completed.stdout)
        self.assertEqual(result["status"], "blocked")
        self.assertIn(
            "sqlite:identity_profile_must_be_linux_reference", result["errors"]
        )

    def test_noncertified_host_dimensions_are_blocked(self):
        cases = (
            (
                ("platform", "distribution_version"),
                "22.04",
                "counter:ubuntu_24_04_x86_64_reference_platform_required",
            ),
            (
                ("hardware", "logical_cpu_count"),
                8,
                "counter:reference_4vcpu_8gib_hardware_required",
            ),
            (
                ("hardware", "memory_bytes"),
                16 * 1024 * 1024 * 1024,
                "counter:reference_4vcpu_8gib_hardware_required",
            ),
            (
                ("filesystem", "type"),
                "xfs",
                "counter:reference_ext4_local_ssd_disclosure_required",
            ),
            (
                ("filesystem", "storage_class"),
                "network_block_device",
                "counter:reference_ext4_local_ssd_disclosure_required",
            ),
        )
        for field, value, expected_error in cases:
            with self.subTest(field=field):
                values = {
                    kind: report(kind)
                    for kind in ("counter", "heist", "sqlite", "postgres", "soak")
                }
                values["counter"]["reference_environment"][field[0]][field[1]] = value
                completed = self.run_aggregator(values)
                self.assertEqual(completed.returncode, 2)
                result = json.loads(completed.stdout)
                self.assertEqual(result["status"], "blocked")
                self.assertIn(expected_error, result["errors"])

    def test_packaged_acceptance_identity_must_match_all_five_sources(self):
        values = {
            kind: report(kind)
            for kind in ("counter", "heist", "sqlite", "postgres", "soak")
        }
        values["postgres"]["identity"]["packaged_acceptance_sha256"] = digest(
            "different-acceptance"
        )

        completed = self.run_aggregator(values)

        self.assertEqual(completed.returncode, 2)
        result = json.loads(completed.stdout)
        self.assertEqual(result["status"], "blocked")
        self.assertIn(
            "postgres:identity_packaged_acceptance_mismatch", result["errors"]
        )

    def test_missing_backend_dimension_keeps_summary_incomplete(self):
        values = {
            kind: report(kind)
            for kind in ("counter", "heist", "sqlite", "postgres", "soak")
        }
        values["postgres"]["measurements"].pop("recovery")
        completed = self.run_aggregator(values)
        self.assertEqual(completed.returncode, 2)
        result = json.loads(completed.stdout)
        self.assertEqual(result["status"], "incomplete")
        self.assertFalse(result["coverage"]["postgres"]["recovery"])
        self.assertIn("postgres:recovery_not_reported", result["gaps"])
        self.assertNotEqual(result["status"], "pass")

    def test_numeric_placeholders_do_not_count_as_measured_resources(self):
        values = {
            kind: report(kind)
            for kind in ("counter", "heist", "sqlite", "postgres", "soak")
        }
        values["sqlite"]["measurements"]["memory"] = {
            "status": "not_measured",
            "peak_rss_bytes": 0,
        }
        values["postgres"]["measurements"]["database_growth"] = {
            "status": "not_measured",
            "growth_bytes": 0,
        }

        completed = self.run_aggregator(values)

        self.assertEqual(completed.returncode, 2)
        result = json.loads(completed.stdout)
        self.assertEqual(result["status"], "incomplete")
        self.assertFalse(result["coverage"]["sqlite"]["memory"])
        self.assertFalse(result["coverage"]["postgres"]["database_growth"])
        self.assertIn("sqlite:memory_not_reported", result["gaps"])
        self.assertIn("postgres:database_growth_not_reported", result["gaps"])

    def test_identity_and_schema_are_fail_closed(self):
        values = {
            kind: report(kind)
            for kind in ("counter", "heist", "sqlite", "postgres", "soak")
        }
        values["heist"]["identity"].pop("artifact_sha256")
        values["sqlite"]["schema"] = "worldstream/unknown/v1"
        completed = self.run_aggregator(values)
        self.assertEqual(completed.returncode, 2)
        result = json.loads(completed.stdout)
        self.assertEqual(result["status"], "blocked")
        self.assertIn(
            "heist:identity_artifact_sha256:sha256_reference_required", result["errors"]
        )
        self.assertIn("sqlite:unsupported_schema", result["errors"])
        self.assertEqual(len(result["input_sha256_inventory"]["items"]), 5)

    def test_incomplete_one_hour_and_unmeasured_dimensions_are_honest_gaps(self):
        values = {
            kind: report(kind)
            for kind in ("counter", "heist", "sqlite", "postgres", "soak")
        }
        values["soak"]["one_hour_window_completed"] = False
        for value in values.values():
            value.pop("measurements")
        values["soak"]["statistics"] = {
            "command_duration_ms": {
                "definition": "nearest-rank",
                "sample_count": 2,
                "p50_ms": 1,
                "p95_ms": 2,
                "p99_ms": 2,
            }
        }
        values["soak"]["database"] = {"status": "not_configured"}
        completed = self.run_aggregator(values)
        self.assertEqual(completed.returncode, 2)
        result = json.loads(completed.stdout)
        self.assertEqual(result["status"], "incomplete")
        self.assertIn("soak:one_hour_window_not_completed", result["gaps"])
        self.assertIn("database_growth:not_measured", result["gaps"])
        self.assertIn("load:parameters_not_reported", result["gaps"])


if __name__ == "__main__":
    unittest.main()

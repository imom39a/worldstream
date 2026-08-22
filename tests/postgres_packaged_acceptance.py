"""Boundary tests for the packaged SQLite/PostgreSQL parity lane."""

from __future__ import annotations

import base64
import hashlib
import importlib.util
import io
import json
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "postgres-packaged-acceptance.py"
SCHEMA = "worldstream/packaged-backend-parity/v1"


def _module():
    spec = importlib.util.spec_from_file_location(
        "postgres_packaged_acceptance", SCRIPT
    )
    if spec is None or spec.loader is None:
        raise RuntimeError("packaged acceptance module unavailable")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _package_fixture(root: Path, *, unsafe_link: bool = False) -> tuple[Path, Path]:
    version = "0.1.0"
    archive_root = f"worldstream-{version}-linux-x86_64"
    manifest_toml = b'manifest_kind = "release"\nrelease_ready = true\n'
    authored = {"manifest_kind": "release", "release_ready": True}
    manifest_json = (
        json.dumps(authored, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode()
    json_digest = hashlib.sha256(manifest_json).hexdigest()
    metadata = (
        json.dumps(
            {
                "manifest": {
                    "file": "manifest/compatibility.json",
                    "schema": "worldstream/storage-compatibility-manifest/v1",
                    "sha256": json_digest,
                },
                "target": "linux-x86_64",
                "version": version,
            },
            sort_keys=True,
        )
        + "\n"
    ).encode()
    files = {
        "bin/worldstreamd": b"#!/bin/sh\nexit 0\n",
        "bin/worldstreamctl": b"#!/bin/sh\nexit 0\n",
        "manifest/compatibility.toml": manifest_toml,
        "manifest/compatibility.json": manifest_json,
        "metadata/release.json": metadata,
    }
    checksums = "".join(
        f"{hashlib.sha256(value).hexdigest()}  {path}\n"
        for path, value in sorted(files.items())
    ).encode()
    files["checksums.sha256"] = checksums
    archive = root / f"{archive_root}.tar.gz"
    with tarfile.open(archive, "w:gz") as output:
        for relative, value in sorted(files.items()):
            member = tarfile.TarInfo(f"{archive_root}/{relative}")
            member.mode = 0o755 if relative.startswith("bin/") else 0o644
            member.size = len(value)
            output.addfile(member, io.BytesIO(value))
        if unsafe_link:
            member = tarfile.TarInfo(f"{archive_root}/unsafe-link")
            member.type = tarfile.SYMTYPE
            member.linkname = "/etc/passwd"
            output.addfile(member)
    report = root / "package-report.json"
    archive_digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    report.write_text(
        json.dumps(
            {
                "schema": "worldstream/package-report/v1",
                "artifact": archive.name,
                "kind": "archive",
                "path": archive.name,
                "sha256": f"sha256:{archive_digest}",
                "size_bytes": archive.stat().st_size,
                "inventory": {
                    "archive_verified": True,
                    "manifest_source": "compatibility.toml",
                    "manifest_mirror": "compatibility.json",
                    "release_evidence": False,
                },
                "identity": {
                    "target": "linux-x86_64",
                    "version": version,
                    "manifest_sha256": json_digest,
                    "manifest_json_sha256": json_digest,
                    "manifest_toml_sha256": hashlib.sha256(manifest_toml).hexdigest(),
                },
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    return archive, report


def _valid_absent_broker_report() -> dict:
    hashes = {
        "pack": "blake3:pack",
        "core": "blake3:core",
        "activity": "blake3:activity",
        "aggregate_authoritative": "blake3:aggregate",
        "transition": "blake3:transition",
        "room_id": "room",
        "room_seq": 17,
    }
    return {
        "status": "completed",
        "storage": {"status": "verified", "profile": "sqlite-bundled"},
        "secrets": "not_emitted",
        "measurements": {"fan_out": {"observation_fan_out": 3}},
        "reference_workload": {
            "payload_sizes_bytes": [2, 64],
            "pack_id": "worldstream.agent-heist",
            "participants_per_room": 3,
            "fan_out": 3,
            "snapshot_cadence_transitions": 1,
        },
        "six_phase_order": True,
        "private_contexts_emitted": False,
        "final": {
            "phase": "complete",
            "broker_seat_present": True,
            "commitment_count": 2,
            "replay_verified": True,
            "outcome": {
                "outcome": "success",
                "score": 5,
                "reason": "scored_selected_plan",
                "missing_roles": ["broker"],
                "vote_counts": {"plan": 2},
                "checks": {
                    "route": True,
                    "entry_window": True,
                    "required_tool": True,
                    "extraction": True,
                    "resource_contributed": True,
                },
            },
            "replay_hash_parity": {
                "verified": True,
                "expected": hashes,
                "replayed": dict(hashes),
                "fields": list(hashes),
            },
        },
        "lost_claim_reply": {
            "status": "completed",
            "transport_loss_observed": True,
            "durable_duplicate_result_matches": True,
        },
        "timers": [
            {"transition_id_present": True, "duplicate": False} for _ in range(5)
        ],
    }


class PostgreSQLPackagedAcceptanceTests(unittest.TestCase):
    @staticmethod
    def _privacy_capture(module, root: Path):
        capture = module.PrivacyCapture(root)
        for kind, value in {
            "authority-secret": b"a" * 32,
            "operator-capability": b"wsb1:" + b"b" * 64,
            "postgres-admin-password": b"c" * 48,
            "postgres-runtime-password": b"d" * 48,
            "postgres-admin-dsn": b"host=db user=admin password=" + b"e" * 48,
            "postgres-runtime-dsn": b"host=db user=runtime password=" + b"f" * 48,
        }.items():
            capture.register_sentinel(kind, value)
        return capture

    def test_help_is_side_effect_free_and_script_is_executable(self) -> None:
        self.assertTrue(SCRIPT.stat().st_mode & stat.S_IXUSR)
        completed = subprocess.run(
            [sys.executable, str(SCRIPT), "--help"],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(completed.returncode, 0)
        self.assertIn("PostgreSQL", completed.stdout)
        self.assertIn("PgBouncer", completed.stdout)

    def test_privacy_capture_retains_child_channel_digests(self) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(prefix="worldstream-privacy-capture-") as name:
            root = Path(name)
            capture = self._privacy_capture(module, root)
            capture._record_bytes("stdout", b"clean stdout\n")
            capture._record_bytes("stderr", b"")
            capture._record_bytes("config", b'{"secret_source":"file"}\n')
            capture._record_bytes("report", b'{"secrets":"not_emitted"}\n')
            result = capture.scan()

        self.assertEqual(result["schema"], "worldstream/secret-absence-matrix/v1")
        self.assertEqual(result["status"], "pass")
        self.assertFalse(result["secrets_emitted"])
        self.assertEqual(
            result["encodings_scanned"], ["base64", "base64url", "hex", "raw"]
        )
        self.assertEqual(len(result["channels"]), 4)
        self.assertTrue(
            all(row["sha256"].startswith("sha256:") for row in result["channels"])
        )

    def test_privacy_capture_rejects_each_encoding_in_every_child_channel(self) -> None:
        module = _module()
        sentinel = b"runtime-password-" + b"9" * 48
        encoded = {
            "raw": sentinel,
            "hex": sentinel.hex().encode("ascii"),
            "base64": base64.b64encode(sentinel),
            "base64url": base64.urlsafe_b64encode(sentinel).rstrip(b"="),
        }
        for channel in ("daemon-log", "stdout", "stderr", "config", "report"):
            for encoding, injected in encoded.items():
                with (
                    self.subTest(channel=channel, encoding=encoding),
                    tempfile.TemporaryDirectory(
                        prefix="worldstream-privacy-negative-"
                    ) as name,
                ):
                    root = Path(name)
                    capture = self._privacy_capture(module, root)
                    capture.register_sentinel("injected-password", sentinel)
                    for candidate in (
                        "daemon-log",
                        "stdout",
                        "stderr",
                        "config",
                        "report",
                    ):
                        capture._record_bytes(
                            candidate,
                            injected if candidate == channel else b"clean\n",
                        )
                    with self.assertRaisesRegex(
                        module.LaneFailure, "privacy_secret_absence_scan_failed"
                    ):
                        capture.scan()

    def test_safe_run_retains_stdout_stderr_and_value_free_config(self) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(prefix="worldstream-child-channel-") as name:
            root = Path(name)
            capture = module.PrivacyCapture(root)
            module.ACTIVE_PRIVACY_CAPTURE = capture
            try:
                completed = module._safe_run(
                    [
                        sys.executable,
                        "-c",
                        "import sys; print('out'); print('err', file=sys.stderr)",
                    ],
                    cwd=ROOT,
                    environment={"PATH": "not-retained", "PGPASSWORD": "not-retained"},
                )
            finally:
                module.ACTIVE_PRIVACY_CAPTURE = None
            self.assertEqual(completed.returncode, 0)
            contents = [path.read_text() for path in capture.channels.values()]
        self.assertTrue(any(value == "out\n" for value in contents))
        self.assertTrue(any(value == "err\n" for value in contents))
        config = next(value for value in contents if "environment_keys" in value)
        self.assertIn('"PGPASSWORD"', config)
        self.assertIn('"environment_values_retained":false', config)
        self.assertNotIn("not-retained", config)

    def test_missing_package_inputs_are_closed_without_starting_docker(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-packaged-boundary-"
        ) as name:
            root = Path(name)
            report = root / "report.json"
            completed = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--repository",
                    str(root),
                    "--worldstreamd",
                    "missing-worldstreamd",
                    "--worldstreamctl",
                    "missing-worldstreamctl",
                    "--docker",
                    "missing-docker",
                    "--psql",
                    "missing-psql",
                    "--report",
                    str(report),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            value = json.loads(report.read_text(encoding="utf-8"))
        self.assertEqual(completed.returncode, 10)
        self.assertEqual(value["schema"], SCHEMA)
        self.assertEqual(value["status"], "unavailable")
        self.assertEqual(value["reason_code"], "package_bound_inputs_required")
        self.assertFalse(value["release_evidence"])
        self.assertFalse(value["secrets_emitted"])

    def test_exact_report_archive_and_manifest_pair_bind_both_binaries(self) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(prefix="worldstream-package-bind-") as name:
            root = Path(name)
            archive, report = _package_fixture(root)
            daemon, ctl, binding = module._bind_package(
                archive, report, root / "extracted"
            )
            self.assertTrue(daemon.is_file())
            self.assertTrue(ctl.is_file())
            self.assertEqual(binding["status"], "pass")
            self.assertEqual(
                binding["identity"]["manifest_sha256"],
                binding["identity"]["manifest_json_sha256"],
            )
            self.assertTrue(binding["archive"]["checksums_exact"])

    def test_linked_archive_member_is_rejected_before_extraction(self) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(prefix="worldstream-package-link-") as name:
            root = Path(name)
            archive, report = _package_fixture(root, unsafe_link=True)
            with self.assertRaisesRegex(
                module.LaneFailure, "package_archive_member_safety_failed"
            ):
                module._bind_package(archive, report, root / "extracted")
            self.assertFalse((root / "extracted").exists())

    def test_report_must_bind_explicit_toml_and_json_digests(self) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(
            prefix="worldstream-package-manifest-"
        ) as name:
            root = Path(name)
            archive, report = _package_fixture(root)
            value = json.loads(report.read_text(encoding="utf-8"))
            value["identity"]["manifest_toml_sha256"] = "0" * 64
            report.write_text(json.dumps(value) + "\n", encoding="utf-8")
            with self.assertRaisesRegex(
                module.LaneFailure, "package_manifest_report_binding_failed"
            ):
                module._bind_package(archive, report, root / "extracted")

    def test_secret_files_are_owner_only(self) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(prefix="worldstream-packaged-secret-") as name:
            target = Path(name) / "runtime.dsn"
            module._owner_file(target, "opaque")
            self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o600)

    def test_source_uses_file_only_product_dsn_and_passwordless_psql(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        self.assertIn("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE", source)
        self.assertNotIn('environment["WORLDSTREAM__STORAGE__POSTGRESQL__DSN"]', source)
        self.assertNotIn("POSTGRES_PASSWORD=", source)
        self.assertNotIn("DATABASE_URL=", source)
        self.assertIn('"PGPASSWORD": password or self.admin_password', source)
        self.assertIn(
            'dsn = f"host=127.0.0.1 port={target_port} dbname={database} user={user}"',
            source,
        )

    def test_heist_validator_preserves_immutable_absent_broker_seat(self) -> None:
        module = _module()
        report = _valid_absent_broker_report()
        module._validate_cell("heist", "sqlite", report)

        absent_seat = json.loads(json.dumps(report))
        absent_seat["final"]["broker_seat_present"] = False
        with self.assertRaisesRegex(
            module.LaneFailure, "heist_sqlite_story_contract_failed"
        ):
            module._validate_cell("heist", "sqlite", absent_seat)

        wrong_missing_role = json.loads(json.dumps(report))
        wrong_missing_role["final"]["outcome"]["missing_roles"] = []
        with self.assertRaisesRegex(
            module.LaneFailure, "heist_sqlite_absent_broker_outcome_failed"
        ):
            module._validate_cell("heist", "sqlite", wrong_missing_role)

    def test_normalized_comparator_excludes_only_per_run_identity_hashes(self) -> None:
        module = _module()
        heist_left = _valid_absent_broker_report()
        heist_right = json.loads(json.dumps(heist_left))
        heist_right["final"]["replay_hash_parity"]["expected"]["core"] = (
            "blake3:other-core"
        )
        heist_right["final"]["replay_hash_parity"]["replayed"]["core"] = (
            "blake3:other-core"
        )
        heist_left["lost_claim_reply"]["recovery_duration_ms"] = 10.0
        heist_right["lost_claim_reply"]["recovery_duration_ms"] = 500.0
        self.assertEqual(
            module._heist_normalized(heist_left),
            module._heist_normalized(heist_right),
        )
        heist_right["final"]["outcome"]["score"] = 4
        self.assertNotEqual(
            module._heist_normalized(heist_left),
            module._heist_normalized(heist_right),
        )

        counter = {
            "criteria": {
                "restart_reconnect_hash_parity": {
                    "status": "passed",
                    "participant": {
                        "before": {
                            "room_seq": 3,
                            "head_hashes": {
                                "pack_digest": "blake3:pack",
                                "activity_state_hash": "blake3:activity",
                                "core_state_hash": "blake3:core-a",
                            },
                            "replay_head_hashes": {
                                "pack_digest": "blake3:pack",
                                "activity_state_hash": "blake3:activity",
                                "core_state_hash": "blake3:core-a",
                            },
                            "projection_hash": "blake3:projection",
                            "replay_hash": "blake3:replay",
                        },
                        "after": {},
                    },
                }
            }
        }
        counter["criteria"]["restart_reconnect_hash_parity"]["participant"]["after"] = (
            json.loads(
                json.dumps(
                    counter["criteria"]["restart_reconnect_hash_parity"]["participant"][
                        "before"
                    ]
                )
            )
        )
        counter_other = json.loads(json.dumps(counter))
        participant = counter_other["criteria"]["restart_reconnect_hash_parity"][
            "participant"
        ]
        participant["before"]["head_hashes"]["core_state_hash"] = "blake3:core-b"
        participant["before"]["replay_head_hashes"]["core_state_hash"] = "blake3:core-b"
        participant["after"] = json.loads(json.dumps(participant["before"]))
        self.assertEqual(
            module._counter_normalized(counter),
            module._counter_normalized(counter_other),
        )


if __name__ == "__main__":
    unittest.main()

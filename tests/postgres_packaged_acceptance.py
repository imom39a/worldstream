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
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "postgres-packaged-acceptance.py"
SCHEMA = "worldstream/packaged-backend-parity/v1"
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


def _assert_runtime_role_sql(query: str) -> None:
    assert query.count("|| '|' ||") == 16
    for fragment in RUNTIME_ROLE_SQL_FRAGMENTS:
        assert fragment in query
    assert "unnest(ARRAY['INSERT'" not in query


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
        "ui/index.html": b"<!doctype html><main id='root'></main>\n",
        "ui/compatibility-identity.json": b"{}\n",
        "sdk/python/src/worldstream_sdk/__init__.py": b"__all__ = []\n",
        "examples/heist/wave10_live/seed_browser_room.py": b"# fixture\n",
        "examples/heist/wave10_live/run_browser_story.py": b"# fixture\n",
        "examples/heist/wave10_live/run_absent_broker_live.py": b"# fixture\n",
        "examples/heist/wave10_live/browser_trace_init.js": b"// fixture\n",
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


def _valid_browser_story(module, binding: dict) -> dict:
    digest = "sha256:" + "1" * 64
    assets = binding["runtime_assets"]
    binary = binding["binaries"]["worldstreamd"]
    return {
        "schema": module.BROWSER_STORY_SCHEMA,
        "canonical_encoding": "utf8-sorted-key-compact-json-lf",
        "status": "pass",
        "release_evidence": True,
        "source_mode": "package-extracted",
        "elapsed_ms": 1234,
        "browser": module.PINNED_BROWSER_IDENTITY,
        "tools": {
            "adapter": {
                "name": "worldstream-cdp-browser",
                "protocol": "Chrome DevTools Protocol",
                "sha256": "sha256:"
                + module._sha256_file(module.ROOT / "scripts/cdp-browser.py"),
                "size_bytes": (module.ROOT / "scripts/cdp-browser.py").stat().st_size,
            },
            "python": {"implementation": "cpython", "version": "3.14.7"},
        },
        "runtime": {
            "worldstreamd": {
                "sha256": binary["sha256"],
                "size_bytes": binary["size_bytes"],
                "origin": "package:bin/worldstreamd",
            },
            "ui": {**assets["ui"], "origin": "package:ui"},
            "sdk": {
                **assets["sdk_python_source"],
                "origin": "package:sdk/python/src",
            },
            "heist_reference_clients": {
                **assets["heist_reference_clients"],
                "origin": "package:examples/heist",
            },
        },
        "story": {
            "phase_path": [
                "Briefing",
                "Negotiation",
                "Commitment",
                "Resolution",
                "Result",
                "Complete",
            ],
            "public_projection": {
                "broker_present": True,
                "commitment_count": 2,
                "aggregate_outcome_present": True,
            },
            "final_replay": {"verified": True, "hash_parity": {"verified": True}},
        },
        "dom_evidence": {
            key: digest
            for key in (
                "stale_rejection",
                "precomplete_reveal",
                "public_final",
                "participant_final",
                "operator_final",
                "replay_final",
                "briefing",
                "negotiation",
                "commitment",
                "result",
                "complete",
                "resync",
                "browser_diagnostics",
            )
        },
        "typed_actions": {
            key: digest
            for key in (
                "inspect_clue",
                "publish_clue",
                "propose_plan",
                "commit_move",
                "acknowledge_result",
            )
        },
        "checks": {
            key: True
            for key in (
                "browser_identity_verified",
                "catch_up_or_reset_installed",
                "embedded_ui_loaded",
                "final_reveal_dom_visible",
                "new_session_resynchronized",
                "package_bound_reference_clients",
                "package_bound_runtime",
                "precomplete_reveal_locked",
                "privacy_negative_dom_and_browser_channels",
                "replay_hashes_verified",
                "six_phase_story_complete",
                "stale_head_rejected",
                "typed_actions_accepted_in_dom",
            )
        },
        "privacy": {
            "status": "pass",
            "private_canary_absent": True,
            "credentials_absent": True,
            "private_claim_absent_from_retained_evidence": True,
        },
    }


class PostgreSQLPackagedAcceptanceTests(unittest.TestCase):
    def test_runtime_role_witness_rejects_each_individual_escalation(self) -> None:
        module = _module()
        self.assertEqual(
            module.RUNTIME_ROLE_ADMISSION_FIELDS,
            (
                "superuser",
                "create_role",
                "create_database",
                "replication",
                "bypass_row_security",
                "database_create",
                "other_role_membership",
                "public_schema_create",
                "owns_public_schema_object",
                "migration_insert",
                "migration_update",
                "migration_delete",
                "migration_truncate",
                "transfer_control_insert",
                "transfer_control_update",
                "transfer_control_delete",
                "transfer_control_truncate",
            ),
        )
        module._validate_runtime_role_admission(module.RUNTIME_ROLE_ADMISSION_EXPECTED)
        witness = module.RUNTIME_ROLE_ADMISSION_EXPECTED.split("|")
        for index, field in enumerate(module.RUNTIME_ROLE_ADMISSION_FIELDS):
            with (
                self.subTest(field=field),
                self.assertRaisesRegex(
                    module.LaneFailure, "postgres_runtime_role_not_least_privileged"
                ),
            ):
                changed = list(witness)
                changed[index] = "true"
                module._validate_runtime_role_admission("|".join(changed))
        _assert_runtime_role_sql(module.RUNTIME_ROLE_ADMISSION_SQL)
        for fragment in RUNTIME_ROLE_SQL_FRAGMENTS:
            with self.subTest(sql_fragment=fragment), self.assertRaises(AssertionError):
                _assert_runtime_role_sql(
                    module.RUNTIME_ROLE_ADMISSION_SQL.replace(fragment, "mutated")
                )
        self.assertIn("NOREPLICATION NOBYPASSRLS", SCRIPT.read_text(encoding="utf-8"))

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

    @staticmethod
    def _record_required_privacy_channels(
        module,
        capture,
        *,
        injected_class: str | None = None,
        injected: bytes = b"clean\n",
    ) -> None:
        for channel_class in module.REQUIRED_PRIVACY_CHANNEL_CLASSES:
            capture._record_bytes(
                channel_class.replace(".", "-"),
                injected if channel_class == injected_class else b"clean\n",
                channel_class=channel_class,
            )

    @staticmethod
    def _fake_docker(
        root: Path,
        *,
        provider_stdout: bytes = b"provider stdout\n",
        provider_stderr: bytes = b"provider stderr\n",
        logs_returncode: int = 0,
        logs_delay_seconds: float = 0.0,
        volume_owner: str = "owner-123",
    ) -> tuple[Path, Path]:
        trace = root / "docker.trace"
        docker = root / "docker"
        docker.write_text(
            "#!/usr/bin/env python3\n"
            "import pathlib\n"
            "import json\n"
            "import sys\n"
            "import time\n"
            f"trace = pathlib.Path({str(trace)!r})\n"
            "with trace.open('a', encoding='utf-8') as output:\n"
            "    output.write(' '.join(sys.argv[1:]) + '\\n')\n"
            "if sys.argv[1:2] == ['logs']:\n"
            f"    time.sleep({logs_delay_seconds!r})\n"
            f"    sys.stdout.buffer.write({provider_stdout!r})\n"
            f"    sys.stderr.buffer.write({provider_stderr!r})\n"
            f"    raise SystemExit({logs_returncode})\n"
            "if sys.argv[1:3] == ['volume', 'ls']:\n"
            "    print('provider-postgresql-data')\n"
            "if sys.argv[1:3] == ['volume', 'inspect']:\n"
            "    print(json.dumps({\n"
            "        'Name': 'provider-postgresql-data',\n"
            "        'Driver': 'local',\n"
            "        'Scope': 'local',\n"
            "        'Options': None,\n"
            f"        'Labels': {{'io.worldstream.packaged-parity.owner': {volume_owner!r}}},\n"
            "        'Mountpoint': '/var/lib/docker/volumes/provider-postgresql-data/_data',\n"
            "    }))\n",
            encoding="utf-8",
        )
        docker.chmod(0o700)
        return docker, trace

    @staticmethod
    def _provider(module, root: Path, docker: Path):
        return module.Provider(
            root=root,
            repository=root,
            docker=str(docker),
            psql="unused-psql",
            worldstreamctl=root / "worldstreamctl",
            admin_password="a" * 48,
            runtime_password="b" * 48,
            network="provider-network",
            postgres_name="provider-postgresql",
            pooler_name="provider-pgbouncer",
            postgres_volume="provider-postgresql-data",
            volume_ownership_id="owner-123",
            network_started=True,
            postgres_started=True,
            pooler_started=True,
            volume_started=True,
            volume_creation_attempted=True,
        )

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
            self._record_required_privacy_channels(module, capture)
            result = capture.scan()

        self.assertEqual(result["schema"], "worldstream/secret-absence-matrix/v1")
        self.assertEqual(result["status"], "pass")
        self.assertFalse(result["secrets_emitted"])
        self.assertEqual(
            result["encodings_scanned"], ["base64", "base64url", "hex", "raw"]
        )
        self.assertEqual(
            len(result["channels"]), len(module.REQUIRED_PRIVACY_CHANNEL_CLASSES)
        )
        self.assertTrue(
            all(row["sha256"].startswith("sha256:") for row in result["channels"])
        )
        inventory = result["channel_class_inventory"]
        self.assertEqual(inventory["schema"], module.PRIVACY_CHANNEL_CLASS_SCHEMA)
        self.assertEqual(
            inventory["required"], list(module.REQUIRED_PRIVACY_CHANNEL_CLASSES)
        )
        self.assertEqual(
            {row["class"] for row in inventory["classes"]},
            set(module.REQUIRED_PRIVACY_CHANNEL_CLASSES),
        )

    def test_privacy_capture_rejects_missing_required_channel_class(self) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(prefix="worldstream-privacy-class-") as name:
            root = Path(name)
            capture = self._privacy_capture(module, root)
            for channel_class in module.REQUIRED_PRIVACY_CHANNEL_CLASSES[:-1]:
                capture._record_bytes(
                    channel_class.replace(".", "-"),
                    b"clean\n",
                    channel_class=channel_class,
                )
            with self.assertRaisesRegex(
                module.LaneFailure, "privacy_channel_class_incomplete"
            ):
                capture.scan()

    def test_privacy_capture_rejects_each_encoding_in_every_child_channel(self) -> None:
        module = _module()
        sentinel = b"runtime-password-" + b"9" * 48
        encoded = {
            "raw": sentinel,
            "hex": sentinel.hex().encode("ascii"),
            "base64": base64.b64encode(sentinel),
            "base64url": base64.urlsafe_b64encode(sentinel).rstrip(b"="),
        }
        for channel_class in module.REQUIRED_PRIVACY_CHANNEL_CLASSES:
            for encoding, injected in encoded.items():
                with (
                    self.subTest(channel_class=channel_class, encoding=encoding),
                    tempfile.TemporaryDirectory(
                        prefix="worldstream-privacy-negative-"
                    ) as name,
                ):
                    root = Path(name)
                    capture = self._privacy_capture(module, root)
                    capture.register_sentinel("injected-password", sentinel)
                    self._record_required_privacy_channels(
                        module,
                        capture,
                        injected_class=channel_class,
                        injected=injected,
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

    def test_provider_cleanup_captures_both_containers_before_removal(self) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(
            prefix="worldstream-provider-capture-"
        ) as name:
            root = Path(name)
            docker, trace = self._fake_docker(root)
            capture = self._privacy_capture(module, root)
            provider = self._provider(module, root, docker)
            module.ACTIVE_PRIVACY_CAPTURE = capture
            try:
                cleanup = provider.cleanup()
            finally:
                module.ACTIVE_PRIVACY_CAPTURE = None

            self.assertEqual(cleanup, "pass")
            self.assertEqual(
                trace.read_text(encoding="utf-8").splitlines(),
                [
                    "logs --timestamps provider-pgbouncer",
                    "logs --timestamps provider-postgresql",
                    "rm -f provider-pgbouncer",
                    "rm -f provider-postgresql",
                    "volume ls --quiet --filter name=^provider-postgresql-data$",
                    "volume inspect provider-postgresql-data --format {{json .}}",
                    "volume rm provider-postgresql-data",
                    "network rm provider-network",
                ],
            )
            self.assertTrue(
                {
                    "provider.pgbouncer.stdout",
                    "provider.pgbouncer.stderr",
                    "provider.postgresql.stdout",
                    "provider.postgresql.stderr",
                }.issubset(set(capture.channel_classes.values()))
            )
            observed_classes = set(capture.channel_classes.values())
            for channel_class in module.REQUIRED_PRIVACY_CHANNEL_CLASSES:
                if channel_class not in observed_classes:
                    capture._record_bytes(
                        channel_class.replace(".", "-"),
                        b"clean\n",
                        channel_class=channel_class,
                    )
            scan = capture.scan()
            scanned_names = {row["channel"] for row in scan["channels"]}
            provider_inventory = [
                row
                for row in scan["channel_class_inventory"]["classes"]
                if row["class"].startswith("provider.")
            ]
            self.assertEqual(len(provider_inventory), 4)
            self.assertTrue(
                all(
                    set(row["channels"]).issubset(scanned_names)
                    for row in provider_inventory
                )
            )
            with self.assertRaisesRegex(
                module.LaneFailure, "provider_capture_after_removal"
            ):
                provider._capture_container_output(provider.postgres_name, "postgresql")

    def test_provider_cleanup_fails_closed_on_incomplete_capture(self) -> None:
        scenarios = (
            (
                "unavailable",
                {"logs_returncode": 9},
                {},
                "provider_capture_unavailable",
            ),
            (
                "oversized",
                {"provider_stdout": b"x" * 17},
                {"maximum": 16},
                "provider_capture_oversized",
            ),
            (
                "truncated",
                {"logs_delay_seconds": 0.25},
                {"timeout": 0.01},
                "provider_capture_truncated",
            ),
        )
        for label, docker_options, bounds, reason_code in scenarios:
            with (
                self.subTest(label=label),
                tempfile.TemporaryDirectory(
                    prefix="worldstream-provider-capture-failure-"
                ) as name,
            ):
                module = _module()
                root = Path(name)
                docker, trace = self._fake_docker(root, **docker_options)
                if "maximum" in bounds:
                    module.MAX_PROVIDER_CHANNEL_BYTES = bounds["maximum"]
                if "timeout" in bounds:
                    module.PROVIDER_CAPTURE_TIMEOUT_SECONDS = bounds["timeout"]
                capture = module.PrivacyCapture(root)
                provider = self._provider(module, root, docker)
                provider.pooler_started = False
                provider.network_started = False
                module.ACTIVE_PRIVACY_CAPTURE = capture
                try:
                    with self.assertRaisesRegex(module.LaneFailure, reason_code):
                        provider._capture_container_output(
                            provider.postgres_name, "postgresql"
                        )
                    cleanup = provider.cleanup()
                finally:
                    module.ACTIVE_PRIVACY_CAPTURE = None

                self.assertEqual(cleanup, "failed")
                self.assertEqual(
                    trace.read_text(encoding="utf-8").splitlines()[-1],
                    "volume rm provider-postgresql-data",
                )

    def test_reference_environment_observes_workload_and_database_storage(self) -> None:
        module = _module()
        workload = Path("/srv/worldstream/workload")
        database = Path("/var/lib/docker/volumes/provider/_data")
        platform = {
            "system": "Linux",
            "distribution": "Ubuntu",
            "distribution_version": "24.04",
            "machine": "x86_64",
        }
        hardware = {
            "cpu_model": "fixture CPU",
            "logical_cpu_count": 4,
            "memory_bytes": 8 * 1024 * 1024 * 1024,
        }
        filesystem = {
            "type": "ext4",
            "mount_options": ["relatime", "rw"],
            "storage_class": "local_ssd_or_nvme",
        }
        observed: list[Path] = []

        def observe(path: Path):
            observed.append(path)
            return {
                "platform": platform,
                "hardware": hardware,
                "filesystem": filesystem,
            }

        provider_storage = {
            "driver": "local",
            "scope": "local",
            "driver_options": {},
            "container_destination": module.POSTGRES_DATA_DESTINATION,
            "container_mount_type": "volume",
            "read_write": True,
            "source_matches_volume_mountpoint": True,
            "run_unique_ownership_label_verified": True,
        }
        with (
            mock.patch.object(module.REFERENCE_HOST, "observe", side_effect=observe),
            mock.patch.object(
                module,
                "_mount_identity",
                side_effect=[
                    {"mount_id": 31, "device_id": "259:1"},
                    {"mount_id": 42, "device_id": "259:2"},
                ],
            ),
        ):
            environment = module._reference_environment(
                workload, database, provider_storage
            )

        self.assertEqual(observed, [workload, database])
        self.assertEqual(environment["filesystem"], filesystem)
        bindings = environment["storage_bindings"]
        self.assertEqual(bindings["schema"], module.STORAGE_BINDING_SCHEMA)
        self.assertEqual(bindings["profile"], module.FROZEN_STORAGE_PROFILE)
        self.assertEqual(bindings["layout"], "split_host_mounts")
        self.assertFalse(bindings["raw_paths_retained"])
        self.assertEqual(bindings["workload"]["filesystem"], filesystem)
        self.assertEqual(bindings["postgresql_database"]["filesystem"], filesystem)
        self.assertEqual(
            bindings["postgresql_database"]["docker_volume"], provider_storage
        )
        self.assertNotIn(str(workload), json.dumps(environment))
        self.assertNotIn(str(database), json.dumps(environment))

    def test_reference_database_storage_must_be_frozen_local_ext4(self) -> None:
        module = _module()
        common = {
            "platform": {"system": "Linux"},
            "hardware": {"cpu_model": "fixture"},
        }
        workload = {
            **common,
            "filesystem": {
                "type": "ext4",
                "mount_options": ["rw"],
                "storage_class": "local_ssd_or_nvme",
            },
        }
        database = {
            **common,
            "filesystem": {
                "type": "xfs",
                "mount_options": ["rw"],
                "storage_class": "local_ssd_or_nvme",
            },
        }
        with (
            mock.patch.object(
                module.REFERENCE_HOST,
                "observe",
                side_effect=[workload, database],
            ),
            self.assertRaisesRegex(
                module.LaneFailure,
                "reference_database_storage_not_frozen_local_ext4",
            ),
        ):
            module._reference_environment(Path("/workload"), Path("/database"), {})

    def test_provider_database_storage_requires_exact_local_volume_mount(self) -> None:
        module = _module()
        volume = {
            "Name": "provider-data",
            "Driver": "local",
            "Scope": "local",
            "Options": None,
            "Labels": {
                module.POSTGRES_VOLUME_OWNERSHIP_LABEL: "owner-123",
            },
            "Mountpoint": "/var/lib/docker/volumes/provider-data/_data",
        }
        mount = {
            "Type": "volume",
            "Name": "provider-data",
            "Driver": "local",
            "Source": volume["Mountpoint"],
            "Destination": module.POSTGRES_DATA_DESTINATION,
            "RW": True,
        }
        path, disclosure = module._provider_database_storage_binding(
            volume, {"Mounts": [mount]}, "provider-data", "owner-123"
        )
        self.assertEqual(path, Path(volume["Mountpoint"]))
        self.assertEqual(disclosure["driver"], "local")
        self.assertTrue(disclosure["source_matches_volume_mountpoint"])

        for field, value in (
            ("Type", "bind"),
            ("Source", "/different/source"),
            ("Destination", "/different/destination"),
            ("RW", False),
        ):
            with self.subTest(field=field):
                tampered = dict(mount)
                tampered[field] = value
                with self.assertRaises(module.LaneFailure):
                    module._provider_database_storage_binding(
                        volume,
                        {"Mounts": [tampered]},
                        "provider-data",
                        "owner-123",
                    )

        network_volume = {**volume, "Options": {"type": "nfs"}}
        with self.assertRaisesRegex(
            module.LaneFailure, "postgres_database_volume_not_owned_local"
        ):
            module._provider_database_storage_binding(
                network_volume,
                {"Mounts": [mount]},
                "provider-data",
                "owner-123",
            )

    def test_provider_database_storage_binding_uses_run_owner(self) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(prefix="worldstream-volume-binding-") as name:
            root = Path(name)
            docker, _trace = self._fake_docker(root)
            provider = self._provider(module, root, docker)
            provider.postgres_started = True
            provider.volume_started = True
            volume = {
                "Name": provider.postgres_volume,
                "Driver": "local",
                "Scope": "local",
                "Options": None,
                "Labels": {
                    module.POSTGRES_VOLUME_OWNERSHIP_LABEL: provider.volume_ownership_id,
                },
                "Mountpoint": (
                    f"/var/lib/docker/volumes/{provider.postgres_volume}/_data"
                ),
            }
            container = {
                "Mounts": [
                    {
                        "Type": "volume",
                        "Name": provider.postgres_volume,
                        "Driver": "local",
                        "Source": volume["Mountpoint"],
                        "Destination": module.POSTGRES_DATA_DESTINATION,
                        "RW": True,
                    }
                ]
            }
            with mock.patch.object(
                module,
                "_bounded_docker_object",
                side_effect=[volume, container],
            ):
                path, disclosure = provider.database_storage_binding()

            self.assertEqual(path, Path(volume["Mountpoint"]))
            self.assertTrue(disclosure["run_unique_ownership_label_verified"])

    def test_provider_volume_collision_is_never_claimed_or_removed(self) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(
            prefix="worldstream-volume-collision-"
        ) as name:
            root = Path(name)
            docker, trace = self._fake_docker(root)
            provider = self._provider(module, root, docker)
            provider.volume_started = False
            provider.volume_creation_attempted = False
            with (
                mock.patch.object(
                    module,
                    "_bounded_docker_text",
                    return_value=provider.postgres_volume + "\n",
                ),
                self.assertRaisesRegex(
                    module.LaneFailure, "postgres_database_volume_name_collision"
                ),
            ):
                provider._create_database_volume()
            self.assertFalse(provider.volume_started)
            self.assertFalse(trace.exists())
            provider.network_started = False
            provider.postgres_started = False
            provider.pooler_started = False
            self.assertEqual(provider.cleanup(), "pass")
            self.assertFalse(trace.exists())

    def test_provider_foreign_volume_label_is_never_claimed_or_removed(self) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(prefix="worldstream-volume-foreign-") as name:
            root = Path(name)
            docker, trace = self._fake_docker(root, volume_owner="foreign-owner")
            provider = self._provider(module, root, docker)
            provider.volume_started = False
            provider.volume_creation_attempted = False
            foreign = {
                "Name": provider.postgres_volume,
                "Driver": "local",
                "Scope": "local",
                "Options": None,
                "Labels": {module.POSTGRES_VOLUME_OWNERSHIP_LABEL: "foreign-owner"},
                "Mountpoint": (
                    f"/var/lib/docker/volumes/{provider.postgres_volume}/_data"
                ),
            }
            with (
                mock.patch.object(module, "_bounded_docker_text", return_value=""),
                mock.patch.object(
                    module, "_bounded_docker_object", return_value=foreign
                ),
                self.assertRaisesRegex(
                    module.LaneFailure, "postgres_database_volume_not_owned_local"
                ),
            ):
                provider._create_database_volume()
            self.assertFalse(provider.volume_started)
            self.assertEqual(
                trace.read_text(encoding="utf-8").splitlines(),
                [
                    (
                        "volume create --driver local --label "
                        f"{module.POSTGRES_VOLUME_OWNERSHIP_LABEL}=owner-123 "
                        "provider-postgresql-data"
                    )
                ],
            )
            provider.network_started = False
            provider.postgres_started = False
            provider.pooler_started = False
            self.assertEqual(provider.cleanup(), "failed")
            cleanup_trace = trace.read_text(encoding="utf-8")
            self.assertIn(
                "volume inspect provider-postgresql-data --format {{json .}}",
                cleanup_trace,
            )
            self.assertNotIn("volume rm", cleanup_trace)

    def test_post_create_inspect_failure_still_cleans_proven_owned_volume(self) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(prefix="worldstream-volume-recovery-") as name:
            root = Path(name)
            docker, trace = self._fake_docker(root)
            provider = self._provider(module, root, docker)
            provider.volume_started = False
            provider.volume_creation_attempted = False
            with (
                mock.patch.object(module, "_bounded_docker_text", return_value=""),
                mock.patch.object(
                    module,
                    "_bounded_docker_object",
                    side_effect=module.LaneFailure(
                        "postgres_database_volume_inspect_failed"
                    ),
                ),
                self.assertRaisesRegex(
                    module.LaneFailure,
                    "postgres_database_volume_inspect_failed",
                ),
            ):
                provider._create_database_volume()
            self.assertTrue(provider.volume_creation_attempted)
            self.assertFalse(provider.volume_started)

            provider.network_started = False
            provider.postgres_started = False
            provider.pooler_started = False
            self.assertEqual(provider.cleanup(), "pass")
            cleanup_trace = trace.read_text(encoding="utf-8")
            self.assertIn(
                "volume inspect provider-postgresql-data --format {{json .}}",
                cleanup_trace,
            )
            self.assertIn("volume rm provider-postgresql-data", cleanup_trace)

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
            self.assertTrue((root / "extracted/ui/index.html").is_file())
            self.assertTrue(
                (
                    root / "extracted/sdk/python/src/worldstream_sdk/__init__.py"
                ).is_file()
            )
            self.assertTrue(
                (
                    root / "extracted/examples/heist/wave10_live/run_browser_story.py"
                ).is_file()
            )
            self.assertEqual(
                set(binding["runtime_assets"]),
                {"ui", "sdk_python_source", "heist_reference_clients"},
            )

    def test_archive_is_hashed_and_parsed_from_one_private_stable_descriptor(
        self,
    ) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(prefix="worldstream-package-fd-") as name:
            root = Path(name)
            archive, report = _package_fixture(root)
            real_open = module.tarfile.open
            observed_file_objects = []

            def checked_open(*args, **kwargs):
                observed_file_objects.append(kwargs.get("fileobj"))
                self.assertIsNotNone(kwargs.get("fileobj"))
                self.assertEqual(kwargs.get("mode"), "r:gz")
                return real_open(*args, **kwargs)

            with mock.patch.object(module.tarfile, "open", side_effect=checked_open):
                module._bind_package(archive, report, root / "extracted")

            self.assertEqual(len(observed_file_objects), 1)
            self.assertTrue(observed_file_objects[0].closed)
            self.assertFalse(
                any(root.glob(".worldstream-verified-package-*")),
                "the private archive copy must be removed after extraction",
            )

    def test_archive_path_inode_replacement_during_copy_fails_closed(self) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(prefix="worldstream-package-race-") as name:
            root = Path(name)
            archive, report = _package_fixture(root)
            replacement = root / "replacement.tar.gz"
            replacement.write_bytes(archive.read_bytes())
            real_lstat = module.pathlib.Path.lstat
            archive_lstat_calls = 0

            def replacing_lstat(path):
                nonlocal archive_lstat_calls
                if path == archive:
                    archive_lstat_calls += 1
                    if archive_lstat_calls == 2:
                        replacement.replace(archive)
                return real_lstat(path)

            with (
                mock.patch.object(
                    module.pathlib.Path,
                    "lstat",
                    autospec=True,
                    side_effect=replacing_lstat,
                ),
                self.assertRaisesRegex(
                    module.LaneFailure,
                    "package_archive_changed_during_verification",
                ),
            ):
                module._bind_package(archive, report, root / "extracted")

    def test_oversized_package_report_is_rejected_before_reading(self) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(prefix="worldstream-package-size-") as name:
            root = Path(name)
            archive, report = _package_fixture(root)
            with report.open("wb") as output:
                output.truncate(64 * 1024 * 1024 + 1)
            with self.assertRaisesRegex(module.LaneFailure, "package_report_invalid"):
                module._bind_package(archive, report, root / "extracted")

    def test_oversized_cell_report_is_rejected_before_reading(self) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(prefix="worldstream-cell-size-") as name:
            report = Path(name) / "cell-report.json"
            with report.open("wb") as output:
                output.truncate(module.MAX_CELL_REPORT_BYTES + 1)
            with self.assertRaisesRegex(module.LaneFailure, "cell_report_invalid"):
                module._load_report(report)

    def test_browser_story_is_exactly_bound_and_adversarial_drift_is_rejected(
        self,
    ) -> None:
        module = _module()
        with tempfile.TemporaryDirectory(prefix="worldstream-browser-bind-") as name:
            root = Path(name)
            archive, package_report = _package_fixture(root)
            _daemon, _ctl, binding = module._bind_package(
                archive, package_report, root / "extracted"
            )
            valid = _valid_browser_story(module, binding)
            module._validate_browser_story(
                valid, binding, module.PINNED_BROWSER_IDENTITY
            )
            mutations = {
                "browser": ("browser", "sha256", "sha256:" + "0" * 64),
                "ui": ("runtime", "ui", "tree_sha256", "sha256:" + "0" * 64),
                "privacy": (
                    "privacy",
                    "private_canary_absent",
                    False,
                ),
                "stale": ("checks", "stale_head_rejected", False),
            }
            for label, mutation in mutations.items():
                with self.subTest(label=label):
                    changed = json.loads(json.dumps(valid))
                    target = changed
                    for key in mutation[:-2]:
                        target = target[key]
                    target[mutation[-2]] = mutation[-1]
                    with self.assertRaises(module.LaneFailure):
                        module._validate_browser_story(
                            changed, binding, module.PINNED_BROWSER_IDENTITY
                        )

            for label, change in {
                "unlisted-tool": lambda item: item["tools"].__setitem__(
                    "unexpected", {}
                ),
                "python-field": lambda item: item["tools"]["python"].__setitem__(
                    "executable", "/usr/bin/python"
                ),
                "python-version": lambda item: item["tools"]["python"].__setitem__(
                    "version", "3.14.6"
                ),
            }.items():
                with self.subTest(label=label):
                    changed = json.loads(json.dumps(valid))
                    change(changed)
                    with self.assertRaisesRegex(
                        module.LaneFailure, "package_browser_tool_identity_invalid"
                    ):
                        module._validate_browser_story(
                            changed, binding, module.PINNED_BROWSER_IDENTITY
                        )

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

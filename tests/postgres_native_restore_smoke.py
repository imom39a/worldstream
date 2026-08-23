"""Boundary tests for the PostgreSQL-native restore runner."""

from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RUNNER = ROOT / "scripts" / "postgres-native-restore-smoke.sh"


def write_fake_committed_native_cargo(path: Path) -> None:
    """Write a fake cargo that publishes an identity-bound committed receipt."""
    path.write_text(
        f"#!{sys.executable}\n"
        "import importlib.util, json, os, sys\n"
        "from pathlib import Path\n"
        f"root = Path({str(ROOT)!r})\n"
        "spec = importlib.util.spec_from_file_location(\n"
        "    'worldstream_fake_restore_validator',\n"
        "    root / 'scripts' / 'release-platform-diagnostic.py',\n"
        ")\n"
        "if spec is None or spec.loader is None: raise RuntimeError('validator unavailable')\n"
        "validator = importlib.util.module_from_spec(spec)\n"
        "sys.modules[spec.name] = validator\n"
        "spec.loader.exec_module(validator)\n"
        "args = sys.argv[1:]\n"
        "def value(flag):\n"
        "    return args[args.index(flag) + 1]\n"
        "dump = Path(value('--dump'))\n"
        "report = Path(value('--report'))\n"
        "recovery = report.parent / "
        "'.worldstream_native_recovery_0123456789abcdef0123456789abcdef.json'\n"
        "dump_bytes = b'exact fake native dump'\n"
        "dump_digest = validator.BLAKE3(dump_bytes).hex()\n"
        "domain_digest = validator.BLAKE3(b'fake durable domains').hex()\n"
        "source_provider = {\n"
        "    'system_identifier': '7400000000000000001',\n"
        "    'database_oid': '16384',\n"
        "    'database_name': 'worldstream_source',\n"
        "}\n"
        "target_provider = {\n"
        "    'system_identifier': '7400000000000000002',\n"
        "    'database_oid': '16385',\n"
        "    'database_name': 'worldstream_target',\n"
        "}\n"
        "inventory = [\n"
        "    {\n"
        "        'domain': domain,\n"
        "        'source_row_count': 1 if domain == 'authority_state' else 0,\n"
        "        'restored_row_count': 1 if domain == 'authority_state' else 0,\n"
        "        'source_digest': domain_digest,\n"
        "        'restored_digest': domain_digest,\n"
        "    }\n"
        "    for domain in validator.NATIVE_RESTORE_DURABLE_DOMAINS\n"
        "]\n"
        "report_value = {\n"
        "    'schema': 'worldstream/native-postgres-restore-evidence/v2',\n"
        "    'status': 'ready',\n"
        "    'reason': 'postgres_native_restore_verified_by_unified_verifier',\n"
        "    'release_evidence': False,\n"
        "    'native_dump_restore': 'pass',\n"
        "    'backup_id': 'postgres-native-' + dump_digest,\n"
        "    'native_point_digest': validator.postgres_native_point_digest(dump_digest),\n"
        "    'native_dump_digest': dump_digest,\n"
        "    'native_dump_size_bytes': len(dump_bytes),\n"
        "    'source_provider_identity': source_provider,\n"
        "    'target_provider_identity': target_provider,\n"
        "    'verifier': {\n"
        "        'readiness': 'Ready',\n"
        "        'rooms': {'01ARZ3NDEKTSV4RRFFQ69G5FAV': 'Verified'},\n"
        "        'diagnostics': [],\n"
        "    },\n"
        "    'source_unchanged': True,\n"
        "    'exact_restored_row_set': True,\n"
        "    'snapshots_disposable': True,\n"
        "    'target_isolated': True,\n"
        "    'target_published': False,\n"
        "    'cleanup_required': True,\n"
        "    'native_witness_minted': True,\n"
        "    'secrets_emitted': False,\n"
        "    'source_version_num': 170011,\n"
        "    'restored_version_num': 170011,\n"
        "    'semantic_receipts_verified': True,\n"
        "    'activation_intents_verified': True,\n"
        "    'activation_operation_receipts_verified': True,\n"
        "    'activation_request_evidence': 'stored_canonical_hash_only_verified',\n"
        "    'authority_state_verified': True,\n"
        "    'durable_domains_verified': True,\n"
        "    'verifier_scope': {\n"
        "        'profile': 'full_deployment_all_durable_domains',\n"
        "        'source_pack_identity_count': 2,\n"
        "        'restored_pack_identity_count': 2,\n"
        "        'source_resource_identity_count': 1,\n"
        "        'restored_resource_identity_count': 1,\n"
        "        'source_fired_timer_count': 1,\n"
        "        'restored_fired_timer_count': 1,\n"
        "        'general_deployment_support_verified': True,\n"
        "    },\n"
        "    'source_durable_domains_digest': domain_digest,\n"
        "    'restored_durable_domains_digest': domain_digest,\n"
        "    'durable_domain_inventory': inventory,\n"
        "    'restored_snapshot_count_before': 1,\n"
        "    'restored_snapshot_count_after': 0,\n"
        "}\n"
        "mode = os.environ.get('WORLDSTREAM_FAKE_RECEIPT_MODE', '')\n"
        "if mode == 'non_ready_report':\n"
        "    report_value['status'] = 'incomplete'\n"
        "    report_value['reason'] = 'forged_non_ready_report'\n"
        "report_bytes = (json.dumps(report_value, sort_keys=True, "
        "separators=(',', ':')) + '\\n').encode()\n"
        "dump.write_bytes(dump_bytes)\n"
        "report.write_bytes(report_bytes)\n"
        "recovery.write_text('acknowledged\\n', encoding='utf-8')\n"
        "for artifact in (dump, report, recovery): artifact.chmod(0o600)\n"
        "if mode == 'unknown_placeholder':\n"
        "    unknown = report.parent / '.worldstream_unknown_cleanup_probe'\n"
        "    unknown.write_bytes(b'')\n"
        "    unknown.chmod(0o600)\n"
        "def identity(artifact):\n"
        "    value = os.stat(artifact, follow_symlinks=False)\n"
        "    return {'storage_id': f'{value.st_dev:016x}', "
        "'file_id': f'{value.st_ino:032x}'}\n"
        "receipt = {\n"
        "    'schema': 'worldstream/postgres-native-restore-receipt/v1',\n"
        "    'status': 'committed',\n"
        "    'report_digest': 'blake3:' + validator.BLAKE3(report_bytes).hex(),\n"
        "    'report_size_bytes': len(report_bytes),\n"
        "    'native_dump_digest': dump_digest,\n"
        "    'native_dump_size_bytes': len(dump_bytes),\n"
        "    'native_dump_identity': identity(dump),\n"
        "    'report_identity': identity(report),\n"
        "    'recovery_record_identity': identity(recovery),\n"
        "    'recovery_record_name': recovery.name,\n"
        "    'source_provider_identity': source_provider,\n"
        "    'target_provider_identity': target_provider,\n"
        "    'secrets_emitted': False,\n"
        "}\n"
        "if mode == 'missing_receipt_field': receipt.pop('report_digest')\n"
        "if mode == 'extra_receipt_field': receipt['unexpected'] = True\n"
        "if mode == 'report_digest_mismatch': receipt['report_digest'] = 'blake3:' + '0' * 64\n"
        "if mode == 'dump_digest_mismatch': receipt['native_dump_digest'] = '0' * 64\n"
        "print(json.dumps(receipt, sort_keys=True, separators=(',', ':')))\n",
        encoding="utf-8",
    )
    path.chmod(0o700)


def fake_external_restore_environment(temporary: Path) -> dict[str, str]:
    passfile = temporary / "operator.pgpass"
    passfile.write_text(
        "source:5432:worldstream:postgres:TOP_SECRET\n", encoding="utf-8"
    )
    passfile.chmod(0o600)
    fake_cargo = temporary / "fake-cargo"
    write_fake_committed_native_cargo(fake_cargo)
    fake_tool = temporary / "fake-tool"
    fake_tool.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    fake_tool.chmod(0o700)
    environment = os.environ.copy()
    environment.update(
        {
            "WORLDSTREAM_NATIVE_PG_SOURCE_HOST": "source-host",
            "WORLDSTREAM_NATIVE_PG_SOURCE_PORT": "5432",
            "WORLDSTREAM_NATIVE_PG_TARGET_HOST": "target-host",
            "WORLDSTREAM_NATIVE_PG_TARGET_PORT": "5433",
            "WORLDSTREAM_NATIVE_PG_PGPASSFILE": str(passfile),
            "WORLDSTREAM_NATIVE_PG_CARGO": str(fake_cargo),
            "WORLDSTREAM_NATIVE_PG_DUMP": str(fake_tool),
            "WORLDSTREAM_NATIVE_PG_RESTORE": str(fake_tool),
            "WORLDSTREAM_NATIVE_PG_PSQL": str(fake_tool),
            "WORLDSTREAM_NATIVE_PG_PYTHON": sys.executable,
        }
    )
    return environment


def run_linux_hosted_wrapper_failure(
    temporary: Path, *, transfer_exit: int, kill_supervisor: bool
) -> tuple[subprocess.CompletedProcess[str], Path, Path]:
    workspace = temporary / "workspace"
    runner_temp = temporary / "runner"
    scripts = workspace / "scripts"
    reports = workspace / "reports"
    scripts.mkdir(parents=True)
    reports.mkdir()
    runner_temp.mkdir()

    transfer = scripts / "postgres-transfer-smoke.sh"
    transfer.write_text(
        "#!/bin/sh\n"
        + (
            ": > reports/native-linux-transfer-seed.json\n"
            if transfer_exit == 0
            else ""
        )
        + f"exit {transfer_exit}\n",
        encoding="utf-8",
    )
    transfer.chmod(0o700)
    (workspace / "compatibility.toml").write_text(
        '[contracts]\nproduct = "0.0.0-test"\n', encoding="utf-8"
    )
    archive = workspace / "dist/linux/worldstream-0.0.0-test-linux-x86_64.tar.gz"
    archive.parent.mkdir(parents=True)
    archive.write_bytes(b"test archive")
    supervisor = scripts / "postgres-native-restore-hosted-report.py"
    supervisor.write_text(
        (
            "import os,signal\nos.kill(os.getpid(), signal.SIGKILL)\n"
            if kill_supervisor
            else "raise SystemExit(0)\n"
        ),
        encoding="utf-8",
    )

    cleanup_log = temporary / "cleanup.sql"
    commands = temporary / "commands"
    commands.mkdir()
    fake_stat = commands / "stat"
    fake_stat.write_text(
        f"#!{sys.executable}\n"
        "import os,stat,sys\n"
        "print(f'{stat.S_IMODE(os.lstat(sys.argv[-1]).st_mode):o}')\n",
        encoding="utf-8",
    )
    fake_stat.chmod(0o700)
    fake_psql = temporary / "psql"
    fake_psql.write_text(
        f"#!{sys.executable}\n"
        "import os,sys\n"
        "args = sys.argv[1:]\n"
        "if '--file=-' in args or ('--file' in args and args[args.index('--file') + 1] == '-'):\n"
        "    sql = sys.stdin.read()\n"
        "    with open(os.environ['WORLDSTREAM_TEST_CLEANUP_LOG'], 'a', encoding='utf-8') as output:\n"
        "        output.write(sql)\n"
        "    print('123456789\\t0\\t0\\t0\\t0\\t0')\n"
        "elif '--command' in args:\n"
        "    sql = args[args.index('--command') + 1]\n"
        "    if 'system_identifier::text' in sql:\n"
        "        print('123456789')\n"
        "    elif \"datname='worldstream_native_source'\" in sql:\n"
        "        print('16384')\n"
        "    elif \"datname='worldstream_native_restore'\" in sql:\n"
        "        print('16385')\n"
        "    elif \"datname='worldstream_native_abort'\" in sql:\n"
        "        print('16386')\n"
        "    elif \"rolname='worldstream_native_linux_runtime'\" in sql:\n"
        "        print('16387')\n",
        encoding="utf-8",
    )
    fake_psql.chmod(0o700)

    executable = temporary / "exact-tool"
    executable.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    executable.chmod(0o700)
    password = temporary / "bootstrap-password"
    password.write_text("a" * 48 + "\n", encoding="utf-8")
    password.chmod(0o600)
    environment = os.environ.copy()
    environment.update(
        {
            "GITHUB_WORKSPACE": str(workspace),
            "RUNNER_TEMP": str(runner_temp),
            "WORLDSTREAM_BUILD_REVISION": "1" * 40,
            "WORLDSTREAM_PACKAGED_CTL": str(executable),
            "WORLDSTREAM_POSTGRES_PASSWORD_FILE": str(password),
            "WORLDSTREAM_PG_DUMP": str(executable),
            "WORLDSTREAM_PG_RESTORE": str(executable),
            "WORLDSTREAM_PSQL": str(fake_psql),
            "WORLDSTREAM_TEST_CLEANUP_LOG": str(cleanup_log),
            "PATH": f"{commands}{os.pathsep}{environment['PATH']}",
        }
    )
    result = subprocess.run(
        [str(ROOT / "scripts" / "postgres-native-restore-linux-live.sh")],
        cwd=ROOT,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
    )
    return result, runner_temp, cleanup_log


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

    @unittest.skipIf(os.name == "nt", "POSIX no-clobber publication regression")
    def test_static_evidence_refuses_regular_symlink_and_hardlink_destinations(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-native-pg-evidence-"
        ) as name:
            directory = Path(name)
            victim = directory / "victim"
            sentinel = b"operator evidence sentinel"
            victim.write_bytes(sentinel)
            destinations = {
                "regular": directory / "regular.json",
                "symlink": directory / "symlink.json",
                "hardlink": directory / "hardlink.json",
            }
            destinations["regular"].write_bytes(sentinel)
            destinations["symlink"].symlink_to(victim)
            os.link(victim, destinations["hardlink"])

            for label, destination in destinations.items():
                with self.subTest(label=label):
                    before = victim.read_bytes()
                    environment = os.environ.copy()
                    environment.update(
                        {
                            "WORLDSTREAM_NATIVE_PG_CARGO": str(ROOT / "missing-cargo"),
                            "WORLDSTREAM_NATIVE_PG_PYTHON": sys.executable,
                            "WORLDSTREAM_NATIVE_PG_EVIDENCE_FILE": str(destination),
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
                    self.assertNotEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(victim.read_bytes(), before)
                    if not destination.is_symlink():
                        self.assertEqual(destination.read_bytes(), sentinel)

    @unittest.skipIf(os.name == "nt", "POSIX retained-parent regression")
    def test_evidence_publication_stays_under_admitted_parent_after_substitution(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-native-pg-evidence-parent-"
        ) as name:
            directory = Path(name)
            admitted = directory / "admitted"
            held = directory / "held-admitted"
            victim = directory / "victim"
            admitted.mkdir()
            victim.mkdir()
            marker = victim / "must-survive"
            marker.write_text("sentinel", encoding="utf-8")
            hook = directory / "replace-parent"
            hook.write_text(
                "#!/bin/sh\n"
                "set -eu\n"
                ': "${WORLDSTREAM_EVIDENCE_HELD:?}"\n'
                ': "${WORLDSTREAM_EVIDENCE_VICTIM:?}"\n'
                'mv -- "$1" "$WORLDSTREAM_EVIDENCE_HELD"\n'
                'mv -- "$WORLDSTREAM_EVIDENCE_VICTIM" "$1"\n',
                encoding="utf-8",
            )
            hook.chmod(0o700)
            evidence = admitted / "evidence.json"
            environment = os.environ.copy()
            environment.update(
                {
                    "WORLDSTREAM_NATIVE_PG_CARGO": str(ROOT / "missing-cargo"),
                    "WORLDSTREAM_NATIVE_PG_PYTHON": sys.executable,
                    "WORLDSTREAM_NATIVE_PG_EVIDENCE_FILE": str(evidence),
                    "WORLDSTREAM_NATIVE_PG_TEST_EVIDENCE_PARENT_HOOK": str(hook),
                    "WORLDSTREAM_EVIDENCE_HELD": str(held),
                    "WORLDSTREAM_EVIDENCE_VICTIM": str(victim),
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

            self.assertEqual(result.returncode, 10, result.stderr)
            self.assertEqual(
                (admitted / "must-survive").read_text(encoding="utf-8"),
                "sentinel",
            )
            self.assertFalse(evidence.exists())
            published = held / "evidence.json"
            self.assertEqual(json.loads(published.read_text())["status"], "unavailable")

    @unittest.skipIf(os.name == "nt", "POSIX retained-root regression")
    def test_temp_root_substitution_fails_before_operational_victim_writes(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-native-pg-operational-root-"
        ) as name:
            directory = Path(name)
            victim = directory / "victim"
            victim.mkdir()
            names = ("driver.stderr", "source.dump", "docker.env")
            sentinels = {artifact: (victim / artifact) for artifact in names}
            for artifact in sentinels.values():
                artifact.write_text("sentinel", encoding="utf-8")
            hook = directory / "replace-temp-root"
            trace = directory / "replacement-path"
            hook.write_text(
                "#!/bin/sh\n"
                "set -eu\n"
                ': "${WORLDSTREAM_TEMP_ROOT_VICTIM:?}"\n'
                ': "${WORLDSTREAM_TEMP_ROOT_TRACE:?}"\n'
                'printf "%s" "$1" >"$WORLDSTREAM_TEMP_ROOT_TRACE"\n'
                'mv -- "$1" "$1.admitted"\n'
                'mv -- "$WORLDSTREAM_TEMP_ROOT_VICTIM" "$1"\n',
                encoding="utf-8",
            )
            hook.chmod(0o700)
            environment = os.environ.copy()
            environment.update(
                {
                    "TMPDIR": str(directory),
                    "WORLDSTREAM_NATIVE_PG_TEST_AFTER_TEMP_ROOT_ADMISSION": str(hook),
                    "WORLDSTREAM_TEMP_ROOT_VICTIM": str(victim),
                    "WORLDSTREAM_TEMP_ROOT_TRACE": str(trace),
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

            self.assertEqual(result.returncode, 12, result.stderr)
            replacement = Path(trace.read_text(encoding="utf-8"))
            for artifact in (replacement / name for name in names):
                self.assertEqual(artifact.read_text(encoding="utf-8"), "sentinel")

    @unittest.skipIf(os.name == "nt", "POSIX Docker mount substitution regression")
    def test_docker_mount_source_substitution_fails_without_victim_mount(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-native-pg-mount-root-"
        ) as name:
            directory = Path(name)
            victim = directory / "victim"
            victim.mkdir()
            marker = victim / "must-survive"
            marker.write_bytes(b"docker mount victim sentinel")
            trace = directory / "substituted-path"
            docker_log = directory / "docker.log"
            commands = directory / "commands"
            commands.mkdir()
            fake_uname = commands / "uname"
            fake_uname.write_text("#!/bin/sh\nprintf '%s\\n' Linux\n", encoding="utf-8")
            fake_uname.chmod(0o700)
            fake_docker = commands / "docker"
            fake_docker.write_text(
                "#!/bin/sh\n"
                "set -eu\n"
                ': "${WORLDSTREAM_DOCKER_LOG:?}"\n'
                'printf "%s\\n" "$*" >>"$WORLDSTREAM_DOCKER_LOG"\n'
                'if [ "${1-}" = info ]; then exit 0; fi\n'
                "printf '%064d\\n' 1\n",
                encoding="utf-8",
            )
            fake_docker.chmod(0o700)
            hook = directory / "replace-root-before-docker"
            hook.write_text(
                "#!/bin/sh\n"
                "set -eu\n"
                ': "${WORLDSTREAM_DOCKER_VICTIM:?}"\n'
                ': "${WORLDSTREAM_DOCKER_TRACE:?}"\n'
                'mv -- "$1" "$1.admitted"\n'
                'mv -- "$WORLDSTREAM_DOCKER_VICTIM" "$1"\n'
                'printf "%s" "$1" >"$WORLDSTREAM_DOCKER_TRACE"\n',
                encoding="utf-8",
            )
            hook.chmod(0o700)
            environment = os.environ.copy()
            environment.update(
                {
                    "PATH": f"{commands}:{environment['PATH']}",
                    "TMPDIR": str(directory),
                    "WORLDSTREAM_NATIVE_PG_CARGO": "/usr/bin/true",
                    "WORLDSTREAM_NATIVE_PG_DOCKER": str(fake_docker),
                    "WORLDSTREAM_NATIVE_PG_TEST_BEFORE_DOCKER_MOUNT": str(hook),
                    "WORLDSTREAM_DOCKER_VICTIM": str(victim),
                    "WORLDSTREAM_DOCKER_TRACE": str(trace),
                    "WORLDSTREAM_DOCKER_LOG": str(docker_log),
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

            self.assertEqual(result.returncode, 13, result.stderr)
            substituted = Path(trace.read_text(encoding="utf-8"))
            self.assertEqual(
                (substituted / "must-survive").read_bytes(),
                b"docker mount victim sentinel",
            )
            docker_commands = docker_log.read_text(encoding="utf-8")
            self.assertIn("info", docker_commands)
            self.assertNotIn("run --detach", docker_commands)
            self.assertEqual(
                json.loads(result.stdout.splitlines()[-1])["reason"],
                "native_docker_mount_source_identity_failed",
            )

    def test_pinned_uv_fallback_bypasses_a_broken_python_shim(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-native-pg-test-") as name:
            directory = Path(name)
            broken_python = directory / "python3"
            broken_python.write_text("#!/bin/sh\nexit 127\n", encoding="utf-8")
            broken_python.chmod(broken_python.stat().st_mode | stat.S_IXUSR)
            fake_uv = directory / "uv"
            fake_uv.write_text(
                "#!/bin/sh\n" + f"printf '%s\\n' {json.dumps(sys.executable)}\n",
                encoding="utf-8",
            )
            fake_uv.chmod(fake_uv.stat().st_mode | stat.S_IXUSR)
            environment = os.environ.copy()
            environment.update(
                {
                    "PATH": f"{directory}:{environment['PATH']}",
                    "WORLDSTREAM_NATIVE_PG_CARGO": str(ROOT / "missing-cargo"),
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
        self.assertEqual(evidence["reason"], "cargo_bin_unavailable")
        self.assertNotIn("pyenv", result.stdout + result.stderr)

    def test_source_seed_uses_a_distinct_provider_abort_database(self) -> None:
        script = RUNNER.read_text(encoding="utf-8")
        self.assertIn(
            'transfer_abort_db="worldstream_transfer_abort_probe"',
            script,
        )
        self.assertIn(
            'WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN_FILE="$artifact_root/'
            'transfer-abort-admin.dsn"',
            script,
        )
        self.assertIn("transfer-abort-admin.dsn", script)
        self.assertNotIn("WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN=", script)
        self.assertIn("native_source_abort_database_setup_failed", script)

    def test_native_driver_uses_explicit_tls_and_sanitized_provider_environment(
        self,
    ) -> None:
        script = RUNNER.read_text(encoding="utf-8")
        rust = (
            ROOT / "crates" / "worldstream-postgres" / "src" / "native_restore.rs"
        ).read_text(encoding="utf-8")

        self.assertIn(
            'source_tls_mode="${WORLDSTREAM_NATIVE_PG_SOURCE_TLS_MODE:-require}"',
            script,
        )
        self.assertIn(
            'target_tls_mode="${WORLDSTREAM_NATIVE_PG_TARGET_TLS_MODE:-require}"',
            script,
        )
        self.assertIn('source_tls_mode="disable"', script)
        self.assertIn('target_tls_mode="disable"', script)
        self.assertIn('--source-tls-mode "$source_tls_mode"', script)
        self.assertIn(
            '--target-tls-mode "$target_tls_mode" --passfile "$passfile"', script
        )
        self.assertIn("-p worldstream-server --bin worldstreamctl", script)
        self.assertIn("worldstream/postgres-native-restore-receipt/v1", script)
        self.assertIn("retain_receipt_artifact source.dump", script)
        self.assertIn(
            "validator.verify_native_restore_report(native_report)",
            script,
        )
        self.assertIn("set(receipt) != expected_fields", script)
        self.assertIn(
            '"schema": "worldstream/native-postgres-restore-smoke-evidence/v1"',
            script,
        )
        self.assertIn('"native_restore": native_report', script)
        self.assertIn('"live_restore_concurrency": live_restore_concurrency', script)
        self.assertIn('pg_dump_bin="/proc/$$/fd/$owned_pg_dump_fd"', script)
        self.assertIn('pg_restore_bin="/proc/$$/fd/$owned_pg_restore_fd"', script)
        self.assertIn('psql_bin="/proc/$$/fd/$owned_psql_fd"', script)
        self.assertIn(".env_clear()", rust)
        self.assertIn('.env("PGPASSFILE", passfile)', rust)
        self.assertIn('.env("PGSSLMODE", endpoint.tls_mode.as_libpq())', rust)
        self.assertIn('.env("PGGSSENCMODE", "disable")', rust)
        self.assertIn('command.env("PGSSLROOTCERT", "system")', rust)
        self.assertIn('Self::Require => "verify-full"', rust)
        self.assertNotIn("isolation.release()?", rust)
        self.assertIn("isolation.seal_fail_closed()", rust)
        self.assertIn("target_seal_observation", script)
        self.assertIn("datconnlimit::text", script)
        self.assertIn("native_restore_credential_cleanup_failed", script)
        self.assertIn(
            "CREATE EVENT TRIGGER worldstream_smoke_restore_ddl_hold "
            "ON ddl_command_start",
            script,
        )
        self.assertIn(
            "WHILE NOT (SELECT released FROM worldstream_smoke_control.restore_gate)",
            script,
        )
        self.assertIn("a.pid<>pg_backend_pid()", script)
        self.assertIn("a.backend_type='client backend'", script)
        self.assertIn("count(*) FILTER (WHERE s.rolsuper)", script)
        self.assertIn("count(*) FILTER (WHERE s.usename='$restore_role')", script)
        self.assertIn('"$observed_limit" == "2"', script)
        self.assertIn('"$observed_total" == "2"', script)
        self.assertIn('"$observed_super" == "1"', script)
        self.assertIn('"$observed_restore" == "1"', script)
        self.assertIn('"$observed_login" == "true"', script)
        self.assertIn('"$observed_role_super" == "false"', script)
        self.assertIn('"$observed_role_limit" == "1"', script)
        self.assertIn("/proc/self/fd/8 --host 127.0.0.1", script)
        self.assertIn('before="$(sha256sum /proc/self/fd/8', script)
        self.assertIn('after="$(sha256sum /proc/self/fd/8', script)
        self.assertIn("live_restore_concurrency", script)
        self.assertIn("target_client_backends_excluding_observer", script)
        self.assertIn("direct_superuser_keeper_backends", script)
        self.assertIn("one_use_restore_role_backends", script)
        self.assertIn("keeper_and_restore_pids_distinct", script)
        self.assertIn("final_captured_role_sessions_across_cluster", script)
        self.assertIn("final_captured_role_exists", script)
        self.assertIn("create_restore_credential", rust)
        self.assertIn("credential.passfile_path", rust)
        self.assertIn("role_passfile_identity", rust)
        self.assertIn("pin_pgpassfile_in", rust)
        self.assertIn("pinned_operator.remove()", rust)
        self.assertNotIn('"--file"', rust)
        self.assertIn("0|worldstream/native-postgres-disposable-target/v1", script)

    @unittest.skipUnless(shutil.which("bash"), "Bash is required for wrapper syntax")
    def test_generated_live_restore_probe_is_bash_syntax_valid(self) -> None:
        script = RUNNER.read_text(encoding="utf-8")
        start = script.index("template = r'''#!/bin/bash") + len("template = r'''")
        end = script.index("\n'''", start)
        wrapper = script[start:end]
        for marker, value in {
            "@DOCKER@": "/usr/bin/docker",
            "@TARGET_CONTAINER@": "target-container",
            "@TARGET_USER@": "postgres",
            "@TARGET_DB@": "worldstream",
            "@TARGET_DB_SQL_LITERAL@": "'worldstream'",
            "@PYTHON@": sys.executable,
            "@RESTORE_INPUT_FD@": "40",
            "@OBSERVATION_FD@": "41",
        }.items():
            wrapper = wrapper.replace(marker, value)
        result = subprocess.run(
            [shutil.which("bash") or "bash", "-n"],
            input=wrapper,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

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

    def test_success_evidence_is_withheld_when_verified_cleanup_fails(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-native-pg-test-") as name:
            directory = Path(name)
            environment = fake_external_restore_environment(directory)
            environment["WORLDSTREAM_NATIVE_PG_TEST_CLEANUP_FAILURE"] = "1"
            result = subprocess.run(
                [str(RUNNER)],
                cwd=ROOT,
                env=environment,
                text=True,
                capture_output=True,
                check=False,
            )

        self.assertEqual(result.returncode, 14)
        evidence = json.loads(result.stdout.splitlines()[-1])
        self.assertEqual(evidence["status"], "incomplete")
        self.assertEqual(evidence["reason"], "native_cleanup_failed")
        self.assertNotIn('"status":"ready"', result.stdout)

    @unittest.skipIf(os.name == "nt", "POSIX keeper lifecycle regression")
    def test_dead_artifact_keeper_fails_without_signaling_unrelated_process(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-native-pg-dead-keeper-"
        ) as name:
            directory = Path(name)
            unrelated = subprocess.Popen(
                [sys.executable, "-c", "import time; time.sleep(60)"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                environment = fake_external_restore_environment(directory)
                environment.update(
                    {
                        "WORLDSTREAM_NATIVE_PG_TEST_ARTIFACT_KEEPER_EXIT_AFTER_READY": (
                            "1"
                        ),
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

                self.assertEqual(result.returncode, 14, result.stderr)
                self.assertIsNone(unrelated.poll())
                evidence = json.loads(result.stdout.splitlines()[-1])
                self.assertEqual(evidence["reason"], "native_cleanup_failed")
                script = RUNNER.read_text(encoding="utf-8")
                self.assertNotIn("artifact_keeper_pid", script)
                self.assertNotIn("windows_keeper_pid", script)
                self.assertNotRegex(script, r"kill(?:\s+-\w+)?\s+.*keeper")
                self.assertIn("exec 7<&- || failed=1", script)
                self.assertIn(
                    'wait "$artifact_keeper_worker_pid"',
                    script,
                )
                self.assertIn(
                    'wait "$windows_keeper_worker_pid"',
                    script,
                )
            finally:
                unrelated.terminate()
                unrelated.wait(timeout=10)

    def test_committed_receipt_publishes_native_evidence_not_receipt(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-native-pg-receipt-"
        ) as name:
            environment = fake_external_restore_environment(Path(name))
            result = subprocess.run(
                [str(RUNNER)],
                cwd=ROOT,
                env=environment,
                text=True,
                capture_output=True,
                check=False,
            )

        self.assertEqual(result.returncode, 0, result.stderr)
        evidence = json.loads(result.stdout.splitlines()[-1])
        self.assertEqual(
            evidence["schema"],
            "worldstream/native-postgres-restore-smoke-evidence/v1",
        )
        self.assertEqual(
            evidence["native_restore"]["schema"],
            "worldstream/native-postgres-restore-evidence/v2",
        )
        self.assertEqual(evidence["status"], "ready")
        self.assertFalse(evidence["release_evidence"])
        self.assertFalse(evidence["secrets_emitted"])
        concurrency = evidence["live_restore_concurrency"]
        self.assertFalse(concurrency["observed"])

    def test_receipt_and_full_native_report_validation_fail_closed(self) -> None:
        cases = {
            "missing_receipt_field": "native_driver_receipt_invalid",
            "extra_receipt_field": "native_driver_receipt_invalid",
            "report_digest_mismatch": "native_driver_report_invalid",
            "dump_digest_mismatch": "native_driver_report_invalid",
            "non_ready_report": "native_driver_report_invalid",
        }
        for mode, expected_reason in cases.items():
            with (
                self.subTest(mode=mode),
                tempfile.TemporaryDirectory(
                    prefix="worldstream-native-pg-invalid-receipt-"
                ) as name,
            ):
                environment = fake_external_restore_environment(Path(name))
                environment["WORLDSTREAM_FAKE_RECEIPT_MODE"] = mode
                result = subprocess.run(
                    [str(RUNNER)],
                    cwd=ROOT,
                    env=environment,
                    text=True,
                    capture_output=True,
                    check=False,
                )

                self.assertEqual(result.returncode, 13, result.stderr)
                evidence = json.loads(result.stdout.splitlines()[-1])
                self.assertEqual(evidence["reason"], expected_reason)
                self.assertIsNone(evidence["native_restore"])
                self.assertFalse(evidence["release_evidence"])
                self.assertNotIn('"status":"ready"', result.stdout)

    @unittest.skipIf(os.name == "nt", "POSIX same-inode dump tamper regression")
    def test_retained_dump_same_inode_same_size_content_tamper_fails_closed(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-native-pg-dump-tamper-"
        ) as name:
            directory = Path(name)
            trace = directory / "tamper-trace.json"
            hook = directory / "tamper-retained-dump"
            hook.write_text(
                f"#!{sys.executable}\n"
                "import hashlib, json, os, sys\n"
                "from pathlib import Path\n"
                "path = Path(sys.argv[1]) / 'source.dump'\n"
                "before = os.stat(path, follow_symlinks=False)\n"
                "original = path.read_bytes()\n"
                "if not original: raise RuntimeError('dump unexpectedly empty')\n"
                "tampered = bytes([original[0] ^ 1]) + original[1:]\n"
                "with path.open('r+b', buffering=0) as output:\n"
                "    output.seek(0)\n"
                "    if output.write(tampered) != len(tampered):\n"
                "        raise RuntimeError('tamper write was incomplete')\n"
                "    os.fsync(output.fileno())\n"
                "after = os.stat(path, follow_symlinks=False)\n"
                "Path(os.environ['WORLDSTREAM_DUMP_TAMPER_TRACE']).write_text(\n"
                "    json.dumps({\n"
                "        'same_identity': (before.st_dev, before.st_ino)\n"
                "            == (after.st_dev, after.st_ino),\n"
                "        'same_size': before.st_size == after.st_size,\n"
                "        'before_sha256': hashlib.sha256(original).hexdigest(),\n"
                "        'after_sha256': hashlib.sha256(tampered).hexdigest(),\n"
                "    }, sort_keys=True),\n"
                "    encoding='utf-8',\n"
                ")\n",
                encoding="utf-8",
            )
            hook.chmod(0o700)
            environment = fake_external_restore_environment(directory)
            environment.update(
                {
                    "WORLDSTREAM_NATIVE_PG_TEST_AFTER_RECEIPT_RETENTION": str(hook),
                    "WORLDSTREAM_DUMP_TAMPER_TRACE": str(trace),
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

            self.assertEqual(result.returncode, 13, result.stderr)
            observed = json.loads(trace.read_text(encoding="utf-8"))
            self.assertTrue(observed["same_identity"])
            self.assertTrue(observed["same_size"])
            self.assertNotEqual(observed["before_sha256"], observed["after_sha256"])
            evidence = json.loads(result.stdout.splitlines()[-1])
            self.assertEqual(evidence["reason"], "native_driver_report_invalid")
            self.assertIsNone(evidence["native_restore"])
            self.assertNotIn('"status":"ready"', result.stdout)

    @unittest.skipIf(
        os.name == "nt", "portable Windows helper simulation is POSIX-only"
    )
    def test_windows_nt_helper_route_executes_without_dir_fd_or_procfs(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-native-pg-windows-route-"
        ) as name:
            directory = Path(name)
            python_hooks = directory / "python-hooks"
            python_hooks.mkdir()
            (python_hooks / "sitecustomize.py").write_text(
                "import os\n"
                "_open = os.open\n"
                "_stat = os.stat\n"
                "_listdir = os.listdir\n"
                "def guarded_open(*args, **kwargs):\n"
                "    if kwargs.get('dir_fd') is not None:\n"
                "        raise RuntimeError('Windows_NT route used os.open(dir_fd=)')\n"
                "    return _open(*args, **kwargs)\n"
                "def guarded_stat(*args, **kwargs):\n"
                "    if kwargs.get('dir_fd') is not None:\n"
                "        raise RuntimeError('Windows_NT route used os.stat(dir_fd=)')\n"
                "    return _stat(*args, **kwargs)\n"
                "def guarded_listdir(path='.'):\n"
                "    if isinstance(path, int):\n"
                "        raise RuntimeError('Windows_NT route listed a Unix directory fd')\n"
                "    return _listdir(path)\n"
                "os.open = guarded_open\n"
                "os.stat = guarded_stat\n"
                "os.listdir = guarded_listdir\n",
                encoding="utf-8",
            )
            environment = fake_external_restore_environment(directory)
            existing_pythonpath = environment.get("PYTHONPATH", "")
            environment.update(
                {
                    "OS": "Windows_NT",
                    "WORLDSTREAM_NATIVE_PG_TEST_PORTABLE_WINDOWS_AUTHORITY": "1",
                    "PYTHONPATH": str(python_hooks)
                    + (os.pathsep + existing_pythonpath if existing_pythonpath else ""),
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

        self.assertEqual(result.returncode, 0, result.stderr)
        evidence = json.loads(result.stdout.splitlines()[-1])
        self.assertEqual(evidence["status"], "ready")
        self.assertNotIn("/proc/", result.stdout + result.stderr)
        script = RUNNER.read_text(encoding="utf-8")
        self.assertIn('PATH_AUTHORITY = os.name == "nt" or sys.argv[5] == "1"', script)
        self.assertIn("elif PATH_AUTHORITY:", script)

    def test_docker_cleanup_is_bound_to_successfully_created_container_ids(
        self,
    ) -> None:
        script = RUNNER.read_text(encoding="utf-8")
        self.assertIn('source_container_id="$("$docker_bin" run --detach --rm', script)
        self.assertIn('target_container_id="$("$docker_bin" run --detach --rm', script)
        self.assertNotIn("--cidfile", script)
        self.assertIn('[[ "$source_container_id" =~ ^[0-9a-f]{64}$ ]]', script)
        self.assertIn('[[ "$target_container_id" =~ ^[0-9a-f]{64}$ ]]', script)
        self.assertIn('rm -f "$source_container_id"', script)
        self.assertIn('rm -f "$target_container_id"', script)
        self.assertNotIn('rm -f "$source_container"', script)
        self.assertNotIn('rm -f "$target_container"', script)

    @unittest.skipIf(os.name == "nt", "Linux descriptor execution regression")
    def test_docker_tool_substitution_executes_only_creation_bound_descriptors(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-native-pg-tool-authority-"
        ) as name:
            directory = Path(name)
            commands = directory / "commands"
            replacements = directory / "replacements"
            commands.mkdir()
            replacements.mkdir()
            trace = directory / "substituted-root"
            proof = directory / "bound-tool-proof.json"
            docker_log = directory / "docker.log"
            replacement_executed = directory / "replacement-executed"

            fake_uname = commands / "uname"
            fake_uname.write_text("#!/bin/sh\nprintf '%s\\n' Linux\n", encoding="utf-8")
            fake_uname.chmod(0o700)

            fake_docker = commands / "docker"
            fake_docker.write_text(
                "#!/bin/sh\n"
                "set -eu\n"
                ': "${WORLDSTREAM_DOCKER_LOG:?}"\n'
                'printf "%s\\n" "$*" >>"$WORLDSTREAM_DOCKER_LOG"\n'
                'case "${1-}" in\n'
                "  info) exit 0 ;;\n"
                "  run)\n"
                '    case "$*" in\n'
                "      *native-pg-source*) printf '%064d\\n' 1 ;;\n"
                "      *) printf '%064d\\n' 2 ;;\n"
                "    esac\n"
                "    exit 0\n"
                "    ;;\n"
                "  port)\n"
                '    case "${2-}" in\n'
                "      *native-pg-source*) printf '%s\\n' 127.0.0.1:15432 ;;\n"
                "      *) printf '%s\\n' 127.0.0.1:15433 ;;\n"
                "    esac\n"
                "    exit 0\n"
                "    ;;\n"
                "  exec)\n"
                '    case "$*" in\n'
                "      *'/bin/cat /run/worldstream-native-pg/mount-authority'*)\n"
                "        /bin/cat ./mount-authority\n"
                "        ;;\n"
                "    esac\n"
                "    exit 0\n"
                "    ;;\n"
                "  rm) exit 0 ;;\n"
                "esac\n"
                "exit 0\n",
                encoding="utf-8",
            )
            fake_docker.chmod(0o700)

            fake_transfer = commands / "postgres-transfer-smoke"
            fake_transfer.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            fake_transfer.chmod(0o700)

            fake_cargo = commands / "cargo"
            fake_cargo.write_text(
                f"#!{sys.executable}\n"
                "import json, os, re, subprocess, sys\n"
                "from pathlib import Path\n"
                "args = sys.argv[1:]\n"
                "if 'postgres-native-prepare' in args:\n"
                "    raise SystemExit(0)\n"
                "def value(flag):\n"
                "    return args[args.index(flag) + 1]\n"
                "expected = {\n"
                "    '--pg-dump': b'\"pg_dump\" --host 127.0.0.1',\n"
                "    '--pg-restore': b'readonly restore_input_fd=',\n"
                "    '--psql': b'selected_port=',\n"
                "}\n"
                "resolved = {}\n"
                "for flag, needle in expected.items():\n"
                "    received = value(flag)\n"
                "    match = re.fullmatch(r'/proc/[1-9][0-9]*/fd/([1-9][0-9]*)', received)\n"
                "    if match is None: raise RuntimeError('tool was not descriptor-bound')\n"
                "    descriptor = int(match.group(1))\n"
                "    os.lseek(descriptor, 0, os.SEEK_SET)\n"
                "    data = os.read(descriptor, 1024 * 1024)\n"
                "    if needle not in data: raise RuntimeError('retained tool bytes changed')\n"
                "    resolved[flag] = (received, descriptor)\n"
                "def executable_command(received, descriptor):\n"
                "    if Path(received).exists(): return [received]\n"
                "    fallback = f'/dev/fd/{descriptor}'\n"
                "    if not Path(fallback).exists(): raise RuntimeError('fd exec path absent')\n"
                "    return ['/bin/bash', fallback]\n"
                "tool_env = os.environ.copy()\n"
                "tool_env.update({\n"
                "    'PGPASSFILE': value('--passfile'),\n"
                "    'PGSSLMODE': 'disable',\n"
                "    'PGGSSENCMODE': 'disable',\n"
                "})\n"
                "statuses = {}\n"
                "for flag, expected_status in (\n"
                "    ('--pg-dump', 0), ('--pg-restore', 92), ('--psql', 0)\n"
                "):\n"
                "    received, descriptor = resolved[flag]\n"
                "    os.lseek(descriptor, 0, os.SEEK_SET)\n"
                "    completed = subprocess.run(\n"
                "        [*executable_command(received, descriptor), '--version'],\n"
                "        env=tool_env, input=b'', stdout=subprocess.DEVNULL,\n"
                "        close_fds=False, check=False,\n"
                "    )\n"
                "    if completed.returncode != expected_status:\n"
                "        raise RuntimeError(\n"
                "            f'bound tool did not execute: {flag}={completed.returncode}'\n"
                "        )\n"
                "    statuses[flag] = completed.returncode\n"
                "Path(os.environ['WORLDSTREAM_BOUND_TOOL_PROOF']).write_text(\n"
                "    json.dumps(statuses, sort_keys=True), encoding='utf-8'\n"
                ")\n"
                'print(\'{"status":"incomplete"}\')\n'
                "raise SystemExit(13)\n",
                encoding="utf-8",
            )
            fake_cargo.chmod(0o700)

            replacement_bytes = (
                b"#!/bin/sh\n"
                b': "${WORLDSTREAM_REPLACEMENT_EXECUTED:?}"\n'
                b'printf \'%s\\n\' "$0" >>"$WORLDSTREAM_REPLACEMENT_EXECUTED"\n'
                b"exit 99\n"
            )
            for tool in ("pg_dump", "pg_restore", "psql"):
                replacement = replacements / tool
                replacement.write_bytes(replacement_bytes)
                replacement.chmod(0o700)

            hook = commands / "replace-tools-before-driver"
            hook.write_text(
                "#!/bin/sh\n"
                "set -eu\n"
                ': "${WORLDSTREAM_REPLACEMENT_ROOT:?}"\n'
                ': "${WORLDSTREAM_TOOL_SUBSTITUTION_TRACE:?}"\n'
                "for tool in pg_dump pg_restore psql; do\n"
                '  mv -- "$1/$tool" "$1/.worldstream_retained_$tool"\n'
                '  mv -- "$WORLDSTREAM_REPLACEMENT_ROOT/$tool" "$1/$tool"\n'
                "done\n"
                'printf "%s" "$1" >"$WORLDSTREAM_TOOL_SUBSTITUTION_TRACE"\n',
                encoding="utf-8",
            )
            hook.chmod(0o700)

            environment = os.environ.copy()
            for variable in (
                "WORLDSTREAM_NATIVE_PG_SOURCE_HOST",
                "WORLDSTREAM_NATIVE_PG_SOURCE_PORT",
                "WORLDSTREAM_NATIVE_PG_TARGET_HOST",
                "WORLDSTREAM_NATIVE_PG_TARGET_PORT",
            ):
                environment.pop(variable, None)
            environment.update(
                {
                    "PATH": f"{commands}{os.pathsep}{environment['PATH']}",
                    "TMPDIR": str(directory),
                    "WORLDSTREAM_NATIVE_PG_CARGO": str(fake_cargo),
                    "WORLDSTREAM_NATIVE_PG_DOCKER": str(fake_docker),
                    "WORLDSTREAM_NATIVE_PG_PYTHON": sys.executable,
                    "WORLDSTREAM_NATIVE_PG_TRANSFER_SMOKE": str(fake_transfer),
                    "WORLDSTREAM_NATIVE_PG_TEST_BEFORE_DRIVER": str(hook),
                    "WORLDSTREAM_REPLACEMENT_ROOT": str(replacements),
                    "WORLDSTREAM_REPLACEMENT_EXECUTED": str(replacement_executed),
                    "WORLDSTREAM_TOOL_SUBSTITUTION_TRACE": str(trace),
                    "WORLDSTREAM_BOUND_TOOL_PROOF": str(proof),
                    "WORLDSTREAM_DOCKER_LOG": str(docker_log),
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

            self.assertEqual(result.returncode, 13, result.stdout + result.stderr)
            self.assertTrue(proof.exists(), result.stdout + result.stderr)
            self.assertEqual(
                json.loads(proof.read_text(encoding="utf-8")),
                {"--pg-dump": 0, "--pg-restore": 92, "--psql": 0},
            )
            substituted_root = Path(trace.read_text(encoding="utf-8"))
            for tool in ("pg_dump", "pg_restore", "psql"):
                self.assertEqual(
                    (substituted_root / tool).read_bytes(), replacement_bytes
                )
            self.assertFalse(replacement_executed.exists())
            self.assertEqual(
                json.loads(result.stdout.splitlines()[-1])["reason"],
                "native_driver_failed",
            )

    def test_help_creates_no_temporary_root(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-native-pg-help-") as name:
            directory = Path(name)
            before = sorted(directory.iterdir())
            environment = os.environ.copy()
            environment["TMPDIR"] = str(directory)
            result = subprocess.run(
                [str(RUNNER), "--help"],
                cwd=ROOT,
                env=environment,
                text=True,
                capture_output=True,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(sorted(directory.iterdir()), before)

    @unittest.skipIf(os.name == "nt", "POSIX retained-placeholder regression")
    def test_cleanup_rechecks_retained_unknown_placeholder_before_mutation(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-native-pg-unknown-cleanup-"
        ) as name:
            directory = Path(name)
            victim = directory / "victim-file"
            sentinel = b"unknown cleanup victim sentinel"
            victim.write_bytes(sentinel)
            trace = directory / "substituted-path"
            hook = directory / "replace-admitted-unknown"
            hook.write_text(
                "#!/bin/sh\n"
                "set -eu\n"
                ': "${WORLDSTREAM_CLEANUP_VICTIM:?}"\n'
                ': "${WORLDSTREAM_CLEANUP_TRACE:?}"\n'
                '[ "$#" -eq 2 ]\n'
                '[ "$2" = .worldstream_unknown_cleanup_probe ]\n'
                '/bin/rm -- "$1/$2"\n'
                'mv -- "$WORLDSTREAM_CLEANUP_VICTIM" "$1/$2"\n'
                'printf "%s" "$1" >"$WORLDSTREAM_CLEANUP_TRACE"\n',
                encoding="utf-8",
            )
            hook.chmod(0o700)
            environment = fake_external_restore_environment(directory)
            environment.update(
                {
                    "TMPDIR": str(directory),
                    "WORLDSTREAM_FAKE_RECEIPT_MODE": "unknown_placeholder",
                    "WORLDSTREAM_NATIVE_PG_TEST_AFTER_UNKNOWN_CLEANUP_ADMISSION": str(
                        hook
                    ),
                    "WORLDSTREAM_CLEANUP_VICTIM": str(victim),
                    "WORLDSTREAM_CLEANUP_TRACE": str(trace),
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

            self.assertEqual(result.returncode, 14, result.stderr)
            retained_root = Path(trace.read_text(encoding="utf-8"))
            self.assertEqual(
                (retained_root / ".worldstream_unknown_cleanup_probe").read_bytes(),
                sentinel,
            )
            self.assertEqual(
                (retained_root / "source.dump").read_bytes(),
                b"exact fake native dump",
            )
            self.assertEqual(
                json.loads(result.stdout.splitlines()[-1])["reason"],
                "native_cleanup_failed",
            )

    @unittest.skipIf(os.name == "nt", "POSIX retained-directory regression")
    def test_cleanup_preflight_preserves_substituted_victim_directory(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-native-pg-retained-"
        ) as name:
            directory = Path(name)
            victim = directory / "victim"
            victim.mkdir()
            marker = victim / "must-survive"
            marker.write_bytes(b"directory victim sentinel")
            trace = directory / "substituted-path"
            hook = directory / "replace-root-before-cleanup"
            hook.write_text(
                "#!/bin/sh\n"
                "set -eu\n"
                ': "${WORLDSTREAM_CLEANUP_VICTIM:?}"\n'
                ': "${WORLDSTREAM_CLEANUP_TRACE:?}"\n'
                'mv -- "$1" "$1.admitted"\n'
                'mv -- "$WORLDSTREAM_CLEANUP_VICTIM" "$1"\n'
                'printf "%s" "$1" >"$WORLDSTREAM_CLEANUP_TRACE"\n',
                encoding="utf-8",
            )
            hook.chmod(0o700)
            environment = fake_external_restore_environment(directory)
            environment.update(
                {
                    "TMPDIR": str(directory),
                    "WORLDSTREAM_NATIVE_PG_TEST_BEFORE_TEMP_CLEANUP": str(hook),
                    "WORLDSTREAM_CLEANUP_VICTIM": str(victim),
                    "WORLDSTREAM_CLEANUP_TRACE": str(trace),
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

            self.assertEqual(result.returncode, 14, result.stderr)
            substituted = Path(trace.read_text(encoding="utf-8"))
            self.assertEqual(
                (substituted / "must-survive").read_bytes(),
                b"directory victim sentinel",
            )
            self.assertEqual(
                json.loads(result.stdout.splitlines()[-1])["reason"],
                "native_cleanup_failed",
            )

    @unittest.skipIf(os.name == "nt", "POSIX retained-child regression")
    def test_cleanup_preflight_preserves_substituted_child_bytes(self) -> None:
        with tempfile.TemporaryDirectory(prefix="worldstream-native-pg-child-") as name:
            directory = Path(name)
            victim = directory / "victim-file"
            victim.write_bytes(b"child victim sentinel")
            trace = directory / "substituted-path"
            hook = directory / "replace-child-before-cleanup"
            hook.write_text(
                "#!/bin/sh\n"
                "set -eu\n"
                ': "${WORLDSTREAM_CLEANUP_VICTIM:?}"\n'
                ': "${WORLDSTREAM_CLEANUP_TRACE:?}"\n'
                'mv -- "$1/driver.stderr" "$1/driver.stderr.admitted"\n'
                'mv -- "$WORLDSTREAM_CLEANUP_VICTIM" "$1/driver.stderr"\n'
                'printf "%s" "$1" >"$WORLDSTREAM_CLEANUP_TRACE"\n',
                encoding="utf-8",
            )
            hook.chmod(0o700)
            environment = fake_external_restore_environment(directory)
            environment.update(
                {
                    "TMPDIR": str(directory),
                    "WORLDSTREAM_NATIVE_PG_TEST_BEFORE_TEMP_CLEANUP": str(hook),
                    "WORLDSTREAM_CLEANUP_VICTIM": str(victim),
                    "WORLDSTREAM_CLEANUP_TRACE": str(trace),
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

            self.assertEqual(result.returncode, 14, result.stderr)
            substituted = Path(trace.read_text(encoding="utf-8"))
            self.assertEqual(
                (substituted / "driver.stderr").read_bytes(),
                b"child victim sentinel",
            )

    def test_manual_imo52_workflow_uses_standalone_non_release_lane(self) -> None:
        diagnostic = (
            ROOT / ".github" / "workflows" / "imo52-live-diagnostic.yml"
        ).read_text(encoding="utf-8")
        release = (
            ROOT / ".github" / "workflows" / "compatibility-gates.yml"
        ).read_text(encoding="utf-8")

        self.assertIn("scripts/postgres-native-restore-smoke.sh", diagnostic)
        self.assertIn(
            "--evidence reports/imo52-native-postgres-restore-smoke.json",
            diagnostic,
        )
        self.assertIn(
            "path: reports/imo52-native-postgres-restore-smoke.json",
            diagnostic,
        )
        self.assertIn('python_bin="$(uv python find 3.14.7)"', diagnostic)
        self.assertIn(
            'cargo_bin="$(rustup which --toolchain 1.97.1 cargo)"', diagnostic
        )
        self.assertIn("docker_bin=/usr/bin/docker", diagnostic)
        self.assertIn('WORLDSTREAM_NATIVE_PG_PYTHON="$python_bin"', diagnostic)
        self.assertIn('WORLDSTREAM_NATIVE_PG_CARGO="$cargo_bin"', diagnostic)
        self.assertIn('WORLDSTREAM_NATIVE_PG_DOCKER="$docker_bin"', diagnostic)
        self.assertNotIn("postgres-native-restore-linux-live.sh", diagnostic)
        self.assertNotIn("scripts/gates-install-postgres-client.sh", diagnostic)
        self.assertNotIn("WORLDSTREAM_PACKAGED_CTL", diagnostic)
        self.assertNotIn("docker run --detach", diagnostic)
        self.assertNotIn("reports/native-linux-postgres-restore.json", diagnostic)
        self.assertNotIn("reports/native-linux-transfer-seed.json", diagnostic)
        self.assertNotIn("dist/", diagnostic)
        self.assertIn("scripts/postgres-native-restore-linux-live.sh", release)

    def test_release_windows_runner_executes_live_native_restore(self) -> None:
        workflow = (
            ROOT / ".github" / "workflows" / "compatibility-gates.yml"
        ).read_text(encoding="utf-8")
        harness = (
            ROOT / "scripts" / "postgres-native-restore-windows-live.ps1"
        ).read_text(encoding="utf-8")
        self.assertIn(
            "Run hosted Windows PostgreSQL native backup restore and recovery",
            workflow,
        )
        self.assertIn(
            "scripts/postgres-native-restore-windows-live.ps1",
            workflow,
        )
        self.assertIn("postgres-transfer-smoke.sh --build-source", harness)
        self.assertIn("scripts/postgres-native-restore-hosted-report.py", harness)
        self.assertIn("'--packaged-control', $env:WORLDSTREAM_PACKAGED_CTL", harness)
        self.assertIn("WORLDSTREAM_PG_TRANSFER_SOURCE_REVISION", harness)
        self.assertIn("WORLDSTREAM_PG_TRANSFER_ADMIN_DSN_FILE", harness)
        self.assertIn("Clear-WorldstreamTransferDsnFiles", harness)
        self.assertIn("$Failures = [Collections.Generic.List[object]]::new()", harness)
        self.assertIn("$Remaining = [Collections.Generic.List[object]]::new()", harness)
        self.assertIn("[void]$Failures.Add($_)", harness)
        self.assertIn("[void]$Remaining.Add($Stream)", harness)
        self.assertIn(
            "$script:TransferDsnScrubComplete = $Failures.Count -eq 0", harness
        )
        provider_cleanup = harness.index("Invoke-WorldstreamProviderCleanup")
        self.assertIn(
            "if ($null -eq $CleanupFailure) {\n                $CleanupFailure = $_",
            harness[provider_cleanup:],
        )
        self.assertNotIn("WORLDSTREAM_PG_TRANSFER_ADMIN_DSN =", harness)
        self.assertIn("$SourceDatabase = 'worldstream_native_source'", harness)
        self.assertIn("CREATE DATABASE $SourceDatabase", harness)
        self.assertIn("CONNECTION LIMIT 0", harness)
        self.assertIn("'--pg-dump', $env:WORLDSTREAM_PG_DUMP", harness)
        self.assertIn("'--passfile-storage-id', $PassfileStorageId", harness)
        self.assertIn("'--passfile-file-id', $PassfileFileId", harness)
        self.assertIn("'--passfile-size', $PassfileSize", harness)
        self.assertIn("'--passfile-sha256', $PassfileSha256", harness)
        self.assertIn("'--work-parent-storage-id', $WorkParentStorageId", harness)
        self.assertIn("'--work-parent-file-id', $WorkParentFileId", harness)
        self.assertIn("'--scrub-passfile-on-success'", harness)
        self.assertIn(
            'DROP DATABASE IF EXISTS `"$SourceDatabase`" WITH (FORCE)',
            harness,
        )
        self.assertIn('DROP ROLE IF EXISTS `"$RuntimeRole`"', harness)
        self.assertIn("$ProviderSystemIdentifier", harness)
        self.assertIn("$PreserveRecovery = $true\n& $ReleasePython", harness)
        self.assertIn("$SupervisorExitCode -eq 43", harness)
        self.assertIn("$PreserveRecovery = $false", harness)
        self.assertIn("Invoke-WorldstreamProviderCleanup", harness)
        self.assertIn("Clear-WorldstreamPassfile", harness)
        retained_check = harness.index("$RetainedIdentity[0] -cne $PassfileStorageId")
        self.assertLess(
            retained_check,
            harness.index("$script:PassfileWriteStream.SetLength(0)"),
        )
        self.assertIn("$RetainedIdentity[1] -cne $PassfileFileId", harness)
        self.assertNotIn("--bin postgres-native-prepare", harness)
        self.assertNotIn("scripts/postgres-native-restore-smoke.sh", harness)
        self.assertIn("native-windows-postgres-restore.json", harness)
        self.assertIn("$Report.native_restore.native_witness_minted", harness)

    @unittest.skipUnless(shutil.which("pwsh"), "PowerShell fault injection")
    def test_windows_transfer_dsn_cleanup_attempts_every_stream_operation(self) -> None:
        harness = ROOT / "scripts" / "postgres-native-restore-windows-live.ps1"
        command = r"""
$Tokens = $null
$Errors = $null
$Ast = [Management.Automation.Language.Parser]::ParseFile(
    $env:WORLDSTREAM_WINDOWS_HARNESS,
    [ref]$Tokens,
    [ref]$Errors
)
if ($Errors.Count -ne 0) { throw $Errors[0] }
$Function = $Ast.FindAll(
    {
        param($Node)
        $Node -is [Management.Automation.Language.FunctionDefinitionAst] `
            -and $Node.Name -eq 'Clear-WorldstreamTransferDsnFiles'
    },
    $true
)
if ($Function.Count -ne 1) { throw 'cleanup function AST not unique' }
Invoke-Expression $Function[0].Extent.Text
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;

public sealed class FaultingWorldstreamDsnStream {
    public static readonly List<string> Log = new List<string>();
    private readonly int id;
    private long length = 7;

    public FaultingWorldstreamDsnStream(int id) { this.id = id; }
    public long Length {
        get { Log.Add(id + ":Length"); return length; }
    }
    public void SetLength(long value) {
        Log.Add(id + ":SetLength");
        if (id == 0) throw new InvalidOperationException("first setlength");
        length = value;
    }
    public void Flush(bool durable) { Log.Add(id + ":Flush"); }
    public void Dispose() {
        Log.Add(id + ":Dispose");
        if (id == 1) throw new InvalidOperationException("second dispose");
    }
}
'@
$script:TransferDsnStreams = @(
    [FaultingWorldstreamDsnStream]::new(0),
    [FaultingWorldstreamDsnStream]::new(1),
    [FaultingWorldstreamDsnStream]::new(2)
)
$script:TransferDsnScrubComplete = $false
$script:TransferDsnCleanupFailure = $null
$Observed = $null
try { Clear-WorldstreamTransferDsnFiles } catch { $Observed = $_.Exception.Message }
$FirstLogCount = [FaultingWorldstreamDsnStream]::Log.Count
try { Clear-WorldstreamTransferDsnFiles } catch { }
[ordered]@{
    observed = $Observed
    log = @([FaultingWorldstreamDsnStream]::Log)
    first_log_count = $FirstLogCount
    final_log_count = [FaultingWorldstreamDsnStream]::Log.Count
    remaining = $script:TransferDsnStreams.Count
    complete = $script:TransferDsnScrubComplete
} | ConvertTo-Json -Compress
"""
        environment = os.environ.copy()
        environment["WORLDSTREAM_WINDOWS_HARNESS"] = str(harness)
        result = subprocess.run(
            ["pwsh", "-NoLogo", "-NoProfile", "-Command", command],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        observation = json.loads(result.stdout)
        self.assertIn("first setlength", observation["observed"])
        self.assertEqual(
            observation["log"],
            [
                "0:SetLength",
                "0:Flush",
                "0:Length",
                "0:Dispose",
                "1:SetLength",
                "1:Flush",
                "1:Length",
                "1:Dispose",
                "2:SetLength",
                "2:Flush",
                "2:Length",
                "2:Dispose",
            ],
        )
        self.assertEqual(observation["first_log_count"], 12)
        self.assertEqual(observation["final_log_count"], 12)
        self.assertEqual(observation["remaining"], 1)
        self.assertFalse(observation["complete"])

    def test_release_linux_runner_executes_exact_hosted_native_restore(self) -> None:
        workflow = (
            ROOT / ".github" / "workflows" / "compatibility-gates.yml"
        ).read_text(encoding="utf-8")
        harness = (
            ROOT / "scripts" / "postgres-native-restore-linux-live.sh"
        ).read_text(encoding="utf-8")
        installer = (ROOT / "scripts" / "gates-install-postgres-client.sh").read_text(
            encoding="utf-8"
        )

        self.assertIn(
            "Run hosted Linux PostgreSQL native backup restore and recovery", workflow
        )
        self.assertIn("scripts/postgres-native-restore-linux-live.sh", workflow)
        self.assertIn("scripts/gates-install-postgres-client.sh", workflow)
        self.assertIn("scripts/postgres-native-restore-hosted-report.py", harness)
        self.assertIn('--packaged-control "$WORLDSTREAM_PACKAGED_CTL"', harness)
        self.assertIn("WORLDSTREAM_PG_TRANSFER_SOURCE_REVISION", harness)
        self.assertIn("WORLDSTREAM_PG_TRANSFER_ADMIN_DSN_FILE", harness)
        self.assertIn("scrub_transfer_dsn_files", harness)
        self.assertNotIn("WORLDSTREAM_PG_TRANSFER_ADMIN_DSN=", harness)
        self.assertIn("source_database='worldstream_native_source'", harness)
        self.assertIn("CREATE DATABASE $source_database", harness)
        self.assertIn("CONNECTION LIMIT 0", harness)
        self.assertIn('--pg-dump "$WORLDSTREAM_PG_DUMP"', harness)
        self.assertIn('--passfile-storage-id "$operator_passfile_storage_id"', harness)
        self.assertIn('--passfile-file-id "$operator_passfile_file_id"', harness)
        self.assertIn('--passfile-size "$operator_passfile_size"', harness)
        self.assertIn('--passfile-sha256 "$operator_passfile_sha256"', harness)
        self.assertIn('--work-parent-storage-id "$work_parent_storage_id"', harness)
        self.assertIn('--work-parent-file-id "$work_parent_file_id"', harness)
        self.assertIn("--scrub-passfile-on-success", harness)
        self.assertIn(
            'DROP DATABASE IF EXISTS \\"$source_database\\" WITH (FORCE)',
            harness,
        )
        self.assertIn('DROP ROLE IF EXISTS \\"$runtime_role\\"', harness)
        self.assertIn("provider_system_identifier", harness)
        self.assertIn("trap cleanup_on_exit EXIT", harness)
        self.assertIn("preserve_recovery=1\nset +e", harness)
        self.assertIn('supervisor_status" -eq 43', harness)
        self.assertIn("preserve_recovery=0", harness)
        self.assertIn("native-linux-postgres-restore.json", harness)
        self.assertIn("postgresql-client-17_17.11-1.pgdg24.04+2_amd64.deb", installer)
        self.assertIn(
            "b3b071b67a814a382516d6f6241b529c52f909672457e2f48083acce047b634f",
            installer,
        )
        self.assertIn("WORLDSTREAM_PG_DUMP=", installer)

    @unittest.skipIf(os.name == "nt", "Linux hosted-wrapper fault injection")
    def test_linux_hosted_wrapper_scrubs_and_tears_down_after_fixture_failure(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-hosted-linux-failure-"
        ) as name:
            temporary = Path(name)
            result, runner_temp, cleanup_log = run_linux_hosted_wrapper_failure(
                temporary, transfer_exit=97, kill_supervisor=False
            )

            self.assertEqual(result.returncode, 97, result.stderr)
            self.assertNotIn("passed", result.stdout.lower())
            secret_root = runner_temp / "worldstream-native-restore-secrets"
            self.assertEqual(
                sorted(path.name for path in secret_root.iterdir()),
                [
                    "transfer-abort-admin.dsn",
                    "transfer-admin.dsn",
                    "transfer-runtime.dsn",
                ],
            )
            self.assertTrue(
                all(path.read_bytes() == b"" for path in secret_root.iterdir()),
                "failed fixture credentials must be scrubbed through retained handles",
            )
            cleanup_sql = cleanup_log.read_text(encoding="utf-8")
            for identity in ("123456789", "16384", "16385", "16386", "16387"):
                self.assertIn(identity, cleanup_sql)
            self.assertIn(
                'DROP DATABASE IF EXISTS "worldstream_native_source"', cleanup_sql
            )
            self.assertIn(
                'DROP DATABASE IF EXISTS "worldstream_native_restore"', cleanup_sql
            )
            self.assertIn(
                'DROP DATABASE IF EXISTS "worldstream_native_abort"', cleanup_sql
            )
            self.assertIn(
                'DROP ROLE IF EXISTS "worldstream_native_linux_runtime"', cleanup_sql
            )

    @unittest.skipIf(os.name == "nt", "Linux hosted-wrapper fault injection")
    def test_linux_hosted_wrapper_preserves_everything_after_supervisor_sigkill(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory(
            prefix="worldstream-hosted-linux-sigkill-"
        ) as name:
            result, runner_temp, cleanup_log = run_linux_hosted_wrapper_failure(
                Path(name), transfer_exit=0, kill_supervisor=True
            )

            self.assertEqual(result.returncode, 137, result.stderr)
            self.assertNotIn("passed", result.stdout.lower())
            self.assertFalse(cleanup_log.exists())
            self.assertNotEqual(
                (
                    runner_temp
                    / "worldstream-native-restore-secrets"
                    / "operator.pgpass"
                ).read_bytes(),
                b"",
            )


if __name__ == "__main__":
    unittest.main()

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
GATES_PATH = ROOT / "scripts/gates.py"
WORKFLOW_PATH = ROOT / ".github/workflows/compatibility-gates.yml"


def load_gates():
    spec = importlib.util.spec_from_file_location("worldstream_gates_smoke", GATES_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load scripts/gates.py")
    gates = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = gates
    spec.loader.exec_module(gates)
    return gates


def test_cargo_evidence_rejects_zero_test_success(monkeypatch):
    gates = load_gates()
    commands: list[list[str]] = []

    monkeypatch.setattr(gates.shutil, "which", lambda name: "/usr/bin/cargo")

    def fake_run(argv, **kwargs):
        commands.append(argv)
        return subprocess.CompletedProcess(
            argv,
            0,
            stdout="test result: ok. 0 passed; 0 failed; 1 filtered out;\n",
            stderr="",
        )

    monkeypatch.setattr(gates.subprocess, "run", fake_run)
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.cargo_test_evidence(
        runner,
        "evidence-test",
        "worldstream-fixture",
        ("missing-filter",),
        "fixture boundary",
    )

    assert commands[0][0:4] == ["/usr/bin/cargo", "test", "--locked", "--offline"]
    assert commands[0][-2:] == ["--", "missing-filter"]
    assert runner.outcomes[0].status == "FAIL"
    assert "no passing test count" in runner.outcomes[0].detail


def test_windows_filesystem_policy_is_not_a_local_pass():
    gates = load_gates()
    runner = gates.GateRunner(strict=False, offline=True, ci=False)

    gates.filesystem_checks(runner, host_os="nt")

    statuses = {outcome.name: outcome.status for outcome in runner.outcomes}
    assert statuses["filesystem-owner-only"] == "SKIP_INCOMPLETE"
    assert statuses["filesystem-acl-policy"] == "SKIP_INCOMPLETE"
    assert statuses["evidence-filesystem-local"] == "SKIP_INCOMPLETE"


def test_darwin_kill_point_is_incomplete_not_failure_or_evidence(monkeypatch):
    gates = load_gates()
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    monkeypatch.setattr(gates.platform_module, "system", lambda: "Darwin")
    commands = []

    def fake_command(name, argv, **kwargs):
        commands.append((name, argv))
        return True

    monkeypatch.setattr(runner, "command", fake_command)
    gates.process_contract_checks(runner)

    kill = next(
        outcome for outcome in runner.outcomes if outcome.name == "process-kill-point"
    )
    assert kill.status == "SKIP_INCOMPLETE"
    assert kill.classification == "incomplete"
    assert not any(name == "process-kill-point" for name, _argv in commands)
    assert all(outcome.classification == "incomplete" for outcome in runner.outcomes)


def test_strict_local_hook_routes_missing_postgres_container_evidence_to_hosted_linux(
    monkeypatch,
):
    gates = load_gates()
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    monkeypatch.setattr(gates.os, "name", "posix")
    monkeypatch.setattr(gates.shutil, "which", lambda _name: None)

    gates.postgres_live_contract(runner, required=False)

    outcome = next(
        item for item in runner.outcomes if item.name == "postgres-live-contract"
    )
    assert outcome.status == "SKIP_INCOMPLETE"
    assert outcome.classification == "incomplete"
    assert "hosted Linux owns" in outcome.detail
    assert runner.finish() == 0


def test_release_inventory_catches_unmapped_release_gate_evidence():
    gates = load_gates()
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    manifest = {
        "evidence": [
            {
                "id": "new-release-contract",
                "release_gate": True,
                "status": "unresolved",
                "artifact_digest": "",
            }
        ]
    }

    gates.contract_inventory(runner, manifest, release=True)

    inventory = [
        outcome
        for outcome in runner.outcomes
        if outcome.name == "release-evidence-inventory"
    ]
    assert inventory and inventory[0].status == "FAIL"
    assert "new-release-contract" in inventory[0].detail


def test_ci_matrix_uses_explicit_platform_route(capsys):
    gates = load_gates()
    manifest = {
        "gate_cells": [
            {
                "id": "native-linux-x86_64",
                "tier": "minimal-ci",
                "required": True,
                "platform": "native-linux-x86_64",
            }
        ]
    }

    assert gates.list_cells(manifest, "minimal-ci", "matrix") == 0
    assert '"runner":"ubuntu-24.04"' in capsys.readouterr().out


def test_workflow_bootstraps_exact_gate_python_before_running_offline_gate():
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    assert "cargo fetch --locked" in workflow
    assert "uv python install 3.14.7" in workflow
    assert (
        "uv run --python 3.14.7 --no-project python scripts/gates.py fast --ci --offline"
        in workflow
    )
    assert (
        "uv run --python 3.14.7 --no-project python scripts/gates.py minimal-ci"
        in workflow
    )


def test_workflow_keeps_native_postgres_provisioning_pinned_and_platform_specific():
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    assert "postgres:17.11-alpine@sha256:" in workflow
    assert "runner.os == 'Linux'" in workflow
    assert "runner.os == 'Windows'" in workflow
    assert "scripts/gates-install-postgres.ps1" in workflow


def test_ci_matrix_materializes_declared_oci_platform(capsys):
    gates = load_gates()
    manifest = {
        "gate_cells": [
            {
                "id": "native-linux-x86_64",
                "tier": "minimal-ci",
                "required": True,
                "platform": "native-linux-x86_64",
            },
            {
                "id": "native-windows-x64",
                "tier": "minimal-ci",
                "required": True,
                "platform": "native-windows-x64",
            },
        ],
        "platforms": [
            {
                "id": "oci-linux-amd64",
                "support": "release",
                "target": "linux/amd64",
                "storage_profiles": ["sqlite-bundled", "postgres-primary"],
            }
        ],
    }

    assert gates.list_cells(manifest, "minimal-ci", "matrix") == 0
    matrix = json.loads(capsys.readouterr().out)
    assert [row["cell"] for row in matrix["include"]] == [
        "native-linux-x86_64",
        "native-windows-x64",
        "oci-linux-amd64",
    ]
    oci = matrix["include"][-1]
    assert oci["platform"] == "oci-linux-amd64"
    assert oci["runner"] == "ubuntu-24.04"
    assert oci["shell"] == "bash"
    assert oci["system"] == "Linux"


def test_ci_platform_mismatch_is_an_exact_incomplete_blocker(monkeypatch):
    gates = load_gates()
    monkeypatch.setattr(gates.platform_module, "system", lambda: "Darwin")
    runner = gates.GateRunner(strict=False, offline=True, ci=False)
    manifest = {
        "gate_cells": [
            {
                "id": "native-windows-x64",
                "tier": "minimal-ci",
                "required": True,
                "platform": "native-windows-x64",
            }
        ]
    }

    assert gates.selected_ci_cell(runner, manifest, "native-windows-x64") is None
    outcome = runner.outcomes[0]
    assert outcome.status == "SKIP_INCOMPLETE"
    assert outcome.classification == "incomplete"
    assert "platform=native-windows-x64" in outcome.detail
    assert "requires runner=windows-2025" in outcome.detail
    assert "observed system=Darwin" in outcome.detail


def test_release_handoff_preserves_unresolved_manifest_evidence():
    gates = load_gates()
    manifest = {
        "manifest_kind": "specification",
        "release_ready": False,
        "release_candidate": "0.1.0",
        "release_artifacts": [
            {
                "id": "oci-linux-amd64-image",
                "profile": "oci-linux-amd64",
                "digest_algorithm": "sha256",
                "digest": "",
                "status": "unresolved",
            },
            {
                "id": "sigstore-bundle",
                "profile": "all",
                "digest_algorithm": "sha256",
                "digest": "",
                "status": "unresolved",
            },
        ],
        "evidence": [
            {
                "id": "checksums-signature-sbom-provenance",
                "release_gate": True,
                "status": "unresolved",
                "artifact_digest": "",
            }
        ],
    }

    handoff = gates.release_evidence_handoff(manifest)
    assert handoff["schema"] == "worldstream/release-evidence-handoff/v1"
    assert handoff["release_evidence"] is False
    assert handoff["release_ready"] is False
    artifacts = {row["id"]: row for row in handoff["required_artifacts"]}
    assert artifacts["oci-linux-amd64-image"]["path"] == (
        "worldstream-0.1.0-oci-linux-amd64.oci.tar"
    )
    assert artifacts["oci-linux-amd64-image"]["digest"] == ""
    assert artifacts["sigstore-bundle"]["path"] == "sigstore.bundle.json"
    assert handoff["required_evidence"][0]["artifact_digest"] == ""


def test_nonrelease_tiers_accept_complete_embedded_release_contract():
    gates = load_gates()
    manifest = gates.load_manifest()
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.manifest_gate(runner, manifest, release=False)

    state = [
        outcome
        for outcome in runner.outcomes
        if outcome.name == "manifest-contract-state"
    ]
    assert state and state[0].status == "PASS"
    assert "detached distribution evidence is not asserted" in state[0].detail


def test_release_metadata_requires_exact_inventory_and_digests(tmp_path, monkeypatch):
    gates = load_gates()
    release_dir = tmp_path / "release"
    release_dir.mkdir()
    (release_dir / "release-manifest.json").write_text(
        json.dumps(
            {
                "schema": "worldstream/release-artifact-manifest/v1",
                "product": "0.1.0",
                "source_version": "0.1.0",
                "manifest": {},
                "artifacts": {},
                "artifact_digests": {},
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("WORLDSTREAM_RELEASE_DIR", str(release_dir))
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.verify_release_artifacts(
        runner,
        {
            "release_candidate": "0.1.0",
            "release_artifacts": [
                {
                    "id": "source-archive",
                    "profile": "source",
                    "digest_algorithm": "sha256",
                    "digest": "",
                    "status": "unresolved",
                }
            ],
        },
    )

    assert any(
        outcome.name == "release-artifact-manifest" and outcome.status == "FAIL"
        for outcome in runner.outcomes
    )


def test_release_artifact_identity_diagnostic_keeps_placeholder_unresolved(
    tmp_path, monkeypatch
):
    gates = load_gates()
    release_dir = tmp_path / "release"
    release_dir.mkdir()
    artifact = release_dir / "worldstream-0.1.0-source.tar.gz"
    artifact.write_bytes(b"source fixture")
    observed = "sha256:" + gates.hashlib.sha256(artifact.read_bytes()).hexdigest()
    metadata = {
        "schema": "worldstream/release-artifact-manifest/v1",
        "product": "0.1.0",
        "source_version": "0.1.0",
        "manifest": {
            "source": "compatibility.toml",
            "mirror": "compatibility.json",
            "sha256": gates.hashlib.sha256(gates.MIRROR_PATH.read_bytes()).hexdigest(),
        },
        "artifacts": {"source-archive": artifact.name},
        "artifact_digests": {"source-archive": observed},
    }
    (release_dir / "release-manifest.json").write_text(
        json.dumps(metadata), encoding="utf-8"
    )
    monkeypatch.setenv("WORLDSTREAM_RELEASE_DIR", str(release_dir))
    runner = gates.GateRunner(strict=False, offline=True, ci=False)

    gates.verify_release_artifacts(
        runner,
        {
            "release_candidate": "0.1.0",
            "release_artifacts": [
                {
                    "id": "source-archive",
                    "profile": "source",
                    "digest_algorithm": "sha256",
                    "digest": "",
                    "status": "unresolved",
                }
            ],
        },
    )

    identity = next(
        outcome
        for outcome in runner.outcomes
        if outcome.name == "release-artifact-source-archive"
    )
    assert identity.status == "SKIP_INCOMPLETE"
    assert identity.classification == "incomplete"
    assert "profile='source'" in identity.detail
    assert "manifest_digest=''" in identity.detail
    assert f"observed_bytes='{observed}'" in identity.detail


def test_release_identity_failure_reports_expected_and_observed_values(
    tmp_path, monkeypatch
):
    gates = load_gates()
    release_dir = tmp_path / "release"
    release_dir.mkdir()
    (release_dir / "release-manifest.json").write_text(
        json.dumps(
            {
                "schema": "worldstream/release-artifact-manifest/v1",
                "product": "0.1.0",
                "source_version": "0.1.0",
                "manifest": {
                    "source": "wrong.toml",
                    "mirror": "compatibility.json",
                    "sha256": "0" * 64,
                },
                "artifacts": {},
                "artifact_digests": {},
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("WORLDSTREAM_RELEASE_DIR", str(release_dir))
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.verify_release_artifacts(
        runner,
        {"release_candidate": "0.1.0", "release_artifacts": []},
    )

    details = [
        outcome.detail
        for outcome in runner.outcomes
        if outcome.name == "release-manifest-identity"
    ]
    assert any("expected='compatibility.toml'" in detail for detail in details)
    assert any("observed='wrong.toml'" in detail for detail in details)
    assert any("expected='" in detail and "observed='" in detail for detail in details)


def test_command_start_failure_is_classified_as_gate_failure(monkeypatch):
    gates = load_gates()
    monkeypatch.setattr(gates.shutil, "which", lambda name: "/usr/bin/tool")

    def fail_to_start(*args, **kwargs):
        raise OSError("simulated spawn failure")

    monkeypatch.setattr(gates.subprocess, "run", fail_to_start)
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    assert not runner.command("spawn-test", ["tool"])
    assert runner.outcomes[0].status == "FAIL"
    assert "could not start" in runner.outcomes[0].detail


def test_gate_report_summarizes_blocking_and_incomplete_outcomes(tmp_path, monkeypatch):
    gates = load_gates()
    report_path = tmp_path / "gate-report.json"
    monkeypatch.setenv("WORLDSTREAM_GATE_REPORT", str(report_path))
    runner = gates.GateRunner(strict=False, offline=True, ci=False)
    runner.fail("release-artifacts", "unresolved artifact digest")
    runner.skip("native-windows", "Windows host is unavailable")

    assert runner.finish() == 1
    report = json.loads(report_path.read_text(encoding="utf-8"))
    assert report["summary"] == {
        "checks": 2,
        "failures": 1,
        "incomplete_skips": 1,
    }
    assert report["blocking_failures"][0]["name"] == "release-artifacts"
    assert report["incomplete_skips"][0]["name"] == "native-windows"
    assert report["incomplete_blockers"][0]["classification"] == "incomplete"
    assert report["status"] == "blocked"
    assert report["fail_closed"] is True


def test_release_gate_report_contains_fail_closed_blocker_matrix(tmp_path):
    gates = load_gates()
    report_path = tmp_path / "release-gate-report.json"
    manifest = {
        "manifest_kind": "specification",
        "release_ready": False,
        "unresolved_required_fields": ["release_artifacts[*].digest"],
        "release_artifacts": [
            {
                "id": "native-windows-x64-archive",
                "status": "unresolved",
                "digest": "",
            }
        ],
        "evidence": [
            {
                "id": "native-windows-release-profile",
                "release_gate": True,
                "status": "unresolved",
                "artifact_digest": "",
            }
        ],
    }
    runner = gates.GateRunner(
        strict=True,
        offline=True,
        ci=False,
        tier="release",
        report_path=str(report_path),
    )

    assert runner.finish(manifest=manifest, tier="release") == 1
    report = json.loads(report_path.read_text(encoding="utf-8"))
    matrix = report["release_blocker_matrix"]
    assert matrix["schema"] == "worldstream/release-blocker-matrix/v1"
    assert matrix["status"] == "blocked"
    assert report["status"] == "blocked"
    assert report["fail_closed"] is True
    rows = {row["id"]: row for row in matrix["rows"]}
    assert rows["manifest-state"]["status"] == "blocked"
    assert rows["release-artifact:native-windows-x64-archive"]["status"] == "blocked"
    assert (
        rows["release-evidence:native-windows-release-profile"]["status"] == "blocked"
    )
    assert (
        "native Windows"
        in rows["release-artifact:native-windows-x64-archive"]["environment"]
    )
    assert not any(
        row["id"] == "release-artifact:native-windows-x64-archive"
        and row["status"] == "ready"
        for row in matrix["rows"]
    )


def test_release_report_is_fail_closed_for_malformed_manifest_collections(tmp_path):
    gates = load_gates()
    report_path = tmp_path / "malformed-release-report.json"
    runner = gates.GateRunner(
        strict=True,
        offline=True,
        ci=False,
        tier="release",
        report_path=str(report_path),
    )

    assert (
        runner.finish(
            manifest={
                "manifest_kind": "specification",
                "release_ready": False,
                "release_artifacts": None,
                "evidence": None,
                "platforms": None,
            },
            tier="release",
        )
        == 1
    )
    report = json.loads(report_path.read_text(encoding="utf-8"))
    assert report["status"] == "blocked"
    assert report["fail_closed"] is True
    assert report["release_blocker_matrix"]["status"] == "blocked"


def test_detached_release_rows_require_verified_artifact_and_evidence_outcomes():
    gates = load_gates()
    manifest = {
        "manifest_kind": "release",
        "release_ready": True,
        "unresolved_required_fields": [],
        "release_artifact_digest_source": "detached_release_manifest",
        "release_artifacts": [
            {
                "id": "native-windows-x64-archive",
                "profile": "native-windows-x64",
                "digest_algorithm": "sha256",
                "status": "detached",
                "digest": "",
                "digest_location": "release-manifest.json",
            }
        ],
        "evidence": [
            {
                "id": "native-windows-release-profile",
                "release_gate": True,
                "status": "detached",
                "artifact_digest": "",
                "artifact_digest_location": "release-manifest.json",
            }
        ],
    }

    declared_only = gates.release_blocker_matrix(manifest, [])
    declared_rows = {row["id"]: row for row in declared_only["rows"]}
    assert (
        declared_rows["release-artifact:native-windows-x64-archive"]["status"]
        == "blocked"
    )
    assert (
        declared_rows["release-evidence:native-windows-release-profile"]["status"]
        == "blocked"
    )

    verified = gates.release_blocker_matrix(
        manifest,
        [
            gates.Outcome(
                "release-artifact-native-windows-x64-archive",
                "PASS",
                "exact detached artifact digest verified",
                "pass",
            ),
            gates.Outcome(
                "release-evidence-native-windows-release-profile",
                "PASS",
                "exact detached evidence digest verified",
                "pass",
            ),
        ],
    )
    verified_rows = {row["id"]: row for row in verified["rows"]}
    assert (
        verified_rows["release-artifact:native-windows-x64-archive"]["status"]
        == "ready"
    )
    assert (
        verified_rows["release-evidence:native-windows-release-profile"]["status"]
        == "ready"
    )


def test_macos_quickstart_is_locked_and_reports_missing_tooling():
    script = (ROOT / "scripts/macos-source-quickstart.sh").read_text(encoding="utf-8")
    assert "missing_commands=()" in script
    assert "for command_name in cargo git node python3 pnpm rustc uv; do" in script
    assert "uv sync --project sdk/python --locked" in script
    assert "uv run --project sdk/python --locked pytest -q" in script
    assert "pnpm install --frozen-lockfile" in script
    assert "pnpm --dir web/console test" in script
    checked = subprocess.run(
        ["bash", "-n", str(ROOT / "scripts/macos-source-quickstart.sh")],
        check=False,
        capture_output=True,
        text=True,
    )
    assert checked.returncode == 0, checked.stderr


def test_fast_ci_primes_locked_rust_dependencies_before_offline_gate():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    fetch = workflow.index("cargo fetch --locked")
    offline_gate = workflow.index(
        "uv run --python 3.14.7 --no-project python scripts/gates.py fast --ci --offline"
    )

    assert fetch < offline_gate


def test_native_ci_provisions_exact_postgresql_before_manifest_cells():
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )
    gate = workflow.index(
        "uv run --python 3.14.7 --no-project python scripts/gates.py minimal-ci --ci --cell"
    )

    assert workflow.index("postgres:17.11-alpine@sha256:") < gate
    assert workflow.index("scripts/gates-install-postgres.ps1") < gate
    assert "WORLDSTREAM_POSTGRES_URL=" not in workflow
    windows = (ROOT / "scripts/gates-install-postgres.ps1").read_text(encoding="utf-8")
    hosted_provisioning = workflow + windows
    assert hosted_provisioning.count("WORLDSTREAM_POSTGRES_DSN_FILE=") == 2
    assert hosted_provisioning.count("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE=") == 2
    assert 'chmod 600 "$postgres_dsn_file"' in workflow
    assert "POSTGRES_PASSWORD_FILE=/run/secrets/postgres-password" in workflow


def test_windows_ci_installs_and_proves_byte_pinned_postgresql_17_11():
    script = (ROOT / "scripts/gates-install-postgres.ps1").read_text(encoding="utf-8")
    assert "postgresql-17.11-1-windows-x64-binaries.zip" in script
    assert "6eabdf00d2893713b75db4336a23c3fdf505f056e217ec6e2e95d901750cfea3" in script
    assert "https://get.enterprisedb.com/postgresql/" in script
    assert "--pwfile $BootstrapSecret" in script
    assert "postgres (PostgreSQL) $PostgresVersion" in script
    assert "'170011'" in script
    assert "WORLDSTREAM_POSTGRES_DSN_FILE=" in script
    assert "WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE=" in script
    assert "POSTGRES_PASSWORD=" not in script
    assert "--superpassword" not in script.lower()
    assert "function Protect-WorldstreamPath" in script
    assert '"*S-1-5-18:$Permission"' in script
    assert '"*S-1-5-32-544:$Permission"' in script
    assert "if ($Directory) { '(OI)(CI)(F)' } else { '(F)' }" in script
    assert "Protect-WorldstreamPath -Path $SecretRoot -Directory" in script
    assert "Protect-WorldstreamPath -Path $BootstrapSecret" in script
    assert "Protect-WorldstreamPath -Path $PostgresDsnFile" in script
    secret_root_dacl = script.index(
        "Protect-WorldstreamPath -Path $SecretRoot -Directory"
    )
    secret_write = script.index("[IO.File]::WriteAllText($BootstrapSecret")
    secret_file_dacl = script.index("Protect-WorldstreamPath -Path $BootstrapSecret")
    assert secret_root_dacl < secret_write < secret_file_dacl
    assert (
        "could not apply the protected owner/SYSTEM/Administrators PostgreSQL credential DACL"
        in script
    )


def test_release_workflow_uses_verified_safe_package_extraction():
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    assert "scripts/release-package-extract.py" in workflow
    assert "tar -xzf" not in workflow
    assert "reports/package-extraction.json" in workflow


def test_release_job_bootstraps_pinned_gate_toolchain_before_release_gate():
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    release = workflow[workflow.index("  release-evidence:") :]
    assert "astral-sh/setup-uv@" in release
    assert "uv python install 3.14.7" in release
    assert "rustup toolchain install 1.97.1" in release
    assert "actions/setup-node@" in release
    assert "corepack install" in release
    assert release.index("uv python install 3.14.7") < release.index(
        "scripts/gates.py release --ci --strict"
    )
    assert "id-token: write" in release
    assert "id-token: write" not in workflow[: workflow.index("  release-evidence:")]


def test_release_workflow_builds_only_supported_distribution_surfaces():
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    release = workflow[workflow.index("  release-evidence:") :]
    assert "--target linux-x86_64" in workflow
    assert "--target windows-x64" in workflow
    assert "--target source" in release
    assert (
        "docker/setup-buildx-action@37fe631027851001ddb9b187196cc803df7f5f0e"
        in workflow
    )
    assert "version: v0.36.1" in workflow
    assert "driver: docker-container" in workflow
    assert "docker buildx build" in workflow
    assert "--platform linux/amd64" in workflow
    assert "--load" in workflow
    assert "--provenance=false" in workflow
    assert (
        '--output "type=oci,dest=$artifact,compression=gzip,force-compression=true"'
        in workflow
    )
    assert '--build-arg "SOURCE_DATE_EPOCH=0"' in workflow
    assert "scripts/oci-runtime-smoke.sh" in workflow
    assert '--artifact "$artifact"' in workflow
    assert '--image "$image_tag"' in workflow
    assert "--skip-build" in workflow
    assert "--report reports/oci-runtime-smoke.json" in workflow
    assert "scripts/verify-release.sh dist" in release
    assert "release-evidence-assemble.sh" in release
    assert "release-supply-chain.py" in release
    assert "SLSA provenance" in release
    assert "sigstore/cosign-installer@" in release
    assert "cosign-release: v3.1.3" in release
    assert "cosign sign-blob --yes" in release
    assert "--bundle dist/sigstore.bundle.json" in release
    assert "dist/release-manifest.json" in release


def test_workflow_types_the_exact_dual_profile_oci_runtime_report():
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    native = workflow[workflow.index("  native:") : workflow.index("  macos-source:")]
    smoke = native.index("scripts/oci-runtime-smoke.sh")
    diagnostic = native.index("scripts/release-platform-diagnostic.py oci")
    producer = native.index("--report oci-linux/oci=")

    assert smoke < diagnostic < producer
    assert "--report reports/oci-runtime-smoke.json" in native[smoke:diagnostic]
    assert "--context dist/oci-context" in native[diagnostic:producer]
    assert "--runtime-report reports/oci-runtime-smoke.json" in native[diagnostic:]
    assert '--artifact "$artifact"' in native[smoke:diagnostic]
    assert "--skip-build" in native[smoke:diagnostic]


def test_local_postgresql_redacts_keyword_dsn_credentials(monkeypatch, capsys):
    gates = load_gates()
    dsn = "host=db.internal port=5432 dbname=worldstream user=operator password=SECRET"
    monkeypatch.setenv("WORLDSTREAM_POSTGRES_URL", dsn)
    monkeypatch.setattr(gates.shutil, "which", lambda name: "/usr/bin/psql")
    monkeypatch.setattr(
        gates.subprocess,
        "run",
        lambda argv, **kwargs: subprocess.CompletedProcess(
            argv, 0, stdout="", stderr=""
        ),
    )
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.local_postgresql(runner, required=True)

    output = capsys.readouterr().out
    assert "SECRET" not in output
    assert "password=SECRET" not in output
    assert "host=db.internal port=5432 dbname=worldstream" not in output
    assert "explicit local environment" in output
    redacted_uri = gates.redact_command_argument(
        "postgresql" + "://operator:" + "SECRET@db.internal:5432/worldstream"
    )
    assert "SECRET" not in redacted_uri
    assert "db.internal:5432/worldstream" in redacted_uri
    assert runner.outcomes[0].status == "PASS"


def test_local_postgresql_reads_owner_only_dsn_file_and_scopes_password(
    tmp_path, monkeypatch, capsys
):
    gates = load_gates()
    dsn_file = tmp_path / "postgres.dsn"
    dsn_file.write_text(
        "host=db.internal port=5432 dbname=worldstream user=operator password=SECRET\n",
        encoding="utf-8",
    )
    dsn_file.chmod(0o600)
    monkeypatch.setenv("WORLDSTREAM_POSTGRES_DSN_FILE", str(dsn_file))
    monkeypatch.delenv("WORLDSTREAM_POSTGRES_URL", raising=False)
    monkeypatch.setattr(gates.shutil, "which", lambda name: "/usr/bin/psql")
    observed = {}

    def fake_run(argv, **kwargs):
        observed["argv"] = argv
        observed["env"] = kwargs["env"]
        return subprocess.CompletedProcess(argv, 0, stdout="", stderr="")

    monkeypatch.setattr(gates.subprocess, "run", fake_run)
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.local_postgresql(runner, required=True)

    assert all("SECRET" not in argument for argument in observed["argv"])
    assert observed["env"]["PGPASSWORD"] == "SECRET"
    output = capsys.readouterr().out
    assert "SECRET" not in output
    assert "owner-only DSN file" in output
    assert runner.outcomes[0].status == "PASS"


def test_local_postgresql_rejects_group_readable_dsn_file(tmp_path, monkeypatch):
    if sys.platform == "win32":
        return
    gates = load_gates()
    dsn_file = tmp_path / "postgres.dsn"
    dsn_file.write_text("host=db.invalid dbname=worldstream\n", encoding="utf-8")
    dsn_file.chmod(0o640)
    monkeypatch.setenv("WORLDSTREAM_POSTGRES_DSN_FILE", str(dsn_file))
    monkeypatch.delenv("WORLDSTREAM_POSTGRES_URL", raising=False)
    runner = gates.GateRunner(strict=True, offline=True, ci=False)

    gates.local_postgresql(runner, required=True)

    assert runner.outcomes[0].status == "FAIL"
    assert "owner-only" in runner.outcomes[0].detail


def test_root_python_gate_uses_explicit_sorted_test_inventory(monkeypatch):
    gates = load_gates()
    monkeypatch.setattr(gates.shutil, "which", lambda name: "/usr/bin/uv")
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    commands = []
    monkeypatch.setattr(
        runner,
        "command",
        lambda name, argv, **_kwargs: commands.append((name, argv)) or True,
    )

    gates.root_python_checks(runner)

    test_command = next(argv for name, argv in commands if name == "root-python-tests")
    assert test_command[:7] == [
        "/usr/bin/uv",
        "run",
        "--project",
        "sdk/python",
        "--locked",
        "python",
        "-m",
    ]
    assert "tests/gate_smoke.py" in test_command
    assert test_command[-len(gates.ROOT_PYTHON_TESTS) :] == list(
        gates.ROOT_PYTHON_TESTS
    )


def test_posix_gate_launcher_falls_back_to_pinned_uv_python():
    script = (ROOT / "scripts/gates.sh").read_text(encoding="utf-8")
    assert "uv run --python 3.14.7 --no-project python" in script
    checked = subprocess.run(
        ["bash", "-n", str(ROOT / "scripts/gates.sh")],
        check=False,
        capture_output=True,
        text=True,
    )
    assert checked.returncode == 0, checked.stderr


def test_hosted_scanner_inventory_is_version_and_byte_pinned():
    gates = load_gates()
    linux = (ROOT / "scripts/gates-install-tools.sh").read_text(encoding="utf-8")
    windows = (ROOT / "scripts/gates-install-tools.ps1").read_text(encoding="utf-8")
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")

    assert gates.HOSTED_REQUIRED_TOOL_VERSIONS == {
        "cargo-audit": "0.22.2",
        "gitleaks": "8.29.1",
    }
    for version in gates.HOSTED_REQUIRED_TOOL_VERSIONS.values():
        assert version in linux
        assert version in windows
    assert "e4eb209d04e20339d77122a3bdf9cd41351255cfb27ebcb75e85325e04f88924" in linux
    assert "ab28a1bdb54db4d5d8ad5981cf1f959410370b3d28250dbd35f6a44248620e39" in linux
    assert "e4b7d556f0cddbe23d10d8fac2ab0f29f68f019091c6599ffbeaa8a4fb71ac78" in windows
    assert "0a7316540862c13d954f648917ceacca593747baed6eec180fafa590be2710ab" in windows
    assert workflow.count("scripts/gates-install-tools.sh") >= 3
    assert "scripts/gates-install-tools.ps1" in workflow


def test_secret_scan_includes_untracked_candidate_files(tmp_path, monkeypatch):
    gates = load_gates()
    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True)
    (tmp_path / "tracked.txt").write_text("ordinary source\n", encoding="utf-8")
    subprocess.run(["git", "-C", str(tmp_path), "add", "tracked.txt"], check=True)
    credential = "postgresql" + "://operator:" + "candidate@db.invalid/example\n"
    (tmp_path / "candidate.txt").write_text(credential, encoding="utf-8")
    monkeypatch.setattr(gates, "ROOT", tmp_path)
    monkeypatch.setattr(gates.shutil, "which", lambda _name: None)
    runner = gates.GateRunner(strict=False, offline=True, ci=False)

    gates.secret_scan(runner)

    secret = next(
        outcome for outcome in runner.outcomes if outcome.name == "secret-scan"
    )
    assert secret.status == "FAIL"
    assert "candidate.txt" in secret.detail


def test_detached_release_identity_requires_explicit_locations():
    gates = load_gates()
    manifest = {
        "release_artifacts": [
            {
                "id": artifact_id,
                "profile": profile,
                "digest_algorithm": "sha256",
                "digest": "",
                "status": "detached",
                "digest_location": "release-manifest.json",
            }
            for artifact_id, profile in gates.RELEASE_ARTIFACT_PROFILES.items()
        ],
        "evidence": [
            {
                "id": "evidence-a",
                "release_gate": True,
                "status": "detached",
                "artifact_digest": "",
                "artifact_digest_location": "release-manifest.json",
            }
        ],
    }

    assert gates.uses_detached_release_identity(manifest)
    failures, incompletes = gates.release_artifact_inventory_diagnostics(
        manifest, release=True
    )
    assert failures == []
    assert incompletes == []

    manifest["release_artifacts"][0]["digest_location"] = "compatibility.toml"
    assert not gates.uses_detached_release_identity(manifest)
    failures, _ = gates.release_artifact_inventory_diagnostics(manifest, release=True)
    assert any(
        "detached release artifact identity is not enabled" in error
        for error in failures
    )


def test_detached_inventory_binds_exact_artifact_and_evidence_maps(tmp_path):
    gates = load_gates()
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    artifact_ids = set(gates.RELEASE_ARTIFACT_PROFILES)
    metadata = {
        "schema": gates.DETACHED_RELEASE_MANIFEST_SCHEMA,
        "product": "0.1.0",
        "source_version": "0.1.0",
        "manifest": {
            "source": "compatibility.toml",
            "mirror": "compatibility.json",
            "sha256": gates.hashlib.sha256(gates.MIRROR_PATH.read_bytes()).hexdigest(),
        },
        "artifacts": {
            artifact_id: f"artifacts/{artifact_id}.bin" for artifact_id in artifact_ids
        },
        "artifact_digests": {
            artifact_id: "sha256:" + "1" * 64
            for artifact_id in artifact_ids
            if artifact_id != "sigstore-bundle"
        },
        "evidence": {"evidence-a": "evidence/evidence-a.json"},
        "evidence_digests": {"evidence-a": "sha256:" + "2" * 64},
        "verification_material": {
            "sigstore-bundle": {"path": "artifacts/sigstore-bundle.bin"}
        },
    }
    manifest = {
        "release_candidate": "0.1.0",
        "release_artifacts": [
            {
                "id": artifact_id,
                "profile": profile,
                "digest_algorithm": "sha256",
                "digest": "",
                "status": "detached",
                "digest_location": "release-manifest.json",
            }
            for artifact_id, profile in gates.RELEASE_ARTIFACT_PROFILES.items()
        ],
        "evidence": [
            {
                "id": "evidence-a",
                "release_gate": True,
                "status": "detached",
                "artifact_digest": "",
                "artifact_digest_location": "release-manifest.json",
            }
        ],
    }

    assert gates.detached_release_inventory(runner, metadata, manifest) is not None
    assert runner.outcomes[-1].name == "release-detached-inventory"
    assert runner.outcomes[-1].status == "PASS"

    metadata["evidence_digests"]["unexpected"] = "sha256:" + "3" * 64
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    assert gates.detached_release_inventory(runner, metadata, manifest) is None
    assert any(
        outcome.name == "release-detached-inventory" and outcome.status == "FAIL"
        for outcome in runner.outcomes
    )


def test_spdx_subject_identity_requires_exact_detached_subjects():
    gates = load_gates()
    subjects = {
        "worldstream-0.1.0-source.tar.gz": "sha256:" + "a" * 64,
        "evidence/evidence-a.json": "sha256:" + "b" * 64,
    }
    valid = {
        "files": [
            {
                "fileName": path,
                "checksums": [
                    {
                        "algorithm": "SHA256",
                        "checksumValue": digest.removeprefix("sha256:"),
                    }
                ],
            }
            for path, digest in subjects.items()
        ]
    }
    assert gates.spdx_subject_identity_error(valid, subjects) is None
    valid["files"][0]["checksums"][0]["checksumValue"] = "0" * 64
    assert "mismatch" in gates.spdx_subject_identity_error(valid, subjects)


def test_sigstore_is_verification_material_not_a_signed_subject():
    gates = load_gates()
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    artifact_ids = set(gates.RELEASE_ARTIFACT_PROFILES)
    metadata = {
        "schema": gates.DETACHED_RELEASE_MANIFEST_SCHEMA,
        "product": "0.1.0",
        "source_version": "0.1.0",
        "manifest": {
            "source": "compatibility.toml",
            "mirror": "compatibility.json",
            "sha256": gates.hashlib.sha256(gates.MIRROR_PATH.read_bytes()).hexdigest(),
        },
        "artifacts": {
            artifact_id: f"artifacts/{artifact_id}.bin" for artifact_id in artifact_ids
        },
        "artifact_digests": {
            artifact_id: "sha256:" + "1" * 64
            for artifact_id in artifact_ids
            if artifact_id != "sigstore-bundle"
        },
        "evidence": {"evidence-a": "evidence/evidence-a.json"},
        "evidence_digests": {"evidence-a": "sha256:" + "3" * 64},
        "verification_material": {
            "sigstore-bundle": {"path": "artifacts/sigstore-bundle.bin"}
        },
    }
    manifest = {
        "release_candidate": "0.1.0",
        "release_artifacts": [
            {
                "id": artifact_id,
                "profile": profile,
                "digest_algorithm": "sha256",
                "digest": "",
                "status": "detached",
                "digest_location": "release-manifest.json",
            }
            for artifact_id, profile in gates.RELEASE_ARTIFACT_PROFILES.items()
        ],
        "evidence": [
            {
                "id": "evidence-a",
                "release_gate": True,
                "status": "detached",
                "artifact_digest": "",
                "artifact_digest_location": "release-manifest.json",
            }
        ],
    }

    result = gates.detached_release_inventory(runner, metadata, manifest)
    assert result is not None
    assert "sigstore-bundle" not in result[1]
    assert runner.outcomes[-1].status == "PASS"

    metadata["artifact_digests"]["sigstore-bundle"] = "sha256:" + "2" * 64
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    assert gates.detached_release_inventory(runner, metadata, manifest) is None
    assert any(
        "Sigstore bundle must be path-only verification material" in outcome.detail
        for outcome in runner.outcomes
    )

    metadata["artifact_digests"].pop("sigstore-bundle")
    metadata["verification_material"] = {
        "sigstore-bundle": {"digest": "sha256:" + "4" * 64}
    }
    runner = gates.GateRunner(strict=True, offline=True, ci=False)
    assert gates.detached_release_inventory(runner, metadata, manifest) is None
    assert any(
        "Sigstore bundle must be path-only verification material" in outcome.detail
        for outcome in runner.outcomes
    )

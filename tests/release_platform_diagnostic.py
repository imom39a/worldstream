"""Focused validation for typed hosted platform diagnostic emitters."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/release-platform-diagnostic.py"
PLATFORM_PRODUCER = ROOT / "scripts/release-evidence-produce-platform.py"


def load_module():
    spec = importlib.util.spec_from_file_location(
        "worldstream_release_platform_diagnostic", SCRIPT
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def load_platform_producer():
    spec = importlib.util.spec_from_file_location(
        "worldstream_release_evidence_produce_platform", PLATFORM_PRODUCER
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_json(path: Path, value: object) -> Path:
    path.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    return path


def gate_report(system: str, outcomes: set[str]) -> dict:
    return {
        "schema": "worldstream/compatibility-gate-report/v1",
        "platform": f"{system}-hosted-runner",
        "tier": "minimal-ci",
        "strict": True,
        "outcomes": [
            {
                "name": name,
                "status": "PASS",
                "detail": f"native-{system.lower()} exact typed gate passed",
                "classification": "pass",
            }
            for name in sorted(outcomes)
        ],
    }


def native_args(tmp_path: Path, module, source: str) -> argparse.Namespace:
    system = "Linux" if source == "native-linux" else "Windows"
    target = "linux-x86_64" if source == "native-linux" else "windows-x64"
    version = module.manifest()["release_candidate"]
    artifact = tmp_path / f"worldstream-{version}-{target}.archive"
    artifact.write_bytes(b"exact packaged native archive")
    manifest_json_sha = "1" * 64
    manifest_toml_sha = "2" * 64
    package_report = {
        "schema": "worldstream/package-report/v1",
        "kind": "archive",
        "artifact": artifact.name,
        "path": artifact.name,
        "sha256": module.digest(artifact),
        "size_bytes": artifact.stat().st_size,
        "identity": {
            "target": target,
            "version": version,
            "manifest_sha256": manifest_json_sha,
            "manifest_json_sha256": manifest_json_sha,
            "manifest_toml_sha256": manifest_toml_sha,
        },
        "inventory": {
            "archive_verified": True,
            "manifest_source": "compatibility.toml",
            "manifest_mirror": "compatibility.json",
            "release_evidence": False,
        },
    }
    package_report_path = write_json(tmp_path / "package-report.json", package_report)
    runtime_required = {
        "source-version-drift",
        "compatibility-manifest-verify",
        "rust-tests",
        "sqlite-critical-matrix",
        "postgres-local",
        "evidence-backup-restore-local",
        "evidence-privacy-local",
        "evidence-capability-local",
        "evidence-lease-local",
        "evidence-telemetry-local",
        "filesystem-owner-only",
        "evidence-filesystem-local",
        "ci-cell",
    }
    if source == "native-linux":
        runtime_required |= {
            "postgres-live-contract",
            "process-kill-point",
            "telemetry-failure-pressure",
            "bounded-sqlite-soak",
            "filesystem-link-policy",
        }
    else:
        runtime_required |= {"filesystem-acl-policy"}
    runtime_report = {
        "schema": "worldstream/native-package-runtime-smoke/v1",
        "status": "pass",
        "release_evidence": False,
        "secrets_emitted": False,
        "platform": {"system": system, "machine": "x86_64"},
        "package_binding": {
            "artifact": artifact.name,
            "target": target,
            "version": version,
            "archive_sha256": module.digest(artifact),
            "archive_size_bytes": artifact.stat().st_size,
            "package_report_sha256": module.digest(package_report_path),
            "manifest_sha256": "sha256:" + manifest_json_sha,
            "manifest_json_sha256": "sha256:" + manifest_json_sha,
            "manifest_toml_sha256": "sha256:" + manifest_toml_sha,
            "worldstreamd_sha256": "sha256:" + "3" * 64,
            "worldstreamd_size_bytes": 100,
            "worldstreamctl_sha256": "sha256:" + "4" * 64,
            "worldstreamctl_size_bytes": 101,
            "canonical_archive_verified": True,
            "exact_archive_bytes_executed": True,
        },
        "profiles": {
            "sqlite-bundled": {
                "status": "pass",
                "healthz": "pass",
                "readyz": "pass",
                "version": "manifest_and_engine_exact",
                "engine_identity": "sqlite/3.53.4; journal_mode=WAL; synchronous=FULL",
                "packaged_ctl": {
                    "config_validate": "pass",
                    "config_effective": "redacted_and_precedence_exact",
                    "doctor": "bounded_diagnostics_exposed",
                    "health": "pass",
                    "version": "pass",
                    "binary_version": version,
                },
            },
            "postgres-primary": {
                "status": "pass",
                "healthz": "pass",
                "readyz": "pass",
                "version": "manifest_and_engine_exact",
                "engine_identity": "postgresql/17.11; server_version_num=170011",
                "packaged_ctl": {
                    "config_validate": "pass",
                    "config_effective": "redacted_and_precedence_exact",
                    "doctor": "bounded_diagnostics_exposed",
                    "health": "pass",
                    "version": "pass",
                    "binary_version": version,
                },
            },
        },
        "postgres_admin": {
            "server_version_num": "170011",
            "dsn_delivery": "owner_only_file",
            "migrate": "pass",
            "verify": "pass",
            "runtime_role": "least_privilege",
        },
        "cleanup": "pass",
    }
    return argparse.Namespace(
        source=source,
        package_report=package_report_path,
        gate_report=write_json(
            tmp_path / "gate.json", gate_report(system, runtime_required)
        ),
        runtime_report=write_json(tmp_path / "runtime.json", runtime_report),
        artifact=artifact,
        output_dir=tmp_path / "diagnostics",
    )


def stub_native_archive_verification(monkeypatch, module, args):
    package_report = json.loads(args.package_report.read_text(encoding="utf-8"))
    runtime_report = json.loads(args.runtime_report.read_text(encoding="utf-8"))
    binding = runtime_report["package_binding"]
    archive_binaries = {
        key: binding[key]
        for key in (
            "worldstreamd_sha256",
            "worldstreamd_size_bytes",
            "worldstreamctl_sha256",
            "worldstreamctl_size_bytes",
        )
    }
    monkeypatch.setattr(
        module,
        "independently_verify_native_archive",
        lambda artifact, source_id, version: (package_report, archive_binaries),
    )


@pytest.mark.parametrize("source", ["native-linux", "native-windows"])
def test_native_diagnostic_requires_exact_packaged_binary_runtime_profiles(
    tmp_path, monkeypatch, source
):
    module = load_module()
    args = native_args(tmp_path, module, source)
    stub_native_archive_verification(monkeypatch, module, args)

    module.emit_native(args)

    runtime = json.loads(
        (args.output_dir / f"{source}-runtime.json").read_text(encoding="utf-8")
    )
    assert runtime["facts"]["runtime_report_sha256"] == module.digest(
        args.runtime_report
    )
    assert runtime["facts"]["packaged_binary_sha256"] == "sha256:" + "3" * 64
    assert runtime["facts"]["packaged_control_sha256"] == "sha256:" + "4" * 64
    assert "migrate/verify" in runtime["facts"]["postgres_admin"]


@pytest.mark.parametrize(
    ("tamper", "message"),
    [
        ("archive", "exact archive"),
        ("binary", "exact archive"),
        ("machine", "runtime smoke is incomplete"),
        ("profile", "both storage profiles"),
        ("health", "contract is incomplete"),
        ("admin", "administration contract"),
    ],
)
def test_native_diagnostic_rejects_unbound_or_partial_runtime(
    tmp_path, monkeypatch, tamper, message
):
    module = load_module()
    args = native_args(tmp_path, module, "native-linux")
    stub_native_archive_verification(monkeypatch, module, args)
    runtime = json.loads(args.runtime_report.read_text(encoding="utf-8"))
    if tamper == "archive":
        runtime["package_binding"]["archive_sha256"] = "sha256:" + "0" * 64
    elif tamper == "binary":
        runtime["package_binding"]["worldstreamd_sha256"] = "sha256:" + "0" * 64
    elif tamper == "machine":
        runtime["platform"]["machine"] = "aarch64"
    elif tamper == "profile":
        del runtime["profiles"]["postgres-primary"]
    elif tamper == "health":
        runtime["profiles"]["postgres-primary"]["healthz"] = "not_run"
    else:
        runtime["postgres_admin"]["verify"] = "not_run"
    write_json(args.runtime_report, runtime)

    with pytest.raises(module.DiagnosticError, match=message):
        module.emit_native(args)


@pytest.mark.parametrize("source", ["native-linux", "native-windows"])
def test_native_diagnostic_rejects_arbitrary_non_archive_bytes(tmp_path, source):
    module = load_module()
    args = native_args(tmp_path, module, source)

    with pytest.raises(
        module.DiagnosticError, match="native archive verification failed"
    ):
        module.emit_native(args)


def https_report(module) -> dict:
    return {
        "schema": "worldstream/telemetry-https-evidence/v1",
        "status": "passed",
        "release_evidence": False,
        "evidence_class": "local_transport_integration",
        "platform": {"system": "Linux", "machine": "x86_64"},
        "timeout_seconds_per_test": 300,
        "checks": dict(module.TELEMETRY_HTTPS_CHECKS),
        "tests": [
            {
                "id": test_id,
                "package": package,
                "filter": test_filter,
                "passed_count": 1,
            }
            for test_id, (package, test_filter) in module.TELEMETRY_HTTPS_TESTS.items()
        ],
    }


def macos_report(module, architecture: str) -> dict:
    sha = "sha256:" + "a" * 64
    browser_checks = {
        "browser_identity_verified": True,
        "catch_up_or_reset_installed": True,
        "embedded_ui_loaded": True,
        "final_reveal_dom_visible": True,
        "new_session_resynchronized": True,
        "package_bound_reference_clients": False,
        "package_bound_runtime": False,
        "precomplete_reveal_locked": True,
        "privacy_negative_dom_and_browser_channels": True,
        "replay_hashes_verified": True,
        "six_phase_story_complete": True,
        "stale_head_rejected": True,
        "typed_actions_accepted_in_dom": True,
    }
    return {
        "schema": "worldstream/macos-source-quickstart/v2",
        "status": "passed",
        "release_evidence": False,
        "signed_or_notarized_binary": False,
        "version": module.manifest()["release_candidate"],
        "platform": {
            "system": "Darwin",
            "version": "15.6.1",
            "filesystem": "apfs",
            "machine": architecture,
        },
        "toolchains": {
            **module.pinned_macos_toolchains(),
        },
        "source_revision": "1" * 40,
        "elapsed_seconds": 500,
        "browser_story": {
            "schema": "worldstream/package-browser-heist/v1",
            "canonical_encoding": "utf8-sorted-key-compact-json-lf",
            "status": "pass",
            "release_evidence": False,
            "source_mode": "source-build",
            "elapsed_ms": 12_000,
            "browser": module.MACOS_BROWSER_IDENTITIES.get(
                architecture, module.MACOS_BROWSER_IDENTITIES["arm64"]
            ),
            "tools": {
                "adapter": {
                    "name": "worldstream-cdp-browser",
                    "protocol": "Chrome DevTools Protocol",
                    "sha256": module.digest(ROOT / "scripts/cdp-browser.py"),
                    "size_bytes": (ROOT / "scripts/cdp-browser.py").stat().st_size,
                },
                "python": {"implementation": "cpython", "version": "3.14.7"},
            },
            "checks": browser_checks,
            "runtime": {
                "worldstreamd": {
                    "origin": "source-build:target/debug/worldstreamd",
                    "sha256": sha,
                    "size_bytes": 10,
                },
                "ui": {
                    "origin": "source-build:web/console/dist",
                    "tree_sha256": sha,
                    "index_sha256": sha,
                    "file_count": 2,
                    "total_bytes": 10,
                },
                "sdk": {
                    "origin": "source:sdk/python/src",
                    "tree_sha256": sha,
                    "file_count": 2,
                    "total_bytes": 10,
                },
                "heist_reference_clients": {
                    "origin": "source:examples/heist",
                    "tree_sha256": sha,
                    "file_count": 4,
                    "total_bytes": 10,
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
                "final_replay": {
                    "verified": True,
                    "hash_parity": {"verified": True},
                },
            },
            "dom_evidence": {
                key: sha
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
                key: sha
                for key in (
                    "inspect_clue",
                    "publish_clue",
                    "propose_plan",
                    "commit_move",
                    "acknowledge_result",
                )
            },
            "privacy": {
                "status": "pass",
                "private_canary_absent": True,
                "credentials_absent": True,
                "private_claim_absent_from_retained_evidence": True,
            },
        },
        "checks": {
            "complete_heist": True,
            "embedded_ui": True,
            "pinned_toolchain": True,
            "privacy": True,
            "real_browser": True,
            "replay": True,
            "source_revision": True,
            "source_build": True,
            "stale_resync": True,
            "quickstart": True,
        },
    }


def macos_args(tmp_path: Path, module, architectures: list[str]) -> argparse.Namespace:
    return argparse.Namespace(
        quickstart_report=[
            write_json(
                tmp_path / f"macos-{index}-{architecture}.json",
                macos_report(module, architecture),
            )
            for index, architecture in enumerate(architectures)
        ],
        source_revision="1" * 40,
        artifact_output=tmp_path / "macos-source-matrix.json",
        output_dir=tmp_path / "diagnostics",
    )


def test_macos_diagnostic_requires_and_binds_both_supported_architectures(tmp_path):
    module = load_module()
    args = macos_args(tmp_path, module, ["x86_64", "arm64"])

    module.emit_macos(args)

    artifact = json.loads(args.artifact_output.read_text(encoding="utf-8"))
    assert artifact["schema"] == "worldstream/macos-source-matrix/v1"
    assert artifact["architectures"] == ["arm64", "x86_64"]
    assert artifact["source_revision"] == "1" * 40
    assert artifact["toolchains"] == module.pinned_macos_toolchains()
    assert [report["architecture"] for report in artifact["quickstart_reports"]] == [
        "arm64",
        "x86_64",
    ]
    assert all(
        report["sha256"].startswith("sha256:")
        for report in artifact["quickstart_reports"]
    )
    diagnostic = json.loads(
        (args.output_dir / "macos-source-source-quickstart.json").read_text(
            encoding="utf-8"
        )
    )
    assert diagnostic["facts"]["architectures"] == ["arm64", "x86_64"]
    assert diagnostic["facts"]["source_revision"] == "1" * 40
    assert "arm64 and x86_64" in diagnostic["facts"]["quickstart"]


@pytest.mark.parametrize(
    ("architectures", "message"),
    [
        (["arm64"], "exactly arm64 and x86_64"),
        (["arm64", "arm64"], "duplicate"),
        (["arm64", "i386"], "unexpected architecture"),
    ],
)
def test_macos_diagnostic_rejects_missing_duplicate_or_unexpected_architecture(
    tmp_path, architectures, message
):
    module = load_module()
    args = macos_args(tmp_path, module, architectures)

    with pytest.raises(module.DiagnosticError, match=message):
        module.emit_macos(args)


@pytest.mark.parametrize("toolchain", ["rust", "python", "node", "pnpm", "uv"])
def test_macos_diagnostic_rejects_every_noncanonical_toolchain_pin(tmp_path, toolchain):
    module = load_module()
    args = macos_args(tmp_path, module, ["arm64", "x86_64"])
    report_path = args.quickstart_report[0]
    report = json.loads(report_path.read_text(encoding="utf-8"))
    report["toolchains"][toolchain] = "0.0.0"
    write_json(report_path, report)

    with pytest.raises(module.DiagnosticError, match="incomplete"):
        module.emit_macos(args)


@pytest.mark.parametrize(
    ("path", "value"),
    [
        (("browser_story", "checks", "stale_head_rejected"), False),
        (("browser_story", "checks", "package_bound_runtime"), True),
        (("browser_story", "browser", "sha256"), "sha256:" + "0" * 64),
        (("elapsed_seconds",), 600),
    ],
)
def test_macos_diagnostic_rejects_incomplete_browser_story(tmp_path, path, value):
    module = load_module()
    args = macos_args(tmp_path, module, ["arm64", "x86_64"])
    report_path = args.quickstart_report[0]
    report = json.loads(report_path.read_text(encoding="utf-8"))
    target = report
    for key in path[:-1]:
        target = target[key]
    target[path[-1]] = value
    write_json(report_path, report)

    with pytest.raises(module.DiagnosticError):
        module.emit_macos(args)


@pytest.mark.parametrize(
    "package_manager",
    [
        "pnpm@11.19.0",
        "pnpm@11.19.0+sha512." + "0" * 127,
        "pnpm@11.19.0+sha256." + "0" * 64,
    ],
)
def test_macos_toolchain_pin_requires_exact_pnpm_tarball_integrity(
    tmp_path, package_manager
):
    module = load_module()
    for relative in (
        "rust-toolchain.toml",
        ".python-version",
        ".node-version",
        ".uv-version",
    ):
        (tmp_path / relative).write_bytes((ROOT / relative).read_bytes())
    package = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))
    package["packageManager"] = package_manager
    write_json(tmp_path / "package.json", package)
    module.ROOT = tmp_path

    with pytest.raises(module.DiagnosticError, match="integrity-bound pnpm pin"):
        module.pinned_macos_toolchains()


def test_macos_diagnostic_rejects_wrong_or_mismatched_source_revision(tmp_path):
    module = load_module()
    args = macos_args(tmp_path, module, ["arm64", "x86_64"])
    args.source_revision = "not-a-commit"
    with pytest.raises(module.DiagnosticError, match="40-hex"):
        module.emit_macos(args)

    mismatch_root = tmp_path / "mismatch"
    mismatch_root.mkdir()
    args = macos_args(mismatch_root, module, ["arm64", "x86_64"])
    report_path = args.quickstart_report[0]
    report = json.loads(report_path.read_text(encoding="utf-8"))
    report["source_revision"] = "2" * 40
    write_json(report_path, report)
    with pytest.raises(module.DiagnosticError, match="incomplete"):
        module.emit_macos(args)


def security_args(tmp_path: Path, module, report: dict) -> argparse.Namespace:
    common = {
        "secret-scan",
        "evidence-privacy-local",
        "evidence-capability-local",
        "evidence-lease-local",
        "evidence-telemetry-local",
        "filesystem-owner-only",
        "evidence-filesystem-local",
        "config-contract",
    }
    runtime = {
        "schema": "worldstream/native-package-runtime-smoke/v1",
        "status": "pass",
        "release_evidence": False,
        "profiles": {
            profile: {
                "packaged_ctl": {
                    "config_validate": "pass",
                    "config_effective": "redacted_and_precedence_exact",
                    "doctor": "bounded_diagnostics_exposed",
                }
            }
            for profile in ("sqlite-bundled", "postgres-primary")
        },
    }
    linux_runtime = {**runtime, "platform": {"system": "Linux"}}
    windows_runtime = {**runtime, "platform": {"system": "Windows"}}
    return argparse.Namespace(
        linux_gate_report=write_json(
            tmp_path / "linux.json",
            gate_report("Linux", common | {"telemetry-failure-pressure"}),
        ),
        windows_gate_report=write_json(
            tmp_path / "windows.json",
            gate_report("Windows", common | {"filesystem-acl-policy"}),
        ),
        linux_runtime_report=write_json(tmp_path / "linux-runtime.json", linux_runtime),
        windows_runtime_report=write_json(
            tmp_path / "windows-runtime.json", windows_runtime
        ),
        https_report=write_json(tmp_path / "https.json", report),
        artifact_output=tmp_path / "security-profile.json",
        output_dir=tmp_path / "diagnostics",
    )


def test_security_diagnostic_requires_and_binds_exact_https_transport_report(tmp_path):
    module = load_module()
    args = security_args(tmp_path, module, https_report(module))

    module.emit_security(args)

    artifact = json.loads(args.artifact_output.read_text(encoding="utf-8"))
    assert artifact["status"] == "passed"
    assert artifact["release_evidence"] is True
    assert [item["platform"] for item in artifact["platform_reports"]] == [
        "native-linux-x86_64",
        "native-windows-x64",
        "native-linux-x86_64/telemetry-https",
        "native-linux-x86_64/config-contract",
        "native-windows-x64/config-contract",
    ]
    diagnostic = json.loads(
        (args.output_dir / "security-observability-security.json").read_text(
            encoding="utf-8"
        )
    )
    remote_tls = diagnostic["facts"]["remote_tls"]
    assert "trusted-CA" in remote_tls
    assert "hostname mismatch" in remote_tls
    assert "no provider credentials" not in remote_tls


@pytest.mark.parametrize("tamper", ["check", "filter", "count"])
def test_security_diagnostic_rejects_incomplete_https_claim(tmp_path, tamper):
    module = load_module()
    report = https_report(module)
    if tamper == "check":
        report["checks"]["hostname_verification"] = False
    elif tamper == "filter":
        report["tests"][0]["filter"] = "generic_exit_code_only"
    else:
        report["tests"][0]["passed_count"] = 0
    args = security_args(tmp_path, module, report)

    with pytest.raises(module.DiagnosticError, match="telemetry HTTPS"):
        module.emit_security(args)


@pytest.mark.parametrize("system", ["linux", "windows"])
def test_security_diagnostic_rejects_missing_packaged_config_command(tmp_path, system):
    module = load_module()
    args = security_args(tmp_path, module, https_report(module))
    path = getattr(args, f"{system}_runtime_report")
    runtime = json.loads(path.read_text(encoding="utf-8"))
    runtime["profiles"]["postgres-primary"]["packaged_ctl"]["doctor"] = "not-run"
    write_json(path, runtime)

    with pytest.raises(module.DiagnosticError, match="config/effective/doctor"):
        module.emit_security(args)


def test_oci_diagnostic_requires_binary_healthcheck_to_fail_without_daemon(
    tmp_path, monkeypatch
):
    module = load_module()
    actual_context_verifier = module.independently_verify_oci_context
    actual_archive_verifier = module.independently_verify_oci_archive
    sqlite = module.manifest()["storage"]["sqlite"]
    sqlite_identity = f"sqlite/{sqlite['version']}; source_id={sqlite['source_id']}"
    context = {
        "schema": "worldstream/oci-context-report/v1",
        "kind": "oci-context",
        "verified": True,
        "release_evidence": False,
        "sha256": "sha256:" + "d" * 64,
    }
    runtime = {
        "schema": "worldstream/oci-runtime-smoke/v1",
        "status": "PASS",
        "release_evidence": False,
        "health": "healthy",
        "healthcheck_without_daemon": "rejected",
        "read_only_root": True,
        "non_root": "65532:65532",
        "persistent_volume": "/var/lib/worldstream",
        "sqlite_volume": {
            "path": "/var/lib/worldstream",
            "type": "docker-volume",
            "driver": "local",
            "scope": "local",
            "driver_options": {},
            "mount_device": "254:1",
            "filesystem": "ext4",
            "mount_source": "/dev/vda1",
            "locality": "local-block-device",
        },
        "authority_secret_source": "owner-readable-read-only-volume-file",
        "standalone_config": "valid",
        "rejected_layouts": [
            "tmpfs",
            "wrong-data-directory",
            "network-configured-volume",
        ],
        "profiles": {
            "sqlite-bundled": {
                "status": "pass",
                "health": "healthy",
                "filesystem": "ext4-or-xfs-explicit-volume",
                "engine_identity": sqlite_identity,
                "healthz": "pass",
                "readyz": "pass",
                "version": "pass",
            },
            "postgres-primary": {
                "status": "pass",
                "provider_image": module.OCI_POSTGRES_IMAGE,
                "engine_identity": module.OCI_POSTGRES_IDENTITY,
                "packaged_admin_migrate": "pass",
                "packaged_admin_verify": "pass",
                "runtime_role_least_privilege": True,
                "healthz": "pass",
                "readyz": "pass",
                "version": "pass",
                "network": "disabled-unix-socket",
            },
        },
        "secrets_emitted": False,
    }
    metadata = {
        "base_image": "alpine@sha256:" + "a" * 64,
        "version": module.manifest()["release_candidate"],
        "profile": "oci-linux-amd64",
        "target": "linux/amd64",
    }
    context_dir = tmp_path / "oci-context"
    context_dir.mkdir()
    context_metadata = write_json(context_dir / "oci-metadata.json", metadata)
    artifact = tmp_path / "image.oci.tar"
    artifact.write_bytes(b"oci image fixture")
    artifact_sha256 = "sha256:" + hashlib.sha256(artifact.read_bytes()).hexdigest()
    image_config_digest = "sha256:" + "c" * 64
    runtime["artifact_binding"] = {
        "status": "pass",
        "artifact": artifact.name,
        "artifact_sha256": artifact_sha256,
        "artifact_size_bytes": artifact.stat().st_size,
        "artifact_image_manifest_digest": "sha256:" + "b" * 64,
        "artifact_image_config_digest": image_config_digest,
        "tested_image_config_digest": image_config_digest,
        "layer_count": 3,
        "layer_descriptors_bound": True,
        "rootfs_diff_ids_bound": True,
        "closed_blob_inventory": True,
    }
    runtime["secret_scan"] = {
        "schema": "worldstream/secret-absence-scan/v1",
        "status": "pass",
        "secrets_emitted": False,
        "sentinel_sha256": "sha256:" + "e" * 64,
        "encodings_scanned": ["base64", "base64url", "hex", "raw"],
        "channels": [
            {
                "channel": channel,
                "sha256": "sha256:" + "f" * 64,
                "size_bytes": 1,
            }
            for channel in sorted(module.OCI_SECRET_SCAN_CHANNELS)
        ],
    }
    monkeypatch.setattr(
        module, "independently_verify_oci_context", lambda candidate: context
    )
    monkeypatch.setattr(
        module,
        "independently_verify_oci_archive",
        lambda candidate, tested_digest: runtime["artifact_binding"],
    )
    args = argparse.Namespace(
        context=context_dir,
        context_report=write_json(tmp_path / "context.json", context),
        runtime_report=write_json(tmp_path / "runtime.json", runtime),
        context_metadata=context_metadata,
        artifact=artifact,
        output_dir=tmp_path / "diagnostics",
    )

    module.emit_oci(args)
    diagnostic = json.loads(
        (args.output_dir / "oci-linux-oci.json").read_text(encoding="utf-8")
    )
    facts = diagnostic["facts"]
    assert facts["storage_profiles"] == "sqlite-bundled and postgres-primary passed"
    assert facts["postgres_provider_image"] == module.OCI_POSTGRES_IMAGE
    assert facts["postgres_engine_identity"] == module.OCI_POSTGRES_IDENTITY
    assert facts["postgres_packaged_administration"] == "migrate and verify passed"
    assert facts["postgres_runtime_role"] == "least privilege verified"
    assert facts["postgres_network"] == "disabled network with local Unix socket"
    assert facts["secrets"] == "not emitted"
    assert "local block source" in facts["filesystem_policy"]
    assert facts["context_report_sha256"] == module.digest(args.context_report)
    assert facts["context_metadata_sha256"] == module.digest(args.context_metadata)
    assert facts["runtime_report_sha256"] == module.digest(args.runtime_report)
    producer = load_platform_producer()
    producer_manifest = producer.load_manifest(
        ROOT / "compatibility.toml", ROOT / "compatibility.json"
    )
    assert (
        producer.read_report(
            args.output_dir / "oci-linux-oci.json",
            "oci-linux",
            "oci",
            producer_manifest,
        )
        == diagnostic
    )

    monkeypatch.setattr(
        module, "independently_verify_oci_context", actual_context_verifier
    )
    with pytest.raises(module.DiagnosticError, match="context verification failed"):
        module.emit_oci(args)
    monkeypatch.setattr(
        module, "independently_verify_oci_context", lambda candidate: context
    )

    monkeypatch.setattr(
        module, "independently_verify_oci_archive", actual_archive_verifier
    )
    with pytest.raises(module.DiagnosticError, match="archive verification failed"):
        module.emit_oci(args)
    monkeypatch.setattr(
        module,
        "independently_verify_oci_archive",
        lambda candidate, tested_digest: runtime["artifact_binding"],
    )

    write_json(args.context_report, {**context, "verified": False})
    with pytest.raises(module.DiagnosticError, match="exact independently verified"):
        module.emit_oci(args)
    write_json(args.context_report, context)

    external_metadata = write_json(tmp_path / "metadata-copy.json", metadata)
    args.context_metadata = external_metadata
    with pytest.raises(module.DiagnosticError, match="exact metadata"):
        module.emit_oci(args)
    args.context_metadata = context_metadata

    write_json(context_metadata, {"base_image": metadata["base_image"]})
    with pytest.raises(module.DiagnosticError, match="metadata identity"):
        module.emit_oci(args)
    write_json(context_metadata, metadata)

    for required in (
        "healthcheck_without_daemon",
        "authority_secret_source",
        "standalone_config",
        "sqlite_volume",
        "artifact_binding",
        "profiles",
        "secrets_emitted",
        "secret_scan",
    ):
        tampered = dict(runtime)
        tampered.pop(required)
        write_json(args.runtime_report, tampered)
        if required == "secret_scan":
            message = "secret-absence"
        elif required == "sqlite_volume":
            message = "volume locality"
        else:
            message = "runtime diagnostic"
        with pytest.raises(module.DiagnosticError, match=message):
            module.emit_oci(args)

    for field, invalid in (
        ("driver", "nfs-plugin"),
        ("scope", "global"),
        ("driver_options", {"type": "nfs"}),
        ("mount_device", "0:30"),
        ("filesystem", "nfs"),
        ("mount_source", "server:/worldstream"),
        ("locality", "unknown"),
    ):
        tampered = json.loads(json.dumps(runtime))
        tampered["sqlite_volume"][field] = invalid
        write_json(args.runtime_report, tampered)
        with pytest.raises(module.DiagnosticError, match="volume locality"):
            module.emit_oci(args)

    tampered = json.loads(json.dumps(runtime))
    tampered["secret_scan"]["channels"][0]["channel"] = "unscanned-channel"
    write_json(args.runtime_report, tampered)
    with pytest.raises(module.DiagnosticError, match="exact channel set"):
        module.emit_oci(args)

    for profile, field, invalid in (
        ("sqlite-bundled", "status", "not-run"),
        ("sqlite-bundled", "engine_identity", "sqlite/unverified"),
        ("sqlite-bundled", "healthz", "not-run"),
        ("sqlite-bundled", "readyz", "not-run"),
        ("sqlite-bundled", "version", "not-run"),
        ("postgres-primary", "provider_image", "postgres:17.11"),
        ("postgres-primary", "engine_identity", "postgresql/17.10"),
        ("postgres-primary", "packaged_admin_migrate", "not-run"),
        ("postgres-primary", "packaged_admin_verify", "not-run"),
        ("postgres-primary", "runtime_role_least_privilege", False),
        ("postgres-primary", "healthz", "not-run"),
        ("postgres-primary", "readyz", "not-run"),
        ("postgres-primary", "version", "not-run"),
        ("postgres-primary", "network", "bridge"),
    ):
        tampered = json.loads(json.dumps(runtime))
        tampered["profiles"][profile][field] = invalid
        write_json(args.runtime_report, tampered)
        with pytest.raises(module.DiagnosticError, match="runtime diagnostic"):
            module.emit_oci(args)

    for field, invalid in (
        ("layer_count", 0),
        ("layer_descriptors_bound", False),
        ("rootfs_diff_ids_bound", False),
        ("closed_blob_inventory", False),
    ):
        tampered = json.loads(json.dumps(runtime))
        tampered["artifact_binding"][field] = invalid
        write_json(args.runtime_report, tampered)
        with pytest.raises(module.DiagnosticError, match="artifact binding"):
            module.emit_oci(args)

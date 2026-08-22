#!/usr/bin/env python3
"""Emit strict platform diagnostics from owning typed reports and artifacts."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import platform
import re
import stat
import sys
import tempfile
from pathlib import Path
from typing import Any

import tomllib

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = "worldstream/release-platform-diagnostic/v1"
GATE_SCHEMA = "worldstream/compatibility-gate-report/v1"
MANIFEST = ROOT / "compatibility.toml"
PACKAGE_SCRIPT = ROOT / "scripts/package.py"
OCI_LAYOUT_VERIFIER = ROOT / "scripts/verify-oci-layout.py"
BUILD_IDENTITY_PATH = ROOT / "scripts/release_build_identity.py"

PLATFORMS = {
    "native-linux": "native-linux-x86_64",
    "native-windows": "native-windows-x64",
    "oci-linux": "oci-linux-amd64",
    "macos-source": "macos-source",
    "security-observability": "all-supported-platforms",
}

TELEMETRY_HTTPS_CHECKS = {
    "trusted_ca": True,
    "hostname_verification": True,
    "otlp_https_delivery": True,
    "untrusted_ca_rejected": True,
    "hostname_mismatch_rejected": True,
    "non_success_rejected": True,
    "oversized_response_rejected": True,
    "slow_response_bounded": True,
    "endpoint_validation": True,
}
TELEMETRY_HTTPS_TESTS = {
    "trusted_https_otlp": (
        "worldstream-server",
        "tls_transport_uses_trusted_ca_hostname_verification_and_otlp_http",
    ),
    "tls_rejection": (
        "worldstream-server",
        "tls_transport_rejects_untrusted_cert_and_hostname_mismatch",
    ),
    "local_otlp_http": (
        "worldstream-server",
        "standard_http_transport_exercises_a_local_otlp_http_collector",
    ),
    "http_failure_bounds": (
        "worldstream-server",
        "standard_http_transport_rejects_non_success_and_oversized_responses",
    ),
    "slow_response_bounds": (
        "worldstream-server",
        "standard_http_transport_slow_response_is_bounded",
    ),
    "https_configuration": (
        "worldstream-server/worldstreamd",
        "telemetry_selects_validated_https_otlp",
    ),
    "endpoint_rejection": (
        "worldstream-runtime",
        "telemetry_endpoint_rejects_credentials_queries_bad_ports_and_unsupported_schemes",
    ),
}

OCI_POSTGRES_IMAGE = (
    "postgres:17.11-alpine@"
    "sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
)
OCI_POSTGRES_IDENTITY = "postgresql/17.11; server_version_num=170011"
MACOS_ARCHITECTURES = ("arm64", "x86_64")
MACOS_BROWSER_IDENTITIES = {
    "arm64": {
        "product": "chrome-for-testing-headless-shell",
        "version": "152.0.7977.54",
        "sha256": "sha256:4e0c165ef2f0d7265fb1e6b3df2d03d1d6581fb72cdfcebeac19c09760571df6",
        "size_bytes": 167_333_040,
        "version_output": "Google Chrome for Testing 152.0.7977.54",
        "distribution": {
            "url": "https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.54/mac-arm64/chrome-headless-shell-mac-arm64.zip",
            "sha256": "sha256:ef5d61434f13d9d2d9bdc7c9ab4bff92225979e196458cf846640862b25f127d",
            "size_bytes": 98_034_515,
        },
    },
    "x86_64": {
        "product": "chrome-for-testing-headless-shell",
        "version": "152.0.7977.54",
        "sha256": "sha256:49b6e6bdc4a9a14a160a2fcb08576ec3acef1d360cfa61bdc79a400a27a16d99",
        "size_bytes": 182_459_908,
        "version_output": "Google Chrome for Testing 152.0.7977.54",
        "distribution": {
            "url": "https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.54/mac-x64/chrome-headless-shell-mac-x64.zip",
            "sha256": "sha256:a50c716727adf9e4af5b8861d19da23b672b4687a0d631ec4beebb3009d6db97",
            "size_bytes": 102_893_170,
        },
    },
}
OCI_SECRET_SCAN_CHANNELS = {
    "image-config",
    "image-history",
    "negative-probes",
    "sqlite-container-config",
    "sqlite-container-logs",
    "postgres-container-config",
    "postgres-container-logs",
    "provider-container-config",
    "provider-container-logs",
    "probe-output",
    "runtime-report",
}


class DiagnosticError(RuntimeError):
    """An input cannot support a platform diagnostic."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise DiagnosticError(message)


def load_build_identity():
    name = "worldstream_release_platform_strict_json"
    existing = sys.modules.get(name)
    if existing is not None:
        return existing
    spec = importlib.util.spec_from_file_location(name, BUILD_IDENTITY_PATH)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise DiagnosticError(f"cannot load verifier: {BUILD_IDENTITY_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


BUILD_IDENTITY = load_build_identity()


def regular_file(path: Path, label: str) -> Path:
    try:
        mode = path.lstat().st_mode
    except FileNotFoundError as error:
        raise DiagnosticError(f"missing {label}: {path}") from error
    except OSError as error:
        raise DiagnosticError(f"cannot inspect {label}: {error}") from error
    require(not stat.S_ISLNK(mode), f"{label} must not be a symlink")
    require(stat.S_ISREG(mode), f"{label} must be a regular file")
    return path


def read_json(path: Path, label: str) -> dict[str, Any]:
    regular_file(path, label)
    try:
        content = path.read_bytes()
    except OSError as error:
        raise DiagnosticError(f"{label} is not valid JSON: {error}") from error
    try:
        return BUILD_IDENTITY.strict_json(content, label)
    except BUILD_IDENTITY.IdentityError as error:
        raise DiagnosticError(str(error)) from error


def digest(path: Path) -> str:
    try:
        return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        raise DiagnosticError(f"cannot hash {path}: {error}") from error


def load_script(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise DiagnosticError(f"cannot load verifier: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def independently_verify_oci_context(context: Path) -> dict[str, Any]:
    package = load_script("release_platform_package_verifier", PACKAGE_SCRIPT)
    try:
        return package.verify_oci_context(context)
    except package.PackageError as error:
        raise DiagnosticError(f"OCI context verification failed: {error}") from error


def independently_verify_oci_archive(
    artifact: Path, tested_image_config_digest: str
) -> dict[str, Any]:
    verifier = load_script("release_platform_oci_layout_verifier", OCI_LAYOUT_VERIFIER)
    try:
        return verifier.verify_artifact(artifact, tested_image_config_digest)
    except verifier.VerificationError as error:
        raise DiagnosticError(f"OCI archive verification failed: {error}") from error


def independently_verify_native_archive(
    artifact: Path, source_id: str, version: str
) -> tuple[dict[str, Any], dict[str, Any]]:
    package = load_script("release_platform_native_package_verifier", PACKAGE_SCRIPT)
    target = {
        "native-linux": "linux-x86_64",
        "native-windows": "windows-x64",
    }[source_id]
    try:
        package.verify_archive(artifact)
        report = package.archive_report(artifact)
        entries = package.archive_entries(artifact)
    except package.PackageError as error:
        raise DiagnosticError(f"native archive verification failed: {error}") from error
    require(
        report.get("identity", {}).get("target") == target
        and report.get("identity", {}).get("version") == version,
        "independently verified native archive target/version mismatch",
    )
    root = f"worldstream-{version}-{target}"
    suffix = ".exe" if source_id == "native-windows" else ""
    daemon = entries.get(f"{root}/bin/worldstreamd{suffix}")
    control = entries.get(f"{root}/bin/worldstreamctl{suffix}")
    require(
        isinstance(daemon, bytes) and daemon and isinstance(control, bytes) and control,
        "independently verified native archive has no packaged binaries",
    )
    return report, {
        "worldstreamd_sha256": "sha256:" + hashlib.sha256(daemon).hexdigest(),
        "worldstreamd_size_bytes": len(daemon),
        "worldstreamctl_sha256": "sha256:" + hashlib.sha256(control).hexdigest(),
        "worldstreamctl_size_bytes": len(control),
    }


def manifest() -> dict[str, Any]:
    try:
        with MANIFEST.open("rb") as source:
            value = tomllib.load(source)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise DiagnosticError(f"cannot read compatibility manifest: {error}") from error
    require(
        value.get("manifest_kind") == "release"
        and value.get("release_ready") is True
        and value.get("unresolved_required_fields") == [],
        "embedded release contract is incomplete",
    )
    return value


def pinned_macos_toolchains() -> dict[str, str]:
    """Read the exact repository-owned toolchain pins used by macOS evidence."""

    def text_pin(relative: str) -> str:
        path = regular_file(ROOT / relative, f"{relative} toolchain pin")
        try:
            value = path.read_text(encoding="utf-8").strip()
        except (OSError, UnicodeError) as error:
            raise DiagnosticError(f"cannot read {relative} toolchain pin") from error
        require(
            re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", value) is not None,
            f"{relative} is not an exact semantic version pin",
        )
        return value

    try:
        rust = tomllib.loads(
            regular_file(ROOT / "rust-toolchain.toml", "Rust toolchain pin").read_text(
                encoding="utf-8"
            )
        )["toolchain"]["channel"]
    except (
        OSError,
        UnicodeError,
        KeyError,
        TypeError,
        tomllib.TOMLDecodeError,
    ) as error:
        raise DiagnosticError("cannot read the exact Rust toolchain pin") from error
    require(
        isinstance(rust, str)
        and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", rust) is not None,
        "rust-toolchain.toml is not an exact semantic version pin",
    )
    package = read_json(ROOT / "package.json", "root package manifest")
    package_manager = package.get("packageManager")
    package_manager_match = (
        re.fullmatch(
            r"pnpm@(?P<version>[0-9]+\.[0-9]+\.[0-9]+)"
            r"\+sha512\.(?P<integrity>[0-9a-f]{128})",
            package_manager,
        )
        if isinstance(package_manager, str)
        else None
    )
    if package_manager_match is None:
        raise DiagnosticError(
            "root packageManager is not an exact integrity-bound pnpm pin"
        )
    return {
        "rust": rust,
        "python": text_pin(".python-version"),
        "node": text_pin(".node-version"),
        "pnpm": package_manager_match.group("version"),
        "uv": text_pin(".uv-version"),
    }


def atomic_write(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    require(not path.is_symlink() and not path.is_dir(), f"unsafe output path: {path}")
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump(value, output, ensure_ascii=False, indent=2, sort_keys=True)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def diagnostic(
    source_id: str, kind: str, facts: dict[str, Any], contract: dict[str, Any]
) -> dict[str, Any]:
    return {
        "schema": SCHEMA,
        "report_id": f"{source_id}/{kind}",
        "source_id": source_id,
        "platform": PLATFORMS[source_id],
        "version": contract["release_candidate"],
        "contract": contract["contracts"],
        "status": "passed",
        "release_evidence": True,
        "fail_closed": False,
        "facts": facts,
    }


def gate_outcomes(
    report: dict[str, Any], expected_system: str, required: set[str]
) -> dict[str, dict[str, Any]]:
    require(report.get("schema") == GATE_SCHEMA, "platform gate report schema mismatch")
    require(
        report.get("tier") == "minimal-ci" and report.get("strict") is True,
        "platform gate report is not strict minimal CI",
    )
    observed_platform = report.get("platform")
    require(
        isinstance(observed_platform, str)
        and expected_system.lower() in observed_platform.lower(),
        "platform gate report native system mismatch",
    )
    outcomes = report.get("outcomes")
    require(isinstance(outcomes, list), "platform gate report outcomes are missing")
    by_name: dict[str, list[dict[str, Any]]] = {}
    for outcome in outcomes:
        require(isinstance(outcome, dict), "platform gate outcome is malformed")
        name = outcome.get("name")
        require(isinstance(name, str) and name, "platform gate outcome has no name")
        by_name.setdefault(name, []).append(outcome)
    failures = [
        outcome
        for values in by_name.values()
        for outcome in values
        if outcome.get("status") == "FAIL"
    ]
    require(not failures, "platform gate report contains a failed outcome")
    for name in required:
        values = by_name.get(name, [])
        require(
            values and all(value.get("status") == "PASS" for value in values),
            f"required platform outcome did not pass: {name}",
        )
    return {name: values[-1] for name, values in by_name.items()}


def verify_archive_report(
    report: dict[str, Any],
    artifact: Path,
    source_id: str,
    contract: dict[str, Any],
    independently_verified: dict[str, Any],
) -> None:
    target = {
        "native-linux": "linux-x86_64",
        "native-windows": "windows-x64",
    }[source_id]
    require(
        report.get("schema") == "worldstream/package-report/v1",
        "package report schema mismatch",
    )
    require(report.get("kind") == "archive", "package report is not an archive report")
    require(
        report.get("identity", {}).get("target") == target
        and report.get("identity", {}).get("version") == contract["release_candidate"],
        "package target/version identity mismatch",
    )
    require(
        report.get("inventory", {}).get("archive_verified") is True
        and report.get("inventory", {}).get("manifest_source") == "compatibility.toml"
        and report.get("inventory", {}).get("manifest_mirror") == "compatibility.json"
        and report.get("inventory", {}).get("release_evidence") is False,
        "package archive verification boundary is incomplete",
    )
    regular_file(artifact, "release archive")
    require(
        report.get("artifact") == artifact.name, "package report artifact name mismatch"
    )
    require(
        report.get("sha256") == digest(artifact),
        "package report archive digest mismatch",
    )
    require(
        report.get("path") == artifact.name
        and report.get("size_bytes") == artifact.stat().st_size,
        "package report archive size mismatch",
    )
    require(
        report == independently_verified,
        "package report is not the exact independently verified archive projection",
    )


def verify_native_runtime_report(
    report: dict[str, Any],
    package_report: dict[str, Any],
    package_report_path: Path,
    artifact: Path,
    source_id: str,
    archive_binaries: dict[str, Any],
) -> dict[str, Any]:
    expected_target = {
        "native-linux": "linux-x86_64",
        "native-windows": "windows-x64",
    }[source_id]
    expected_system = "Linux" if source_id == "native-linux" else "Windows"
    expected_machines = {"x86_64", "amd64"}
    binding = report.get("package_binding")
    profiles = report.get("profiles")
    postgres_admin = report.get("postgres_admin")
    package_identity = package_report.get("identity")
    require(
        set(report)
        == {
            "schema",
            "status",
            "release_evidence",
            "secrets_emitted",
            "platform",
            "package_binding",
            "profiles",
            "postgres_admin",
            "cleanup",
        }
        and report.get("schema") == "worldstream/native-package-runtime-smoke/v1"
        and report.get("status") == "pass"
        and report.get("release_evidence") is False
        and report.get("secrets_emitted") is False
        and report.get("cleanup") == "pass"
        and report.get("platform", {}).get("system") == expected_system
        and str(report.get("platform", {}).get("machine", "")).lower()
        in expected_machines,
        "native packaged runtime smoke is incomplete",
    )
    require(
        isinstance(binding, dict)
        and binding.get("artifact") == artifact.name
        and binding.get("target") == expected_target
        and binding.get("version") == package_identity.get("version")
        and binding.get("archive_sha256") == digest(artifact)
        and binding.get("archive_size_bytes") == artifact.stat().st_size
        and binding.get("package_report_sha256") == digest(package_report_path)
        and binding.get("manifest_sha256")
        == "sha256:" + package_identity.get("manifest_sha256", "")
        and binding.get("manifest_json_sha256")
        == "sha256:" + package_identity.get("manifest_json_sha256", "")
        and binding.get("manifest_toml_sha256")
        == "sha256:" + package_identity.get("manifest_toml_sha256", "")
        and isinstance(binding.get("worldstreamd_sha256"), str)
        and re.fullmatch(r"sha256:[0-9a-f]{64}", binding["worldstreamd_sha256"])
        and type(binding.get("worldstreamd_size_bytes")) is int
        and binding["worldstreamd_size_bytes"] > 0
        and isinstance(binding.get("worldstreamctl_sha256"), str)
        and re.fullmatch(r"sha256:[0-9a-f]{64}", binding["worldstreamctl_sha256"])
        and type(binding.get("worldstreamctl_size_bytes")) is int
        and binding["worldstreamctl_size_bytes"] > 0
        and all(
            binding.get(field) == value for field, value in archive_binaries.items()
        )
        and binding.get("canonical_archive_verified") is True
        and binding.get("exact_archive_bytes_executed") is True,
        "native packaged runtime smoke is not bound to the exact archive/report/manifests/binaries",
    )
    require(
        isinstance(profiles, dict)
        and set(profiles) == {"sqlite-bundled", "postgres-primary"},
        "native packaged runtime smoke did not run both storage profiles",
    )
    for profile, value in profiles.items():
        expected_engine_prefix = (
            "sqlite/" if profile == "sqlite-bundled" else "postgresql/17.11;"
        )
        require(
            isinstance(value, dict)
            and set(value)
            == {
                "status",
                "healthz",
                "readyz",
                "version",
                "engine_identity",
                "packaged_ctl",
            }
            and value.get("status") == "pass"
            and value.get("healthz") == "pass"
            and value.get("readyz") == "pass"
            and value.get("version") == "manifest_and_engine_exact"
            and isinstance(value.get("engine_identity"), str)
            and value["engine_identity"].startswith(expected_engine_prefix)
            and value.get("packaged_ctl")
            == {
                "config_validate": "pass",
                "config_effective": "redacted_and_precedence_exact",
                "doctor": "bounded_diagnostics_exposed",
                "health": "pass",
                "version": "pass",
                "binary_version": package_identity["version"],
            },
            f"native packaged runtime smoke {profile} contract is incomplete",
        )
    require(
        postgres_admin
        == {
            "server_version_num": "170011",
            "dsn_delivery": "owner_only_file",
            "migrate": "pass",
            "verify": "pass",
            "runtime_role": "least_privilege",
        },
        "native packaged PostgreSQL administration contract is incomplete",
    )
    return binding


def emit_native(args: argparse.Namespace) -> None:
    contract = manifest()
    source_id = args.source
    expected_system = "Linux" if source_id == "native-linux" else "Windows"
    package_report = read_json(args.package_report, "package report")
    gate_report = read_json(args.gate_report, "platform gate report")
    runtime_report = read_json(args.runtime_report, "native packaged runtime report")
    verified_package_report, archive_binaries = independently_verify_native_archive(
        args.artifact, source_id, contract["release_candidate"]
    )
    verify_archive_report(
        package_report,
        args.artifact,
        source_id,
        contract,
        verified_package_report,
    )
    runtime_binding = verify_native_runtime_report(
        runtime_report,
        package_report,
        args.package_report,
        args.artifact,
        source_id,
        archive_binaries,
    )
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
        "ci-cell",
    }
    if source_id == "native-linux":
        runtime_required |= {
            "postgres-live-contract",
            "process-kill-point",
            "telemetry-failure-pressure",
            "bounded-sqlite-soak",
        }
        filesystem_required = {
            "filesystem-owner-only",
            "filesystem-link-policy",
            "evidence-filesystem-local",
        }
    else:
        filesystem_required = {
            "filesystem-owner-only",
            "filesystem-acl-policy",
            "evidence-filesystem-local",
        }
    outcomes = gate_outcomes(
        gate_report, expected_system, runtime_required | filesystem_required
    )
    cell_detail = outcomes["ci-cell"].get("detail", "")
    require(
        source_id.removeprefix("native-") in cell_detail, "gate cell identity mismatch"
    )
    output = args.output_dir
    output.mkdir(parents=True, exist_ok=True)
    atomic_write(
        output / f"{source_id}-package.json",
        diagnostic(
            source_id,
            "package",
            {
                "archive_identity": package_report["sha256"],
                "release_candidate": contract["release_candidate"],
                "platform_identity": PLATFORMS[source_id],
                "artifact_path": args.artifact.name,
            },
            contract,
        ),
    )
    atomic_write(
        output / f"{source_id}-runtime.json",
        diagnostic(
            source_id,
            "runtime",
            {
                "platform_identity": PLATFORMS[source_id],
                "release_candidate": contract["release_candidate"],
                "runtime_smoke": ",".join(sorted(runtime_required)),
                "storage_profile": "sqlite-bundled,postgres-primary",
                "runtime_report_sha256": digest(args.runtime_report),
                "packaged_binary_sha256": runtime_binding["worldstreamd_sha256"],
                "packaged_control_sha256": runtime_binding["worldstreamctl_sha256"],
                "postgres_admin": "packaged ctl migrate/verify and least-privilege file-only runtime passed",
            },
            contract,
        ),
    )
    filesystem_kind = "filesystem" if source_id == "native-linux" else "acl-or-reparse"
    filesystem_facts = (
        {
            "platform_identity": PLATFORMS[source_id],
            "filesystem_policy": "owner-only and local-filesystem policy passed",
            "owner_only_paths": "data and secret paths verified",
            "symlink_rejection": "symlink policy probe passed",
        }
        if source_id == "native-linux"
        else {
            "platform_identity": PLATFORMS[source_id],
            "acl_policy": "protected owner/SYSTEM/Administrators DACL tests passed",
            "reparse_policy": "nonlocal and ambiguous Windows path tests passed",
            "broad_write_rejected": "broad directory and secret-file DACLs rejected",
        }
    )
    atomic_write(
        output / f"{source_id}-{filesystem_kind}.json",
        diagnostic(source_id, filesystem_kind, filesystem_facts, contract),
    )


def emit_oci(args: argparse.Namespace) -> None:
    contract = manifest()
    context = read_json(args.context_report, "OCI context report")
    runtime = read_json(args.runtime_report, "OCI runtime report")
    metadata = read_json(args.context_metadata, "OCI context metadata")
    verified_context = independently_verify_oci_context(args.context)
    require(
        context == verified_context,
        "OCI context report is not the exact independently verified context projection",
    )
    expected_metadata = args.context / "oci-metadata.json"
    regular_file(expected_metadata, "verified OCI context metadata")
    require(
        args.context_metadata.resolve() == expected_metadata.resolve()
        and args.context_metadata.read_bytes() == expected_metadata.read_bytes(),
        "OCI context metadata is not the exact metadata inside the verified context",
    )
    runtime_binding = runtime.get("artifact_binding")
    tested_image_config_digest = (
        runtime_binding.get("tested_image_config_digest")
        if isinstance(runtime_binding, dict)
        else None
    )
    require(
        isinstance(tested_image_config_digest, str)
        and re.fullmatch(r"sha256:[0-9a-f]{64}", tested_image_config_digest)
        is not None,
        "OCI runtime diagnostic has no tested image config digest",
    )
    verified_artifact = independently_verify_oci_archive(
        args.artifact, tested_image_config_digest
    )
    require(
        runtime_binding == verified_artifact,
        "OCI runtime artifact binding is not the exact independently verified archive projection",
    )
    artifact_digest = verified_artifact["artifact_sha256"]
    profiles = runtime.get("profiles")
    secret_scan = runtime.get("secret_scan")
    sqlite_volume = runtime.get("sqlite_volume")
    sqlite = contract.get("storage", {}).get("sqlite", {})
    sqlite_identity = (
        f"sqlite/{sqlite.get('version')}; source_id={sqlite.get('source_id')}"
    )
    require(
        isinstance(secret_scan, dict)
        and set(secret_scan)
        == {
            "schema",
            "status",
            "secrets_emitted",
            "sentinel_sha256",
            "encodings_scanned",
            "channels",
        }
        and secret_scan.get("schema") == "worldstream/secret-absence-scan/v1"
        and secret_scan.get("status") == "pass"
        and secret_scan.get("secrets_emitted") is False
        and isinstance(secret_scan.get("sentinel_sha256"), str)
        and re.fullmatch(r"sha256:[0-9a-f]{64}", secret_scan["sentinel_sha256"])
        is not None
        and secret_scan.get("encodings_scanned")
        == ["base64", "base64url", "hex", "raw"]
        and isinstance(secret_scan.get("channels"), list)
        and len(secret_scan["channels"]) == len(OCI_SECRET_SCAN_CHANNELS),
        "OCI secret-absence scan is incomplete",
    )
    observed_secret_channels: set[str] = set()
    for channel in secret_scan["channels"]:
        require(
            isinstance(channel, dict)
            and set(channel) == {"channel", "sha256", "size_bytes"}
            and isinstance(channel.get("channel"), str)
            and channel["channel"] not in observed_secret_channels
            and isinstance(channel.get("sha256"), str)
            and re.fullmatch(r"sha256:[0-9a-f]{64}", channel["sha256"]) is not None
            and type(channel.get("size_bytes")) is int
            and channel["size_bytes"] >= 0,
            "OCI secret-absence scan channel is malformed",
        )
        observed_secret_channels.add(channel["channel"])
    require(
        observed_secret_channels == OCI_SECRET_SCAN_CHANNELS,
        "OCI secret-absence scan does not cover the exact channel set",
    )
    require(
        isinstance(sqlite_volume, dict)
        and set(sqlite_volume)
        == {
            "path",
            "type",
            "driver",
            "scope",
            "driver_options",
            "mount_device",
            "filesystem",
            "mount_source",
            "locality",
        }
        and sqlite_volume.get("path") == "/var/lib/worldstream"
        and sqlite_volume.get("type") == "docker-volume"
        and sqlite_volume.get("driver") == "local"
        and sqlite_volume.get("scope") == "local"
        and sqlite_volume.get("driver_options") == {}
        and isinstance(sqlite_volume.get("mount_device"), str)
        and re.fullmatch(r"[1-9][0-9]*:[0-9]+", sqlite_volume["mount_device"])
        is not None
        and sqlite_volume.get("filesystem") in {"ext4", "xfs"}
        and isinstance(sqlite_volume.get("mount_source"), str)
        and re.fullmatch(r"/dev/[^\s]+", sqlite_volume["mount_source"]) is not None
        and sqlite_volume.get("locality") == "local-block-device",
        "OCI SQLite volume locality is incomplete",
    )
    require(
        context.get("schema") == "worldstream/oci-context-report/v1"
        and context.get("kind") == "oci-context"
        and context.get("verified") is True
        and context.get("release_evidence") is False,
        "OCI context was not exactly verified",
    )
    require(
        runtime.get("schema") == "worldstream/oci-runtime-smoke/v1"
        and runtime.get("status") == "PASS"
        and runtime.get("release_evidence") is False
        and runtime.get("health") == "healthy"
        and runtime.get("healthcheck_without_daemon") == "rejected"
        and runtime.get("read_only_root") is True
        and runtime.get("non_root") == "65532:65532"
        and runtime.get("persistent_volume") == "/var/lib/worldstream"
        and runtime.get("authority_secret_source")
        == "owner-readable-read-only-volume-file"
        and runtime.get("standalone_config") == "valid"
        and runtime.get("rejected_layouts")
        == ["tmpfs", "wrong-data-directory", "network-configured-volume"]
        and runtime.get("secrets_emitted") is False
        and secret_scan.get("secrets_emitted") is False
        and isinstance(profiles, dict)
        and set(profiles) == {"sqlite-bundled", "postgres-primary"}
        and profiles.get("sqlite-bundled")
        == {
            "status": "pass",
            "health": "healthy",
            "filesystem": "ext4-or-xfs-explicit-volume",
            "engine_identity": sqlite_identity,
            "healthz": "pass",
            "readyz": "pass",
            "version": "pass",
        }
        and profiles.get("postgres-primary")
        == {
            "status": "pass",
            "provider_image": OCI_POSTGRES_IMAGE,
            "engine_identity": OCI_POSTGRES_IDENTITY,
            "packaged_admin_migrate": "pass",
            "packaged_admin_verify": "pass",
            "runtime_role_least_privilege": True,
            "healthz": "pass",
            "readyz": "pass",
            "version": "pass",
            "network": "disabled-unix-socket",
        }
        and runtime_binding == verified_artifact,
        "OCI runtime diagnostic did not pass the frozen profile",
    )
    base_image = metadata.get("base_image")
    require(
        isinstance(base_image, str)
        and re.fullmatch(r"[^\s@]+@sha256:[0-9a-f]{64}", base_image) is not None
        and metadata.get("version") == contract["release_candidate"]
        and metadata.get("profile") == "oci-linux-amd64"
        and metadata.get("target") == "linux/amd64",
        "OCI metadata identity or pinned base image is invalid",
    )
    args.output_dir.mkdir(parents=True, exist_ok=True)
    atomic_write(
        args.output_dir / "oci-linux-oci.json",
        diagnostic(
            "oci-linux",
            "oci",
            {
                "platform_identity": "oci-linux-amd64",
                "release_candidate": contract["release_candidate"],
                "pinned_base_image": base_image,
                "image_digest": artifact_digest,
                "tested_image_config_digest": tested_image_config_digest,
                "context_inventory_sha256": verified_context["sha256"],
                "context_report_sha256": digest(args.context_report),
                "context_metadata_sha256": digest(expected_metadata),
                "runtime_report_sha256": digest(args.runtime_report),
                "runtime_smoke": "healthy non-root read-only runtime with verified local persistent volume",
                "storage_profiles": "sqlite-bundled and postgres-primary passed",
                "postgres_provider_image": OCI_POSTGRES_IMAGE,
                "postgres_engine_identity": OCI_POSTGRES_IDENTITY,
                "postgres_packaged_administration": "migrate and verify passed",
                "postgres_runtime_role": "least privilege verified",
                "postgres_network": "disabled network with local Unix socket",
                "secrets": "not emitted",
                "filesystem_policy": "ext4/xfs local block source, local driver without options, and negative layout probes passed",
            },
            contract,
        ),
    )


def verify_macos_browser_story(value: Any, architecture: str) -> None:
    expected_checks = {
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
    require(
        isinstance(value, dict)
        and set(value)
        == {
            "schema",
            "canonical_encoding",
            "status",
            "release_evidence",
            "source_mode",
            "elapsed_ms",
            "browser",
            "tools",
            "runtime",
            "story",
            "dom_evidence",
            "typed_actions",
            "checks",
            "privacy",
        }
        and value.get("schema") == "worldstream/package-browser-heist/v1"
        and value.get("canonical_encoding") == "utf8-sorted-key-compact-json-lf"
        and value.get("status") == "pass"
        and value.get("release_evidence") is False
        and value.get("source_mode") == "source-build"
        and type(value.get("elapsed_ms")) is int
        and 0 < value["elapsed_ms"] <= 600_000
        and value.get("browser") == MACOS_BROWSER_IDENTITIES[architecture]
        and value.get("checks") == expected_checks
        and value.get("privacy")
        == {
            "status": "pass",
            "private_canary_absent": True,
            "credentials_absent": True,
            "private_claim_absent_from_retained_evidence": True,
        },
        "macOS source quickstart browser story contract is incomplete",
    )
    dom = value["dom_evidence"]
    actions = value["typed_actions"]
    require(
        isinstance(dom, dict)
        and set(dom)
        == {
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
        }
        and all(re.fullmatch(r"sha256:[0-9a-f]{64}", item) for item in dom.values())
        and isinstance(actions, dict)
        and set(actions)
        == {
            "inspect_clue",
            "publish_clue",
            "propose_plan",
            "commit_move",
            "acknowledge_result",
        }
        and all(
            re.fullmatch(r"sha256:[0-9a-f]{64}", item) for item in actions.values()
        ),
        "macOS source quickstart DOM/action evidence is incomplete",
    )
    story = value["story"]
    require(
        isinstance(story, dict)
        and story.get("phase_path")
        == ["Briefing", "Negotiation", "Commitment", "Resolution", "Result", "Complete"]
        and story.get("public_projection", {}).get("broker_present") is True
        and story.get("public_projection", {}).get("commitment_count") == 2
        and story.get("public_projection", {}).get("aggregate_outcome_present") is True
        and story.get("final_replay", {}).get("verified") is True
        and story.get("final_replay", {}).get("hash_parity", {}).get("verified")
        is True,
        "macOS source quickstart story/replay evidence is incomplete",
    )
    runtime = value["runtime"]
    require(
        isinstance(runtime, dict)
        and set(runtime) == {"worldstreamd", "ui", "sdk", "heist_reference_clients"}
        and runtime.get("worldstreamd", {}).get("origin")
        == "source-build:target/debug/worldstreamd"
        and runtime.get("ui", {}).get("origin") == "source-build:web/console/dist"
        and runtime.get("sdk", {}).get("origin") == "source:sdk/python/src"
        and runtime.get("heist_reference_clients", {}).get("origin")
        == "source:examples/heist"
        and all(
            re.fullmatch(r"sha256:[0-9a-f]{64}", str(record.get("sha256")))
            and type(record.get("size_bytes")) is int
            and record["size_bytes"] > 0
            for record in (runtime["worldstreamd"],)
        )
        and all(
            re.fullmatch(r"sha256:[0-9a-f]{64}", str(record.get("tree_sha256")))
            and type(record.get("file_count")) is int
            and record["file_count"] > 0
            and type(record.get("total_bytes")) is int
            and record["total_bytes"] > 0
            for record in (
                runtime["ui"],
                runtime["sdk"],
                runtime["heist_reference_clients"],
            )
        ),
        "macOS source quickstart runtime identity is incomplete",
    )
    tools = value["tools"]
    require(
        isinstance(tools, dict)
        and tools.get("adapter")
        == {
            "name": "worldstream-cdp-browser",
            "protocol": "Chrome DevTools Protocol",
            "sha256": digest(ROOT / "scripts/cdp-browser.py"),
            "size_bytes": (ROOT / "scripts/cdp-browser.py").stat().st_size,
        }
        and tools.get("python")
        == {
            "implementation": "cpython",
            "version": pinned_macos_toolchains()["python"],
        },
        "macOS source quickstart browser tool identity is incomplete",
    )


def emit_macos(args: argparse.Namespace) -> None:
    contract = manifest()
    require(
        isinstance(args.source_revision, str)
        and re.fullmatch(r"[0-9a-f]{40}", args.source_revision) is not None,
        "macOS source diagnostic requires an exact 40-hex source revision",
    )
    expected_toolchains = pinned_macos_toolchains()
    quickstarts: dict[str, tuple[Path, dict[str, Any]]] = {}
    for report_path in args.quickstart_report:
        quickstart = read_json(report_path, "macOS quickstart report")
        observed_architecture = quickstart.get("platform", {}).get("machine")
        require(
            observed_architecture in MACOS_ARCHITECTURES,
            "macOS source quickstart has an unexpected architecture",
        )
        require(
            observed_architecture not in quickstarts,
            f"duplicate macOS source quickstart architecture: {observed_architecture}",
        )
        require(
            set(quickstart)
            == {
                "schema",
                "status",
                "release_evidence",
                "signed_or_notarized_binary",
                "version",
                "platform",
                "toolchains",
                "source_revision",
                "elapsed_seconds",
                "browser_story",
                "checks",
            }
            and quickstart.get("schema") == "worldstream/macos-source-quickstart/v2"
            and quickstart.get("status") == "passed"
            and quickstart.get("release_evidence") is False
            and quickstart.get("signed_or_notarized_binary") is False
            and quickstart.get("platform", {}).get("system") == "Darwin"
            and quickstart.get("platform", {}).get("filesystem") == "apfs"
            and quickstart.get("version") == contract["release_candidate"]
            and quickstart.get("source_revision") == args.source_revision
            and quickstart.get("toolchains") == expected_toolchains
            and type(quickstart.get("elapsed_seconds")) is int
            and 0 < quickstart["elapsed_seconds"] < 600
            and quickstart.get("checks")
            == {
                "complete_heist": True,
                "embedded_ui": True,
                "pinned_toolchain": True,
                "privacy": True,
                "quickstart": True,
                "real_browser": True,
                "replay": True,
                "source_build": True,
                "source_revision": True,
                "stale_resync": True,
            },
            "macOS source quickstart diagnostic is incomplete",
        )
        verify_macos_browser_story(quickstart["browser_story"], observed_architecture)
        quickstarts[observed_architecture] = (report_path, quickstart)
    require(
        set(quickstarts) == set(MACOS_ARCHITECTURES),
        "macOS source quickstart requires exactly arm64 and x86_64 reports",
    )
    artifact = {
        "schema": "worldstream/macos-source-matrix/v1",
        "status": "passed",
        "release_evidence": False,
        "signed_or_notarized_binary": False,
        "version": contract["release_candidate"],
        "source_revision": args.source_revision,
        "toolchains": expected_toolchains,
        "architectures": list(MACOS_ARCHITECTURES),
        "quickstart_reports": [
            {
                "architecture": architecture,
                "sha256": digest(quickstarts[architecture][0]),
                "report": quickstarts[architecture][1],
            }
            for architecture in MACOS_ARCHITECTURES
        ],
    }
    atomic_write(args.artifact_output, artifact)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    atomic_write(
        args.output_dir / "macos-source-source-quickstart.json",
        diagnostic(
            "macos-source",
            "source-quickstart",
            {
                "platform_identity": "macos-source",
                "release_candidate": contract["release_candidate"],
                "architectures": list(MACOS_ARCHITECTURES),
                "source_revision": args.source_revision,
                "pinned_toolchain": expected_toolchains,
                "source_build": "workspace, Python SDK, and UI source builds passed on arm64 and x86_64",
                "quickstart": "pinned real browser completed the six-phase Heist reference-client story through the source-built daemon and embedded UI, including stale/resync, privacy, replay, and final reveal, in under ten minutes on arm64 and x86_64",
            },
            contract,
        ),
    )


def emit_security(args: argparse.Namespace) -> None:
    contract = manifest()
    linux = read_json(args.linux_gate_report, "Linux platform gate report")
    windows = read_json(args.windows_gate_report, "Windows platform gate report")
    linux_runtime = read_json(
        args.linux_runtime_report, "Linux native packaged runtime report"
    )
    windows_runtime = read_json(
        args.windows_runtime_report, "Windows native packaged runtime report"
    )
    https = read_json(args.https_report, "telemetry HTTPS report")
    required = {
        "secret-scan",
        "evidence-privacy-local",
        "evidence-capability-local",
        "evidence-lease-local",
        "evidence-telemetry-local",
        "filesystem-owner-only",
        "evidence-filesystem-local",
        "config-contract",
    }
    linux_outcomes = gate_outcomes(
        linux, "Linux", required | {"telemetry-failure-pressure"}
    )
    windows_outcomes = gate_outcomes(
        windows, "Windows", required | {"filesystem-acl-policy"}
    )
    for expected_system, runtime in (
        ("Linux", linux_runtime),
        ("Windows", windows_runtime),
    ):
        profiles = runtime.get("profiles")
        require(
            runtime.get("schema") == "worldstream/native-package-runtime-smoke/v1"
            and runtime.get("status") == "pass"
            and runtime.get("release_evidence") is False
            and runtime.get("platform", {}).get("system") == expected_system
            and isinstance(profiles, dict)
            and set(profiles) == {"sqlite-bundled", "postgres-primary"}
            and all(
                profile.get("packaged_ctl", {}).get("config_validate") == "pass"
                and profile.get("packaged_ctl", {}).get("config_effective")
                == "redacted_and_precedence_exact"
                and profile.get("packaged_ctl", {}).get("doctor")
                == "bounded_diagnostics_exposed"
                for profile in profiles.values()
                if isinstance(profile, dict)
            )
            and all(isinstance(profile, dict) for profile in profiles.values()),
            f"{expected_system} packaged config/effective/doctor contract is incomplete",
        )
    require(
        set(https)
        == {
            "schema",
            "status",
            "release_evidence",
            "evidence_class",
            "platform",
            "timeout_seconds_per_test",
            "checks",
            "tests",
        },
        "telemetry HTTPS report has wrong fields",
    )
    https_platform = https.get("platform")
    require(
        https.get("schema") == "worldstream/telemetry-https-evidence/v1"
        and https.get("status") == "passed"
        and https.get("release_evidence") is False
        and https.get("evidence_class") == "local_transport_integration"
        and isinstance(https_platform, dict)
        and https_platform.get("system") == "Linux"
        and https_platform.get("machine") in {"x86_64", "amd64"}
        and type(https.get("timeout_seconds_per_test")) is int
        and 1 <= https["timeout_seconds_per_test"] <= 900
        and https.get("checks") == TELEMETRY_HTTPS_CHECKS,
        "telemetry HTTPS transport identity or checks are incomplete",
    )
    tests = https.get("tests")
    require(
        isinstance(tests, list) and len(tests) == len(TELEMETRY_HTTPS_TESTS),
        "telemetry HTTPS exact test results are missing",
    )
    observed_tests: dict[str, tuple[str, str]] = {}
    for test in tests:
        require(
            isinstance(test, dict)
            and set(test) == {"id", "package", "filter", "passed_count"}
            and isinstance(test.get("id"), str)
            and test.get("passed_count") == 1,
            "telemetry HTTPS test result is malformed or did not pass exactly once",
        )
        test_id = test["id"]
        require(test_id not in observed_tests, "duplicate telemetry HTTPS test result")
        observed_tests[test_id] = (test.get("package"), test.get("filter"))
    require(
        observed_tests == TELEMETRY_HTTPS_TESTS,
        "telemetry HTTPS exact test identity mismatch",
    )
    inputs = [
        {
            "platform": "native-linux-x86_64",
            "sha256": digest(args.linux_gate_report),
            "size_bytes": args.linux_gate_report.stat().st_size,
        },
        {
            "platform": "native-windows-x64",
            "sha256": digest(args.windows_gate_report),
            "size_bytes": args.windows_gate_report.stat().st_size,
        },
        {
            "platform": "native-linux-x86_64/telemetry-https",
            "sha256": digest(args.https_report),
            "size_bytes": args.https_report.stat().st_size,
        },
        {
            "platform": "native-linux-x86_64/config-contract",
            "sha256": digest(args.linux_runtime_report),
            "size_bytes": args.linux_runtime_report.stat().st_size,
        },
        {
            "platform": "native-windows-x64/config-contract",
            "sha256": digest(args.windows_runtime_report),
            "size_bytes": args.windows_runtime_report.stat().st_size,
        },
    ]
    artifact = {
        "schema": "worldstream/security-observability-profile/v1",
        "status": "passed",
        "release_evidence": True,
        "version": contract["release_candidate"],
        "contract": contract["contracts"],
        "platform_reports": inputs,
        "verified_outcomes": sorted(
            required | {"telemetry-failure-pressure", "filesystem-acl-policy"}
        ),
    }
    atomic_write(args.artifact_output, artifact)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    atomic_write(
        args.output_dir / "security-observability-config-redaction-observability.json",
        diagnostic(
            "security-observability",
            "config-redaction-observability",
            {
                "platform_identity": "all-supported-platforms",
                "config_validation": {
                    "gate": "config-contract",
                    "linux_gate": linux_outcomes["config-contract"]["detail"],
                    "windows_gate": windows_outcomes["config-contract"]["detail"],
                    "linux_runtime_sha256": digest(args.linux_runtime_report),
                    "windows_runtime_sha256": digest(args.windows_runtime_report),
                    "packaged_commands": [
                        "config validate",
                        "config effective",
                        "doctor",
                    ],
                },
                "secret_redaction": "secret-scan and privacy fixtures passed on Linux and Windows",
                "observability_bounds": "telemetry fixtures and Linux failure-pressure probe passed",
            },
            contract,
        ),
    )
    atomic_write(
        args.output_dir / "security-observability-security.json",
        diagnostic(
            "security-observability",
            "security",
            {
                "platform_identity": "all-supported-platforms",
                "security_probes": "capability, lease, privacy, and secret probes passed",
                "remote_tls": "trusted-CA and hostname-verified OTLP HTTPS delivery passed; untrusted CA, hostname mismatch, invalid endpoints, non-success, oversized, and slow responses were rejected or bounded",
                "filesystem_policy": "native POSIX and Windows DACL policies passed",
            },
            contract,
        ),
    )


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    subcommands = command.add_subparsers(dest="command", required=True)
    native = subcommands.add_parser("native")
    native.add_argument(
        "--source", choices=("native-linux", "native-windows"), required=True
    )
    native.add_argument("--package-report", type=Path, required=True)
    native.add_argument("--gate-report", type=Path, required=True)
    native.add_argument("--runtime-report", type=Path, required=True)
    native.add_argument("--artifact", type=Path, required=True)
    native.add_argument("--output-dir", type=Path, required=True)
    native.set_defaults(function=emit_native)
    oci = subcommands.add_parser("oci")
    oci.add_argument("--context", type=Path, required=True)
    oci.add_argument("--context-report", type=Path, required=True)
    oci.add_argument("--runtime-report", type=Path, required=True)
    oci.add_argument("--context-metadata", type=Path, required=True)
    oci.add_argument("--artifact", type=Path, required=True)
    oci.add_argument("--output-dir", type=Path, required=True)
    oci.set_defaults(function=emit_oci)
    macos = subcommands.add_parser("macos")
    macos.add_argument("--quickstart-report", type=Path, action="append", required=True)
    macos.add_argument("--source-revision", required=True)
    macos.add_argument("--artifact-output", type=Path, required=True)
    macos.add_argument("--output-dir", type=Path, required=True)
    macos.set_defaults(function=emit_macos)
    security = subcommands.add_parser("security")
    security.add_argument("--linux-gate-report", type=Path, required=True)
    security.add_argument("--windows-gate-report", type=Path, required=True)
    security.add_argument("--linux-runtime-report", type=Path, required=True)
    security.add_argument("--windows-runtime-report", type=Path, required=True)
    security.add_argument("--https-report", type=Path, required=True)
    security.add_argument("--artifact-output", type=Path, required=True)
    security.add_argument("--output-dir", type=Path, required=True)
    security.set_defaults(function=emit_security)
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        args.function(args)
    except (DiagnosticError, KeyError, TypeError, ValueError) as error:
        print(f"platform diagnostic failed: {error}", file=sys.stderr)
        return 1
    print(f"platform diagnostic emitted by {platform.system()} verifier")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

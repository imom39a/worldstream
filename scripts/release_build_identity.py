#!/usr/bin/env python3
"""Canonical release build identities and SPDX/SLSA graph validation."""

from __future__ import annotations

import ast
import base64
import binascii
import hashlib
import json
import math
import os
import re
import stat
import subprocess
import tarfile
import zipfile
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath, PureWindowsPath
from typing import Any
from urllib.parse import quote

import tomllib

BUILD_IDENTITY_SCHEMA = "worldstream/release-build-identity/v2"
REPOSITORY = "https://github.com/imom39a/worldstream"
WORKFLOW_PATH = ".github/workflows/compatibility-gates.yml"
BUILD_TYPE_PATH = "docs/build-types/pre-sign-subject-aggregation-v1.md"
BUILD_TYPE_EXAMPLE_PATH = "docs/release-build-type-v1.example.json"
BUILD_TYPE = (
    f"{REPOSITORY}/blob/79c79e9626a1dac192053b2830487a6763557f1c/{BUILD_TYPE_PATH}"
)
RELEASE_AGGREGATION_SCHEMA = "worldstream/release-aggregation/v1"
SOURCE_REVISION_FILE = ".worldstream-source-revision"
BUILD_METADATA_PATH = "metadata/build.json"
OCI_BASE_IMAGE_PATH = "packaging/oci/base-image.txt"
THIRD_PARTY_NOTICE_SCHEMA = "worldstream/third-party-notices/v1"
THIRD_PARTY_NOTICE_MANIFEST_PATH = "licenses/THIRD-PARTY-NOTICES.json"
THIRD_PARTY_NOTICE_TEXT_PATH = "licenses/THIRD-PARTY-NOTICES.txt"
THIRD_PARTY_NOTICE_GENERATOR_PATH = "scripts/generate-third-party-notices.py"
SPDX_LICENSE_LIST_REVISION = "c4a7237ec8f4654e867546f9f409749300f1bf4c"
BUILDKIT_IMAGE = "moby/buildkit@sha256:28a898719c18a33f4e8000685287fa36fd0dd9560c6440227d3a732d79bb41d8"
BUILDX_VERSION = "0.36.1"
DOCKERFILE_FRONTEND = "docker/dockerfile:1.7@sha256:a57df69d0ea827fb7266491f2813635de6f17269be881f696fbfdf2d83dda33e"
ZIG_VERSION = "0.15.2"
ZIG_LINUX_X86_64_ARCHIVE = "zig-x86_64-linux-0.15.2.tar.xz"
ZIG_LINUX_X86_64_URL = (
    "https://ziglang.org/download/0.15.2/zig-x86_64-linux-0.15.2.tar.xz"
)
ZIG_LINUX_X86_64_SIZE = 53_733_924
ZIG_LINUX_X86_64_SHA256 = (
    "02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239"
)
ZIG_MUSL_CC_PATH = "scripts/zig-musl-cc.sh"
ZIG_MUSL_CC_SHA256 = "24183e6edf8601ad753098835d41ae01eb2d94ea6f41d60fd0bec41a52fa2fa4"
ZIG_MUSL_AR_PATH = "scripts/zig-musl-ar.sh"
ZIG_MUSL_AR_SHA256 = "927f9780c14ee598171af9f1ff25c1f06f63c21b6f45d2b0d3e6534b11a252a2"
PNPM_VERSION = "11.19.0"
PNPM_SHA512 = (
    "7881f3ed590d472c4a955e2b88b2121791116066dcc88cbca3849ec9b60f1bb"
    "aa6d2ccb221fa91da4e1c65bef2bcbe379365aea7ac539c7bf86dedc3a1b22dce"
)
PNPM_PACKAGE_MANAGER = f"pnpm@{PNPM_VERSION}+sha512.{PNPM_SHA512}"
PNPM_TARBALL_URL = f"https://registry.npmjs.org/pnpm/-/pnpm-{PNPM_VERSION}.tgz"
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
SHA256_REF = re.compile(r"sha256:[0-9a-f]{64}\Z")
GIT_REVISION = re.compile(r"[0-9a-f]{40}\Z")
VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?\Z")
COMPONENT_VERSION = re.compile(r"[0-9A-Za-z][0-9A-Za-z._+~-]*\Z")
MAX_RELEASE_JSON_BYTES = 64 * 1024 * 1024
PINNED_MATERIAL_PATHS = (
    ".github/workflows/compatibility-gates.yml",
    ".node-version",
    ".python-version",
    ".uv-version",
    "Cargo.lock",
    "Cargo.toml",
    "compatibility.json",
    "compatibility.toml",
    BUILD_TYPE_PATH,
    "licenses/LICENSE-APACHE-2.0.txt",
    THIRD_PARTY_NOTICE_MANIFEST_PATH,
    THIRD_PARTY_NOTICE_TEXT_PATH,
    "package.json",
    "pnpm-lock.yaml",
    "rust-toolchain.toml",
    "scripts/gates.py",
    THIRD_PARTY_NOTICE_GENERATOR_PATH,
    "scripts/package.py",
    "scripts/package-oci.ps1",
    "scripts/package-oci.sh",
    "scripts/package-release.ps1",
    "scripts/package-release.sh",
    "scripts/release-evidence-collect.py",
    "scripts/release-evidence-produce.py",
    "scripts/release-evidence-assemble.py",
    "scripts/release-evidence-assemble.sh",
    "scripts/release-supply-chain.py",
    "scripts/release-supply-chain.sh",
    "scripts/release_build_identity.py",
    "scripts/reference_host_environment.py",
    "scripts/verify-release.ps1",
    "scripts/verify-release.sh",
    "scripts/verify-oci-layout.py",
    ZIG_MUSL_AR_PATH,
    ZIG_MUSL_CC_PATH,
    "sdk/python/pyproject.toml",
    "sdk/python/uv.lock",
    "web/console/package.json",
    OCI_BASE_IMAGE_PATH,
    "packaging/oci/Dockerfile",
    "packaging/oci/entrypoint.sh",
)
PROVENANCE_MATERIAL_PATHS: tuple[str, ...] = ()
SOURCE_ENTRY_PATHS = tuple(
    dict.fromkeys((*PINNED_MATERIAL_PATHS, *PROVENANCE_MATERIAL_PATHS))
)
PRE_SIGN_DYNAMIC_IMPORT_PATHS = (
    "scripts/package.py",
    "scripts/release-evidence-assemble.py",
    "scripts/release-evidence-collect.py",
    "scripts/release-evidence-produce.py",
    "scripts/release_build_identity.py",
    "scripts/verify-oci-layout.py",
)
PAYLOAD_TARGETS = {
    "source-archive": "source",
    "native-linux-x86_64-archive": "linux-x86_64",
    "native-windows-x64-archive": "windows-x64",
    "oci-linux-amd64-image": "oci-linux-amd64",
}
TARGET_TRIPLES = {
    "source": "source",
    "linux-x86_64": "x86_64-unknown-linux-musl",
    "windows-x64": "x86_64-pc-windows-msvc",
    "oci-linux-amd64": "x86_64-unknown-linux-musl",
}
TARGET_RUNNERS = {
    "source": "ubuntu-24.04",
    "linux-x86_64": "ubuntu-24.04",
    "windows-x64": "windows-2025-vs2026",
    "oci-linux-amd64": "ubuntu-24.04",
}
HOSTED_RUNNER_FACTS = {
    "source": {"os": "Linux", "architecture": "X64", "image": "ubuntu24"},
    "linux-x86_64": {
        "os": "Linux",
        "architecture": "X64",
        "image": "ubuntu24",
    },
    "windows-x64": {
        "os": "Windows",
        "architecture": "X64",
        "image": "win25-vs2026",
    },
    "oci-linux-amd64": {
        "os": "Linux",
        "architecture": "X64",
        "image": "ubuntu24",
    },
}
RUNNER_IMAGE_VERSION = re.compile(r"20[0-9]{6}\.[0-9]+\.[0-9]+\Z")
TOOL_ENVIRONMENT = {
    "linux-x86_64": {
        "archiver": "AR_x86_64_unknown_linux_musl",
        "c_compiler": "CC_x86_64_unknown_linux_musl",
        "linker": "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER",
    },
    "windows-x64": {
        "archiver": "AR_x86_64_pc_windows_msvc",
        "c_compiler": "CC_x86_64_pc_windows_msvc",
        "linker": "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER",
    },
    "oci-linux-amd64": {
        "archiver": "AR_x86_64_unknown_linux_musl",
        "c_compiler": "CC_x86_64_unknown_linux_musl",
        "linker": "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER",
    },
}
PAYLOAD_STEP_CONTRACTS = {
    ("native", "Build Linux release archive"): {
        "if": "${{ github.event_name == 'workflow_dispatch' && inputs.release == true && matrix.platform == 'native-linux-x86_64' }}",
        "shell": "bash",
        "sha256": "78d5b2d0e9156a69177d70b5af47f6a5412a864e5f3b5d27c4dc1ff54732a650",
    },
    ("native", "Build Windows release archive"): {
        "if": "${{ github.event_name == 'workflow_dispatch' && inputs.release == true && matrix.platform == 'native-windows-x64' }}",
        "shell": "pwsh",
        "sha256": "a4ab2d5a7cfbdbe001c5470bff67cc23feec96e79e4b8abf57841d71ab37e4b9",
    },
    ("native", "Build and test OCI release image"): {
        "if": "${{ github.event_name == 'workflow_dispatch' && inputs.release == true && matrix.platform == 'oci-linux-amd64' }}",
        "shell": "bash",
        "sha256": "0f7414d82c193fd98ffdbb24c2d781dcd759c08542b37a50242e19fe328948dd",
    },
    ("release-evidence", "Build and verify source archive from the clean checkout"): {
        "if": "${{ github.event_name == 'workflow_dispatch' && inputs.release == true }}",
        "shell": "bash",
        "sha256": "497edf59383271a810fc657264a7b0dda3645f70667697e9a585534a5c81179f",
    },
}
OCI_SETUP_STEP_SHA256 = (
    "3b9c521d50676c35c381996bf93439fbae58e8c7764f33c38f5513c1e6fbe587"
)
EVIDENCE_UPSTREAM_JOBS = {
    "manifest-contract": ("conformance-release", ("ubuntu-24.04",)),
    "sqlite-conformance": ("conformance-release", ("ubuntu-24.04",)),
    "postgres-conformance": ("conformance-release", ("ubuntu-24.04",)),
    "migration-history": ("conformance-release", ("ubuntu-24.04",)),
    "transfer": ("conformance-release", ("ubuntu-24.04",)),
    "restore": ("conformance-release", ("ubuntu-24.04",)),
    "native-linux": ("native", ("ubuntu-24.04",)),
    "native-windows": ("native", ("windows-2025-vs2026",)),
    "oci-linux": ("native", ("ubuntu-24.04",)),
    "macos-source": (
        "macos-source-release",
        ("macos-15", "macos-15-intel", "ubuntu-24.04"),
    ),
    "security-observability": ("release-evidence", ("ubuntu-24.04",)),
    "failure-soak": (
        "failure-soak-release",
        ("ubuntu-24.04-x86_64-ext4-4vcpu-8gib-local-ssd",),
    ),
    "reference-performance": (
        "reference-performance-release",
        ("ubuntu-24.04-x86_64-ext4-4vcpu-8gib-local-ssd",),
    ),
}
PAYLOAD_UPSTREAM_JOBS = {
    "source-archive": ("release-evidence", "ubuntu-24.04"),
    "native-linux-x86_64-archive": ("native", "ubuntu-24.04"),
    "native-windows-x64-archive": ("native", "windows-2025-vs2026"),
    "oci-linux-amd64-image": ("native", "ubuntu-24.04"),
}
ACTION_TOKEN = re.compile(
    r"(?P<repository>[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)"
    r"(?:/[A-Za-z0-9_.-]+)*@(?P<commit>[0-9a-f]{40})\Z"
)


class IdentityError(RuntimeError):
    """A release build or dependency graph is not exact."""


def reject(message: str) -> None:
    raise IdentityError(message)


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def strict_json(value: bytes, label: str) -> dict[str, Any]:
    if not (0 < len(value) <= MAX_RELEASE_JSON_BYTES):
        reject(
            f"{label} is empty or exceeds the "
            f"{MAX_RELEASE_JSON_BYTES}-byte release JSON limit"
        )

    def pairs(items: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, item in items:
            if key in result:
                reject(f"{label} contains duplicate key {key!r}")
            result[key] = item
        return result

    def reject_constant(_value: str) -> None:
        raise ValueError("non-finite JSON number")

    def finite_float(encoded: str) -> float:
        decoded = float(encoded)
        if not math.isfinite(decoded):
            raise ValueError("non-finite JSON number")
        return decoded

    try:
        decoded = json.loads(
            value,
            object_pairs_hook=pairs,
            parse_constant=reject_constant,
            parse_float=finite_float,
        )
    except IdentityError:
        raise
    except (UnicodeError, ValueError, RecursionError) as error:
        raise IdentityError(f"{label} is not strict JSON") from error
    if not isinstance(decoded, dict):
        reject(f"{label} must be an object")
    return decoded


def regular_bytes(path: Path, label: str, maximum: int | None = None) -> bytes:
    if maximum is None:
        maximum = MAX_RELEASE_JSON_BYTES
    if maximum <= 0:
        reject(f"{label} has an invalid byte limit")
    try:
        metadata = path.lstat()
        if not stat.S_ISREG(metadata.st_mode):
            reject(f"{label} is not a regular file")
        if not (0 < metadata.st_size <= maximum):
            reject(f"{label} is empty or too large")
        with path.open("rb") as stream:
            opened = os.fstat(stream.fileno())
            if not stat.S_ISREG(opened.st_mode) or (
                opened.st_dev,
                opened.st_ino,
                opened.st_size,
                opened.st_mtime_ns,
                opened.st_ctime_ns,
            ) != (
                metadata.st_dev,
                metadata.st_ino,
                metadata.st_size,
                metadata.st_mtime_ns,
                metadata.st_ctime_ns,
            ):
                reject(f"{label} changed before it could be read")
            content = stream.read(maximum + 1)
            finished = os.fstat(stream.fileno())
        if (
            not (0 < len(content) <= maximum)
            or len(content) != opened.st_size
            or (
                finished.st_dev,
                finished.st_ino,
                finished.st_size,
                finished.st_mtime_ns,
                finished.st_ctime_ns,
            )
            != (
                opened.st_dev,
                opened.st_ino,
                opened.st_size,
                opened.st_mtime_ns,
                opened.st_ctime_ns,
            )
        ):
            reject(f"{label} changed or exceeded its byte limit while being read")
        return content
    except OSError as error:
        raise IdentityError(f"cannot read {label}: {error}") from error


def source_revision(
    root: Path,
    explicit: str | None = None,
    *,
    require_clean_checkout: bool = False,
) -> str:
    """Resolve an exact commit from an explicit release input, Git, or source archive."""

    candidate = explicit or os.environ.get("WORLDSTREAM_BUILD_REVISION")
    observed_git: str | None = None
    try:
        completed = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=root,
            check=False,
            capture_output=True,
            text=True,
            timeout=10,
        )
        if completed.returncode == 0:
            observed_git = completed.stdout.strip().lower()
    except (OSError, subprocess.SubprocessError):
        observed_git = None
    revision_file = root / SOURCE_REVISION_FILE
    if candidate is None and observed_git is not None:
        candidate = observed_git
    if candidate is None and revision_file.is_file() and not revision_file.is_symlink():
        candidate = revision_file.read_text(encoding="utf-8").strip().lower()
    if not isinstance(candidate, str) or GIT_REVISION.fullmatch(candidate) is None:
        reject(
            "release build source revision must be exactly 40 lowercase hex characters"
        )
    if observed_git is not None and observed_git != candidate:
        reject(
            f"release build source revision differs from checkout HEAD: {candidate} != {observed_git}"
        )
    if observed_git is not None and require_clean_checkout:
        try:
            status = subprocess.run(
                [
                    "git",
                    "status",
                    "--porcelain=v1",
                    "--untracked-files=all",
                    "--ignore-submodules=none",
                ],
                cwd=root,
                check=False,
                capture_output=True,
                timeout=30,
            )
        except (OSError, subprocess.SubprocessError) as error:
            raise IdentityError(
                "release build could not verify that the Git checkout is clean"
            ) from error
        if status.returncode != 0:
            reject("release build could not verify that the Git checkout is clean")
        if status.stdout:
            reject(
                "release build requires a clean Git checkout so the claimed commit "
                "binds every packaged source byte"
            )
    return candidate


def tracked_source_paths(root: Path) -> set[str] | None:
    """Enumerate only commit-bound source files, including submodule contents."""

    try:
        inside = subprocess.run(
            ["git", "rev-parse", "--is-inside-work-tree"],
            cwd=root,
            check=False,
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise IdentityError(
            "release build could not inspect tracked source files"
        ) from error
    if inside.returncode != 0:
        return None
    try:
        listed = subprocess.run(
            ["git", "ls-files", "--cached", "--recurse-submodules", "-z"],
            cwd=root,
            check=False,
            capture_output=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise IdentityError(
            "release build could not enumerate tracked source files"
        ) from error
    if listed.returncode != 0 or not listed.stdout or not listed.stdout.endswith(b"\0"):
        reject("release build could not enumerate tracked source files")
    try:
        paths = [item.decode("utf-8") for item in listed.stdout[:-1].split(b"\0")]
    except UnicodeDecodeError as error:
        raise IdentityError(
            "release build tracked source paths are not UTF-8"
        ) from error
    if len(paths) != len(set(paths)) or any(
        not path or not _safe_archive_name(path) for path in paths
    ):
        reject("release build tracked source inventory is malformed")
    return set(paths)


def pinned_materials_from_root(root: Path) -> dict[str, str]:
    materials: dict[str, str] = {}
    for relative in PINNED_MATERIAL_PATHS:
        content = regular_bytes(root / relative, f"build material {relative}")
        materials[relative] = "sha256:" + sha256_bytes(content)
    return materials


def pinned_materials_from_source(entries: dict[str, bytes]) -> dict[str, str]:
    materials: dict[str, str] = {}
    for relative in PINNED_MATERIAL_PATHS:
        content = entries.get(relative)
        if not isinstance(content, bytes) or not content:
            reject(f"source archive is missing pinned build material {relative}")
        materials[relative] = "sha256:" + sha256_bytes(content)
    return materials


def toolchains_from_materials(entries: dict[str, bytes]) -> dict[str, dict[str, str]]:
    try:
        rust = tomllib.loads(entries["rust-toolchain.toml"].decode("utf-8"))[
            "toolchain"
        ]["channel"]
        node = entries[".node-version"].decode("utf-8").strip()
        python = entries[".python-version"].decode("utf-8").strip()
        uv = entries[".uv-version"].decode("utf-8").strip()
        package = strict_json(entries["package.json"], "package.json")
        package_manager = package["packageManager"]
    except (
        KeyError,
        TypeError,
        UnicodeError,
        json.JSONDecodeError,
        tomllib.TOMLDecodeError,
    ) as error:
        raise IdentityError("checked-in toolchain pins are malformed") from error
    if (
        not isinstance(rust, str)
        or VERSION.fullmatch(rust) is None
        or VERSION.fullmatch(node) is None
        or VERSION.fullmatch(python) is None
        or VERSION.fullmatch(uv) is None
        or package_manager != PNPM_PACKAGE_MANAGER
    ):
        reject("checked-in toolchain pins are not exact semantic versions")
    return {
        "node": {"version": node, "pin": ".node-version"},
        "pnpm": {
            "version": PNPM_VERSION,
            "pin": f"package.json#packageManager={PNPM_PACKAGE_MANAGER}",
        },
        "python": {"version": python, "pin": ".python-version"},
        "rustc": {"version": rust, "pin": "rust-toolchain.toml#toolchain.channel"},
        "uv": {"version": uv, "pin": ".uv-version"},
    }


def source_entries_from_root(root: Path) -> dict[str, bytes]:
    return {
        relative: regular_bytes(root / relative, f"source material {relative}")
        for relative in SOURCE_ENTRY_PATHS
    }


def expected_base_image(entries: dict[str, bytes]) -> str:
    try:
        value = entries[OCI_BASE_IMAGE_PATH].decode("utf-8").strip()
    except (KeyError, UnicodeError) as error:
        raise IdentityError("source has no valid OCI base-image pin") from error
    if re.fullmatch(r"[^\s@]+@sha256:[0-9a-f]{64}", value) is None:
        reject("OCI base-image pin must be an exact image digest")
    return value


def expected_buildkit_image(entries: dict[str, bytes]) -> str:
    try:
        workflow = entries[WORKFLOW_PATH].decode("utf-8")
    except (KeyError, UnicodeError) as error:
        raise IdentityError("source has no valid BuildKit image pin") from error
    matches = re.findall(
        r"(?m)^\s*image=(moby/buildkit@sha256:[0-9a-f]{64})\s*$", workflow
    )
    if matches != [BUILDKIT_IMAGE]:
        reject("release workflow BuildKit image pin is not exact")
    return matches[0]


def expected_buildx_version(entries: dict[str, bytes]) -> str:
    try:
        workflow = entries[WORKFLOW_PATH].decode("utf-8")
    except (KeyError, UnicodeError) as error:
        raise IdentityError("source has no valid Buildx version pin") from error
    matches = re.findall(
        r"(?m)^          version: v([0-9]+\.[0-9]+\.[0-9]+)$", workflow
    )
    if matches != [BUILDX_VERSION]:
        reject("release workflow Buildx version pin is not exact")
    return matches[0]


def expected_zig_toolchain(entries: dict[str, bytes]) -> dict[str, str]:
    """Validate the exact Zig archive and committed musl compiler driver route."""

    try:
        workflow = entries[WORKFLOW_PATH].decode("utf-8")
        archiver_wrapper = entries[ZIG_MUSL_AR_PATH]
        compiler_wrapper = entries[ZIG_MUSL_CC_PATH]
    except (KeyError, UnicodeError) as error:
        raise IdentityError("source has no valid Zig musl toolchain pin") from error
    expected_fragments = {
        ZIG_LINUX_X86_64_URL: 2,
        str(ZIG_LINUX_X86_64_SIZE): 2,
        ZIG_LINUX_X86_64_SHA256: 2,
        ZIG_MUSL_AR_PATH: 2,
        ZIG_MUSL_CC_PATH: 2,
    }
    if any(
        workflow.count(fragment) != count
        for fragment, count in expected_fragments.items()
    ):
        reject("release workflow Zig musl toolchain pin is not exact")
    if sha256_bytes(compiler_wrapper) != ZIG_MUSL_CC_SHA256:
        reject("committed Zig musl C compiler driver bytes drifted")
    if sha256_bytes(archiver_wrapper) != ZIG_MUSL_AR_SHA256:
        reject("committed Zig musl archiver driver bytes drifted")
    return {
        "version": ZIG_VERSION,
        "pin": (
            f"{ZIG_LINUX_X86_64_URL}#sha256:{ZIG_LINUX_X86_64_SHA256};"
            f"size:{ZIG_LINUX_X86_64_SIZE}"
        ),
    }


def expected_dockerfile_frontend(entries: dict[str, bytes]) -> str:
    try:
        dockerfile = entries["packaging/oci/Dockerfile"].decode("utf-8")
    except (KeyError, UnicodeError) as error:
        raise IdentityError("source has no valid Dockerfile frontend pin") from error
    first_line = dockerfile.splitlines()[0] if dockerfile else ""
    prefix = "# syntax="
    value = first_line.removeprefix(prefix) if first_line.startswith(prefix) else ""
    if value != DOCKERFILE_FRONTEND:
        reject("Dockerfile frontend image pin is not exact")
    return value


def cargo_arguments(target: str) -> list[str]:
    if target == "source":
        return []
    arguments = ["build", "--release", "--locked"]
    if target in {"linux-x86_64", "oci-linux-amd64"}:
        arguments.extend(("--target", "x86_64-unknown-linux-musl"))
    elif target == "windows-x64":
        arguments.extend(("--target", "x86_64-pc-windows-msvc"))
    else:
        reject(f"unsupported build target: {target}")
    arguments.append("--workspace")
    return arguments


def package_arguments(target: str, epoch: int) -> list[str]:
    if target == "oci-linux-amd64":
        return ["oci-context", "--source-date-epoch", str(epoch)]
    return ["package", "--target", target, "--source-date-epoch", str(epoch)]


def _is_absolute_tool_path(value: str, target: str) -> bool:
    if (
        not value
        or len(value) > 4096
        or any(ord(character) < 0x20 for character in value)
    ):
        return False
    if target == "windows-x64":
        return PureWindowsPath(value).is_absolute()
    return PurePosixPath(value).is_absolute()


def _validate_observed_tool(
    value: object,
    *,
    target: str,
    label: str,
    expected_rustc_version: str | None = None,
) -> None:
    if not isinstance(value, dict) or set(value) != {
        "path",
        "version",
        "target",
        "reported_target",
    }:
        reject(f"observed {label} identity is incomplete")
    path = value.get("path")
    version = value.get("version")
    if not isinstance(path, str) or not _is_absolute_tool_path(path, target):
        reject(f"observed {label} path is not absolute and canonical")
    if (
        not isinstance(version, str)
        or not version
        or len(version) > 512
        or any(ord(character) < 0x20 for character in version)
    ):
        reject(f"observed {label} version is invalid")
    if value.get("target") != TARGET_TRIPLES[target]:
        reject(f"observed {label} target differs from the payload target")
    reported_target = value.get("reported_target")
    if (
        not isinstance(reported_target, str)
        or not reported_target
        or len(reported_target) > 256
        or any(ord(character) < 0x20 for character in reported_target)
    ):
        reject(f"observed {label} reported target is invalid")
    if expected_rustc_version is not None and not (
        version == f"rustc {expected_rustc_version}"
        or version.startswith(f"rustc {expected_rustc_version} ")
    ):
        reject("observed rustc version differs from the pinned Rust toolchain")


def validate_observed_build_environment(
    value: object,
    *,
    target: str,
    expected_rustc_version: str,
    require_hosted: bool,
) -> None:
    """Validate exact upstream runner and native SQLite build-tool observations."""

    if target not in TARGET_TRIPLES:
        reject(f"unsupported observed build target: {target}")
    if not isinstance(value, dict) or set(value) != {
        "runner",
        "rustc",
        "bundled_sqlite",
        "final_linker",
    }:
        reject("observed build environment has the wrong fields")
    runner = value.get("runner")
    if not isinstance(runner, dict) or set(runner) != {
        "provider",
        "os",
        "architecture",
        "image",
        "image_version",
    }:
        reject("observed payload runner identity is incomplete")
    if any(
        not isinstance(item, str)
        or not item
        or len(item) > 256
        or any(ord(character) < 0x20 for character in item)
        for item in runner.values()
    ):
        reject("observed payload runner identity is malformed")
    provider = runner.get("provider")
    if provider == "github-actions":
        expected_runner = {
            "provider": "github-actions",
            **HOSTED_RUNNER_FACTS[target],
            "image_version": runner.get("image_version"),
        }
        if (
            runner != expected_runner
            or RUNNER_IMAGE_VERSION.fullmatch(str(runner.get("image_version", "")))
            is None
        ):
            reject("observed hosted payload runner facts differ from the trusted route")
    elif require_hosted:
        reject("release payload was not built on the trusted GitHub-hosted runner")
    elif runner != {
        "provider": "local-test",
        "os": "local",
        "architecture": "local",
        "image": "local",
        "image_version": "local",
    }:
        reject("non-release payload runner identity is not the canonical local fixture")

    if target == "source":
        if (
            value.get("rustc") is not None
            or value.get("bundled_sqlite") is not None
            or value.get("final_linker") is not None
        ):
            reject("source archive must not claim native compiler observations")
        return
    _validate_observed_tool(
        value.get("rustc"),
        target=target,
        label="rustc",
        expected_rustc_version=expected_rustc_version,
    )
    bundled_sqlite = value.get("bundled_sqlite")
    if not isinstance(bundled_sqlite, dict) or set(bundled_sqlite) != {
        "archiver",
        "c_compiler",
    }:
        reject("observed bundled SQLite toolchain is incomplete")
    _validate_observed_tool(
        bundled_sqlite.get("c_compiler"),
        target=target,
        label="bundled SQLite C compiler",
    )
    _validate_observed_tool(
        bundled_sqlite.get("archiver"),
        target=target,
        label="bundled SQLite archiver",
    )
    _validate_observed_tool(
        value.get("final_linker"),
        target=target,
        label="final native linker",
    )
    if provider == "github-actions":
        hosted_targets = {
            "linux-x86_64": {
                "rustc": "x86_64-unknown-linux-gnu",
                "archiver": "gnu-archive",
                "c_compiler": "x86_64-unknown-linux-musl",
                "linker": "elf_x86_64",
            },
            "windows-x64": {
                "rustc": "x86_64-pc-windows-msvc",
                "archiver": "x64-coff-library",
                "c_compiler": "x64",
                "linker": "x64",
            },
            "oci-linux-amd64": {
                "rustc": "x86_64-unknown-linux-gnu",
                "archiver": "gnu-archive",
                "c_compiler": "x86_64-unknown-linux-musl",
                "linker": "elf_x86_64",
            },
        }[target]
        if (
            value["rustc"]["reported_target"] != hosted_targets["rustc"]
            or bundled_sqlite["c_compiler"]["reported_target"]
            != hosted_targets["c_compiler"]
            or bundled_sqlite["archiver"]["reported_target"]
            != hosted_targets["archiver"]
            or value["final_linker"]["reported_target"] != hosted_targets["linker"]
        ):
            reject("observed native tool target differs from the trusted hosted route")


def local_observed_build_environment(
    target: str, *, rustc_version: str
) -> dict[str, Any]:
    """Return an explicit local-only identity for structural test artifacts."""

    runner = {
        "provider": "local-test",
        "os": "local",
        "architecture": "local",
        "image": "local",
        "image_version": "local",
    }
    if target == "source":
        value: dict[str, Any] = {
            "runner": runner,
            "rustc": None,
            "bundled_sqlite": None,
            "final_linker": None,
        }
    else:
        suffix = ".exe" if target == "windows-x64" else ""
        root = (
            "C:\\WorldStream\\local-test"
            if target == "windows-x64"
            else "/worldstream/local-test"
        )
        value = {
            "runner": runner,
            "rustc": {
                "path": f"{root}/rustc{suffix}",
                "version": f"rustc {rustc_version} (local-test)",
                "target": TARGET_TRIPLES[target],
                "reported_target": "local-test",
            },
            "bundled_sqlite": {
                "archiver": {
                    "path": f"{root}/sqlite-ar{suffix}",
                    "version": "WorldStream local-test archiver",
                    "target": TARGET_TRIPLES[target],
                    "reported_target": "local-test",
                },
                "c_compiler": {
                    "path": f"{root}/sqlite-cc{suffix}",
                    "version": "WorldStream local-test C compiler",
                    "target": TARGET_TRIPLES[target],
                    "reported_target": "local-test",
                },
            },
            "final_linker": {
                "path": f"{root}/final-linker{suffix}",
                "version": "WorldStream local-test linker",
                "target": TARGET_TRIPLES[target],
                "reported_target": "local-test",
            },
        }
    validate_observed_build_environment(
        value,
        target=target,
        expected_rustc_version=rustc_version,
        require_hosted=False,
    )
    return value


def _observed_command_lines(path: str, arguments: list[str], label: str) -> list[str]:
    try:
        completed = subprocess.run(
            [path, *arguments],
            check=False,
            capture_output=True,
            text=True,
            timeout=15,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise IdentityError(f"cannot observe {label} version") from error
    if completed.returncode not in {0, 1, 2}:
        reject(f"{label} version probe failed")
    lines = [
        line.strip()
        for line in (completed.stdout + "\n" + completed.stderr).splitlines()
        if line.strip()
    ]
    if not lines:
        reject(f"{label} version probe returned no identity")
    if any(
        len(line) > 512 or any(ord(character) < 0x20 for character in line)
        for line in lines
    ):
        reject(f"{label} version probe returned malformed identity")
    return lines


def _observed_command_version(path: str, arguments: list[str], label: str) -> str:
    return _observed_command_lines(path, arguments, label)[0]


def _observed_rustc_host(path: str) -> str:
    hosts = [
        line.removeprefix("host: ")
        for line in _observed_command_lines(path, ["-vV"], "rustc host")
        if line.startswith("host: ")
    ]
    if len(hosts) != 1:
        reject("rustc host target probe is not exact")
    return hosts[0]


def _observed_native_tool_targets(
    *, target: str, archiver_path: str, c_compiler_path: str, linker_path: str
) -> tuple[str, str, str]:
    if target == "windows-x64":
        compiler_lines = _observed_command_lines(
            c_compiler_path, [], "C compiler target"
        )
        if not any(
            re.search(r"\bfor x64\b", line, re.IGNORECASE) for line in compiler_lines
        ):
            reject("MSVC compiler target probe did not report x64")
        normalized_linker_path = linker_path.replace("\\", "/").casefold()
        if "/hostx64/x64/link.exe" not in normalized_linker_path:
            reject("MSVC linker path does not identify the x64 hosted target")
        normalized_archiver_path = archiver_path.replace("\\", "/").casefold()
        if "/hostx64/x64/lib.exe" not in normalized_archiver_path:
            reject("MSVC librarian path does not identify the x64 hosted target")
        _observed_command_lines(archiver_path, ["/?"], "archiver target")
        _observed_command_lines(linker_path, ["/?"], "linker target")
        return "x64", "x64-coff-library", "x64"
    compiler_target = _observed_command_version(
        c_compiler_path, ["-dumpmachine"], "C compiler target"
    )
    if compiler_target != "x86_64-unknown-linux-musl":
        reject("hosted Linux C compiler target is not x86_64-unknown-linux-musl")
    _observed_command_lines(archiver_path, ["--version"], "archiver format")
    _observed_command_lines(
        linker_path,
        ["-flavor", "gnu", "-m", "elf_x86_64", "--version"],
        "linker target",
    )
    return compiler_target, "gnu-archive", "elf_x86_64"


def _observed_tool_path(environment_name: str, target: str, label: str) -> str:
    value = os.environ.get(environment_name, "")
    if not _is_absolute_tool_path(value, target):
        reject(f"{label} selection environment is missing an absolute path")
    path = Path(value)
    try:
        if path.is_symlink() or not path.is_file():
            reject(f"{label} selection is not an exact regular executable")
    except OSError as error:
        raise IdentityError(f"cannot inspect selected {label}") from error
    return value


def capture_observed_build_environment(
    target: str, *, rustc_version: str
) -> dict[str, Any]:
    """Capture the trusted hosted runner and the tools selected for this payload."""

    repository = os.environ.get("GITHUB_REPOSITORY", "")
    workflow_ref = os.environ.get("GITHUB_WORKFLOW_REF", "")
    if (
        os.environ.get("GITHUB_ACTIONS") != "true"
        or repository != "imom39a/worldstream"
        or workflow_ref != f"{repository}/{WORKFLOW_PATH}@refs/heads/main"
    ):
        reject("payload build environment was not captured by the trusted workflow")
    runner = {
        "provider": "github-actions",
        "os": os.environ.get("RUNNER_OS", ""),
        "architecture": os.environ.get("RUNNER_ARCH", ""),
        "image": os.environ.get("ImageOS", ""),
        "image_version": os.environ.get("ImageVersion", ""),
    }
    if target == "source":
        value: dict[str, Any] = {
            "runner": runner,
            "rustc": None,
            "bundled_sqlite": None,
            "final_linker": None,
        }
    else:
        rustc_path = _observed_tool_path("WORLDSTREAM_RUSTC", target, "rustc")
        selections = TOOL_ENVIRONMENT[target]
        archiver_path = _observed_tool_path(
            selections["archiver"], target, "bundled SQLite archiver"
        )
        c_compiler_path = _observed_tool_path(
            selections["c_compiler"], target, "bundled SQLite C compiler"
        )
        linker_path = _observed_tool_path(
            selections["linker"], target, "final native linker"
        )
        windows = target == "windows-x64"
        if not windows:
            zig_path = _observed_tool_path(
                "WORLDSTREAM_ZIG", target, "Zig musl compiler"
            )
            runner_temp = os.environ.get("RUNNER_TEMP", "")
            workspace = os.environ.get("GITHUB_WORKSPACE", "")
            expected_zig_path = str(
                Path(runner_temp)
                / ZIG_LINUX_X86_64_ARCHIVE.removesuffix(".tar.xz")
                / "zig"
            )
            expected_wrapper_path = str(Path(workspace) / ZIG_MUSL_CC_PATH)
            expected_archiver_path = str(Path(workspace) / ZIG_MUSL_AR_PATH)
            if (
                not runner_temp
                or not workspace
                or zig_path != expected_zig_path
                or c_compiler_path != expected_wrapper_path
                or archiver_path != expected_archiver_path
                or sha256_bytes(
                    regular_bytes(Path(c_compiler_path), "Zig musl compiler driver")
                )
                != ZIG_MUSL_CC_SHA256
                or sha256_bytes(
                    regular_bytes(Path(archiver_path), "Zig musl archiver driver")
                )
                != ZIG_MUSL_AR_SHA256
                or _observed_command_version(zig_path, ["version"], "Zig")
                != ZIG_VERSION
            ):
                reject("selected Zig musl compiler route differs from the exact pin")
        c_compiler_target, archiver_target, linker_target = (
            _observed_native_tool_targets(
                target=target,
                archiver_path=archiver_path,
                c_compiler_path=c_compiler_path,
                linker_path=linker_path,
            )
        )
        value = {
            "runner": runner,
            "rustc": {
                "path": rustc_path,
                "version": _observed_command_version(
                    rustc_path, ["--version"], "rustc"
                ),
                "target": TARGET_TRIPLES[target],
                "reported_target": _observed_rustc_host(rustc_path),
            },
            "bundled_sqlite": {
                "archiver": {
                    "path": archiver_path,
                    "version": _observed_command_version(
                        archiver_path,
                        ["/?"] if windows else ["--version"],
                        "archiver",
                    ),
                    "target": TARGET_TRIPLES[target],
                    "reported_target": archiver_target,
                },
                "c_compiler": {
                    "path": c_compiler_path,
                    "version": _observed_command_version(
                        c_compiler_path, [] if windows else ["--version"], "C compiler"
                    ),
                    "target": TARGET_TRIPLES[target],
                    "reported_target": c_compiler_target,
                },
            },
            "final_linker": {
                "path": linker_path,
                "version": _observed_command_version(
                    linker_path,
                    ["/?"] if windows else ["-flavor", "gnu", "--version"],
                    "linker",
                ),
                "target": TARGET_TRIPLES[target],
                "reported_target": linker_target,
            },
        }
    validate_observed_build_environment(
        value,
        target=target,
        expected_rustc_version=rustc_version,
        require_hosted=True,
    )
    return value


def observed_build_environment_label(value: dict[str, Any]) -> str:
    """Encode one canonical observed environment for an OCI string label."""

    return base64.b64encode(canonical_json(value)).decode("ascii")


def observed_build_environment_from_label(value: str) -> dict[str, Any]:
    """Decode a canonical OCI environment label without accepting aliases."""

    try:
        content = base64.b64decode(value, validate=True)
    except (ValueError, binascii.Error) as error:
        raise IdentityError("OCI observed build environment is not base64") from error
    decoded = strict_json(content, "OCI observed build environment")
    if observed_build_environment_label(decoded) != value:
        reject("OCI observed build environment label is not canonical")
    return decoded


def build_identity(
    *,
    target: str,
    revision: str,
    source_entries: dict[str, bytes],
    source_date_epoch: int,
    manifest_sha256: str,
    base_image: str | None = None,
    observed_build_environment: dict[str, Any] | None = None,
) -> dict[str, Any]:
    if target not in TARGET_TRIPLES:
        reject(f"unsupported release build target: {target}")
    if GIT_REVISION.fullmatch(revision) is None:
        reject("release build identity has an invalid source revision")
    if (
        isinstance(source_date_epoch, bool)
        or not isinstance(source_date_epoch, int)
        or source_date_epoch < 0
    ):
        reject("release build identity has an invalid SOURCE_DATE_EPOCH")
    if SHA256.fullmatch(manifest_sha256) is None:
        reject("release build identity has an invalid manifest digest")
    missing_materials = sorted(set(PINNED_MATERIAL_PATHS) - set(source_entries))
    if missing_materials or any(
        not isinstance(source_entries.get(relative), bytes)
        or not source_entries[relative]
        for relative in PINNED_MATERIAL_PATHS
    ):
        reject(
            "release build identity is missing pinned source materials: "
            + ", ".join(missing_materials or ["empty material"])
        )
    materials = {
        relative: "sha256:" + sha256_bytes(source_entries[relative])
        for relative in PINNED_MATERIAL_PATHS
    }
    toolchains = toolchains_from_materials(source_entries)
    if target in {"linux-x86_64", "oci-linux-amd64"}:
        toolchains["zig"] = expected_zig_toolchain(source_entries)
    if observed_build_environment is None:
        observed_build_environment = local_observed_build_environment(
            target, rustc_version=toolchains["rustc"]["version"]
        )
    validate_observed_build_environment(
        observed_build_environment,
        target=target,
        expected_rustc_version=toolchains["rustc"]["version"],
        require_hosted=False,
    )
    compiler: dict[str, Any] | None = None
    if target != "source":
        compiler = {
            "arguments": cargo_arguments(target),
            "name": "rustc",
            "target_triple": TARGET_TRIPLES[target],
            "version": toolchains["rustc"]["version"],
        }
    if target == "oci-linux-amd64":
        toolchains["buildx"] = {
            "version": expected_buildx_version(source_entries),
            "pin": f"{WORKFLOW_PATH}#native.setup-buildx.version",
        }
        expected = expected_base_image(source_entries)
        if base_image != expected:
            reject("OCI build base image differs from the checked-in exact pin")
        oci: dict[str, Any] | None = {
            "base_image": expected,
            "buildkit_image": expected_buildkit_image(source_entries),
            "dockerfile_frontend": expected_dockerfile_frontend(source_entries),
            "build_arguments": {
                "MANIFEST_SHA256": manifest_sha256,
                "SOURCE_DATE_EPOCH": str(source_date_epoch),
                "SOURCE_REVISION": revision,
                "VERSION": _product_version(source_entries),
                "WORLDSTREAM_BASE_IMAGE": expected,
                "BUILD_ENVIRONMENT_BASE64": observed_build_environment_label(
                    observed_build_environment
                ),
            },
            "platform": "linux/amd64",
        }
    elif base_image is not None:
        reject("non-OCI build identity must not claim an OCI base image")
    else:
        oci = None
    return {
        "schema": BUILD_IDENTITY_SCHEMA,
        "source": {"repository": REPOSITORY, "revision": revision},
        "target": {
            "profile": target,
            "runner": TARGET_RUNNERS[target],
            "triple": TARGET_TRIPLES[target],
        },
        "observed_build_environment": observed_build_environment,
        "toolchains": toolchains,
        "compiler": compiler,
        "arguments": {
            "cargo": cargo_arguments(target),
            "package": package_arguments(target, source_date_epoch),
        },
        "materials": materials,
        "manifest_sha256": "sha256:" + manifest_sha256,
        "source_date_epoch": source_date_epoch,
        "oci": oci,
    }


def _product_version(entries: dict[str, bytes]) -> str:
    try:
        value = tomllib.loads(entries["compatibility.toml"].decode("utf-8"))[
            "contracts"
        ]["product"]
    except (KeyError, TypeError, UnicodeError, tomllib.TOMLDecodeError) as error:
        raise IdentityError(
            "source compatibility contract has no product version"
        ) from error
    if not isinstance(value, str) or VERSION.fullmatch(value) is None:
        reject("source compatibility contract product version is invalid")
    return value


def validate_build_identity(
    value: dict[str, Any],
    *,
    target: str,
    source_entries: dict[str, bytes],
    revision: str,
    source_date_epoch: int,
    manifest_sha256: str,
    base_image: str | None = None,
    require_hosted_environment: bool = False,
) -> None:
    observed_build_environment = value.get("observed_build_environment")
    toolchains = toolchains_from_materials(source_entries)
    validate_observed_build_environment(
        observed_build_environment,
        target=target,
        expected_rustc_version=toolchains["rustc"]["version"],
        require_hosted=require_hosted_environment,
    )
    expected = build_identity(
        target=target,
        revision=revision,
        source_entries=source_entries,
        source_date_epoch=source_date_epoch,
        manifest_sha256=manifest_sha256,
        base_image=base_image,
        observed_build_environment=observed_build_environment,
    )
    if value != expected:
        reject(
            f"{target} build identity is not the exact canonical source/build projection"
        )


def validate_build_identity_shape(
    value: dict[str, Any],
    *,
    target: str,
    manifest_sha256: str,
    source_date_epoch: int,
) -> None:
    """Validate a standalone archive identity before the source payload is available."""

    if (
        set(value)
        != {
            "schema",
            "source",
            "target",
            "observed_build_environment",
            "toolchains",
            "compiler",
            "arguments",
            "materials",
            "manifest_sha256",
            "source_date_epoch",
            "oci",
        }
        or value.get("schema") != BUILD_IDENTITY_SCHEMA
    ):
        reject("archive build identity has the wrong schema or fields")
    source = value.get("source")
    target_value = value.get("target")
    if (
        source
        != {
            "repository": REPOSITORY,
            "revision": source.get("revision") if isinstance(source, dict) else None,
        }
        or GIT_REVISION.fullmatch(str(source.get("revision", ""))) is None
    ):
        reject("archive build identity source commit is invalid")
    if target_value != {
        "profile": target,
        "runner": TARGET_RUNNERS[target],
        "triple": TARGET_TRIPLES[target],
    }:
        reject("archive build identity target is invalid")
    materials = value.get("materials")
    if not isinstance(materials, dict) or set(materials) != set(PINNED_MATERIAL_PATHS):
        reject("archive build identity material inventory is not exact")
    if any(
        not isinstance(item, str) or SHA256_REF.fullmatch(item) is None
        for item in materials.values()
    ):
        reject("archive build identity contains an invalid material digest")
    if value.get("manifest_sha256") != "sha256:" + manifest_sha256:
        reject("archive build identity manifest digest is invalid")
    if value.get("source_date_epoch") != source_date_epoch:
        reject("archive build identity SOURCE_DATE_EPOCH is invalid")
    arguments = value.get("arguments")
    if arguments != {
        "cargo": cargo_arguments(target),
        "package": package_arguments(target, source_date_epoch),
    }:
        reject("archive build identity arguments are not canonical")
    toolchains = value.get("toolchains")
    expected_toolchains = {
        "node",
        "pnpm",
        "python",
        "rustc",
        "uv",
    }
    if target in {"linux-x86_64", "oci-linux-amd64"}:
        expected_toolchains.add("zig")
    if target == "oci-linux-amd64":
        expected_toolchains.add("buildx")
    if not isinstance(toolchains, dict) or set(toolchains) != expected_toolchains:
        reject("archive build identity toolchain graph is incomplete")
    for name, pin in toolchains.items():
        if (
            not isinstance(pin, dict)
            or set(pin) != {"version", "pin"}
            or not isinstance(pin.get("version"), str)
            or VERSION.fullmatch(pin["version"]) is None
            or not isinstance(pin.get("pin"), str)
            or not pin["pin"]
        ):
            reject(f"archive build identity toolchain {name} is invalid")
    validate_observed_build_environment(
        value.get("observed_build_environment"),
        target=target,
        expected_rustc_version=toolchains["rustc"]["version"],
        require_hosted=False,
    )
    compiler = value.get("compiler")
    if target == "source":
        if compiler is not None or value.get("oci") is not None:
            reject("source build identity must not claim a compiler or OCI base")
    else:
        if compiler != {
            "arguments": cargo_arguments(target),
            "name": "rustc",
            "target_triple": TARGET_TRIPLES[target],
            "version": toolchains["rustc"]["version"],
        }:
            reject("archive compiler identity is not canonical")
        if target != "oci-linux-amd64" and value.get("oci") is not None:
            reject("native build identity must not claim an OCI base")


def build_identity_digest(value: dict[str, Any]) -> str:
    return "sha256:" + sha256_bytes(canonical_json(value))


def _safe_archive_name(name: str) -> bool:
    parts = PurePosixPath(name).parts
    return (
        bool(parts)
        and not name.startswith("/")
        and "\\" not in name
        and all(part not in {"", ".", ".."} for part in parts)
    )


def native_archive_entries(path: Path) -> dict[str, bytes]:
    """Read bounded regular files needed for identity validation."""

    result: dict[str, bytes] = {}
    maximum_member = 64 * 1024 * 1024
    maximum_total = 512 * 1024 * 1024
    total = 0
    try:
        if path.suffix == ".zip":
            with zipfile.ZipFile(path) as archive:
                for info in archive.infolist():
                    if info.is_dir():
                        continue
                    if (
                        not _safe_archive_name(info.filename)
                        or info.flag_bits & 0x1
                        or not (0 <= info.file_size <= maximum_member)
                        or info.filename in result
                    ):
                        reject("native archive identity member is unsafe")
                    total += info.file_size
                    if total > maximum_total:
                        reject("native archive identity inventory is too large")
                    result[info.filename] = archive.read(info)
        else:
            with tarfile.open(path, "r:*") as archive:
                for member in archive:
                    if member.isdir():
                        continue
                    if (
                        not member.isfile()
                        or not _safe_archive_name(member.name)
                        or not (0 <= member.size <= maximum_member)
                        or member.name in result
                    ):
                        reject("native archive identity member is unsafe")
                    total += member.size
                    if total > maximum_total:
                        reject("native archive identity inventory is too large")
                    source = archive.extractfile(member)
                    if source is None:
                        reject("native archive identity member is unreadable")
                    result[member.name] = source.read(maximum_member + 1)
    except (OSError, tarfile.TarError, zipfile.BadZipFile) as error:
        raise IdentityError("native archive identity is unreadable") from error
    if not result:
        reject("native archive identity is empty")
    return result


def relative_archive_entries(path: Path) -> dict[str, bytes]:
    entries = native_archive_entries(path)
    roots = {PurePosixPath(name).parts[0] for name in entries}
    if len(roots) != 1:
        reject("native archive identity must have one root")
    root = next(iter(roots))
    return {
        PurePosixPath(name).relative_to(root).as_posix(): content
        for name, content in entries.items()
    }


def _oci_json_blob(
    archive: tarfile.TarFile, members: dict[str, tarfile.TarInfo], name: str
) -> dict[str, Any]:
    member = members.get(name)
    if (
        member is None
        or not member.isfile()
        or not (0 < member.size <= 16 * 1024 * 1024)
    ):
        reject("OCI identity JSON blob is missing")
    source = archive.extractfile(member)
    if source is None:
        reject("OCI identity JSON blob is unreadable")
    value = source.read(16 * 1024 * 1024 + 1)
    if len(value) != member.size:
        reject("OCI identity JSON blob size mismatch")
    return strict_json(value, "OCI identity JSON")


def oci_config_labels(path: Path) -> dict[str, str]:
    try:
        with tarfile.open(path, "r:") as archive:
            members = {
                member.name: member
                for member in archive
                if member.isfile() and _safe_archive_name(member.name)
            }
            index = _oci_json_blob(archive, members, "index.json")
            manifests = index.get("manifests")
            if not isinstance(manifests, list) or len(manifests) != 1:
                reject("OCI identity index must contain one manifest")
            manifest_digest = (
                manifests[0].get("digest") if isinstance(manifests[0], dict) else None
            )
            if (
                not isinstance(manifest_digest, str)
                or SHA256_REF.fullmatch(manifest_digest) is None
            ):
                reject("OCI identity manifest digest is invalid")
            manifest = _oci_json_blob(
                archive,
                members,
                "blobs/sha256/" + manifest_digest.removeprefix("sha256:"),
            )
            config = manifest.get("config")
            config_digest = config.get("digest") if isinstance(config, dict) else None
            if (
                not isinstance(config_digest, str)
                or SHA256_REF.fullmatch(config_digest) is None
            ):
                reject("OCI identity config digest is invalid")
            config_value = _oci_json_blob(
                archive,
                members,
                "blobs/sha256/" + config_digest.removeprefix("sha256:"),
            )
    except (OSError, tarfile.TarError) as error:
        raise IdentityError("OCI artifact identity is unreadable") from error
    labels = config_value.get("config", {}).get("Labels")
    if not isinstance(labels, dict) or any(
        not isinstance(key, str) or not isinstance(value, str)
        for key, value in labels.items()
    ):
        reject("OCI artifact has no exact string label identity")
    return labels


def release_payload_identities(
    payloads: dict[str, Path], version: str
) -> tuple[dict[str, dict[str, Any]], dict[str, bytes]]:
    if set(payloads) != set(PAYLOAD_TARGETS):
        reject("release build identity requires exactly all four payloads")
    source_relative = relative_archive_entries(payloads["source-archive"])
    source_entries = {
        path.removeprefix("source/"): content
        for path, content in source_relative.items()
        if path.startswith("source/")
    }
    revision_bytes = source_entries.get(SOURCE_REVISION_FILE)
    if not isinstance(revision_bytes, bytes):
        reject("source archive has no exact revision file")
    try:
        revision = revision_bytes.decode("ascii").strip()
    except UnicodeError as error:
        raise IdentityError("source archive revision is not ASCII") from error
    if GIT_REVISION.fullmatch(revision) is None:
        reject("source archive revision is invalid")
    materials = pinned_materials_from_source(source_entries)
    manifest_sha = materials["compatibility.json"].removeprefix("sha256:")
    if _product_version(source_entries) != version:
        reject("source archive product version differs from release")
    identities: dict[str, dict[str, Any]] = {}
    for artifact_id in (
        "source-archive",
        "native-linux-x86_64-archive",
        "native-windows-x64-archive",
    ):
        relative = (
            source_relative
            if artifact_id == "source-archive"
            else relative_archive_entries(payloads[artifact_id])
        )
        raw = relative.get(BUILD_METADATA_PATH)
        if not isinstance(raw, bytes):
            reject(f"{artifact_id} has no {BUILD_METADATA_PATH}")
        identity = strict_json(raw, f"{artifact_id} build identity")
        target = PAYLOAD_TARGETS[artifact_id]
        validate_build_identity(
            identity,
            target=target,
            source_entries=source_entries,
            revision=revision,
            source_date_epoch=identity.get("source_date_epoch"),
            manifest_sha256=manifest_sha,
            require_hosted_environment=True,
        )
        identities[artifact_id] = identity

    labels = oci_config_labels(payloads["oci-linux-amd64-image"])
    required_labels = {
        "org.opencontainers.image.version": version,
        "org.opencontainers.image.revision": revision,
        "io.worldstream.target": "linux/amd64",
        "io.worldstream.base-image": expected_base_image(source_entries),
    }
    for label, expected in required_labels.items():
        if labels.get(label) != expected:
            reject(f"OCI artifact label {label} differs from exact build identity")
    encoded_environment = labels.get("io.worldstream.build-environment")
    if not isinstance(encoded_environment, str):
        reject("OCI artifact has no observed build-environment label")
    observed_build_environment = observed_build_environment_from_label(
        encoded_environment
    )
    validate_observed_build_environment(
        observed_build_environment,
        target="oci-linux-amd64",
        expected_rustc_version=toolchains_from_materials(source_entries)["rustc"][
            "version"
        ],
        require_hosted=True,
    )
    oci_identity = build_identity(
        target="oci-linux-amd64",
        revision=revision,
        source_entries=source_entries,
        source_date_epoch=0,
        manifest_sha256=manifest_sha,
        base_image=required_labels["io.worldstream.base-image"],
        observed_build_environment=observed_build_environment,
    )
    if labels.get("io.worldstream.build-identity") != build_identity_digest(
        oci_identity
    ):
        reject(
            "OCI artifact build-identity label is not bound to canonical build inputs"
        )
    identities["oci-linux-amd64-image"] = oci_identity
    return identities, source_entries


def _spdx_id(prefix: str, value: str) -> str:
    return f"SPDXRef-{prefix}-{sha256_bytes(value.encode('utf-8'))[:24]}"


def purl_component(value: str, *, safe: str = "") -> str:
    """Percent-encode one package-url component with an explicit safe set."""

    return quote(value, safe=safe)


def product_purl(version: str, revision: str) -> str:
    """Return the canonical WorldStream product purl and exact VCS qualifier."""

    if VERSION.fullmatch(version) is None or GIT_REVISION.fullmatch(revision) is None:
        reject("product package-url input is malformed")
    vcs_url = purl_component(f"git+{REPOSITORY}@{revision}", safe=":")
    return f"pkg:generic/worldstream@{version}?vcs_url={vcs_url}"


def docker_repository_url(repository: str) -> str:
    """Expand one Docker familiar repository name to an explicit registry path."""

    if re.fullmatch(r"[a-z0-9._/-]+(?::[a-zA-Z0-9._-]+)?", repository) is None:
        reject("OCI repository name is not canonical")
    first = repository.split("/", 1)[0]
    if "." in first or ":" in first or first == "localhost":
        return repository
    if "/" in repository:
        return "docker.io/" + repository
    return "docker.io/library/" + repository


def oci_purl(reference: str) -> str:
    """Return the canonical package-url identity for one pinned OCI reference."""

    match = re.fullmatch(
        r"(?P<repository>[a-z0-9._/-]+?)(?::(?P<tag>[a-zA-Z0-9._-]+))?"
        r"@sha256:(?P<digest>[0-9a-f]{64})",
        reference,
    )
    if match is None:
        reject("OCI package-url input is not an exact lowercase digest reference")
    repository = match.group("repository")
    name = repository.rsplit("/", 1)[-1]
    repository_url = docker_repository_url(repository)
    qualifiers: list[tuple[str, str]] = [("repository_url", repository_url)]
    tag = match.group("tag")
    if tag is not None:
        qualifiers.append(("tag", tag))
    suffix = ""
    if qualifiers:
        suffix = "?" + "&".join(
            f"{key}={purl_component(value)}" for key, value in sorted(qualifiers)
        )
    return (
        f"pkg:oci/{purl_component(name, safe='._-')}@"
        f"sha256:{match.group('digest')}{suffix}"
    )


def spdx_document_namespace(
    version: str,
    mirror_digest: str,
    subjects: dict[str, Path],
    created: str,
) -> str:
    """Return the unique namespace for one exact generated SPDX document."""

    if VERSION.fullmatch(version) is None or SHA256.fullmatch(mirror_digest) is None:
        reject("SPDX namespace inputs are malformed")
    validate_spdx_created(created)
    namespace_seed = {
        "schema": "worldstream/spdx-document-namespace/v1",
        "product": version,
        "manifest_sha256": mirror_digest,
        "created": created,
        "subjects": [
            {
                "path": relative,
                "sha256": "sha256:" + sha256_path(path),
                "size_bytes": path.stat().st_size,
            }
            for relative, path in sorted(subjects.items())
        ],
    }
    document_version = sha256_bytes(canonical_json(namespace_seed))
    return f"{REPOSITORY}/spdx/{version}/{document_version}"


def validate_spdx_created(value: object) -> None:
    """Require one real, canonical UTC timestamp with whole-second precision."""

    if not isinstance(value, str):
        reject("SPDX creation time is not a string")
    try:
        parsed = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(
            tzinfo=timezone.utc
        )
    except ValueError as error:
        raise IdentityError("SPDX creation time is not a real UTC second") from error
    if parsed.strftime("%Y-%m-%dT%H:%M:%SZ") != value:
        reject("SPDX creation time is not canonical UTC-second text")


def pnpm_locked_packages(content: bytes) -> list[tuple[str, str]]:
    """Parse the exact package locators from a generated pnpm v9 lockfile."""

    try:
        text = content.decode("utf-8")
    except UnicodeError as error:
        raise IdentityError("pnpm lockfile is not UTF-8") from error
    if "\r" in text or not text.endswith("\n"):
        reject("pnpm lockfile must use canonical LF-terminated text")
    if re.search(r"(?m)^lockfileVersion: ['\"]?9\.0['\"]?$", text) is None:
        reject("pnpm lockfile is not the supported exact v9 format")
    lines = text.splitlines()
    package_markers = [index for index, line in enumerate(lines) if line == "packages:"]
    snapshot_markers = [
        index for index, line in enumerate(lines) if line == "snapshots:"
    ]
    if (
        len(package_markers) != 1
        or len(snapshot_markers) != 1
        or package_markers[0] >= snapshot_markers[0]
    ):
        reject("pnpm lockfile has no unambiguous packages graph")

    packages: set[tuple[str, str]] = set()
    for line in lines[package_markers[0] + 1 : snapshot_markers[0]]:
        if not line or line.startswith("    "):
            continue
        if not line.startswith("  ") or not line.endswith(":"):
            reject("pnpm packages graph contains a malformed locator")
        encoded = line[2:-1]
        if encoded.startswith("'") and encoded.endswith("'"):
            locator = encoded[1:-1].replace("''", "'")
        elif encoded.startswith('"') and encoded.endswith('"'):
            try:
                locator = json.loads(encoded)
            except json.JSONDecodeError as error:
                raise IdentityError(
                    "pnpm package locator has invalid quoting"
                ) from error
        elif not encoded.startswith(("'", '"')) and encoded:
            locator = encoded
        else:
            reject("pnpm package locator has invalid quoting")
        if not isinstance(locator, str):
            reject("pnpm package locator is not text")
        separator = locator.rfind("@")
        name = locator[:separator]
        package_version = locator[separator + 1 :]
        if (
            separator <= 0
            or not name
            or any(character.isspace() for character in name)
            or VERSION.fullmatch(package_version) is None
        ):
            reject(f"pnpm package locator is not an exact name/version: {locator!r}")
        key = (name, package_version)
        if key in packages:
            reject(f"pnpm packages graph repeats a component: {locator}")
        packages.add(key)
    if not packages:
        reject("pnpm lockfile has no component graph")
    return sorted(packages)


def pnpm_locked_components(content: bytes) -> list[tuple[str, str, str]]:
    """Return exact pnpm name/version/integrity rows from the locked graph."""

    packages = pnpm_locked_packages(content)
    text = content.decode("utf-8")
    lines = text.splitlines()
    packages_index = lines.index("packages:")
    snapshots_index = lines.index("snapshots:")
    components: dict[tuple[str, str], str] = {}
    index = packages_index + 1
    while index < snapshots_index:
        line = lines[index]
        if not (
            line.startswith("  ") and not line.startswith("    ") and line.endswith(":")
        ):
            index += 1
            continue
        encoded = line[2:-1]
        if encoded.startswith("'") and encoded.endswith("'"):
            locator = encoded[1:-1].replace("''", "'")
        elif encoded.startswith('"') and encoded.endswith('"'):
            try:
                locator = json.loads(encoded)
            except json.JSONDecodeError as error:
                raise IdentityError(
                    "pnpm package locator has invalid quoting"
                ) from error
        else:
            locator = encoded
        end = index + 1
        while end < snapshots_index and not (
            lines[end].startswith("  ")
            and not lines[end].startswith("    ")
            and lines[end].endswith(":")
        ):
            end += 1
        matches = re.findall(
            r"integrity:\s*(sha512-[A-Za-z0-9+/]+={0,2})(?:[,} ]|$)",
            "\n".join(lines[index + 1 : end]),
        )
        separator = locator.rfind("@")
        key = (locator[:separator], locator[separator + 1 :])
        if separator <= 0 or len(matches) != 1 or key in components:
            reject(f"pnpm package has no unambiguous integrity: {locator!r}")
        try:
            digest = base64.b64decode(matches[0].removeprefix("sha512-"), validate=True)
        except (ValueError, binascii.Error) as error:
            raise IdentityError(
                f"pnpm package integrity is malformed: {locator!r}"
            ) from error
        if len(digest) != hashlib.sha512().digest_size:
            reject(f"pnpm package integrity has the wrong size: {locator!r}")
        components[key] = matches[0]
        index = end
    if set(components) != set(packages):
        reject("pnpm integrity graph differs from the locked package graph")
    return sorted((*key, integrity) for key, integrity in components.items())


def _third_party_license_ids(expression: str) -> list[str]:
    identifiers = [
        token
        for token in re.findall(r"[A-Za-z0-9][A-Za-z0-9.+-]*", expression)
        if token not in {"AND", "OR", "WITH"}
    ]
    if not identifiers:
        reject("third-party component has no declared license identifiers")
    return sorted(set(identifiers))


def _third_party_notice_sections(content: bytes) -> set[str]:
    prefix = (
        b"WorldStream Third-Party Notices\n"
        b"\n"
        b"This deterministic bundle preserves upstream declared-license metadata, "
        b"license terms, and notices for the exact locked Cargo, pnpm, and pinned "
        b"OCI-base component inventories. Component-to-section bindings are in "
        b"THIRD-PARTY-NOTICES.json.\n"
        b"\n"
    )
    if not content.startswith(prefix):
        reject("third-party notice text has a noncanonical preamble")
    pattern = re.compile(
        rb"===== BEGIN NOTICE (sha256:[0-9a-f]{64}) =====\n"
        rb"(.*?)"
        rb"===== END NOTICE \1 =====\n\n",
        re.DOTALL,
    )
    cursor = len(prefix)
    observed: list[str] = []
    for match in pattern.finditer(content, cursor):
        if match.start() != cursor:
            reject("third-party notice text has unbound bytes")
        identifier = match.group(1).decode("ascii")
        notice = match.group(2)
        if not notice.endswith(b"\n") or "sha256:" + sha256_bytes(notice) != identifier:
            reject("third-party notice section digest differs")
        observed.append(identifier)
        cursor = match.end()
    if cursor != len(content) or not observed or observed != sorted(set(observed)):
        reject("third-party notice sections are incomplete, repeated, or unsorted")
    return set(observed)


def validate_third_party_notices(
    source_entries: dict[str, bytes],
) -> dict[tuple[str, str, str], str]:
    """Bind legal notices to exact Cargo, pnpm, and pinned OCI-base inputs."""

    try:
        manifest = strict_json(
            source_entries[THIRD_PARTY_NOTICE_MANIFEST_PATH],
            "third-party notice manifest",
        )
        notice_text = source_entries[THIRD_PARTY_NOTICE_TEXT_PATH]
        cargo_lock = source_entries["Cargo.lock"]
        pnpm_lock = source_entries["pnpm-lock.yaml"]
        base_image_pin = source_entries[OCI_BASE_IMAGE_PATH]
    except KeyError as error:
        raise IdentityError(
            "source is missing the third-party notice bundle"
        ) from error
    if (
        set(manifest)
        != {
            "components",
            "inputs",
            "license_texts",
            "notices",
            "schema",
        }
        or manifest.get("schema") != THIRD_PARTY_NOTICE_SCHEMA
    ):
        reject("third-party notice manifest shape is invalid")
    inputs = manifest.get("inputs")
    if inputs != {
        "Cargo.lock": "sha256:" + sha256_bytes(cargo_lock),
        OCI_BASE_IMAGE_PATH: "sha256:" + sha256_bytes(base_image_pin),
        "pnpm-lock.yaml": "sha256:" + sha256_bytes(pnpm_lock),
        "spdx_license_list_revision": SPDX_LICENSE_LIST_REVISION,
    }:
        reject("third-party notice input identity differs from the locked sources")
    notices = manifest.get("notices")
    if notices != {
        "path": THIRD_PARTY_NOTICE_TEXT_PATH,
        "sha256": "sha256:" + sha256_bytes(notice_text),
        "size_bytes": len(notice_text),
    }:
        reject("third-party notice text identity differs from the manifest")
    sections = _third_party_notice_sections(notice_text)
    license_texts = manifest.get("license_texts")
    if (
        not isinstance(license_texts, dict)
        or not license_texts
        or any(
            not isinstance(license_id, str)
            or not isinstance(identifier, str)
            or SHA256_REF.fullmatch(identifier) is None
            or identifier not in sections
            for license_id, identifier in license_texts.items()
        )
    ):
        reject("third-party standard-license mapping is invalid")

    components = manifest.get("components")
    if not isinstance(components, dict) or set(components) != {
        "cargo",
        "npm",
        "oci_base",
    }:
        reject("third-party component inventory shape is invalid")
    try:
        cargo_document = tomllib.loads(cargo_lock.decode("utf-8"))
    except (UnicodeError, tomllib.TOMLDecodeError) as error:
        raise IdentityError("Cargo.lock is malformed") from error
    cargo_packages = cargo_document.get("package")
    if not isinstance(cargo_packages, list):
        reject("Cargo.lock has no package graph")
    expected_cargo = sorted(
        (
            package.get("name"),
            package.get("version"),
            package.get("source"),
            package.get("checksum"),
        )
        for package in cargo_packages
        if isinstance(package, dict) and isinstance(package.get("source"), str)
    )
    expected_npm = pnpm_locked_components(pnpm_lock)
    declared: dict[tuple[str, str, str], str] = {}
    referenced_sections: set[str] = set()
    observed_license_ids: set[str] = set()

    def validate_row(
        row: object,
        *,
        expected_keys: set[str],
        ecosystem: str,
    ) -> tuple[str, str, str, list[str]]:
        if not isinstance(row, dict) or set(row) != expected_keys:
            reject(f"third-party {ecosystem} component shape is invalid")
        name, version = row.get("name"), row.get("version")
        expression, notice_ids = row.get("declared_license"), row.get("notice_ids")
        if (
            not isinstance(name, str)
            or not name
            or not isinstance(version, str)
            or COMPONENT_VERSION.fullmatch(version) is None
            or not isinstance(expression, str)
            or not expression
            or len(expression) > 256
            or any(ord(character) < 0x20 for character in expression)
            or not isinstance(notice_ids, list)
            or len(notice_ids) < 2
            or notice_ids != sorted(set(notice_ids))
            or any(
                not isinstance(identifier, str) or identifier not in sections
                for identifier in notice_ids
            )
        ):
            reject(f"third-party {ecosystem} component notice is invalid")
        identifiers = _third_party_license_ids(expression)
        if any(
            license_texts.get(identifier) not in notice_ids
            for identifier in identifiers
        ):
            reject(f"third-party {ecosystem} component omits declared license terms")
        key = (ecosystem, name, version)
        if key in declared:
            reject(f"third-party {ecosystem} component is repeated")
        declared[key] = expression
        referenced_sections.update(notice_ids)
        observed_license_ids.update(identifiers)
        return name, version, expression, notice_ids

    cargo_rows = components.get("cargo")
    if not isinstance(cargo_rows, list):
        reject("third-party Cargo component inventory is missing")
    observed_cargo = []
    for row in cargo_rows:
        validate_row(
            row,
            expected_keys={
                "checksum",
                "declared_license",
                "name",
                "notice_ids",
                "source",
                "version",
            },
            ecosystem="cargo",
        )
        observed_cargo.append(
            (row["name"], row["version"], row["source"], row["checksum"])
        )
    if observed_cargo != expected_cargo:
        reject("third-party Cargo notice coverage differs from Cargo.lock")

    npm_rows = components.get("npm")
    if not isinstance(npm_rows, list):
        reject("third-party npm component inventory is missing")
    observed_npm = []
    for row in npm_rows:
        validate_row(
            row,
            expected_keys={
                "declared_license",
                "integrity",
                "name",
                "notice_ids",
                "version",
            },
            ecosystem="npm",
        )
        observed_npm.append((row["name"], row["version"], row["integrity"]))
    if observed_npm != expected_npm:
        reject("third-party npm notice coverage differs from pnpm-lock.yaml")

    oci_base = components.get("oci_base")
    if not isinstance(oci_base, dict) or set(oci_base) != {
        "image",
        "installed_database_sha256",
        "packages",
    }:
        reject("third-party OCI-base inventory shape is invalid")
    if (
        oci_base.get("image") != expected_base_image(source_entries)
        or SHA256_REF.fullmatch(str(oci_base.get("installed_database_sha256"))) is None
        or not isinstance(oci_base.get("packages"), list)
        or not oci_base["packages"]
    ):
        reject("third-party OCI-base identity is invalid")
    observed_oci = []
    for row in oci_base["packages"]:
        validate_row(
            row,
            expected_keys={
                "architecture",
                "declared_license",
                "name",
                "notice_ids",
                "source",
                "version",
            },
            ecosystem="apk",
        )
        if row["architecture"] != "x86_64" or not isinstance(row["source"], str):
            reject("third-party OCI-base package identity is invalid")
        observed_oci.append((row["name"], row["version"]))
    if observed_oci != sorted(set(observed_oci)):
        reject("third-party OCI-base package inventory is repeated or unsorted")
    if set(license_texts) != observed_license_ids or referenced_sections != sections:
        reject("third-party notice sections do not exactly cover the component graph")
    return declared


def spdx_license_expression(declared: str, known_identifiers: set[str]) -> str | None:
    """Return a valid SPDX expression without erasing the raw upstream value."""

    candidate = "MIT OR Apache-2.0" if declared == "MIT/Apache-2.0" else declared
    tokens = re.findall(r"\(|\)|AND|OR|WITH|[A-Za-z0-9][A-Za-z0-9.+-]*", candidate)
    if (
        not tokens
        or " ".join(tokens).replace("( ", "(").replace(" )", ")") != candidate
    ):
        return None
    position = 0

    def primary() -> bool:
        nonlocal position
        if position >= len(tokens):
            return False
        if tokens[position] == "(":
            position += 1
            if not disjunction() or position >= len(tokens) or tokens[position] != ")":
                return False
            position += 1
            return True
        token = tokens[position]
        if token in {"AND", "OR", "WITH", ")"} or token not in known_identifiers:
            return False
        position += 1
        if position < len(tokens) and tokens[position] == "WITH":
            position += 1
            if position >= len(tokens) or tokens[position] not in known_identifiers:
                return False
            position += 1
        return True

    def conjunction() -> bool:
        nonlocal position
        if not primary():
            return False
        while position < len(tokens) and tokens[position] == "AND":
            position += 1
            if not primary():
                return False
        return True

    def disjunction() -> bool:
        nonlocal position
        if not conjunction():
            return False
        while position < len(tokens) and tokens[position] == "OR":
            position += 1
            if not conjunction():
                return False
        return True

    return candidate if disjunction() and position == len(tokens) else None


def component_purl(ecosystem: str, name: str, package_version: str) -> str:
    """Encode one locked package identity as a canonical package-url."""

    namespace = {"cargo": "cargo", "python": "pypi", "npm": "npm"}.get(ecosystem)
    if namespace is None or COMPONENT_VERSION.fullmatch(package_version) is None:
        reject("component package-url input is malformed")
    plain_name = re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._~-]*", name) is not None
    scoped_npm_name = (
        ecosystem == "npm"
        and re.fullmatch(
            r"@[A-Za-z0-9][A-Za-z0-9._~-]*/[A-Za-z0-9][A-Za-z0-9._~-]*",
            name,
        )
        is not None
    )
    if not plain_name and not scoped_npm_name:
        reject("component package-url name is malformed")
    encoded_name = purl_component(name, safe="/._-~")
    encoded_version = purl_component(package_version, safe="._-~")
    return f"pkg:{namespace}/{encoded_name}@{encoded_version}"


def apk_component_purl(name: str, version: str, architecture: str) -> str:
    """Return the canonical purl for one exact installed Alpine package."""

    if (
        re.fullmatch(r"[a-z0-9][a-z0-9+._-]*", name) is None
        or COMPONENT_VERSION.fullmatch(version) is None
        or architecture != "x86_64"
    ):
        reject("APK component package-url input is malformed")
    return (
        f"pkg:apk/alpine/{purl_component(name, safe='._-')}@"
        f"{purl_component(version, safe='._-')}?arch=x86_64"
    )


def apk_component_packages(source_entries: dict[str, bytes]) -> list[dict[str, Any]]:
    """Represent every package installed in the exact pinned OCI base image."""

    raw_licenses = validate_third_party_notices(source_entries)
    manifest = strict_json(
        source_entries[THIRD_PARTY_NOTICE_MANIFEST_PATH],
        "third-party notice manifest",
    )
    known_identifiers = set(manifest["license_texts"])
    base = manifest["components"]["oci_base"]
    packages = []
    for row in base["packages"]:
        key = ("apk", row["name"], row["version"])
        raw = raw_licenses[key]
        declared = spdx_license_expression(raw, known_identifiers) or "NOASSERTION"
        packages.append(
            {
                "SPDXID": _spdx_id("OCIBaseComponent", "\0".join(key)),
                "name": row["name"],
                "versionInfo": row["version"],
                "downloadLocation": row["source"],
                "filesAnalyzed": False,
                "licenseConcluded": "NOASSERTION",
                "licenseDeclared": declared,
                "copyrightText": "NOASSERTION",
                "externalRefs": [
                    {
                        "referenceCategory": "PACKAGE-MANAGER",
                        "referenceType": "purl",
                        "referenceLocator": apk_component_purl(
                            row["name"], row["version"], row["architecture"]
                        ),
                    }
                ],
                "attributionTexts": [
                    (
                        f"Upstream declared license: {raw}. See "
                        f"{THIRD_PARTY_NOTICE_TEXT_PATH} and "
                        f"{THIRD_PARTY_NOTICE_MANIFEST_PATH}."
                    )
                ],
                "comment": (
                    f"architecture={row['architecture']}; "
                    f"base_image={base['image']}; "
                    f"installed_database={base['installed_database_sha256']}"
                ),
            }
        )
    packages.sort(key=lambda item: item["SPDXID"])
    return packages


def component_packages(
    source_entries: dict[str, bytes], version: str
) -> tuple[list[dict[str, Any]], list[tuple[str, str]]]:
    """Return deterministic first-party and locked dependency package identities."""

    third_party_licenses = validate_third_party_notices(source_entries)
    notice_manifest = strict_json(
        source_entries[THIRD_PARTY_NOTICE_MANIFEST_PATH],
        "third-party notice manifest",
    )
    known_license_identifiers = set(notice_manifest["license_texts"])
    packages: list[dict[str, Any]] = []
    relationships: list[tuple[str, str]] = []
    observed: set[tuple[str, str, str]] = set()

    def add(ecosystem: str, name: str, package_version: str, source: str) -> str:
        key = (ecosystem, name, package_version)
        spdx_id = _spdx_id("Component", "\0".join(key))
        if key in observed:
            return spdx_id
        observed.add(key)
        raw_declared_license = third_party_licenses.get(key)
        declared_license = (
            spdx_license_expression(raw_declared_license, known_license_identifiers)
            if raw_declared_license is not None
            else None
        )
        concluded_license = "Apache-2.0" if source == REPOSITORY else "NOASSERTION"
        packages.append(
            {
                "SPDXID": spdx_id,
                "name": name,
                "versionInfo": package_version,
                "downloadLocation": source or "NOASSERTION",
                "filesAnalyzed": False,
                "licenseConcluded": concluded_license,
                "licenseDeclared": declared_license or concluded_license,
                "copyrightText": "NOASSERTION",
                "externalRefs": [
                    {
                        "referenceCategory": "PACKAGE-MANAGER",
                        "referenceType": "purl",
                        "referenceLocator": component_purl(
                            ecosystem, name, package_version
                        ),
                    }
                ],
                **(
                    {
                        "attributionTexts": [
                            (
                                f"Upstream declared license: {raw_declared_license}. "
                                f"See {THIRD_PARTY_NOTICE_TEXT_PATH} and "
                                f"{THIRD_PARTY_NOTICE_MANIFEST_PATH}."
                            )
                        ]
                    }
                    if raw_declared_license is not None
                    else {}
                ),
            }
        )
        return spdx_id

    try:
        cargo_lock = tomllib.loads(source_entries["Cargo.lock"].decode("utf-8"))
        uv_lock = tomllib.loads(source_entries["sdk/python/uv.lock"].decode("utf-8"))
        pnpm_packages = pnpm_locked_packages(source_entries["pnpm-lock.yaml"])
        ui = strict_json(
            source_entries["web/console/package.json"], "web/console/package.json"
        )
    except (
        KeyError,
        TypeError,
        UnicodeError,
        json.JSONDecodeError,
        tomllib.TOMLDecodeError,
    ) as error:
        raise IdentityError("locked component manifests are malformed") from error
    cargo_packages = cargo_lock.get("package")
    uv_packages = uv_lock.get("package")
    if not isinstance(cargo_packages, list) or not cargo_packages:
        reject("Cargo.lock has no component graph")
    if not isinstance(uv_packages, list) or not uv_packages:
        reject("uv.lock has no component graph")
    for item in cargo_packages:
        if not isinstance(item, dict):
            reject("Cargo.lock contains an invalid package")
        name, item_version = item.get("name"), item.get("version")
        if not isinstance(name, str) or not isinstance(item_version, str):
            reject("Cargo.lock package has no exact name/version")
        source = item.get("source")
        add(
            "cargo",
            name,
            item_version,
            source if isinstance(source, str) else REPOSITORY,
        )
    for item in uv_packages:
        if not isinstance(item, dict):
            reject("uv.lock contains an invalid package")
        name, item_version = item.get("name"), item.get("version")
        if not isinstance(name, str) or not isinstance(item_version, str):
            reject("uv.lock package has no exact name/version")
        source_value = item.get("source")
        if (
            isinstance(source_value, dict)
            and set(source_value) == {"registry"}
            and isinstance(source_value["registry"], str)
            and re.fullmatch(r"https://[^\s]+", source_value["registry"]) is not None
        ):
            source = source_value["registry"]
        elif source_value == {"editable": "."}:
            source = REPOSITORY
        else:
            reject(f"uv.lock package {name} has an unsupported source identity")
        add("python", name, item_version, source)
    for name, item_version in pnpm_packages:
        add("npm", name, item_version, "https://registry.npmjs.org/")
    if not isinstance(ui, dict) or not isinstance(ui.get("name"), str):
        reject("UI package has no component identity")
    ui_version = ui.get("version")
    if ui_version != version:
        reject("UI component version differs from release")
    add("npm", ui["name"], ui_version, REPOSITORY)
    packages.sort(key=lambda item: item["SPDXID"])
    return packages, relationships


def spdx_graph(
    *,
    version: str,
    revision: str,
    subjects: dict[str, Path],
    identities: dict[str, dict[str, Any]],
    source_entries: dict[str, bytes],
) -> tuple[list[dict[str, Any]], list[dict[str, str]], list[str]]:
    product_id = "SPDXRef-WorldStream-Product"
    source_id = "SPDXRef-WorldStream-Source"
    base_id = "SPDXRef-WorldStream-OCI-Base"
    buildkit_id = "SPDXRef-WorldStream-OCI-BuildKit"
    frontend_id = "SPDXRef-WorldStream-OCI-Dockerfile-Frontend"
    packages = [
        {
            "SPDXID": product_id,
            "name": "worldstream",
            "versionInfo": version,
            "downloadLocation": f"{REPOSITORY}@{revision}",
            "filesAnalyzed": False,
            "licenseConcluded": "Apache-2.0",
            "copyrightText": "NOASSERTION",
            "externalRefs": [
                {
                    "referenceCategory": "PACKAGE-MANAGER",
                    "referenceType": "purl",
                    "referenceLocator": product_purl(version, revision),
                }
            ],
        },
        {
            "SPDXID": source_id,
            "name": "worldstream-source",
            "versionInfo": revision,
            "downloadLocation": f"git+{REPOSITORY}@{revision}",
            "filesAnalyzed": False,
            "licenseConcluded": "Apache-2.0",
            "copyrightText": "NOASSERTION",
        },
    ]
    components, _unused = component_packages(source_entries, version)
    apk_components = apk_component_packages(source_entries)
    packages.extend(components)
    packages.extend(apk_components)
    base = expected_base_image(source_entries)
    base_name, base_digest = base.split("@sha256:", 1)
    packages.append(
        {
            "SPDXID": base_id,
            "name": base_name,
            "versionInfo": "sha256:" + base_digest,
            "downloadLocation": "NOASSERTION",
            "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION",
            "copyrightText": "NOASSERTION",
            "externalRefs": [
                {
                    "referenceCategory": "PACKAGE-MANAGER",
                    "referenceType": "purl",
                    "referenceLocator": oci_purl(base),
                }
            ],
        }
    )
    for identifier, reference in (
        (buildkit_id, expected_buildkit_image(source_entries)),
        (frontend_id, expected_dockerfile_frontend(source_entries)),
    ):
        image_name, image_digest = reference.split("@sha256:", 1)
        packages.append(
            {
                "SPDXID": identifier,
                "name": image_name,
                "versionInfo": "sha256:" + image_digest,
                "downloadLocation": "NOASSERTION",
                "filesAnalyzed": False,
                "licenseConcluded": "NOASSERTION",
                "copyrightText": "NOASSERTION",
                "externalRefs": [
                    {
                        "referenceCategory": "PACKAGE-MANAGER",
                        "referenceType": "purl",
                        "referenceLocator": oci_purl(reference),
                    }
                ],
            }
        )
    tool_ids: list[str] = []
    toolchains: dict[str, dict[str, str]] = {}
    for identity in identities.values():
        for name, pin in identity["toolchains"].items():
            if name in toolchains and toolchains[name] != pin:
                reject(f"SPDX build tool identity conflicts for {name}")
            toolchains[name] = pin
    for name, pin in sorted(toolchains.items()):
        tool_id = _spdx_id("BuildTool", f"{name}@{pin['version']}")
        tool_ids.append(tool_id)
        package = {
            "SPDXID": tool_id,
            "name": name,
            "versionInfo": pin["version"],
            "downloadLocation": "NOASSERTION",
            "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION",
            "copyrightText": "NOASSERTION",
        }
        if name == "zig":
            package["downloadLocation"] = ZIG_LINUX_X86_64_URL
            package["checksums"] = [
                {
                    "algorithm": "SHA256",
                    "checksumValue": ZIG_LINUX_X86_64_SHA256,
                }
            ]
        elif name == "pnpm":
            package["downloadLocation"] = PNPM_TARBALL_URL
            package["checksums"] = [
                {
                    "algorithm": "SHA512",
                    "checksumValue": PNPM_SHA512,
                }
            ]
            package["comment"] = pin["pin"]
        packages.append(package)
    for identity in identities.values():
        profile = identity["target"]["profile"]
        environment = identity["observed_build_environment"]
        if profile == "source":
            continue
        observed_tools = {
            "rustc": environment["rustc"],
            "bundled-sqlite-c-compiler": environment["bundled_sqlite"]["c_compiler"],
            "bundled-sqlite-archiver": environment["bundled_sqlite"]["archiver"],
            "final-linker": environment["final_linker"],
        }
        for role, observed in sorted(observed_tools.items()):
            encoded = json.dumps(observed, sort_keys=True, separators=(",", ":"))
            tool_id = _spdx_id("ObservedBuildTool", f"{profile}\0{role}\0{encoded}")
            tool_ids.append(tool_id)
            packages.append(
                {
                    "SPDXID": tool_id,
                    "name": f"{profile}-{role}",
                    "versionInfo": observed["version"],
                    "downloadLocation": "NOASSERTION",
                    "filesAnalyzed": False,
                    "licenseConcluded": "NOASSERTION",
                    "copyrightText": "NOASSERTION",
                    "comment": encoded,
                }
            )
    file_ids = {relative: _spdx_id("ReleaseSubject", relative) for relative in subjects}
    relationships = [
        {
            "spdxElementId": "SPDXRef-DOCUMENT",
            "relationshipType": "DESCRIBES",
            "relatedSpdxElement": product_id,
        },
        {
            "spdxElementId": product_id,
            "relationshipType": "GENERATED_FROM",
            "relatedSpdxElement": source_id,
        },
    ]
    relationships.extend(
        {
            "spdxElementId": product_id,
            "relationshipType": "DEPENDS_ON",
            "relatedSpdxElement": item["SPDXID"],
        }
        for item in components
    )
    relationships.extend(
        {
            "spdxElementId": base_id,
            "relationshipType": "CONTAINS",
            "relatedSpdxElement": item["SPDXID"],
        }
        for item in apk_components
    )
    relationships.extend(
        {
            "spdxElementId": tool_id,
            "relationshipType": "BUILD_TOOL_OF",
            "relatedSpdxElement": product_id,
        }
        for tool_id in tool_ids
    )
    relationships.extend(
        {
            "spdxElementId": file_id,
            "relationshipType": "GENERATED_FROM",
            "relatedSpdxElement": source_id,
        }
        for file_id in file_ids.values()
    )
    oci_relative = next(
        (
            relative
            for relative in subjects
            if relative.endswith("-oci-linux-amd64.oci.tar")
        ),
        None,
    )
    if oci_relative is None:
        reject("SPDX graph has no OCI payload subject")
    relationships.append(
        {
            "spdxElementId": file_ids[oci_relative],
            "relationshipType": "DEPENDS_ON",
            "relatedSpdxElement": base_id,
        }
    )
    relationships.extend(
        {
            "spdxElementId": tool_id,
            "relationshipType": "BUILD_TOOL_OF",
            "relatedSpdxElement": file_ids[oci_relative],
        }
        for tool_id in (buildkit_id, frontend_id)
    )
    packages.sort(key=lambda item: item["SPDXID"])
    relationships.sort(
        key=lambda item: (
            item["spdxElementId"],
            item["relationshipType"],
            item["relatedSpdxElement"],
        )
    )
    return packages, relationships, [product_id]


def sha256_path(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as stream:
            while chunk := stream.read(1024 * 1024):
                digest.update(chunk)
    except OSError as error:
        raise IdentityError(f"cannot hash release subject {path}: {error}") from error
    return digest.hexdigest()


def sha1_path(path: Path) -> str:
    digest = hashlib.sha1(usedforsecurity=False)
    try:
        with path.open("rb") as stream:
            while chunk := stream.read(1024 * 1024):
                digest.update(chunk)
    except OSError as error:
        raise IdentityError(f"cannot hash release path {path}: {error}") from error
    return digest.hexdigest()


def workflow_steps(workflow: str) -> list[str]:
    """Return steps only from canonical top-level job blocks."""

    jobs_marker = "\njobs:\n"
    marker = workflow.find(jobs_marker)
    if marker < 0:
        reject("release workflow has no canonical jobs mapping")
    jobs = workflow[marker + len(jobs_marker) :]
    for line in jobs.splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if (
            len(line) - len(line.lstrip()) == 2
            and re.fullmatch(r"  [A-Za-z0-9_-]+:", line) is None
        ):
            reject("release workflow contains a noncanonical job mapping")
    starts = list(re.finditer(r"(?m)^  [A-Za-z0-9_-]+:\n", jobs))
    if not starts:
        reject("release workflow has no canonical job blocks")
    result: list[str] = []
    for index, start in enumerate(starts):
        block = (
            jobs[start.start() : starts[index + 1].start()]
            if index + 1 < len(starts)
            else jobs[start.start() :]
        )
        if "\n    steps:\n" in block:
            result.extend(workflow_step_blocks(block))
    if not result:
        reject("release workflow has no canonical steps")
    return result


def validate_workflow_step_structure(step: str) -> None:
    """Reject YAML key spellings that could hide or synthesize step actions."""

    lines = step.splitlines()
    first = re.fullmatch(r"      - (?P<key>uses|name|id):(?:\s+.*)?", lines[0])
    if first is None:
        reject("release workflow contains a noncanonical step mapping")
    keys = {first["key"]}
    for line in lines[1:]:
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        indent = len(line) - len(line.lstrip())
        if indent != 8:
            continue
        key = re.fullmatch(r"        (?P<key>[A-Za-z_][A-Za-z0-9_-]*):(?:\s+.*)?", line)
        if key is None:
            reject("release workflow contains a noncanonical step mapping")
        name = key["key"]
        if name in keys:
            reject(f"release workflow step contains duplicate key {name!r}")
        keys.add(name)


def workflow_action_dependencies(
    source_entries: dict[str, bytes],
) -> list[dict[str, Any]]:
    try:
        workflow = source_entries[WORKFLOW_PATH].decode("utf-8")
    except (KeyError, UnicodeError) as error:
        raise IdentityError("release workflow is missing or not UTF-8") from error
    steps = workflow_steps(workflow)
    for step in steps:
        validate_workflow_step_structure(step)
    uses: set[tuple[str, str]] = set()
    action_line_count = 0
    for line in workflow.splitlines():
        stripped = line.lstrip()
        if not stripped or stripped.startswith("#"):
            continue
        occurrences = re.findall(r"(?<![A-Za-z0-9_.-])uses\s*:", line)
        if not occurrences:
            continue
        match = re.fullmatch(r"\s*(?:-\s*)?uses:\s*(\S+)(?:\s+#.*)?", line)
        if match is None:
            reject("release workflow contains a noncanonical action reference")
        action_line_count += 1
        token = match.group(1)
        if token.startswith("./"):
            continue
        parsed = ACTION_TOKEN.fullmatch(token)
        if parsed is None:
            reject(
                "release workflow contains an external action not pinned to "
                "40 lowercase hex"
            )
        uses.add((parsed["repository"], parsed["commit"]))
    step_action_count = 0
    for step in steps:
        count = sum(
            re.fullmatch(r"\s*(?:-\s*)?uses:\s*(\S+)(?:\s+#.*)?", line) is not None
            for line in step.splitlines()
        )
        if count > 1:
            reject("release workflow step contains duplicate action references")
        step_action_count += count
    if step_action_count != action_line_count:
        reject("release workflow action reference is outside a canonical step")
    if not uses:
        reject("release workflow has no commit-pinned external actions")
    return [
        {
            "uri": f"https://github.com/{repository}",
            "digest": {"gitCommit": commit},
        }
        for repository, commit in sorted(uses)
    ]


def workflow_job_block(workflow: str, job: str) -> str:
    match = re.search(
        rf"(?ms)^  {re.escape(job)}:\n(?P<body>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
        workflow,
    )
    if match is None:
        reject(f"release workflow is missing producer job {job}")
    return match.group(0)


def workflow_run_lines(job_block: str) -> list[str]:
    """Return executable non-comment lines from YAML `run` scalars."""

    lines = job_block.splitlines()
    result: list[str] = []
    index = 0
    while index < len(lines):
        line = lines[index]
        match = re.match(r"^(?P<indent>\s*)run:\s*(?P<value>.*)$", line)
        if match is None:
            index += 1
            continue
        indent = len(match.group("indent"))
        value = match.group("value").strip()
        if value and value not in {"|", ">", "|-", ">-"}:
            if not value.startswith("#"):
                result.append(value)
            index += 1
            continue
        index += 1
        while index < len(lines):
            candidate = lines[index]
            if candidate.strip() and len(candidate) - len(candidate.lstrip()) <= indent:
                break
            stripped = candidate.strip()
            if stripped and not stripped.startswith("#"):
                result.append(stripped.removesuffix("\\").rstrip())
            index += 1
    return result


def workflow_step_blocks(job_block: str) -> list[str]:
    """Return exact YAML step blocks without interpreting scalar contents."""

    matches = list(re.finditer(r"(?m)^      - ", job_block))
    return [
        job_block[match.start() : matches[index + 1].start()]
        if index + 1 < len(matches)
        else job_block[match.start() :]
        for index, match in enumerate(matches)
    ]


def workflow_named_step(job_block: str, name: str) -> str:
    matches = [
        step
        for step in workflow_step_blocks(job_block)
        if step.splitlines()[0] == f"      - name: {name}"
    ]
    if len(matches) != 1:
        reject(f"release workflow payload step {name!r} is missing or duplicated")
    return matches[0]


def validate_payload_step_contracts(blocks: dict[str, str]) -> None:
    """Bind complete payload producer steps, including control-flow structure."""

    for (job, name), expected in PAYLOAD_STEP_CONTRACTS.items():
        step = workflow_named_step(blocks[job], name)
        lines = step.splitlines()
        top_level = [
            line
            for line in lines
            if line.startswith("        ") and not line.startswith("          ")
        ]
        if top_level != [
            f"        if: {expected['if']}",
            f"        shell: {expected['shell']}",
            "        run: |",
        ]:
            reject(f"release workflow payload step {name!r} control flow drifted")
        if sha256_bytes(step.strip().encode("utf-8")) != expected["sha256"]:
            reject(f"release workflow payload step {name!r} full block drifted")

    setup_name = "Configure pinned docker-container Buildx for OCI export"
    setup = workflow_named_step(blocks["native"], setup_name)
    setup_lines = setup.splitlines()
    setup_top_level = [
        line
        for line in setup_lines
        if line.startswith("        ") and not line.startswith("          ")
    ]
    if (
        setup_top_level
        != [
            "        if: ${{ github.event_name == 'workflow_dispatch' && inputs.release == true && matrix.platform == 'oci-linux-amd64' }}",
            "        uses: docker/setup-buildx-action@37fe631027851001ddb9b187196cc803df7f5f0e # v4.3.0",
            "        with:",
        ]
        or sha256_bytes(setup.strip().encode("utf-8")) != OCI_SETUP_STEP_SHA256
    ):
        reject("release workflow OCI setup step full block drifted")


def workflow_literal_block(step: str, key: str) -> list[str]:
    """Read one exact step-level YAML literal block, excluding comments."""

    marker = f"          {key}: |"
    lines = step.splitlines()
    positions = [index for index, line in enumerate(lines) if line == marker]
    if len(positions) != 1:
        reject(f"release workflow {key} must be one literal block")
    result: list[str] = []
    for line in lines[positions[0] + 1 :]:
        if line.strip() and len(line) - len(line.lstrip()) <= 10:
            break
        stripped = line.strip()
        if stripped and not stripped.startswith("#"):
            result.append(stripped)
    return result


def python_literal_assignment(source: str, name: str) -> object:
    """Read one exact top-level literal assignment without trusting comments."""

    try:
        tree = ast.parse(source)
    except SyntaxError as error:
        raise IdentityError(
            f"release route source is invalid Python: {error}"
        ) from error
    values = []
    for statement in ast.walk(tree):
        if (
            isinstance(statement, ast.Assign)
            and len(statement.targets) == 1
            and isinstance(statement.targets[0], ast.Name)
            and statement.targets[0].id == name
        ):
            try:
                values.append(ast.literal_eval(statement.value))
            except (TypeError, ValueError, SyntaxError) as error:
                raise IdentityError(f"release route {name} is not a literal") from error
    if len(values) != 1:
        reject(f"release route {name} must have one literal assignment")
    return values[0]


def validate_pinned_workflow_python(job: str, lines: list[str]) -> None:
    """Reject release Python entrypoints not routed through the exact pin."""

    uv_prefixes = {
        "uv run --python 3.14.7 --no-project python",
        "uv run --python 3.14.7 --project sdk/python --locked python",
    }
    for index, line in enumerate(lines):
        if (
            re.search(
                r"scripts/(?:release-[A-Za-z0-9_-]+|reference-evidence-[A-Za-z0-9_-]+)\.py(?:\s|$)",
                line,
            )
            is None
        ):
            continue
        direct = any(
            line.startswith(prefix)
            for prefix in (
                '"$release_python" -I scripts/',
                "& $releasePython -I scripts/",
                '"$WORLDSTREAM_RELEASE_PYTHON" -I scripts/',
            )
        )
        uv_inline = any(line.startswith(prefix + " scripts/") for prefix in uv_prefixes)
        uv_continuation = index > 0 and lines[index - 1] in uv_prefixes
        if not (direct or uv_inline or uv_continuation):
            reject(f"release workflow producer interpreter drifted for job {job}")


def validate_workflow_producer_contract(source_entries: dict[str, bytes]) -> None:
    try:
        workflow = source_entries[WORKFLOW_PATH].decode("utf-8")
        gates = source_entries["scripts/gates.py"].decode("utf-8")
    except (KeyError, UnicodeError) as error:
        raise IdentityError("release workflow route materials are missing") from error

    self_hosted = (
        "runs-on: [self-hosted, linux, x64, "
        "ubuntu-24.04-x86_64-ext4-4vcpu-8gib-local-ssd]"
    )
    expected_routes = {
        "native": "runs-on: ${{ matrix.runner }}",
        "macos-source": "runs-on: ${{ matrix.runner }}",
        "macos-source-release": "runs-on: ubuntu-24.04",
        "conformance-release": "runs-on: ubuntu-24.04",
        "packaged-backend-release": self_hosted,
        "failure-soak-release": self_hosted,
        "reference-performance-release": self_hosted,
        "release-evidence": "runs-on: ubuntu-24.04",
    }
    blocks = {job: workflow_job_block(workflow, job) for job in expected_routes}
    validate_payload_step_contracts(blocks)
    for job, route in expected_routes.items():
        expected_line = f"    {route}"
        observed = [
            line for line in blocks[job].splitlines() if line.startswith("    runs-on:")
        ]
        if observed != [expected_line]:
            reject(f"release workflow producer route drifted for job {job}")
    release_condition = (
        "    if: ${{ github.event_name == 'workflow_dispatch' "
        "&& inputs.release == true }}"
    )
    for job in (
        "macos-source-release",
        "conformance-release",
        "packaged-backend-release",
        "failure-soak-release",
        "reference-performance-release",
        "release-evidence",
    ):
        if blocks[job].splitlines().count(release_condition) != 1:
            reject(f"release workflow producer condition drifted for job {job}")
    release_lines_exact = blocks["release-evidence"].splitlines()
    if (
        release_lines_exact.count("    env:") != 1
        or release_lines_exact.count(
            "      WORLDSTREAM_RELEASE_INPUT: ${{ inputs.release }}"
        )
        != 1
    ):
        reject("release workflow external input observation drifted")
    for route in ("runner: macos-15", "runner: macos-15-intel"):
        if (
            len(re.findall(rf"(?m)^\s*{re.escape(route)}\s*$", blocks["macos-source"]))
            != 1
        ):
            reject("release workflow macOS producer route is incomplete")

    route_pairs = {
        "native-linux-x86_64": ("ubuntu-24.04", "bash", "Linux"),
        "native-windows-x64": ("windows-2025-vs2026", "pwsh", "Windows"),
        "oci-linux-amd64": ("ubuntu-24.04", "bash", "Linux"),
    }
    cell_routes = python_literal_assignment(gates, "CELL_ROUTES")
    if not isinstance(cell_routes, dict):
        reject("manifest matrix runner routes are malformed")
    for platform, (runner, shell, system) in route_pairs.items():
        if cell_routes.get(platform) != {
            "runner": runner,
            "shell": shell,
            "system": system,
        }:
            reject(f"manifest matrix runner route drifted for {platform}")
    expected_platform_requirements = {
        "native-linux-x86_64": (
            "x86_64-unknown-linux-musl",
            "ubuntu-24.04",
            "Linux",
            "native-linux-release-profile",
            "native-linux-x86_64-archive",
        ),
        "native-windows-x64": (
            "x86_64-pc-windows-msvc",
            "windows-2025-vs2026",
            "Windows",
            "native-windows-release-profile",
            "native-windows-x64-archive",
        ),
        "oci-linux-amd64": (
            "linux/amd64",
            "ubuntu-24.04",
            "Linux",
            "oci-linux-amd64-release-profile",
            "oci-linux-amd64-image",
        ),
    }
    platform_requirements = python_literal_assignment(gates, "platform_requirements")
    if not isinstance(platform_requirements, dict):
        reject("manifest platform requirements are malformed")
    for platform, expected in expected_platform_requirements.items():
        if platform_requirements.get(platform) != expected:
            reject(f"manifest platform target route drifted for {platform}")

    native = blocks["native"]
    matrix_match = re.search(r"(?ms)^      matrix:\n.*?(?=^    steps:)", native)
    expected_native_matrix = """      matrix:
        include:
          - cell: native-linux-x86_64
            platform: native-linux-x86_64
            runner: ubuntu-24.04
            shell: bash
            system: Linux
          - cell: native-windows-x64
            platform: native-windows-x64
            runner: windows-2025-vs2026
            shell: pwsh
            system: Windows
          - cell: oci-linux-amd64
            platform: oci-linux-amd64
            runner: ubuntu-24.04
            shell: bash
            system: Linux
"""
    if matrix_match is None or matrix_match.group(0) != expected_native_matrix:
        reject("release workflow native matrix is not the static trusted route")
    native_lines = workflow_run_lines(native)
    native_runs = "\n".join(native_lines)
    buildkit = expected_buildkit_image(source_entries)
    buildx_steps = [
        step
        for step in workflow_step_blocks(native)
        if re.search(
            r"(?m)^        uses: docker/setup-buildx-action@[0-9a-f]{40}(?:\s+#.*)?$",
            step,
        )
    ]
    if (
        len(buildx_steps) != 1
        or "        with:" not in buildx_steps[0].splitlines()
        or buildx_steps[0].splitlines().count("          driver: docker-container") != 1
        or workflow_literal_block(buildx_steps[0], "driver-opts")
        != [f"image={buildkit}"]
    ):
        reject("release workflow BuildKit driver image drifted")
    if sum("uv python find 3.14.7" in line for line in native_lines) != 7:
        reject("release workflow native packaging interpreter route drifted")
    required_native_commands = (
        (
            "cargo build --release --locked --target x86_64-unknown-linux-musl --workspace",
            False,
            2,
        ),
        (
            "cargo build --release --locked --target x86_64-pc-windows-msvc --workspace",
            False,
            1,
        ),
        ("SOURCE_DATE_EPOCH=0 scripts/package-release.sh", True, 1),
        ("scripts/package-oci.sh", True, 1),
        ("& scripts/package-release.ps1 --target windows-x64", True, 1),
        ("pnpm --dir web/console build", False, 2),
    )
    for command, allow_arguments, count in required_native_commands:
        observed = sum(
            line == command or (allow_arguments and line.startswith(command + " "))
            for line in native_lines
        )
        if observed != count:
            reject(f"release workflow payload command drifted: {command}")

    if native_lines.count("docker buildx build") != 1:
        reject("release workflow OCI build command is missing")
    docker_start = native_runs.find("docker buildx build")
    docker_end = native_runs.find("dist/oci-context", docker_start)
    if docker_start < 0 or docker_end < 0:
        reject("release workflow OCI build command is missing")
    docker_command = native_runs[docker_start : docker_end + len("dist/oci-context")]
    build_arguments = set(re.findall(r'--build-arg "([A-Z0-9_]+)=', docker_command))
    if build_arguments != {
        "WORLDSTREAM_BASE_IMAGE",
        "VERSION",
        "MANIFEST_SHA256",
        "SOURCE_REVISION",
        "BUILD_IDENTITY_SHA256",
        "BUILD_ENVIRONMENT_BASE64",
        "SOURCE_DATE_EPOCH",
    }:
        reject("release workflow OCI build arguments are not exact")
    for flag in (
        "--platform linux/amd64",
        "--load",
        "--provenance=false",
        '--output "type=oci,dest=$artifact,compression=gzip,force-compression=true"',
        '--tag "$image_tag"',
    ):
        if docker_command.count(flag) != 1:
            reject(f"release workflow OCI build flag drifted: {flag}")

    release = blocks["release-evidence"]
    release_lines = workflow_run_lines(release)
    release_runs = "\n".join(release_lines)
    for command, allow_arguments, count in (
        ("SOURCE_DATE_EPOCH=0 scripts/package-release.sh", True, 1),
        ("--target source", True, 2),
        ('"$WORLDSTREAM_RELEASE_PYTHON" -I scripts/release-supply-chain.py', True, 1),
    ):
        observed = sum(
            line == command or (allow_arguments and line.startswith(command + " "))
            for line in release_lines
        )
        if observed != count:
            reject(f"release workflow aggregation command drifted: {command}")
    install_python = release_runs.find("uv python install 3.14.7")
    build_source = release_runs.find("SOURCE_DATE_EPOCH=0 scripts/package-release.sh")
    if (
        sum("uv python find 3.14.7" in line for line in release_lines) != 1
        or sum("WORLDSTREAM_RELEASE_PYTHON=%s" in line for line in release_lines) != 1
        or install_python < 0
        or build_source <= install_python
    ):
        reject("release workflow source packaging interpreter route drifted")
    if sum(
        line.startswith(
            '"$WORLDSTREAM_RELEASE_PYTHON" -I scripts/release-supply-chain.py'
        )
        for line in release_lines
    ) != 1 or any(
        line.startswith("python3 scripts/release-") for line in release_lines
    ):
        reject("release workflow provenance interpreter route drifted")

    producer_jobs = (
        "native",
        "macos-source",
        "macos-source-release",
        "conformance-release",
        "packaged-backend-release",
        "failure-soak-release",
        "reference-performance-release",
        "release-evidence",
    )
    producer_run_lines: list[str] = []
    for job in producer_jobs:
        lines = workflow_run_lines(blocks[job])
        producer_run_lines.extend(lines)
        validate_pinned_workflow_python(job, lines)
        if any(
            re.match(
                r"^(?:python3|python)\s+scripts/(?:release|reference-evidence)-", line
            )
            for line in lines
        ):
            reject(f"release workflow producer interpreter drifted for job {job}")
    if (
        producer_run_lines.count("corepack enable") != 3
        or producer_run_lines.count("corepack install") != 3
        or any(
            line.startswith("corepack install") and line != "corepack install"
            for line in producer_run_lines
        )
    ):
        reject("release workflow pnpm integrity-pinned installation route drifted")


def evidence_producer_rows(
    release_subjects: dict[str, Path], payload_subjects: set[str]
) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    source_ids: set[str] = set()
    for relative, path in sorted(release_subjects.items()):
        if relative in payload_subjects:
            continue
        expected_prefix = "supply-chain/subjects/"
        if not relative.startswith(expected_prefix) or not relative.endswith(".json"):
            reject(f"release provenance has an untyped non-payload subject: {relative}")
        value = strict_json(
            regular_bytes(path, f"release evidence subject {relative}"),
            f"release evidence subject {relative}",
        )
        source_id = value.get("source_id")
        evidence_id = value.get("evidence_id")
        platform = value.get("platform")
        checks = value.get("checks")
        details = value.get("details")
        producer_id = details.get("producer_id") if isinstance(details, dict) else None
        if (
            not isinstance(source_id, str)
            or source_id not in EVIDENCE_UPSTREAM_JOBS
            or source_id in source_ids
            or not isinstance(evidence_id, str)
            or relative != f"{expected_prefix}{evidence_id}.json"
            or not isinstance(platform, str)
            or not platform
            or not isinstance(producer_id, str)
            or not producer_id
            or not isinstance(checks, dict)
            or not checks
            or any(
                not isinstance(check, str) or result is not True
                for check, result in checks.items()
            )
        ):
            reject(f"release evidence producer identity is malformed: {relative}")
        source_ids.add(source_id)
        upstream_job, configured_runners = EVIDENCE_UPSTREAM_JOBS[source_id]
        digest = sha256_path(path)
        rows.append(
            {
                "source_id": source_id,
                "evidence_id": evidence_id,
                "producer_id": producer_id,
                "subject": relative,
                "subject_sha256": "sha256:" + digest,
                "input_uri": (f"file:release-inputs/source-reports/{evidence_id}.json"),
                "platform": platform,
                "checks": sorted(checks),
                "upstream_job": upstream_job,
                "configured_runners": list(configured_runners),
                "aggregation_job": "release-evidence",
                "aggregation_runner": "ubuntu-24.04",
            }
        )
    if source_ids != set(EVIDENCE_UPSTREAM_JOBS):
        reject(
            "release provenance evidence producers are not exact: "
            f"missing={sorted(set(EVIDENCE_UPSTREAM_JOBS) - source_ids)}; "
            f"extra={sorted(source_ids - set(EVIDENCE_UPSTREAM_JOBS))}"
        )
    return rows


def payload_ui_commands(artifact_id: str) -> list[list[str]]:
    commands = [
        ["pnpm", "install", "--frozen-lockfile"],
        ["pnpm", "--dir", "web/console", "build"],
    ]
    if artifact_id == "source-archive":
        return []
    if artifact_id == "oci-linux-amd64-image":
        return commands + commands
    return commands


def pinned_toolchain_dependencies() -> list[dict[str, Any]]:
    """Return byte-pinned downloaded build tools for SLSA resolvedDependencies."""

    return [
        {
            "uri": ZIG_LINUX_X86_64_URL,
            "digest": {"sha256": ZIG_LINUX_X86_64_SHA256},
        },
        {
            "uri": PNPM_TARBALL_URL,
            "digest": {"sha512": PNPM_SHA512},
        },
    ]


def provenance_graph(
    *,
    version: str,
    revision: str,
    subjects: dict[str, Path],
    release_subjects: dict[str, Path],
    identities: dict[str, dict[str, Any]],
    source_entries: dict[str, bytes],
    invocation_parameters: dict[str, Any] | None = None,
) -> tuple[dict[str, Any], dict[str, Any]]:
    validate_workflow_producer_contract(source_entries)
    if invocation_parameters is None:
        invocation_parameters = local_invocation_parameters()
    validate_invocation_parameters(invocation_parameters, require_github=False)
    if len(release_subjects) != 17:
        reject("release provenance requires exactly seventeen aggregation subjects")
    payload_rows = []
    names = {
        artifact_id: path.name
        for artifact_id, path in subjects.items()
        if artifact_id in PAYLOAD_TARGETS
    }
    if len(names) != len(PAYLOAD_TARGETS) or any(
        name not in release_subjects for name in names.values()
    ):
        reject("release provenance payload subjects are not exact")
    for artifact_id in PAYLOAD_TARGETS:
        identity = identities[artifact_id]
        relative = names[artifact_id]
        digest = sha256_path(release_subjects[relative])
        upstream_job, configured_runner = PAYLOAD_UPSTREAM_JOBS[artifact_id]
        payload_rows.append(
            {
                "artifact_id": artifact_id,
                "subject": relative,
                "subject_sha256": "sha256:" + digest,
                "input_uri": f"file:release-inputs/payload/{relative}",
                "build_identity_sha256": build_identity_digest(identity),
                "source": identity["source"],
                "materials": identity["materials"],
                "target": identity["target"],
                "observed_build_environment": identity["observed_build_environment"],
                "upstream_job": upstream_job,
                "configured_runner": configured_runner,
                "aggregation_job": "release-evidence",
                "aggregation_runner": "ubuntu-24.04",
                "toolchains": identity["toolchains"],
                "compiler": identity["compiler"],
                "arguments": identity["arguments"],
                "ui_commands": payload_ui_commands(artifact_id),
                "oci": identity["oci"],
                "derived_build_arguments": (
                    {
                        "BUILD_IDENTITY_SHA256": build_identity_digest(identity),
                        "BUILD_ENVIRONMENT_BASE64": observed_build_environment_label(
                            identity["observed_build_environment"]
                        ),
                    }
                    if artifact_id == "oci-linux-amd64-image"
                    else {}
                ),
            }
        )
    evidence_rows = evidence_producer_rows(release_subjects, set(names.values()))
    dependencies = [
        {
            "uri": f"git+{REPOSITORY}",
            "digest": {"gitCommit": revision},
        },
    ]
    dependencies.extend(pinned_toolchain_dependencies())
    dependencies.extend(
        {
            "uri": f"file:{relative}",
            "digest": {"sha256": digest.removeprefix("sha256:")},
        }
        for relative, digest in sorted(
            identities["source-archive"]["materials"].items()
        )
    )
    dependencies.extend(
        {
            "uri": f"file:{relative}",
            "digest": {"sha256": sha256_bytes(source_entries[relative])},
        }
        for relative in PROVENANCE_MATERIAL_PATHS
    )
    dependencies.extend(workflow_action_dependencies(source_entries))
    base = expected_base_image(source_entries)
    for image in (
        base,
        expected_buildkit_image(source_entries),
        expected_dockerfile_frontend(source_entries),
    ):
        dependencies.append(
            {
                "uri": "oci://" + docker_repository_url(image.split("@", 1)[0]),
                "digest": {"sha256": image.rsplit("sha256:", 1)[1]},
            }
        )
    input_descriptors = [
        {
            "uri": row["input_uri"],
            "digest": {"sha256": row["subject_sha256"].removeprefix("sha256:")},
        }
        for row in [*payload_rows, *evidence_rows]
    ]
    dependencies.extend(input_descriptors)
    dependencies.sort(
        key=lambda item: (
            item["uri"],
            json.dumps(item["digest"], sort_keys=True, separators=(",", ":")),
        )
    )
    component_rows, _unused = component_packages(source_entries, version)
    component_rows.extend(apk_component_packages(source_entries))
    component_rows.sort(key=lambda item: item["SPDXID"])
    build_definition = {
        "externalParameters": {
            "trigger": invocation_parameters["trigger"],
            "source": invocation_parameters["source"],
            "manifest_inputs": [
                "file:compatibility.toml",
                "file:compatibility.json",
            ],
            "payload_inputs": [
                {"artifact_id": row["artifact_id"], "uri": row["input_uri"]}
                for row in sorted(payload_rows, key=lambda item: item["artifact_id"])
            ],
            "evidence_inputs": [
                {"source_id": row["source_id"], "uri": row["input_uri"]}
                for row in sorted(evidence_rows, key=lambda item: item["source_id"])
            ],
        },
        "internalParameters": {},
        "resolvedDependencies": dependencies,
    }
    aggregation_result = {
        "schema": RELEASE_AGGREGATION_SCHEMA,
        "operation": "validate-and-copy",
        "product": version,
        "source_revision": revision,
        "subject_count": len(release_subjects),
        "component_graph_sha256": "sha256:"
        + sha256_bytes(canonical_json(component_rows)),
        "evidence_producers": evidence_rows,
        "payload_producers": payload_rows,
        "source_date_epoch": identities["source-archive"]["source_date_epoch"],
        "toolchains": identities["source-archive"]["toolchains"],
    }
    return build_definition, aggregation_result


def aggregation_byproduct(value: dict[str, Any]) -> dict[str, Any]:
    """Encode the derived aggregation graph as a SLSA ResourceDescriptor."""

    expected_fields = {
        "schema",
        "operation",
        "product",
        "source_revision",
        "subject_count",
        "component_graph_sha256",
        "evidence_producers",
        "payload_producers",
        "source_date_epoch",
        "toolchains",
    }
    if (
        set(value) != expected_fields
        or value.get("schema") != RELEASE_AGGREGATION_SCHEMA
        or value.get("operation") != "validate-and-copy"
        or value.get("subject_count") != 17
        or GIT_REVISION.fullmatch(value.get("source_revision", "")) is None
        or SHA256_REF.fullmatch(value.get("component_graph_sha256", "")) is None
        or not isinstance(value.get("payload_producers"), list)
        or len(value["payload_producers"]) != 4
        or not isinstance(value.get("evidence_producers"), list)
        or len(value["evidence_producers"]) != 13
    ):
        reject("SLSA aggregation byproduct is malformed")
    content = canonical_json(value)
    return {
        "name": "worldstream-release-aggregation-v1.json",
        "digest": {"sha256": sha256_bytes(content)},
        "mediaType": "application/vnd.worldstream.release-aggregation.v1+json",
        "content": base64.b64encode(content).decode("ascii"),
    }


def runner_byproduct(identity: dict[str, Any]) -> dict[str, Any]:
    """Encode runner identity as a conforming in-toto ResourceDescriptor."""

    expected_fields = {"provider", "os", "architecture", "image", "image_version"}
    if set(identity) != expected_fields or any(
        not isinstance(item, str) or not item for item in identity.values()
    ):
        reject("SLSA runner identity is malformed")
    content = canonical_json(identity)
    return {
        "name": "worldstream-runner-identity.json",
        "digest": {"sha256": sha256_bytes(content)},
        "mediaType": "application/json",
        "content": base64.b64encode(content).decode("ascii"),
    }


def local_invocation_parameters() -> dict[str, Any]:
    """Return an explicit non-release invocation used only by structural tests."""

    return {
        "trigger": {
            "event": "local-test",
            "ref": "local-test",
            "inputs": {"release": False},
        },
        "source": {"repository": REPOSITORY, "ref": "local-test"},
    }


def validate_invocation_parameters(value: object, *, require_github: bool) -> None:
    release = {
        "trigger": {
            "event": "workflow_dispatch",
            "ref": "refs/heads/main",
            "inputs": {"release": True},
        },
        "source": {"repository": REPOSITORY, "ref": "refs/heads/main"},
    }
    if value == release:
        return
    if not require_github and value == local_invocation_parameters():
        return
    reject("SLSA external invocation parameters are not canonical")


def release_invocation_parameters() -> dict[str, Any]:
    """Observe the actual GitHub trigger/ref/input, or label a local test."""

    github_identity = (
        os.environ.get("GITHUB_REPOSITORY", ""),
        os.environ.get("GITHUB_WORKFLOW_REF", ""),
        os.environ.get("GITHUB_RUN_ID", ""),
        os.environ.get("GITHUB_RUN_ATTEMPT", ""),
    )
    event = os.environ.get("GITHUB_EVENT_NAME", "")
    ref = os.environ.get("GITHUB_REF", "")
    release_input = os.environ.get("WORLDSTREAM_RELEASE_INPUT", "")
    context = (event, ref, release_input)
    if any(github_identity) or any(context):
        if not all(github_identity) or not all(context):
            reject("GitHub Actions release invocation environment is partial")
        value = {
            "trigger": {
                "event": event,
                "ref": ref,
                "inputs": {"release": release_input == "true"},
            },
            "source": {"repository": REPOSITORY, "ref": ref},
        }
        if release_input not in {"true", "false"}:
            reject("GitHub Actions release input is not a canonical boolean")
        validate_invocation_parameters(value, require_github=True)
        return value
    return local_invocation_parameters()


def github_run_details(aggregation_result: dict[str, Any]) -> dict[str, Any]:
    repository = os.environ.get("GITHUB_REPOSITORY", "")
    workflow_ref = os.environ.get("GITHUB_WORKFLOW_REF", "")
    server = os.environ.get("GITHUB_SERVER_URL", "https://github.com").rstrip("/")
    run_id = os.environ.get("GITHUB_RUN_ID", "")
    attempt = os.environ.get("GITHUB_RUN_ATTEMPT", "")
    runner_os = os.environ.get("RUNNER_OS", "")
    runner_arch = os.environ.get("RUNNER_ARCH", "")
    image_os = os.environ.get("ImageOS", "")
    image_version = os.environ.get("ImageVersion", "")
    github_values = {
        "GITHUB_REPOSITORY": repository,
        "GITHUB_WORKFLOW_REF": workflow_ref,
        "GITHUB_RUN_ID": run_id,
        "GITHUB_RUN_ATTEMPT": attempt,
        "RUNNER_OS": runner_os,
        "RUNNER_ARCH": runner_arch,
        "ImageOS": image_os,
        "ImageVersion": image_version,
    }
    if any(github_values.values()) and not all(github_values.values()):
        reject("GitHub Actions run identity environment is partial")
    if all(github_values.values()):
        expected_repository = "imom39a/worldstream"
        expected_workflow_ref = f"{expected_repository}/{WORKFLOW_PATH}@refs/heads/main"
        if (
            repository != expected_repository
            or workflow_ref != expected_workflow_ref
            or re.fullmatch(r"[1-9][0-9]*", run_id) is None
            or re.fullmatch(r"[1-9][0-9]*", attempt) is None
            or runner_os != "Linux"
            or runner_arch != "X64"
            or image_os != "ubuntu24"
            or re.fullmatch(r"20[0-9]{6}\.[0-9]+\.[0-9]+", image_version) is None
        ):
            reject("GitHub Actions release aggregation identity is not canonical")
        builder = f"{server}/{workflow_ref}"
        invocation = f"{server}/{repository}/actions/runs/{run_id}/attempts/{attempt}"
        provider = "github-actions"
    else:
        builder = os.environ.get(
            "WORLDSTREAM_BUILDER_ID", f"{REPOSITORY}/{WORKFLOW_PATH}@local-test"
        )
        invocation = os.environ.get(
            "WORLDSTREAM_BUILD_INVOCATION", "urn:worldstream:local-test-build"
        )
        provider = "local-test"
        runner_os = runner_os or "local"
        runner_arch = runner_arch or "local"
        image_os = image_os or "local"
        image_version = image_version or "local"
    return {
        "builder": {"id": builder},
        "metadata": {
            "invocationId": invocation,
        },
        "byproducts": [
            runner_byproduct(
                {
                    "provider": provider,
                    "os": runner_os,
                    "architecture": runner_arch,
                    "image": image_os,
                    "image_version": image_version,
                }
            ),
            aggregation_byproduct(aggregation_result),
        ],
    }


def validate_run_details(
    value: object,
    *,
    require_github: bool,
    expected_aggregation: dict[str, Any] | None = None,
) -> None:
    if not isinstance(value, dict) or set(value) != {
        "builder",
        "metadata",
        "byproducts",
    }:
        reject(
            "SLSA runDetails must contain exact builder, metadata, and runner byproduct"
        )
    builder = value.get("builder")
    metadata = value.get("metadata")
    byproducts = value.get("byproducts")
    if not isinstance(builder, dict) or set(builder) != {"id"}:
        reject("SLSA builder identity is malformed")
    builder_id = builder.get("id")
    if not isinstance(builder_id, str) or f"/{WORKFLOW_PATH}@" not in builder_id:
        reject("SLSA builder is not the WorldStream release workflow")
    if (
        not isinstance(metadata, dict)
        or set(metadata) != {"invocationId"}
        or not isinstance(metadata.get("invocationId"), str)
        or not metadata["invocationId"]
    ):
        reject("SLSA invocation metadata is incomplete")
    expected_count = 2 if expected_aggregation is not None else 1
    if not isinstance(byproducts, list) or len(byproducts) != expected_count:
        reject("SLSA runner identity is missing")
    runner = byproducts[0]
    encoded = runner.get("content") if isinstance(runner, dict) else None
    if not isinstance(encoded, str):
        reject("SLSA runner identity is not a base64 ResourceDescriptor")
    try:
        content_bytes = base64.b64decode(encoded, validate=True)
        content = strict_json(content_bytes, "SLSA runner identity")
    except (ValueError, binascii.Error) as error:
        raise IdentityError(
            "SLSA runner identity is not canonical base64 JSON"
        ) from error
    if runner != runner_byproduct(content):
        reject("SLSA runner ResourceDescriptor is not exact or canonical")
    if expected_aggregation is not None and byproducts[1] != aggregation_byproduct(
        expected_aggregation
    ):
        reject("SLSA aggregation byproduct differs from validated build outputs")
    if require_github and content["provider"] != "github-actions":
        reject("release SLSA provenance was not generated on GitHub Actions")
    if require_github:
        expected_builder = f"{REPOSITORY}/{WORKFLOW_PATH}@refs/heads/main"
        if builder_id != expected_builder:
            reject("release SLSA builder is not the canonical main workflow")
        invocation = metadata["invocationId"]
        if (
            re.fullmatch(
                re.escape(REPOSITORY)
                + r"/actions/runs/[1-9][0-9]*/attempts/[1-9][0-9]*",
                invocation,
            )
            is None
        ):
            reject("release SLSA invocation ID is not canonical")
        if (
            content["os"] != "Linux"
            or content["architecture"] != "X64"
            or content["image"] != "ubuntu24"
            or re.fullmatch(r"20[0-9]{6}\.[0-9]+\.[0-9]+", content["image_version"])
            is None
        ):
            reject("release SLSA aggregation runner facts are not canonical")


def validate_identity_documents(
    *,
    spdx: dict[str, Any],
    provenance: dict[str, Any],
    version: str,
    subjects_by_relative: dict[str, Path],
    payloads_by_id: dict[str, Path],
    require_github: bool,
) -> None:
    identities, source_entries = release_payload_identities(payloads_by_id, version)
    revision = identities["source-archive"]["source"]["revision"]
    expected_packages, expected_relationships, expected_describes = spdx_graph(
        version=version,
        revision=revision,
        subjects=subjects_by_relative,
        identities=identities,
        source_entries=source_entries,
    )
    if spdx.get("packages") != expected_packages:
        reject(
            "SPDX package graph is empty, fabricated, or differs from locked components"
        )
    if spdx.get("relationships") != expected_relationships:
        reject("SPDX relationship graph differs from source/component/build identity")
    if spdx.get("documentDescribes") != expected_describes:
        reject("SPDX documentDescribes does not identify the WorldStream product")
    creation = spdx.get("creationInfo")
    created = creation.get("created") if isinstance(creation, dict) else None
    if not isinstance(created, str):
        reject("SPDX creation time is missing")
    expected_namespace = spdx_document_namespace(
        version,
        sha256_bytes(source_entries["compatibility.json"]),
        subjects_by_relative,
        created,
    )
    if spdx.get("documentNamespace") != expected_namespace:
        reject("SPDX document namespace does not bind the exact document version")
    expected_files = [
        {
            "SPDXID": _spdx_id("ReleaseSubject", relative),
            "fileName": relative,
            "checksums": [
                {"algorithm": "SHA1", "checksumValue": sha1_path(path)},
                {"algorithm": "SHA256", "checksumValue": sha256_path(path)},
            ],
            "copyrightText": "NOASSERTION",
            "licenseConcluded": "NOASSERTION",
        }
        for relative, path in sorted(subjects_by_relative.items())
    ]
    if spdx.get("files") != expected_files:
        reject("SPDX file graph differs from exact release subject identities")
    element_ids = {"SPDXRef-DOCUMENT"}
    for field in ("packages", "files"):
        items = spdx.get(field)
        if not isinstance(items, list):
            reject(f"SPDX {field} graph is missing")
        for item in items:
            identifier = item.get("SPDXID") if isinstance(item, dict) else None
            if not isinstance(identifier, str) or identifier in element_ids:
                reject("SPDX element identifiers are missing or duplicate")
            element_ids.add(identifier)
    for relationship in expected_relationships:
        for endpoint in ("spdxElementId", "relatedSpdxElement"):
            identifier = relationship[endpoint]
            if (
                identifier not in {"NONE", "NOASSERTION"}
                and identifier not in element_ids
            ):
                reject("SPDX relationship contains a dangling element identifier")
    predicate = provenance.get("predicate")
    definition = (
        predicate.get("buildDefinition") if isinstance(predicate, dict) else None
    )
    if not isinstance(definition, dict):
        reject("SLSA buildDefinition is missing")
    external = definition.get("externalParameters")
    invocation_parameters = (
        {
            "trigger": external.get("trigger"),
            "source": external.get("source"),
        }
        if isinstance(external, dict)
        else None
    )
    validate_invocation_parameters(invocation_parameters, require_github=require_github)
    expected_graph, expected_aggregation = provenance_graph(
        version=version,
        revision=revision,
        subjects={
            artifact_id: payloads_by_id[artifact_id] for artifact_id in PAYLOAD_TARGETS
        },
        release_subjects=subjects_by_relative,
        identities=identities,
        source_entries=source_entries,
        invocation_parameters=invocation_parameters,
    )
    if definition.get("buildType") != BUILD_TYPE:
        reject("SLSA buildType is not the WorldStream release build")
    for field, expected in expected_graph.items():
        if definition.get(field) != expected:
            reject(f"SLSA {field} differs from exact source/material/build identity")
    validate_run_details(
        predicate.get("runDetails") if isinstance(predicate, dict) else None,
        require_github=require_github,
        expected_aggregation=expected_aggregation,
    )

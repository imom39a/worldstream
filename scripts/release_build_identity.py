#!/usr/bin/env python3
"""Canonical release build identities and SPDX/SLSA graph validation."""

from __future__ import annotations

import base64
import binascii
import hashlib
import json
import os
import re
import subprocess
import tarfile
import zipfile
from pathlib import Path, PurePosixPath
from typing import Any

import tomllib

BUILD_IDENTITY_SCHEMA = "worldstream/release-build-identity/v1"
REPOSITORY = "https://github.com/imom39a/worldstream"
WORKFLOW_PATH = ".github/workflows/compatibility-gates.yml"
SOURCE_REVISION_FILE = ".worldstream-source-revision"
BUILD_METADATA_PATH = "metadata/build.json"
OCI_BASE_IMAGE_PATH = "packaging/oci/base-image.txt"
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
SHA256_REF = re.compile(r"sha256:[0-9a-f]{64}\Z")
GIT_REVISION = re.compile(r"[0-9a-f]{40}\Z")
VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?\Z")
PINNED_MATERIAL_PATHS = (
    ".github/workflows/compatibility-gates.yml",
    ".node-version",
    ".python-version",
    ".uv-version",
    "Cargo.lock",
    "Cargo.toml",
    "compatibility.json",
    "compatibility.toml",
    "package.json",
    "pnpm-lock.yaml",
    "rust-toolchain.toml",
    "sdk/python/pyproject.toml",
    "sdk/python/uv.lock",
    "web/console/package.json",
    OCI_BASE_IMAGE_PATH,
    "packaging/oci/Dockerfile",
    "packaging/oci/entrypoint.sh",
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
    "windows-x64": "windows-2025",
    "oci-linux-amd64": "ubuntu-24.04",
}


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
    def pairs(items: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, item in items:
            if key in result:
                reject(f"{label} contains duplicate key {key!r}")
            result[key] = item
        return result

    try:
        decoded = json.loads(value, object_pairs_hook=pairs)
    except (UnicodeError, json.JSONDecodeError) as error:
        raise IdentityError(f"{label} is not strict JSON") from error
    if not isinstance(decoded, dict):
        reject(f"{label} must be an object")
    return decoded


def regular_bytes(path: Path, label: str, maximum: int = 64 * 1024 * 1024) -> bytes:
    try:
        if path.is_symlink() or not path.is_file():
            reject(f"{label} is not a regular file")
        size = path.stat().st_size
        if not (0 < size <= maximum):
            reject(f"{label} is empty or too large")
        return path.read_bytes()
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
        package = json.loads(entries["package.json"])
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
        or not isinstance(package_manager, str)
        or re.fullmatch(r"pnpm@(.+)", package_manager) is None
    ):
        reject("checked-in toolchain pins are not exact semantic versions")
    pnpm = package_manager.removeprefix("pnpm@")
    if VERSION.fullmatch(pnpm) is None:
        reject("checked-in pnpm pin is not an exact semantic version")
    return {
        "node": {"version": node, "pin": ".node-version"},
        "pnpm": {"version": pnpm, "pin": "package.json#packageManager"},
        "python": {"version": python, "pin": ".python-version"},
        "rustc": {"version": rust, "pin": "rust-toolchain.toml#toolchain.channel"},
        "uv": {"version": uv, "pin": ".uv-version"},
    }


def source_entries_from_root(root: Path) -> dict[str, bytes]:
    return {
        relative: regular_bytes(root / relative, f"source material {relative}")
        for relative in PINNED_MATERIAL_PATHS
    }


def expected_base_image(entries: dict[str, bytes]) -> str:
    try:
        value = entries[OCI_BASE_IMAGE_PATH].decode("utf-8").strip()
    except (KeyError, UnicodeError) as error:
        raise IdentityError("source has no valid OCI base-image pin") from error
    if re.fullmatch(r"[^\s@]+@sha256:[0-9a-f]{64}", value) is None:
        reject("OCI base-image pin must be an exact image digest")
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


def build_identity(
    *,
    target: str,
    revision: str,
    source_entries: dict[str, bytes],
    source_date_epoch: int,
    manifest_sha256: str,
    base_image: str | None = None,
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
    compiler: dict[str, Any] | None = None
    if target != "source":
        compiler = {
            "arguments": cargo_arguments(target),
            "name": "rustc",
            "target_triple": TARGET_TRIPLES[target],
            "version": toolchains["rustc"]["version"],
        }
    if target == "oci-linux-amd64":
        expected = expected_base_image(source_entries)
        if base_image != expected:
            reject("OCI build base image differs from the checked-in exact pin")
        oci: dict[str, Any] | None = {
            "base_image": expected,
            "build_arguments": {
                "MANIFEST_SHA256": manifest_sha256,
                "SOURCE_DATE_EPOCH": str(source_date_epoch),
                "SOURCE_REVISION": revision,
                "VERSION": _product_version(source_entries),
                "WORLDSTREAM_BASE_IMAGE": expected,
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
) -> None:
    expected = build_identity(
        target=target,
        revision=revision,
        source_entries=source_entries,
        source_date_epoch=source_date_epoch,
        manifest_sha256=manifest_sha256,
        base_image=base_image,
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
    if not isinstance(toolchains, dict) or set(toolchains) != {
        "node",
        "pnpm",
        "python",
        "rustc",
        "uv",
    }:
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
    oci_identity = build_identity(
        target="oci-linux-amd64",
        revision=revision,
        source_entries=source_entries,
        source_date_epoch=0,
        manifest_sha256=manifest_sha,
        base_image=required_labels["io.worldstream.base-image"],
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


def component_packages(
    source_entries: dict[str, bytes], version: str
) -> tuple[list[dict[str, Any]], list[tuple[str, str]]]:
    """Return deterministic first-party and locked dependency package identities."""

    packages: list[dict[str, Any]] = []
    relationships: list[tuple[str, str]] = []
    observed: set[tuple[str, str, str]] = set()

    def add(ecosystem: str, name: str, package_version: str, source: str) -> str:
        key = (ecosystem, name, package_version)
        spdx_id = _spdx_id("Component", "\0".join(key))
        if key in observed:
            return spdx_id
        observed.add(key)
        namespace = {"cargo": "cargo", "python": "pypi", "npm": "npm"}[ecosystem]
        encoded_name = name.replace("@", "%40")
        packages.append(
            {
                "SPDXID": spdx_id,
                "name": name,
                "versionInfo": package_version,
                "downloadLocation": source or "NOASSERTION",
                "filesAnalyzed": False,
                "licenseConcluded": "NOASSERTION",
                "copyrightText": "NOASSERTION",
                "externalRefs": [
                    {
                        "referenceCategory": "PACKAGE-MANAGER",
                        "referenceType": "purl",
                        "referenceLocator": f"pkg:{namespace}/{encoded_name}@{package_version}",
                    }
                ],
            }
        )
        return spdx_id

    try:
        cargo_lock = tomllib.loads(source_entries["Cargo.lock"].decode("utf-8"))
        uv_lock = tomllib.loads(source_entries["sdk/python/uv.lock"].decode("utf-8"))
        pnpm_packages = pnpm_locked_packages(source_entries["pnpm-lock.yaml"])
        ui = json.loads(source_entries["web/console/package.json"])
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
                    "referenceLocator": f"pkg:generic/worldstream@{version}?vcs_url=git%2B{REPOSITORY}%40{revision}",
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
    packages.extend(components)
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
                    "referenceLocator": f"pkg:oci/{base_name}?digest=sha256:{base_digest}",
                }
            ],
        }
    )
    tool_ids: list[str] = []
    for name, pin in sorted(identities["source-archive"]["toolchains"].items()):
        tool_id = _spdx_id("BuildTool", f"{name}@{pin['version']}")
        tool_ids.append(tool_id)
        packages.append(
            {
                "SPDXID": tool_id,
                "name": name,
                "versionInfo": pin["version"],
                "downloadLocation": "NOASSERTION",
                "filesAnalyzed": False,
                "licenseConcluded": "NOASSERTION",
                "copyrightText": "NOASSERTION",
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
    packages.sort(key=lambda item: item["SPDXID"])
    relationships.sort(
        key=lambda item: (
            item["spdxElementId"],
            item["relationshipType"],
            item["relatedSpdxElement"],
        )
    )
    return packages, relationships, [product_id]


def provenance_graph(
    *,
    version: str,
    revision: str,
    subjects: dict[str, Path],
    identities: dict[str, dict[str, Any]],
    source_entries: dict[str, bytes],
) -> dict[str, Any]:
    build_rows = []
    names = {
        artifact_id: path.name
        for artifact_id, path in subjects.items()
        if artifact_id in PAYLOAD_TARGETS
    }
    for artifact_id in PAYLOAD_TARGETS:
        identity = identities[artifact_id]
        build_rows.append(
            {
                "artifact_id": artifact_id,
                "subject": names[artifact_id],
                "build_identity_sha256": build_identity_digest(identity),
                "target": identity["target"],
                "compiler": identity["compiler"],
                "arguments": identity["arguments"],
                "oci": identity["oci"],
            }
        )
    dependencies = [
        {
            "uri": f"git+{REPOSITORY}",
            "digest": {"gitCommit": revision},
        }
    ]
    dependencies.extend(
        {
            "uri": f"file:{relative}",
            "digest": {"sha256": digest.removeprefix("sha256:")},
        }
        for relative, digest in sorted(
            identities["source-archive"]["materials"].items()
        )
    )
    base = expected_base_image(source_entries)
    dependencies.append(
        {
            "uri": "oci://" + base.split("@", 1)[0],
            "digest": {"sha256": base.rsplit("sha256:", 1)[1]},
        }
    )
    component_rows, _unused = component_packages(source_entries, version)
    return {
        "externalParameters": {
            "product": version,
            "repository": REPOSITORY,
            "source_revision": revision,
            "supply_chain_phase": "pre-sign-subject-inventory",
            "builds": build_rows,
        },
        "internalParameters": {
            "component_graph_sha256": "sha256:"
            + sha256_bytes(canonical_json(component_rows)),
            "source_date_epoch": identities["source-archive"]["source_date_epoch"],
            "toolchains": identities["source-archive"]["toolchains"],
        },
        "resolvedDependencies": dependencies,
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


def github_run_details(created: str) -> dict[str, Any]:
    repository = os.environ.get("GITHUB_REPOSITORY", "")
    workflow_ref = os.environ.get("GITHUB_WORKFLOW_REF", "")
    server = os.environ.get("GITHUB_SERVER_URL", "https://github.com").rstrip("/")
    run_id = os.environ.get("GITHUB_RUN_ID", "")
    attempt = os.environ.get("GITHUB_RUN_ATTEMPT", "")
    runner_os = os.environ.get("RUNNER_OS", "")
    runner_arch = os.environ.get("RUNNER_ARCH", "")
    image_os = os.environ.get("ImageOS", "")
    image_version = os.environ.get("ImageVersion", "")
    if repository and workflow_ref and run_id and attempt:
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
            "startedOn": created,
            "finishedOn": created,
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
            )
        ],
    }


def validate_run_details(value: object, *, require_github: bool) -> None:
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
        or set(metadata) != {"invocationId", "startedOn", "finishedOn"}
        or any(
            not isinstance(metadata.get(field), str) or not metadata[field]
            for field in metadata
        )
    ):
        reject("SLSA invocation metadata is incomplete")
    if not isinstance(byproducts, list) or len(byproducts) != 1:
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
    if require_github and content["provider"] != "github-actions":
        reject("release SLSA provenance was not generated on GitHub Actions")


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
    predicate = provenance.get("predicate")
    definition = (
        predicate.get("buildDefinition") if isinstance(predicate, dict) else None
    )
    if not isinstance(definition, dict):
        reject("SLSA buildDefinition is missing")
    expected_graph = provenance_graph(
        version=version,
        revision=revision,
        subjects={
            artifact_id: payloads_by_id[artifact_id] for artifact_id in PAYLOAD_TARGETS
        },
        identities=identities,
        source_entries=source_entries,
    )
    if definition.get("buildType") != f"{REPOSITORY}/release-build/v1":
        reject("SLSA buildType is not the WorldStream release build")
    for field, expected in expected_graph.items():
        if definition.get(field) != expected:
            reject(f"SLSA {field} differs from exact source/material/build identity")
    validate_run_details(
        predicate.get("runDetails") if isinstance(predicate, dict) else None,
        require_github=require_github,
    )

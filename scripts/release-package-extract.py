#!/usr/bin/env python3
"""Safely extract the verified daemon from a fresh Linux release archive."""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import importlib.util
import io
import json
import os
import pathlib
import stat
import sys
import tarfile
from typing import Any, BinaryIO

SHA256_PREFIX = "sha256:"
BUFFER_SIZE = 1024 * 1024
MAX_ARCHIVE_MEMBERS = 20_000
MAX_ARCHIVE_BYTES = 4 * 1024 * 1024 * 1024
MAX_ARCHIVE_UNCOMPRESSED_BYTES = 4 * 1024 * 1024 * 1024
MAX_CONTROL_FILE_BYTES = 16 * 1024 * 1024
MAX_ARCHIVE_CONTROL_FILE_BYTES = 16 * 1024 * 1024
MAX_BINARY_BYTES = 1024 * 1024 * 1024
PACKAGE_PATH = pathlib.Path(__file__).with_name("package.py")


def load_package_verifier():
    spec = importlib.util.spec_from_file_location(
        "worldstream_safe_extraction_package_verifier", PACKAGE_PATH
    )
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {PACKAGE_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


PACKAGE = load_package_verifier()


class ExtractionError(RuntimeError):
    """The package cannot safely supply a release-evidence executable."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ExtractionError(message)


def regular_file(path: pathlib.Path, label: str) -> pathlib.Path:
    try:
        mode = path.lstat().st_mode
    except FileNotFoundError as error:
        raise ExtractionError(f"missing {label}: {path}") from error
    except OSError as error:
        raise ExtractionError(f"cannot inspect {label}: {error}") from error
    require(not stat.S_ISLNK(mode), f"{label} must not be a symlink")
    require(stat.S_ISREG(mode), f"{label} must be a regular file")
    return path


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as source:
            while chunk := source.read(BUFFER_SIZE):
                digest.update(chunk)
    except OSError as error:
        raise ExtractionError(f"cannot hash {path}: {error}") from error
    return SHA256_PREFIX + digest.hexdigest()


def read_json(path: pathlib.Path, label: str) -> dict[str, Any]:
    regular_file(path, label)
    require(
        0 < path.stat().st_size <= MAX_CONTROL_FILE_BYTES,
        f"{label} exceeds its bounded size",
    )
    try:
        content = path.read_bytes()
    except OSError as error:
        raise ExtractionError(f"{label} is not valid UTF-8 JSON: {error}") from error
    try:
        return PACKAGE.json_object(content, label)
    except PACKAGE.PackageError as error:
        raise ExtractionError(str(error)) from error


def member_is_safe(member: tarfile.TarInfo) -> bool:
    raw_name = (
        member.name[:-1]
        if member.isdir() and member.name.endswith("/")
        else member.name
    )
    parts = raw_name.split("/")
    return (
        bool(raw_name)
        and not raw_name.startswith("/")
        and "\\" not in member.name
        and all(part not in {"", ".", ".."} for part in parts)
        and (member.isfile() or member.isdir())
    )


def member_bytes(
    archive: tarfile.TarFile, member: tarfile.TarInfo, label: str
) -> bytes:
    source = archive.extractfile(member)
    require(source is not None, f"cannot read archived {label}")
    try:
        return source.read()
    finally:
        source.close()


def validate_report(
    archive: pathlib.Path,
    report_path: pathlib.Path,
    manifest_toml: pathlib.Path,
    manifest_json: pathlib.Path,
) -> tuple[dict[str, Any], bytes, bytes]:
    regular_file(archive, "Linux package archive")
    report = read_json(report_path, "Linux package report")
    regular_file(manifest_toml, "compatibility TOML")
    regular_file(manifest_json, "compatibility JSON")
    require(
        0 < manifest_toml.stat().st_size <= MAX_CONTROL_FILE_BYTES
        and 0 < manifest_json.stat().st_size <= MAX_CONTROL_FILE_BYTES,
        "release-root compatibility manifests exceed their bounded size",
    )
    root_toml = manifest_toml.read_bytes()
    root_json = manifest_json.read_bytes()
    toml_digest = hashlib.sha256(root_toml).hexdigest()
    json_digest = hashlib.sha256(root_json).hexdigest()
    identity = report.get("identity")
    inventory = report.get("inventory")
    require(
        report.get("schema") == "worldstream/package-report/v1"
        and report.get("kind") == "archive"
        and report.get("artifact") == archive.name
        and report.get("path") == archive.name
        and report.get("sha256") == sha256_file(archive)
        and report.get("size_bytes") == archive.stat().st_size,
        "package report does not exactly bind the archive artifact",
    )
    require(
        isinstance(inventory, dict)
        and inventory.get("archive_verified") is True
        and inventory.get("release_evidence") is False
        and inventory.get("manifest_source") == "compatibility.toml"
        and inventory.get("manifest_mirror") == "compatibility.json",
        "package report inventory is not a verified non-promoting archive inventory",
    )
    require(
        isinstance(identity, dict)
        and identity.get("target") == "linux-x86_64"
        and isinstance(identity.get("version"), str)
        and bool(identity["version"])
        and identity.get("manifest_sha256") == json_digest
        and identity.get("manifest_json_sha256") == json_digest
        and identity.get("manifest_toml_sha256") == toml_digest,
        "package report does not bind both exact compatibility manifests",
    )
    return identity, root_toml, root_json


def preflight_archive(archive_path: pathlib.Path, version: str) -> None:
    require(
        0 < archive_path.stat().st_size <= MAX_ARCHIVE_BYTES,
        "package archive exceeds its bounded size",
    )
    try:
        with tarfile.open(archive_path, "r:gz") as archive:
            members = archive.getmembers()
    except (OSError, tarfile.TarError) as error:
        raise ExtractionError(f"cannot inspect package archive: {error}") from error
    require(
        0 < len(members) <= MAX_ARCHIVE_MEMBERS,
        "package archive member count exceeds its bound",
    )
    require(
        sum(member.size for member in members) <= MAX_ARCHIVE_UNCOMPRESSED_BYTES,
        "package archive uncompressed size exceeds its bound",
    )
    names = [member.name for member in members]
    require(len(names) == len(set(names)), "archive contains duplicate members")
    require(
        all(member_is_safe(member) for member in members),
        "archive contains an unsafe, linked, or special member",
    )
    roots = {pathlib.PurePosixPath(member.name).parts[0] for member in members}
    expected_root = f"worldstream-{version}-linux-x86_64"
    require(
        roots == {expected_root} and archive_path.name == expected_root + ".tar.gz",
        "archive does not use one canonical package root",
    )
    control_suffixes = {
        ("manifest", "compatibility.toml"),
        ("manifest", "compatibility.json"),
        ("metadata", "release.json"),
        ("metadata", "profile.json"),
    }
    for member in members:
        suffix = pathlib.PurePosixPath(member.name).parts[-2:]
        if suffix in control_suffixes or member.name.endswith("/checksums.sha256"):
            require(
                member.isfile() and member.size <= MAX_ARCHIVE_CONTROL_FILE_BYTES,
                "archive control member exceeds its bounded size",
            )
        if suffix == ("bin", "worldstreamd"):
            require(
                0 < member.size <= MAX_BINARY_BYTES,
                "archived worldstreamd exceeds its bounded size",
            )


def canonical_verify_archive(archive_path: pathlib.Path) -> None:
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            PACKAGE.verify_archive(archive_path)
    except (PACKAGE.PackageError, OSError, ValueError, tarfile.TarError) as error:
        raise ExtractionError(
            f"canonical package archive verification failed: {error}"
        ) from error


def stream_member(source: BinaryIO, output: pathlib.Path) -> tuple[str, int]:
    require(not output.exists() and not output.is_symlink(), "output must be new")
    try:
        parent_mode = output.parent.lstat().st_mode
        resolved_parent = output.parent.resolve(strict=True)
    except OSError as error:
        raise ExtractionError(
            "output parent must be a pre-existing real directory"
        ) from error
    require(
        stat.S_ISDIR(parent_mode)
        and not stat.S_ISLNK(parent_mode)
        and resolved_parent == pathlib.Path(os.path.abspath(output.parent)),
        "output parent and all ancestors must be real directories",
    )
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    digest = hashlib.sha256()
    size = 0
    descriptor = os.open(output, flags, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as destination:
            while chunk := source.read(BUFFER_SIZE):
                destination.write(chunk)
                digest.update(chunk)
                size += len(chunk)
            destination.flush()
            os.fchmod(destination.fileno(), 0o755)
            os.fsync(destination.fileno())
        require(
            sha256_file(output) == SHA256_PREFIX + digest.hexdigest(),
            "extracted daemon bytes differ from the archived member",
        )
        require(output.stat().st_size == size, "extracted daemon size changed")
    except BaseException:
        output.unlink(missing_ok=True)
        raise
    return SHA256_PREFIX + digest.hexdigest(), size


def extract(
    archive_path: pathlib.Path,
    report_path: pathlib.Path,
    output: pathlib.Path,
    manifest_toml: pathlib.Path,
    manifest_json: pathlib.Path,
) -> dict[str, Any]:
    destinations = {
        archive_path.resolve(strict=False),
        report_path.resolve(strict=False),
        manifest_toml.resolve(strict=False),
        manifest_json.resolve(strict=False),
        output.resolve(strict=False),
    }
    require(len(destinations) == 5, "output must be outside all package inputs")
    identity, root_toml, root_json = validate_report(
        archive_path, report_path, manifest_toml, manifest_json
    )
    preflight_archive(archive_path, identity["version"])
    canonical_verify_archive(archive_path)
    try:
        with tarfile.open(archive_path, "r:gz") as archive:
            members = archive.getmembers()
            names = [member.name for member in members]
            require(len(names) == len(set(names)), "archive contains duplicate members")
            require(
                all(member_is_safe(member) for member in members),
                "archive contains an unsafe, linked, or special member",
            )
            daemon_members = [
                member
                for member in members
                if pathlib.PurePosixPath(member.name).parts[-2:]
                == ("bin", "worldstreamd")
            ]
            toml_members = [
                member
                for member in members
                if pathlib.PurePosixPath(member.name).parts[-2:]
                == ("manifest", "compatibility.toml")
            ]
            json_members = [
                member
                for member in members
                if pathlib.PurePosixPath(member.name).parts[-2:]
                == ("manifest", "compatibility.json")
            ]
            require(
                len(daemon_members) == 1
                and daemon_members[0].isfile()
                and daemon_members[0].mode & 0o111 != 0,
                "archive must contain exactly one executable bin/worldstreamd",
            )
            require(
                len(toml_members) == 1 and len(json_members) == 1,
                "archive must contain exactly one compatibility manifest pair",
            )
            archived_toml = member_bytes(archive, toml_members[0], "compatibility.toml")
            archived_json = member_bytes(archive, json_members[0], "compatibility.json")
            require(
                archived_toml == root_toml and archived_json == root_json,
                "archived compatibility pair differs from the release root",
            )
            daemon_source = archive.extractfile(daemon_members[0])
            require(daemon_source is not None, "cannot read archived worldstreamd")
            try:
                binary_sha256, binary_size = stream_member(daemon_source, output)
            finally:
                daemon_source.close()
    except (OSError, tarfile.TarError) as error:
        output.unlink(missing_ok=True)
        raise ExtractionError(
            f"cannot safely inspect package archive: {error}"
        ) from error
    return {
        "schema": "worldstream/safe-package-extraction/v1",
        "status": "pass",
        "target": identity["target"],
        "version": identity["version"],
        "artifact": archive_path.name,
        "archive_sha256": sha256_file(archive_path),
        "package_report_sha256": sha256_file(report_path),
        "manifest_json_sha256": SHA256_PREFIX + hashlib.sha256(root_json).hexdigest(),
        "manifest_toml_sha256": SHA256_PREFIX + hashlib.sha256(root_toml).hexdigest(),
        "binary_sha256": binary_sha256,
        "binary_size_bytes": binary_size,
        "output": output.name,
    }


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--archive", required=True, type=pathlib.Path)
    command.add_argument("--package-report", required=True, type=pathlib.Path)
    command.add_argument("--output", required=True, type=pathlib.Path)
    command.add_argument(
        "--manifest-toml", type=pathlib.Path, default=pathlib.Path("compatibility.toml")
    )
    command.add_argument(
        "--manifest-json", type=pathlib.Path, default=pathlib.Path("compatibility.json")
    )
    return command


def main() -> int:
    args = parser().parse_args()
    try:
        result = extract(
            args.archive,
            args.package_report,
            args.output,
            args.manifest_toml,
            args.manifest_json,
        )
    except (ExtractionError, OSError, UnicodeError, json.JSONDecodeError) as error:
        print(f"safe package extraction failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, ensure_ascii=False, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Build and verify the detached subjects carried by a Starter Distribution.

This command never discovers inputs from a checkout.  A release job must name
every subject explicitly.  Except for the original Negotiate ``.wspack``, each
input is converted to a deterministic, self-describing gzip/ustar artifact.
The resulting files are inputs to the existing detached release inventory; no
approval, credential, signature, or release claim is created here.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import importlib.util
import io
import json
import os
import re
import stat
import sys
import tarfile
import tempfile
from pathlib import Path, PurePosixPath
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = "worldstream/starter-release-subject/v1"
MAX_FILES = 20_000
MAX_FILE_BYTES = 2 * 1024 * 1024 * 1024
MAX_TOTAL_BYTES = 8 * 1024 * 1024 * 1024
VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?\Z")
SHA256 = re.compile(r"sha256:[0-9a-f]{64}\Z")

SUBJECT_IDS = (
    "worldstream-a202-adapter",
    "worldstream-deterministic-agents",
    "worldstream-documentation",
    "worldstream-examples",
    "worldstream-licenses",
    "worldstream-negotiate-bundle",
    "worldstream-negotiate-evidence-verifier",
    "worldstream-pack-toolchain",
    "worldstream-participant-console",
    "worldstream-release-metadata",
    "worldstream-studio",
    "worldstream-typescript-pack-sdk",
)
SUBJECT_SET = frozenset(SUBJECT_IDS)
FORBIDDEN_PARTS = frozenset(
    {
        ".git",
        ".env",
        ".pytest_cache",
        ".ruff_cache",
        ".worldstream",
        "__pycache__",
        "node_modules",
        "target",
        "approval",
        "approvals",
        "backup",
        "backups",
        "credential",
        "credentials",
        "database",
        "databases",
        "private-key",
        "private-keys",
        "runtime-state",
        "secret",
        "secrets",
    }
)
FORBIDDEN_SUFFIXES = (".db", ".key", ".p12", ".pfx", ".pem", ".sqlite", ".sqlite3")


class SubjectError(RuntimeError):
    """A Starter release subject is incomplete, mutable, or unsafe."""


def fail(message: str) -> None:
    raise SubjectError(message)


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode()


def strict_json(content: bytes, label: str) -> dict[str, Any]:
    def pairs(items: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in items:
            if key in result:
                fail(f"{label} contains duplicate JSON key {key!r}")
            result[key] = value
        return result

    try:
        value = json.loads(content, object_pairs_hook=pairs)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise SubjectError(f"{label} is not strict JSON") from error
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object")
    return value


def safe_relative(value: object, label: str) -> str:
    if not isinstance(value, str):
        fail(f"{label} is not a path")
    path = PurePosixPath(value)
    if (
        not value
        or value != value.strip()
        or "\\" in value
        or path.is_absolute()
        or any(part in {"", ".", ".."} for part in path.parts)
        or any(ord(character) < 0x20 or ord(character) == 0x7F for character in value)
    ):
        fail(f"{label} is not a safe relative path")
    folded = tuple(part.casefold() for part in path.parts)
    if any(part in FORBIDDEN_PARTS for part in folded) or folded[-1].endswith(
        FORBIDDEN_SUFFIXES
    ):
        fail(
            f"{label} names cache, authority, credential, secret, or mutable state material"
        )
    return value


def regular_metadata(path: Path, label: str) -> os.stat_result:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise SubjectError(f"{label} is unavailable") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail(f"{label} must be a regular non-symlink file")
    if not (0 <= metadata.st_size <= MAX_FILE_BYTES):
        fail(f"{label} exceeds its byte bound")
    return metadata


def read_stable(path: Path, label: str) -> bytes:
    metadata = regular_metadata(path, label)
    try:
        content = path.read_bytes()
        finished = path.lstat()
    except OSError as error:
        raise SubjectError(f"{label} could not be read") from error
    if len(content) != metadata.st_size or (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_size,
        metadata.st_mtime_ns,
    ) != (finished.st_dev, finished.st_ino, finished.st_size, finished.st_mtime_ns):
        fail(f"{label} changed while being read")
    return content


def sha256_reference(content: bytes) -> str:
    return "sha256:" + hashlib.sha256(content).hexdigest()


def output_names(version: str) -> dict[str, str]:
    if VERSION.fullmatch(version) is None:
        fail("release version is invalid")
    return {
        "worldstream-a202-adapter": f"worldstream-{version}-a202-adapter.tar.gz",
        "worldstream-deterministic-agents": f"worldstream-{version}-deterministic-agents.tar.gz",
        "worldstream-documentation": f"worldstream-{version}-documentation.tar.gz",
        "worldstream-examples": f"worldstream-{version}-examples.tar.gz",
        "worldstream-licenses": f"worldstream-{version}-licenses.tar.gz",
        "worldstream-negotiate-bundle": f"worldstream-{version}-negotiate.wspack",
        "worldstream-negotiate-evidence-verifier": f"worldstream-{version}-negotiate-evidence-verifier.tar.gz",
        "worldstream-pack-toolchain": f"worldstream-{version}-pack-toolchain.tar.gz",
        "worldstream-participant-console": f"worldstream-{version}-participant-console.tar.gz",
        "worldstream-release-metadata": f"worldstream-{version}-release-metadata.tar.gz",
        "worldstream-studio": f"worldstream-{version}-studio.tar.gz",
        "worldstream-typescript-pack-sdk": f"worldstream-{version}-typescript-pack-sdk.tar.gz",
    }


def parse_subjects(values: list[str]) -> dict[str, Path]:
    parsed: dict[str, Path] = {}
    identities: dict[Path, str] = {}
    for value in values:
        if "=" not in value:
            fail(f"subject must use ID=PATH syntax: {value!r}")
        subject_id, raw_path = value.split("=", 1)
        if subject_id not in SUBJECT_SET:
            fail(f"unknown Starter subject id: {subject_id}")
        if subject_id in parsed:
            fail(f"duplicate Starter subject id: {subject_id}")
        path = Path(raw_path)
        try:
            resolved = path.resolve(strict=True)
        except OSError as error:
            raise SubjectError(
                f"Starter subject is unavailable: {subject_id}"
            ) from error
        if resolved in identities:
            fail(
                f"Starter subjects alias one input: {identities[resolved]} and {subject_id}"
            )
        identities[resolved] = subject_id
        parsed[subject_id] = path
    if set(parsed) != SUBJECT_SET:
        fail(
            "Starter subject input inventory is not closed: "
            f"missing={sorted(SUBJECT_SET - set(parsed))}; extra={sorted(set(parsed) - SUBJECT_SET)}"
        )
    return parsed


def input_files(path: Path, subject_id: str) -> dict[str, bytes]:
    if path.is_symlink():
        fail(f"Starter subject root must not be a symlink: {subject_id}")
    if path.is_file():
        name = safe_relative(path.name, f"{subject_id} input")
        return {name: read_stable(path, subject_id)}
    if not path.is_dir():
        fail(f"Starter subject root must be a file or directory: {subject_id}")
    files: dict[str, bytes] = {}
    total = 0
    for candidate in sorted(path.rglob("*")):
        relative = candidate.relative_to(path).as_posix()
        safe_relative(relative, f"{subject_id} input")
        if candidate.is_symlink():
            fail(f"Starter subject contains a symlink: {subject_id}/{relative}")
        if candidate.is_dir():
            continue
        content = read_stable(candidate, f"{subject_id}/{relative}")
        total += len(content)
        if total > MAX_TOTAL_BYTES:
            fail(f"Starter subject exceeds its aggregate byte bound: {subject_id}")
        files[relative] = content
        if len(files) > MAX_FILES:
            fail(f"Starter subject exceeds its file-count bound: {subject_id}")
    if not files:
        fail(f"Starter subject has no files: {subject_id}")
    return files


def artifact_files(
    subject_id: str, version: str, inputs: dict[str, bytes]
) -> dict[str, bytes]:
    rows = [
        {"path": path, "sha256": sha256_reference(content), "size_bytes": len(content)}
        for path, content in sorted(inputs.items())
    ]
    manifest = {
        "artifact_id": subject_id,
        "files": rows,
        "policy": {
            "approval_state_carried": False,
            "credentials_carried": False,
            "mutable_database_state_carried": False,
        },
        "schema": SCHEMA,
        "version": version,
    }
    root = subject_id
    return {
        f"{root}/artifact-manifest.json": canonical_json(manifest),
        **{
            f"{root}/payload/{path}": content
            for path, content in sorted(inputs.items())
        },
    }


def archive_bytes(files: dict[str, bytes]) -> bytes:
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        for name, content in sorted(files.items()):
            info = tarfile.TarInfo(name)
            info.size = len(content)
            info.mode = 0o644
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            info.mtime = 0
            archive.addfile(info, io.BytesIO(content))
    compressed = io.BytesIO()
    with gzip.GzipFile(fileobj=compressed, mode="wb", filename="", mtime=0) as stream:
        stream.write(raw.getvalue())
    return compressed.getvalue()


def atomic_write(path: Path, content: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists() or path.is_symlink():
        fail(f"refusing to overwrite Starter release subject: {path}")
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def load_starter():
    name = "worldstream_release_subject_pack_identity"
    existing = sys.modules.get(name)
    if existing is not None:
        return existing
    path = ROOT / "scripts/starter-distribution.py"
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def negotiate_pack_identity(content: bytes, label: str) -> dict[str, Any]:
    """Translate the Pack verifier's failures into this command's trust boundary."""

    starter = load_starter()
    try:
        return starter.pack_identity(content, label)
    except starter.StarterError as error:
        raise SubjectError(str(error)) from error


def verify_wrapped(content: bytes, subject_id: str, version: str) -> dict[str, Any]:
    if len(content) < 10 or content[:2] != b"\x1f\x8b" or content[4:8] != b"\0\0\0\0":
        fail(f"Starter subject is not deterministic gzip: {subject_id}")
    try:
        with tarfile.open(fileobj=io.BytesIO(content), mode="r:gz") as archive:
            members = archive.getmembers()
            names = [member.name for member in members]
            if not members or len(members) > MAX_FILES + 1 or names != sorted(names):
                fail(f"Starter subject archive order/count is invalid: {subject_id}")
            entries: dict[str, bytes] = {}
            for member in members:
                name = safe_relative(member.name, f"{subject_id} archive member")
                if (
                    not member.isfile()
                    or member.pax_headers
                    or member.mode != 0o644
                    or member.uid != 0
                    or member.gid != 0
                    or member.uname
                    or member.gname
                    or member.mtime != 0
                    or name in entries
                ):
                    fail(f"Starter subject contains a non-canonical member: {name}")
                source = archive.extractfile(member)
                if source is None:
                    fail(f"Starter subject member is unreadable: {name}")
                value = source.read(MAX_FILE_BYTES + 1)
                if len(value) != member.size:
                    fail(f"Starter subject member size differs: {name}")
                entries[name] = value
    except (OSError, EOFError, tarfile.TarError) as error:
        raise SubjectError(
            f"Starter subject is not a readable gzip/ustar archive: {subject_id}"
        ) from error
    manifest_name = f"{subject_id}/artifact-manifest.json"
    manifest_bytes = entries.get(manifest_name)
    if manifest_bytes is None:
        fail(f"Starter subject has no artifact manifest: {subject_id}")
    manifest = strict_json(manifest_bytes, f"{subject_id} artifact manifest")
    if manifest_bytes != canonical_json(manifest):
        fail(f"Starter subject artifact manifest is not canonical: {subject_id}")
    if (
        set(manifest) != {"artifact_id", "files", "policy", "schema", "version"}
        or manifest.get("schema") != SCHEMA
        or manifest.get("artifact_id") != subject_id
        or manifest.get("version") != version
        or manifest.get("policy")
        != {
            "approval_state_carried": False,
            "credentials_carried": False,
            "mutable_database_state_carried": False,
        }
        or not isinstance(manifest.get("files"), list)
        or not manifest["files"]
    ):
        fail(f"Starter subject artifact manifest is invalid: {subject_id}")
    expected_entries = {manifest_name}
    previous = ""
    for row in manifest["files"]:
        if not isinstance(row, dict) or set(row) != {"path", "sha256", "size_bytes"}:
            fail(f"Starter subject file row is invalid: {subject_id}")
        relative = safe_relative(row.get("path"), f"{subject_id} file row")
        if relative <= previous or SHA256.fullmatch(str(row.get("sha256"))) is None:
            fail(
                f"Starter subject file inventory is unordered or invalid: {subject_id}"
            )
        previous = relative
        archive_name = f"{subject_id}/payload/{relative}"
        value = entries.get(archive_name)
        if (
            value is None
            or type(row.get("size_bytes")) is not int
            or row["size_bytes"] != len(value)
            or row["sha256"] != sha256_reference(value)
        ):
            fail(f"Starter subject file bytes differ: {subject_id}/{relative}")
        expected_entries.add(archive_name)
    if set(entries) != expected_entries:
        fail(f"Starter subject archive inventory is not closed: {subject_id}")
    return manifest


def verify_payload_dir(
    payload_dir: Path, version: str, compatibility_path: Path
) -> dict[str, Any]:
    names = output_names(version)
    entries = {
        candidate.name: candidate
        for candidate in payload_dir.iterdir()
        if candidate.is_file() and not candidate.is_symlink()
    }
    expected = set(names.values())
    if set(entries) != expected:
        fail(
            "Starter subject payload directory is not closed: "
            f"missing={sorted(expected - set(entries))}; extra={sorted(set(entries) - expected)}"
        )
    compatibility = strict_json(
        read_stable(compatibility_path, "compatibility manifest"),
        "compatibility manifest",
    )
    official_rows = [
        row
        for row in compatibility.get("activity_pack_bundles", [])
        if isinstance(row, dict)
        and row.get("pack_id") == "worldstream.negotiate"
        and row.get("required_for_release") is True
        and row.get("status") == "resolved"
    ]
    if len(official_rows) != 1:
        fail("compatibility manifest has no unique official Negotiate bundle")
    identities: dict[str, dict[str, Any]] = {}
    for subject_id, filename in names.items():
        content = read_stable(entries[filename], f"Starter subject {subject_id}")
        if subject_id == "worldstream-negotiate-bundle":
            identity = negotiate_pack_identity(
                content, "official Negotiate release subject"
            )
            row = official_rows[0]
            if (
                identity.get("pack_id") != row.get("pack_id")
                or identity.get("explanatory_version") != row.get("explanatory_version")
                or identity.get("revision_digest") != row.get("revision_digest")
                or identity.get("bundle_digest") != row.get("bundle_digest")
            ):
                fail(
                    "official Negotiate release subject differs from compatibility identity"
                )
            identities[subject_id] = identity
        else:
            manifest = verify_wrapped(content, subject_id, version)
            identities[subject_id] = {
                "files": len(manifest["files"]),
                "sha256": sha256_reference(content),
            }
    return {
        "schema": "worldstream/starter-release-subject-verification/v1",
        "status": "passed",
        "version": version,
        "subjects": identities,
    }


def build(args: argparse.Namespace) -> None:
    subjects = parse_subjects(args.subject)
    names = output_names(args.version)
    if args.output_dir.exists() and args.output_dir.is_symlink():
        fail("output directory must not be a symlink")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    if any(args.output_dir.iterdir()):
        fail("output directory must be empty")
    for subject_id in SUBJECT_IDS:
        source = subjects[subject_id]
        if subject_id == "worldstream-negotiate-bundle":
            content = read_stable(source, subject_id)
            negotiate_pack_identity(content, "official Negotiate release subject")
        else:
            content = archive_bytes(
                artifact_files(
                    subject_id, args.version, input_files(source, subject_id)
                )
            )
        atomic_write(args.output_dir / names[subject_id], content)
    verify_payload_dir(args.output_dir, args.version, args.compatibility_json)


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    subcommands = command.add_subparsers(dest="command", required=True)
    build_command = subcommands.add_parser("build")
    build_command.add_argument("--version", required=True)
    build_command.add_argument("--output-dir", type=Path, required=True)
    build_command.add_argument("--subject", action="append", default=[])
    build_command.add_argument(
        "--compatibility-json", type=Path, default=ROOT / "compatibility.json"
    )
    verify_command = subcommands.add_parser("verify")
    verify_command.add_argument("--version", required=True)
    verify_command.add_argument("--payload-dir", type=Path, required=True)
    verify_command.add_argument("--compatibility-json", type=Path, required=True)
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "build":
            build(args)
            print(f"built and verified {len(SUBJECT_IDS)} detached Starter subjects")
        else:
            report = verify_payload_dir(
                args.payload_dir, args.version, args.compatibility_json
            )
            sys.stdout.buffer.write(canonical_json(report))
    except (SubjectError, OSError, ValueError, TypeError) as error:
        print(f"Starter release subject verification failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Safely verify and extract one pinned Chrome-for-Testing archive."""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import pathlib
import re
import stat
import sys
import zipfile

SHA256 = re.compile(r"^[0-9a-f]{64}$")
MAX_ARCHIVE_BYTES = 1024 * 1024 * 1024
MAX_EXPANDED_BYTES = 2 * 1024 * 1024 * 1024
MAX_MEMBERS = 10_000


class InstallFailure(RuntimeError):
    """The archive cannot be safely installed."""


def file_identity(metadata: os.stat_result) -> tuple[int, int, int, int, int, int]:
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_mode,
        metadata.st_size,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
    )


def positive_size(value: str) -> int:
    try:
        size = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("size must be an integer") from error
    if not (1 <= size <= MAX_EXPANDED_BYTES):
        raise argparse.ArgumentTypeError("size is outside the supported bound")
    return size


@contextlib.contextmanager
def stable_regular_file(path: pathlib.Path, expected_size: int, code: str):
    try:
        before = path.lstat()
    except OSError as error:
        raise InstallFailure(code) from error
    if (
        stat.S_ISLNK(before.st_mode)
        or not stat.S_ISREG(before.st_mode)
        or before.st_size != expected_size
    ):
        raise InstallFailure(code)
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
        with os.fdopen(descriptor, "rb") as source:
            opened = os.fstat(source.fileno())
            identity = file_identity(before)
            if not stat.S_ISREG(opened.st_mode) or file_identity(opened) != identity:
                raise InstallFailure(code)
            yield source
            after = os.fstat(source.fileno())
        final_path = path.lstat()
    except OSError as error:
        raise InstallFailure(code) from error
    if file_identity(after) != identity or file_identity(final_path) != identity:
        raise InstallFailure(code)


def digest_stream(source, expected_size: int, code: str) -> str:
    digest = hashlib.sha256()
    size = 0
    while chunk := source.read(1024 * 1024):
        digest.update(chunk)
        size += len(chunk)
    if size != expected_size:
        raise InstallFailure(code)
    return digest.hexdigest()


def stable_digest(path: pathlib.Path, expected_size: int, code: str) -> str:
    with stable_regular_file(path, expected_size, code) as source:
        return digest_stream(source, expected_size, code)


def safe_member(info: zipfile.ZipInfo) -> pathlib.PurePosixPath:
    path = pathlib.PurePosixPath(info.filename)
    mode = info.external_attr >> 16
    file_type = stat.S_IFMT(mode)
    if (
        not info.filename
        or "\\" in info.filename
        or path.is_absolute()
        or any(part in {"", ".", ".."} for part in path.parts)
        or info.file_size < 0
        or info.compress_size < 0
        or (info.is_dir() and file_type not in {0, stat.S_IFDIR})
        or (not info.is_dir() and file_type not in {0, stat.S_IFREG})
    ):
        raise InstallFailure("browser_archive_member_unsafe")
    return path


def install(args: argparse.Namespace) -> dict[str, object]:
    archive = pathlib.Path(args.archive)
    output = pathlib.Path(args.output)
    binary_relative = pathlib.PurePosixPath(args.binary_relative)
    if (
        not SHA256.fullmatch(args.archive_sha256)
        or not SHA256.fullmatch(args.binary_sha256)
        or binary_relative.is_absolute()
        or any(part in {"", ".", ".."} for part in binary_relative.parts)
        or "\\" in args.binary_relative
    ):
        raise InstallFailure("browser_install_identity_invalid")
    if args.archive_size > MAX_ARCHIVE_BYTES:
        raise InstallFailure("browser_archive_size_unsupported")
    archive_code = "browser_archive_identity_mismatch"
    with stable_regular_file(archive, args.archive_size, archive_code) as archive_file:
        if (
            digest_stream(archive_file, args.archive_size, archive_code)
            != args.archive_sha256
        ):
            raise InstallFailure(archive_code)
        archive_file.seek(0)
        try:
            output.mkdir(mode=0o700)
        except OSError as error:
            raise InstallFailure("browser_output_directory_unsafe") from error
        try:
            if output.is_symlink() or stat.S_IMODE(output.stat().st_mode) & 0o077:
                raise InstallFailure("browser_output_directory_unsafe")
            with zipfile.ZipFile(archive_file, "r") as source:
                members = source.infolist()
                names = [info.filename for info in members]
                if (
                    not members
                    or len(members) > MAX_MEMBERS
                    or len(names) != len(set(names))
                    or sum(info.file_size for info in members) > MAX_EXPANDED_BYTES
                ):
                    raise InstallFailure("browser_archive_inventory_invalid")
                paths = [(info, safe_member(info)) for info in members]
                roots = {path.parts[0] for _info, path in paths}
                if roots != {binary_relative.parts[0]}:
                    raise InstallFailure("browser_archive_root_mismatch")
                for info, relative in paths:
                    destination = output.joinpath(*relative.parts)
                    if info.is_dir():
                        destination.mkdir(mode=0o700, parents=True, exist_ok=True)
                        continue
                    destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
                    flags |= getattr(os, "O_NOFOLLOW", 0)
                    descriptor = os.open(destination, flags, 0o600)
                    size = 0
                    try:
                        with (
                            source.open(info, "r") as input_file,
                            os.fdopen(descriptor, "wb") as output_file,
                        ):
                            while chunk := input_file.read(1024 * 1024):
                                output_file.write(chunk)
                                size += len(chunk)
                            output_file.flush()
                            os.fsync(output_file.fileno())
                    except BaseException:
                        destination.unlink(missing_ok=True)
                        raise
                    if size != info.file_size:
                        raise InstallFailure("browser_archive_member_truncated")
            binary = output.joinpath(*binary_relative.parts)
            if (
                stable_digest(
                    binary, args.binary_size, "browser_binary_identity_mismatch"
                )
                != args.binary_sha256
            ):
                raise InstallFailure("browser_binary_identity_mismatch")
            os.chmod(binary, 0o700)
            return {
                "schema": "worldstream/pinned-browser-install/v1",
                "status": "pass",
                "archive": {
                    "sha256": "sha256:" + args.archive_sha256,
                    "size_bytes": args.archive_size,
                },
                "binary": {
                    "relative_path": binary_relative.as_posix(),
                    "sha256": "sha256:" + args.binary_sha256,
                    "size_bytes": args.binary_size,
                },
                "member_count": len(members),
                "expanded_bytes": sum(info.file_size for info in members),
            }
        except BaseException:  # noqa: TRY203 - retain failed extraction for diagnosis
            # The caller creates a job-private parent.  Keep a failed extraction in
            # place for diagnosis; it is never selected because no pass receipt is
            # emitted and the final executable identity check did not complete.
            raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", required=True)
    parser.add_argument("--archive-size", required=True, type=positive_size)
    parser.add_argument("--archive-sha256", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--binary-relative", required=True)
    parser.add_argument("--binary-size", required=True, type=positive_size)
    parser.add_argument("--binary-sha256", required=True)
    args = parser.parse_args()
    try:
        result = install(args)
    except (InstallFailure, OSError, zipfile.BadZipFile, zipfile.LargeZipFile) as error:
        code = (
            str(error)
            if isinstance(error, InstallFailure)
            else "browser_archive_invalid"
        )
        print(f"blocked:{code}", file=sys.stderr)
        return 2
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

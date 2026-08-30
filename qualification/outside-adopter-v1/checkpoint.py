#!/usr/bin/env python3
"""Capture redacted outside-adopter artifact and command checkpoints.

The tool emits canonical checkpoint JSON only. It cannot create, sign, or mark
an outside-adopter trial receipt as passed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import signal
import stat
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import BinaryIO, Protocol

SCHEMA = "worldstream/outside-adopter-checkpoint/v1"
MAX_ARTIFACT_BYTES = 8 * 1024 * 1024 * 1024
MAX_CAPTURE_BYTES = 64 * 1024 * 1024
COPY_BYTES = 1024 * 1024
SAFE_CODE = re.compile(r"[a-z][a-z0-9_-]{0,127}\Z")


class CheckpointError(RuntimeError):
    """A checkpoint target or command result is unsafe to capture."""


class HexDigest(Protocol):
    def hexdigest(self) -> str: ...


def fail(message: str) -> None:
    raise CheckpointError(message)


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")


def safe_code(value: str, label: str) -> str:
    if SAFE_CODE.fullmatch(value) is None:
        fail(f"{label} must be a bounded lowercase safe code")
    return value


def sha256_reference(digest: HexDigest) -> str:
    return "sha256:" + digest.hexdigest()


def regular_metadata(path: Path, label: str, maximum: int) -> os.stat_result:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise CheckpointError(f"{label} is unavailable") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail(f"{label} must be a regular non-symlink file")
    if metadata.st_size <= 0 or metadata.st_size > maximum:
        fail(f"{label} is empty or exceeds its byte bound")
    return metadata


def digest_path(path: Path, label: str) -> tuple[str, int]:
    before = regular_metadata(path, label, MAX_ARTIFACT_BYTES)
    digest = hashlib.sha256()
    observed = 0
    try:
        with path.open("rb") as stream:
            while chunk := stream.read(COPY_BYTES):
                digest.update(chunk)
                observed += len(chunk)
        after = path.lstat()
    except OSError as error:
        raise CheckpointError(f"{label} could not be read") from error
    witness = (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
    finished = (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
    if witness != finished or observed != before.st_size:
        fail(f"{label} changed while it was hashed")
    return sha256_reference(digest), observed


def exclusive_write(path: Path, content: bytes) -> None:
    if path.exists() or path.is_symlink():
        fail(f"refusing to overwrite checkpoint: {path}")
    parent = path.parent
    if parent.is_symlink() or not parent.is_dir():
        fail("checkpoint output parent must be a real directory")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags, 0o600)
        with os.fdopen(descriptor, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
    except OSError as error:
        raise CheckpointError(
            f"checkpoint output could not be created: {path}"
        ) from error


def artifact_checkpoint(name: str, artifact: Path) -> dict[str, object]:
    name = safe_code(name, "artifact name")
    digest, size = digest_path(artifact, f"artifact {name}")
    return {
        "checkpoint_id": name,
        "kind": "artifact",
        "release_evidence": False,
        "receipt_row": {"name": name, "sha256": digest, "size_bytes": size},
        "schema": SCHEMA,
    }


def command_checkpoint(
    name: str, argv: list[str], timeout_seconds: int
) -> tuple[dict[str, object], int]:
    name = safe_code(name, "command checkpoint name")
    if not argv or any(not value or "\0" in value for value in argv):
        fail("command must be a non-empty argv vector")
    executable = Path(argv[0]).name
    if not executable or len(executable.encode("utf-8")) > 160:
        fail("command executable basename is invalid")
    argv_digest = hashlib.sha256(canonical_json(argv))
    started = time.monotonic_ns()
    timed_out = False
    try:
        process = subprocess.Popen(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=os.name == "posix",
        )
    except OSError as error:
        raise CheckpointError("command could not be executed") from error

    capture: dict[str, tuple[str, int]] = {}
    capture_errors: list[str] = []
    capture_lock = threading.Lock()

    def kill_process() -> None:
        try:
            if os.name == "posix":
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
        except (OSError, ProcessLookupError):
            pass

    def consume(label: str, stream: BinaryIO) -> None:
        digest = hashlib.sha256()
        size = 0
        try:
            while chunk := stream.read(COPY_BYTES):
                size += len(chunk)
                if size > MAX_CAPTURE_BYTES:
                    with capture_lock:
                        capture_errors.append(
                            f"{label} capture exceeds {MAX_CAPTURE_BYTES} bytes"
                        )
                    kill_process()
                    return
                digest.update(chunk)
        except OSError:
            with capture_lock:
                capture_errors.append(f"{label} capture could not be read")
            kill_process()
            return
        finally:
            stream.close()
        with capture_lock:
            capture[label] = (sha256_reference(digest), size)

    if process.stdout is None or process.stderr is None:  # pragma: no cover
        kill_process()
        fail("command capture pipes are unavailable")
    threads = [
        threading.Thread(target=consume, args=("stdout", process.stdout), daemon=True),
        threading.Thread(target=consume, args=("stderr", process.stderr), daemon=True),
    ]
    for thread in threads:
        thread.start()
    try:
        return_code = process.wait(timeout=timeout_seconds)
    except subprocess.TimeoutExpired:
        timed_out = True
        kill_process()
        process.wait()
        return_code = 124
    for thread in threads:
        thread.join(timeout=5)
    if any(thread.is_alive() for thread in threads):
        fail("command capture pipes did not close")
    if capture_errors:
        fail(capture_errors[0])
    if set(capture) != {"stdout", "stderr"}:  # pragma: no cover
        fail("command capture is incomplete")
    stdout_sha, stdout_size = capture["stdout"]
    stderr_sha, stderr_size = capture["stderr"]
    duration_ms = max(0, (time.monotonic_ns() - started) // 1_000_000)
    normalized_exit = 128 + abs(return_code) if return_code < 0 else return_code
    normalized_exit = min(normalized_exit, 255)
    checkpoint = {
        "capture": {
            "executable_basename": executable,
            "stderr_size_bytes": stderr_size,
            "stdout_size_bytes": stdout_size,
            "timed_out": timed_out,
        },
        "checkpoint_id": name,
        "kind": "command",
        "release_evidence": False,
        "receipt_row": {
            "argv_sha256": sha256_reference(argv_digest),
            "duration_ms": duration_ms,
            "exit_code": normalized_exit,
            "name": name,
            "stderr_sha256": stderr_sha,
            "stdout_sha256": stdout_sha,
        },
        "schema": SCHEMA,
    }
    return checkpoint, normalized_exit


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    subcommands = command.add_subparsers(dest="command", required=True)
    artifact = subcommands.add_parser("artifact")
    artifact.add_argument("--name", required=True)
    artifact.add_argument("--artifact", required=True, type=Path)
    artifact.add_argument("--output", required=True, type=Path)
    run = subcommands.add_parser("command")
    run.add_argument("--name", required=True)
    run.add_argument("--output", required=True, type=Path)
    run.add_argument("--timeout-seconds", type=int, default=600)
    run.add_argument("argv", nargs=argparse.REMAINDER)
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "artifact":
            checkpoint = artifact_checkpoint(args.name, args.artifact)
            exit_code = 0
        else:
            if not (1 <= args.timeout_seconds <= 3600):
                fail("command timeout must be between 1 and 3600 seconds")
            command_argv = args.argv[1:] if args.argv[:1] == ["--"] else args.argv
            checkpoint, exit_code = command_checkpoint(
                args.name, command_argv, args.timeout_seconds
            )
        exclusive_write(args.output, canonical_json(checkpoint))
    except CheckpointError as error:
        print(f"outside-adopter checkpoint failed: {error}", file=sys.stderr)
        return 1
    print(
        f"captured {checkpoint['kind']} checkpoint {checkpoint['checkpoint_id']}",
        file=sys.stderr,
    )
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())

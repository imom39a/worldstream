#!/usr/bin/env python3
"""Fail closed unless secret sentinel forms are absent from bounded channels."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import stat
import sys
import tempfile
from pathlib import Path

SCHEMA = "worldstream/secret-absence-scan/v1"
MATRIX_SCHEMA = "worldstream/secret-absence-matrix/v1"
MAX_SENTINEL_BYTES = 512
MAX_CHANNEL_BYTES = 16 * 1024 * 1024
MAX_CHANNELS = 2048
MAX_SENTINELS = 128


class ScanError(RuntimeError):
    """The supplied scan boundary is unsafe or contains a secret."""


def regular_bytes(
    path: Path, label: str, maximum: int, *, allow_empty: bool = False
) -> bytes:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise ScanError(f"cannot inspect {label}") from error
    if (
        not stat.S_ISREG(metadata.st_mode)
        or stat.S_ISLNK(metadata.st_mode)
        or not ((0 if allow_empty else 1) <= metadata.st_size <= maximum)
    ):
        raise ScanError(f"{label} is not a bounded regular file")
    try:
        value = path.read_bytes()
    except OSError as error:
        raise ScanError(f"cannot read {label}") from error
    if len(value) != metadata.st_size:
        raise ScanError(f"{label} changed while reading")
    return value


def parse_channels(values: list[str]) -> dict[str, Path]:
    if not values or len(values) > MAX_CHANNELS:
        raise ScanError("secret scan requires a bounded non-empty channel set")
    channels: dict[str, Path] = {}
    for assignment in values:
        name, separator, raw_path = assignment.partition("=")
        if (
            not separator
            or not name
            or not name.replace("-", "").isalnum()
            or name in channels
            or not raw_path
        ):
            raise ScanError(f"invalid or duplicate channel assignment: {assignment!r}")
        channels[name] = Path(raw_path)
    return channels


def sentinel_forms(secret: bytes) -> dict[str, bytes]:
    forms = {
        "raw": secret,
        "hex": secret.hex().encode("ascii"),
        "base64": base64.b64encode(secret),
        "base64url": base64.urlsafe_b64encode(secret).rstrip(b"="),
    }
    return dict(sorted(set(forms.items())))


def form_present(encoding: str, encoded: bytes, content: bytes) -> bool:
    """Match common lossless spelling variants without weakening the claim."""

    if encoding == "hex":
        return encoded in content.lower()
    if encoding in {"base64", "base64url"}:
        return encoded.rstrip(b"=") in content
    return encoded in content


def _sentinel_value(value: bytes, label: str) -> bytes:
    if not isinstance(value, bytes) or not (16 <= len(value) <= MAX_SENTINEL_BYTES):
        raise ScanError(f"{label} does not have bounded 128-bit sentinel material")
    return value


def scan_sentinels(sentinels: dict[str, bytes], channels: dict[str, Path]) -> dict:
    """Scan every channel for every named sentinel without retaining material.

    This matrix form is intended for process lanes that exercise several real
    secret classes.  The original one-sentinel ``scan`` contract remains
    byte-compatible for the OCI release lane.
    """

    if not sentinels or len(sentinels) > MAX_SENTINELS:
        raise ScanError("secret scan requires a bounded non-empty sentinel set")
    if not channels or len(channels) > MAX_CHANNELS:
        raise ScanError("secret scan requires a bounded non-empty channel set")
    normalized: dict[str, bytes] = {}
    for name, value in sentinels.items():
        if (
            not isinstance(name, str)
            or not name
            or not name.replace("-", "").isalnum()
            or name in normalized
        ):
            raise ScanError("secret scan sentinel name is invalid or duplicated")
        normalized[name] = _sentinel_value(value, f"secret sentinel {name}")
    for name, path in channels.items():
        if (
            not isinstance(name, str)
            or not name
            or not name.replace("-", "").isalnum()
            or not isinstance(path, Path)
        ):
            raise ScanError("secret scan channel name or path is invalid")
    records = []
    encodings = ["base64", "base64url", "hex", "raw"]
    for name, path in sorted(channels.items()):
        content = regular_bytes(
            path,
            f"secret scan channel {name}",
            MAX_CHANNEL_BYTES,
            allow_empty=True,
        )
        matches = [
            (sentinel_name, encoding)
            for sentinel_name, secret in sorted(normalized.items())
            for encoding, encoded in sentinel_forms(secret).items()
            if form_present(encoding, encoded, content)
        ]
        if matches:
            sentinel_name, encoding = matches[0]
            raise ScanError(
                f"secret sentinel {sentinel_name} detected in channel {name} "
                f"as {encoding}"
            )
        records.append(
            {
                "channel": name,
                "sha256": "sha256:" + hashlib.sha256(content).hexdigest(),
                "size_bytes": len(content),
            }
        )
    return {
        "schema": MATRIX_SCHEMA,
        "status": "pass",
        "secrets_emitted": False,
        "encodings_scanned": encodings,
        "sentinels": [
            {
                "name": name,
                "sha256": "sha256:" + hashlib.sha256(value).hexdigest(),
                "size_bytes": len(value),
            }
            for name, value in sorted(normalized.items())
        ],
        "channels": records,
    }


def scan(sentinel_path: Path, channels: dict[str, Path]) -> dict:
    sentinel = _sentinel_value(
        regular_bytes(sentinel_path, "secret sentinel", MAX_SENTINEL_BYTES),
        "secret sentinel",
    )
    forms = sentinel_forms(sentinel)
    records = []
    for name, path in sorted(channels.items()):
        content = regular_bytes(
            path,
            f"secret scan channel {name}",
            MAX_CHANNEL_BYTES,
            allow_empty=True,
        )
        matches = [
            encoding
            for encoding, value in forms.items()
            if form_present(encoding, value, content)
        ]
        if matches:
            raise ScanError(
                f"secret sentinel detected in channel {name} as {','.join(matches)}"
            )
        records.append(
            {
                "channel": name,
                "sha256": "sha256:" + hashlib.sha256(content).hexdigest(),
                "size_bytes": len(content),
            }
        )
    return {
        "schema": SCHEMA,
        "status": "pass",
        "secrets_emitted": False,
        "sentinel_sha256": "sha256:" + hashlib.sha256(sentinel).hexdigest(),
        "encodings_scanned": sorted(forms),
        "channels": records,
    }


def atomic_write(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.is_symlink() or path.is_dir():
        raise ScanError("unsafe secret scan output path")
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump(value, output, sort_keys=True, separators=(",", ":"))
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o600)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--sentinel-file", type=Path, required=True)
    command.add_argument("--channel", action="append", default=[], required=True)
    command.add_argument("--output", type=Path, required=True)
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        result = scan(args.sentinel_file, parse_channels(args.channel))
        atomic_write(args.output, result)
    except ScanError as error:
        print(f"secret absence verification failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

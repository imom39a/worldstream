#!/usr/bin/env python3
"""Fail closed unless a Docker volume is an unconfigured local volume."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from typing import Any


class VolumeVerificationError(RuntimeError):
    """Docker volume metadata cannot prove the frozen local-volume policy."""


def validate_volume_records(value: Any, expected_name: str) -> dict[str, Any]:
    if not isinstance(value, list) or len(value) != 1 or not isinstance(value[0], dict):
        raise VolumeVerificationError("volume inspect must return exactly one record")
    record = value[0]
    if record.get("Name") != expected_name:
        raise VolumeVerificationError("volume inspect returned the wrong volume")
    if record.get("Driver") != "local":
        raise VolumeVerificationError("volume driver is not local")
    if record.get("Scope") != "local":
        raise VolumeVerificationError("volume scope is not local")
    options = record.get("Options")
    if options not in (None, {}):
        raise VolumeVerificationError(
            "local volume has driver options and may be bind- or network-backed"
        )
    mountpoint = record.get("Mountpoint")
    if (
        not isinstance(mountpoint, str)
        or not os.path.isabs(mountpoint)
        or any(ord(character) < 32 for character in mountpoint)
    ):
        raise VolumeVerificationError("local volume mountpoint is invalid")
    return {
        "driver": "local",
        "driver_options": {},
        "scope": "local",
    }


def inspect_volume(name: str) -> dict[str, Any]:
    try:
        completed = subprocess.run(
            ["docker", "volume", "inspect", name],
            check=False,
            capture_output=True,
            text=True,
        )
    except OSError as error:
        raise VolumeVerificationError(
            f"cannot execute docker volume inspect: {error}"
        ) from error
    if completed.returncode != 0:
        raise VolumeVerificationError("docker volume inspect failed")
    try:
        value = json.loads(completed.stdout)
    except (UnicodeError, json.JSONDecodeError) as error:
        raise VolumeVerificationError(
            "docker volume inspect returned invalid JSON"
        ) from error
    return validate_volume_records(value, name)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--volume", required=True)
    args = parser.parse_args()
    try:
        result = inspect_volume(args.volume)
    except VolumeVerificationError as error:
        print(f"local Docker volume rejected: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

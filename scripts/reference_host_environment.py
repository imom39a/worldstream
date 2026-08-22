#!/usr/bin/env python3
"""Observe the frozen Linux reference host without trusting shell evaluation."""

from __future__ import annotations

import os
import pathlib
import platform
import re
import shlex
from typing import Any

MAX_OS_RELEASE_BYTES = 64 * 1024


class ReferenceHostError(RuntimeError):
    """A required frozen-host fact could not be measured safely."""


def cpu_model() -> str:
    try:
        for line in (
            pathlib.Path("/proc/cpuinfo").read_text(encoding="utf-8").splitlines()
        ):
            if line.startswith("model name") and ":" in line:
                value = line.split(":", 1)[1].strip()
                if value:
                    return value
    except OSError as error:
        raise ReferenceHostError("Linux CPU model could not be measured") from error
    raise ReferenceHostError("Linux CPU model could not be measured")


def effective_memory_bytes() -> int:
    candidates: list[int] = []
    try:
        for line in (
            pathlib.Path("/proc/meminfo").read_text(encoding="utf-8").splitlines()
        ):
            if line.startswith("MemTotal:"):
                candidates.append(int(line.split()[1]) * 1024)
                break
    except (OSError, ValueError) as error:
        raise ReferenceHostError(
            "Linux memory capacity could not be measured"
        ) from error
    for path in (
        pathlib.Path("/sys/fs/cgroup/memory.max"),
        pathlib.Path("/sys/fs/cgroup/memory/memory.limit_in_bytes"),
    ):
        try:
            value = path.read_text(encoding="utf-8").strip()
        except OSError:
            continue
        if value != "max":
            try:
                limit = int(value)
            except ValueError as error:
                raise ReferenceHostError(
                    "Linux cgroup memory limit was invalid"
                ) from error
            if 0 < limit < 1 << 60:
                candidates.append(limit)
    if not candidates or min(candidates) <= 0:
        raise ReferenceHostError("Linux memory capacity could not be measured")
    return min(candidates)


def linux_distribution(
    os_release_path: pathlib.Path = pathlib.Path("/etc/os-release"),
) -> dict[str, str]:
    try:
        raw = os_release_path.read_bytes()
    except OSError as error:
        raise ReferenceHostError(
            "Linux distribution disclosure was unavailable"
        ) from error
    if not raw or len(raw) > MAX_OS_RELEASE_BYTES or b"\0" in raw:
        raise ReferenceHostError("Linux distribution disclosure was invalid")
    try:
        lines = raw.decode("utf-8").splitlines()
    except UnicodeDecodeError as error:
        raise ReferenceHostError(
            "Linux distribution disclosure was not UTF-8"
        ) from error
    values: dict[str, str] = {}
    for line in lines:
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        key, separator, encoded = stripped.partition("=")
        if not separator or re.fullmatch(r"[A-Z][A-Z0-9_]*", key) is None:
            raise ReferenceHostError("Linux distribution disclosure was malformed")
        try:
            parsed = shlex.split(encoded, comments=False, posix=True)
        except ValueError as error:
            raise ReferenceHostError(
                "Linux distribution disclosure was malformed"
            ) from error
        if len(parsed) != 1 or any(ord(character) < 32 for character in parsed[0]):
            raise ReferenceHostError("Linux distribution disclosure was malformed")
        values[key] = parsed[0]
    distribution = values.get("NAME")
    distribution_version = values.get("VERSION_ID")
    if not distribution or not distribution_version:
        raise ReferenceHostError("Linux distribution identity was incomplete")
    return {
        "distribution": distribution,
        "distribution_version": distribution_version,
    }


def unescape_mount_path(value: str) -> str:
    for encoded, decoded in (
        ("\\040", " "),
        ("\\011", "\t"),
        ("\\012", "\n"),
        ("\\134", "\\"),
    ):
        value = value.replace(encoded, decoded)
    return value


def sysfs_block_leaves(
    device_id: str, sys_dev_block: pathlib.Path
) -> list[pathlib.Path]:
    if re.fullmatch(r"[0-9]+:[0-9]+", device_id) is None:
        raise ReferenceHostError("Linux mount block-device identity was invalid")
    try:
        root = (sys_dev_block / device_id).resolve(strict=True)
    except OSError as error:
        raise ReferenceHostError(
            "Linux mount block-device identity was unavailable"
        ) from error
    pending = [root]
    leaves: list[pathlib.Path] = []
    visited: set[pathlib.Path] = set()
    while pending:
        current = pending.pop()
        if current in visited:
            raise ReferenceHostError("Linux block-device topology contained a cycle")
        visited.add(current)
        slaves_dir = current / "slaves"
        try:
            slaves = sorted(slaves_dir.iterdir()) if slaves_dir.is_dir() else []
            resolved_slaves = [slave.resolve(strict=True) for slave in slaves]
        except OSError as error:
            raise ReferenceHostError(
                "Linux block-device topology was unavailable"
            ) from error
        if resolved_slaves:
            pending.extend(resolved_slaves)
        else:
            leaves.append(current)
    if not leaves:
        raise ReferenceHostError("Linux block-device topology had no physical leaves")
    return leaves


def rotational_value(device: pathlib.Path) -> str:
    for candidate in (device, *device.parents):
        value_path = candidate / "queue" / "rotational"
        try:
            if value_path.is_file():
                return value_path.read_text(encoding="utf-8").strip()
        except OSError as error:
            raise ReferenceHostError(
                "Linux block-device rotation status was unavailable"
            ) from error
        if candidate == pathlib.Path("/sys"):
            break
    raise ReferenceHostError("Linux block-device rotation status was unavailable")


def storage_class(device_id: str, sys_dev_block: pathlib.Path) -> str:
    leaves = sysfs_block_leaves(device_id, sys_dev_block)
    for leaf in leaves:
        name = leaf.name.lower()
        if re.match(r"(?:loop|ram|zram|nbd|rbd|drbd)", name):
            raise ReferenceHostError(
                "reference path was not backed by a local SSD/NVMe"
            )
        if rotational_value(leaf) != "0":
            raise ReferenceHostError(
                "reference path was not backed by a local SSD/NVMe"
            )
    return "local_ssd_or_nvme"


def filesystem_environment(
    path: pathlib.Path,
    mountinfo_path: pathlib.Path = pathlib.Path("/proc/self/mountinfo"),
    sys_dev_block: pathlib.Path = pathlib.Path("/sys/dev/block"),
) -> dict[str, Any]:
    target = path.resolve()
    selected: tuple[int, str, list[str], str] | None = None
    try:
        lines = mountinfo_path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise ReferenceHostError(
            "Linux filesystem disclosure was unavailable"
        ) from error
    for line in lines:
        before, separator, after = line.partition(" - ")
        left = before.split()
        right = after.split()
        if not separator or len(left) < 6 or len(right) < 3:
            raise ReferenceHostError("Linux mountinfo row was malformed")
        mount_point = pathlib.Path(unescape_mount_path(left[4]))
        try:
            target.relative_to(mount_point)
        except ValueError:
            continue
        options = sorted(set(left[5].split(",") + right[2].split(",")))
        candidate = (len(mount_point.parts), right[0], options, left[2])
        if selected is None or candidate[0] > selected[0]:
            selected = candidate
    if selected is None or selected[1] not in {"ext4", "xfs"} or not selected[2]:
        raise ReferenceHostError("reference path requires an ext4 or XFS mount")
    return {
        "type": selected[1],
        "mount_options": selected[2],
        "storage_class": storage_class(selected[3], sys_dev_block),
    }


def observe(path: pathlib.Path) -> dict[str, Any]:
    if platform.system() != "Linux":
        raise ReferenceHostError("reference host must be Linux")
    logical_cpu_count = os.cpu_count()
    if logical_cpu_count is None or logical_cpu_count <= 0:
        raise ReferenceHostError("Linux logical CPU count could not be measured")
    machine = platform.machine().lower()
    if machine == "amd64":
        machine = "x86_64"
    return {
        "platform": {
            "system": "Linux",
            **linux_distribution(),
            "machine": machine,
        },
        "hardware": {
            "cpu_model": cpu_model(),
            "logical_cpu_count": logical_cpu_count,
            "memory_bytes": effective_memory_bytes(),
        },
        "filesystem": filesystem_environment(path),
    }

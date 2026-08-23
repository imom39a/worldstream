#!/usr/bin/env python3
"""Run and attest one hosted restore through the exact packaged control CLI."""

from __future__ import annotations

import argparse
import errno
import hashlib
import importlib.util
import json
import os
import platform
import re
import secrets as secrets_module
import signal
import stat
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

SCHEMA = "worldstream/hosted-native-postgres-restore-evidence/v3"
PRIVATE_BINDING_SCHEMA = "worldstream/hosted-native-postgres-private-binding/v1"
PRIVATE_BINDING_ROOT_NAME = "worldstream-native-platform-binding"
PRIVATE_BINDING_FILES = {
    "artifact_directory_stdout": "artifact-directory.stdout",
    "snapshot_rebuild_stdout": "snapshot-rebuild.stdout",
    "native_restore_stdout": "native-restore.stdout",
    "native_report": "native-restore-report.json",
    "native_dump": "native-restore.dump",
}
FIXTURE_SCHEMA = "worldstream/sqlite-postgresql-transfer-evidence/v1"
NATIVE_SCHEMA = "worldstream/native-postgres-restore-evidence/v2"
RUNTIME_SCHEMA = "worldstream/native-package-runtime-smoke/v1"
FIXTURE_CLASSIFICATION = "untrusted_source_bound_input_construction"
PRIVATE_ARTIFACT_DISPOSITION = (
    "exact_retained_root_scrubbed_to_zero_length_placeholders"
)
MAX_JSON_BYTES = 64 * 1024 * 1024
MAX_ARCHIVE_BYTES = 8 * 1024 * 1024 * 1024
MAX_CONTROL_BYTES = 512 * 1024 * 1024
MAX_PROVIDER_TOOL_BYTES = 512 * 1024 * 1024
MAX_COMMAND_OUTPUT_BYTES = 8 * 1024 * 1024
MAX_COMMAND_INPUT_BYTES = 1024 * 1024
MAX_PASSFILE_BYTES = 64 * 1024
MIN_TIMEOUT_SECONDS = 12
MAX_TIMEOUT_SECONDS = 3_600
PRESERVED_RECOVERY_EXIT_CODE = 42
SAFE_FAILURE_EXIT_CODE = 43
SHA256_REF = re.compile(r"^sha256:[0-9a-f]{64}$")
GIT_REVISION = re.compile(r"^[0-9a-f]{40}$")
SAFE_SQL_IDENTIFIER = re.compile(r"^[A-Za-z_][A-Za-z0-9_]{0,62}$")
RECOVERY_JOURNAL = re.compile(r"^\.worldstream_native_recovery_[0-9a-f]{32}\.json$")
DISPOSABLE_TARGET_MARKER = "worldstream/native-postgres-disposable-target/v1"
SNAPSHOT_RECEIPT_SCHEMA = "worldstream/postgres-native-snapshot-rebuild-receipt/v1"
RESTORE_RECEIPT_SCHEMA = "worldstream/postgres-native-restore-receipt/v1"
DIRECTORY_RECEIPT_SCHEMA = "worldstream/postgres-native-artifact-directory-identity/v1"
BLAKE3_IMPLEMENTATION_PATH = Path(__file__).with_name("manifest-evidence-wave6.py")
PLATFORMS = {
    "native-linux": ("Linux", {"x86_64", "amd64"}),
    "native-windows": ("Windows", {"x86_64", "amd64"}),
}
PACKAGE_BINDING_FIELDS = (
    "archive_sha256",
    "archive_size_bytes",
    "package_report_sha256",
    "package_report_size_bytes",
    "source_revision",
    "worldstreamctl_sha256",
    "worldstreamctl_size_bytes",
)


class HostedReportError(RuntimeError):
    """The supplied files cannot support a hosted restore wrapper."""


class ContainmentUncertainError(HostedReportError):
    """A launched command tree has not been proven absent."""


class RecoveryPreservedError(HostedReportError):
    """Recovery authority was deliberately preserved after a failed run."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise HostedReportError(message)


def _load_blake3():
    name = "worldstream_hosted_restore_blake3"
    existing = sys.modules.get(name)
    if existing is not None:
        return existing.blake3
    spec = importlib.util.spec_from_file_location(name, BLAKE3_IMPLEMENTATION_PATH)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise HostedReportError("cannot load the reviewed BLAKE3 verifier")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module.blake3


BLAKE3 = _load_blake3()


def _same_identity(left: os.stat_result, right: os.stat_result) -> bool:
    return left.st_dev == right.st_dev and left.st_ino == right.st_ino


def _canonical_file_identity(descriptor: int) -> dict[str, str]:
    """Return the exact identity format emitted by the packaged Rust CLI."""

    if os.name != "nt":
        metadata = os.fstat(descriptor)
        return {
            "storage_id": f"{metadata.st_dev:016x}",
            "file_id": f"{metadata.st_ino:032x}",
        }

    import ctypes
    import msvcrt
    from ctypes import wintypes

    class FileId128(ctypes.Structure):
        _fields_ = (("identifier", ctypes.c_ubyte * 16),)

    class FileIdInfo(ctypes.Structure):
        _fields_ = (
            ("volume_serial_number", ctypes.c_ulonglong),
            ("file_id", FileId128),
        )

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    get_information = kernel32.GetFileInformationByHandleEx
    get_information.argtypes = (
        wintypes.HANDLE,
        ctypes.c_int,
        wintypes.LPVOID,
        wintypes.DWORD,
    )
    get_information.restype = wintypes.BOOL
    information = FileIdInfo()
    if not get_information(
        msvcrt.get_osfhandle(descriptor),
        18,  # FileIdInfo
        ctypes.byref(information),
        ctypes.sizeof(information),
    ):
        raise OSError(ctypes.get_last_error(), "GetFileInformationByHandleEx failed")
    identifier = bytes(information.file_id.identifier)
    return {
        "storage_id": f"{int(information.volume_serial_number):016x}",
        "file_id": f"{int.from_bytes(identifier, sys.byteorder):032x}",
    }


def _canonical_identity_is_valid(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and set(value) == {"storage_id", "file_id"}
        and isinstance(value.get("storage_id"), str)
        and re.fullmatch(r"[0-9a-f]{16}", value["storage_id"]) is not None
        and isinstance(value.get("file_id"), str)
        and re.fullmatch(r"[0-9a-f]{32}", value["file_id"]) is not None
    )


def _safe_directory_metadata(value: os.stat_result) -> bool:
    if not stat.S_ISDIR(value.st_mode):
        return False
    return not (
        os.name == "nt" and getattr(value, "st_file_attributes", 0) & 0x00000400
    )


def _open_exact(
    path: Path,
    label: str,
    *,
    writable: bool = False,
    share_write: bool = False,
) -> tuple[int, os.stat_result]:
    try:
        admitted = path.lstat()
    except OSError as error:
        raise HostedReportError(f"cannot inspect {label}: {path}") from error
    require(stat.S_ISREG(admitted.st_mode), f"{label} must be a regular file")
    try:
        if os.name == "nt":
            import ctypes
            import msvcrt
            from ctypes import wintypes

            create_file = ctypes.windll.kernel32.CreateFileW
            create_file.argtypes = (
                wintypes.LPCWSTR,
                wintypes.DWORD,
                wintypes.DWORD,
                wintypes.LPVOID,
                wintypes.DWORD,
                wintypes.DWORD,
                wintypes.HANDLE,
            )
            create_file.restype = wintypes.HANDLE
            handle = create_file(
                os.fspath(path),
                0xC0000000 if writable else 0x80000000,  # GENERIC_READ[_WRITE]
                0x00000001 | (0x00000002 if share_write else 0),
                None,
                3,  # OPEN_EXISTING
                0x00000080 | 0x00200000,  # NORMAL | OPEN_REPARSE_POINT
                None,
            )
            invalid_handle = wintypes.HANDLE(-1).value
            if handle == invalid_handle:
                raise OSError(ctypes.get_last_error(), "CreateFileW failed")
            try:
                descriptor = msvcrt.open_osfhandle(
                    handle,
                    (os.O_RDWR if writable else os.O_RDONLY)
                    | getattr(os, "O_BINARY", 0),
                )
            except BaseException:
                ctypes.windll.kernel32.CloseHandle(handle)
                raise
        else:
            flags = (os.O_RDWR if writable else os.O_RDONLY) | getattr(
                os, "O_BINARY", 0
            )
            flags |= getattr(os, "O_NOFOLLOW", 0)
            descriptor = os.open(path, flags)
    except OSError as error:
        raise HostedReportError(f"cannot open {label}: {path}") from error
    try:
        opened = os.fstat(descriptor)
        require(
            stat.S_ISREG(opened.st_mode) and _same_identity(admitted, opened),
            f"{label} changed during admission",
        )
    except BaseException:
        os.close(descriptor)
        raise
    return descriptor, opened


def _read_pass(descriptor: int, maximum_size: int, label: str) -> bytes:
    os.lseek(descriptor, 0, os.SEEK_SET)
    chunks: list[bytes] = []
    observed = 0
    while True:
        chunk = os.read(descriptor, min(1024 * 1024, maximum_size + 1 - observed))
        if not chunk:
            break
        chunks.append(chunk)
        observed += len(chunk)
        require(observed <= maximum_size, f"{label} exceeds its byte bound")
    return b"".join(chunks)


def _digest_pass(descriptor: int, maximum_size: int, label: str) -> tuple[bytes, int]:
    os.lseek(descriptor, 0, os.SEEK_SET)
    digest = hashlib.sha256()
    observed = 0
    while True:
        chunk = os.read(descriptor, 1024 * 1024)
        if not chunk:
            break
        observed += len(chunk)
        require(observed <= maximum_size, f"{label} exceeds its byte bound")
        digest.update(chunk)
    return digest.digest(), observed


def _verify_named_identity(
    path: Path, label: str, descriptor: int, admitted: os.stat_result
) -> None:
    after = os.fstat(descriptor)
    try:
        named = path.lstat()
    except OSError as error:
        raise HostedReportError(f"cannot re-inspect {label}: {path}") from error
    require(
        after.st_size == admitted.st_size
        and _same_identity(admitted, after)
        and _same_identity(admitted, named),
        f"{label} changed while it was retained",
    )


@dataclass
class RetainedFile:
    """One regular file held and repeatedly verified through the same descriptor."""

    path: Path
    label: str
    descriptor: int
    admitted: os.stat_result
    sha256: str
    size_bytes: int
    maximum_size: int
    writable: bool = False
    ancestor_directories: tuple[tuple[Path, int, os.stat_result], ...] = ()

    @classmethod
    def open(
        cls,
        path: Path,
        label: str,
        *,
        maximum_size: int,
        expected_size: int | None = None,
        writable: bool = False,
    ) -> RetainedFile:
        path = Path(os.path.abspath(path))
        ancestor_directories: tuple[tuple[Path, int, os.stat_result], ...] = ()
        if os.name == "nt":
            ancestor_directories = tuple(
                _retained_directory_chain(path.parent, f"{label} parent")
            )
        try:
            if writable:
                descriptor, admitted = _open_exact(path, label, writable=True)
            else:
                descriptor, admitted = _open_exact(path, label)
            try:
                require(
                    not writable or admitted.st_nlink == 1,
                    f"{label} retained for scrub must not have hard links",
                )
                require(
                    0 < admitted.st_size <= maximum_size
                    and (expected_size is None or admitted.st_size == expected_size),
                    f"{label} has an invalid or unexpected byte length",
                )
                observations = [
                    _digest_pass(descriptor, maximum_size, label),
                    _digest_pass(descriptor, maximum_size, label),
                ]
                require(
                    observations[0] == observations[1]
                    and observations[0][1] == admitted.st_size,
                    f"{label} changed while it was read",
                )
                _verify_named_identity(path, label, descriptor, admitted)
            except BaseException:
                os.close(descriptor)
                raise
        except BaseException:
            for _path, directory_descriptor, _admitted in reversed(
                ancestor_directories
            ):
                os.close(directory_descriptor)
            raise
        return cls(
            path=path,
            label=label,
            descriptor=descriptor,
            admitted=admitted,
            sha256="sha256:" + observations[0][0].hex(),
            size_bytes=observations[0][1],
            maximum_size=maximum_size,
            writable=writable,
            ancestor_directories=ancestor_directories,
        )

    def verify(self) -> None:
        for (
            directory_path,
            directory_descriptor,
            directory_admitted,
        ) in self.ancestor_directories:
            after = os.fstat(directory_descriptor)
            try:
                named = directory_path.lstat()
            except OSError as error:
                raise HostedReportError(
                    f"{self.label} parent changed while retained"
                ) from error
            require(
                _safe_directory_metadata(after)
                and _safe_directory_metadata(named)
                and _same_identity(directory_admitted, after)
                and _same_identity(directory_admitted, named),
                f"{self.label} parent changed while retained",
            )
        observations = [
            _digest_pass(self.descriptor, self.maximum_size, self.label),
            _digest_pass(self.descriptor, self.maximum_size, self.label),
        ]
        require(
            observations[0] == observations[1]
            and observations[0][1] == self.size_bytes
            and "sha256:" + observations[0][0].hex() == self.sha256,
            f"{self.label} changed while it was retained",
        )
        _verify_named_identity(self.path, self.label, self.descriptor, self.admitted)

    def execution_path(self) -> str:
        if sys.platform.startswith("linux"):
            return f"/proc/self/fd/{self.descriptor}"
        return os.fspath(self.path)

    def scrub_exact(self) -> None:
        """Zero this retained file through its admitted descriptor."""

        if not self.writable and os.name == "nt":
            self.verify()
            original = self.admitted
            descriptor, self.descriptor = self.descriptor, -1
            os.close(descriptor)
            reopened, admitted = _open_exact(self.path, self.label, writable=True)
            try:
                require(
                    original.st_nlink == 1
                    and admitted.st_nlink == 1
                    and _same_identity(original, admitted),
                    f"{self.label} changed before exact scrub",
                )
                self.descriptor = reopened
                self.admitted = admitted
                self.writable = True
                self.verify()
            except BaseException:
                if self.descriptor < 0:
                    os.close(reopened)
                else:
                    self.close()
                raise
        require(self.writable, f"{self.label} was not retained for exact scrub")
        self.verify()
        os.ftruncate(self.descriptor, 0)
        os.fsync(self.descriptor)
        after = os.fstat(self.descriptor)
        try:
            named = self.path.lstat()
        except OSError as error:
            raise HostedReportError(
                f"cannot re-inspect scrubbed {self.label}: {self.path}"
            ) from error
        require(
            stat.S_ISREG(after.st_mode)
            and after.st_size == 0
            and _same_identity(self.admitted, after)
            and _same_identity(self.admitted, named),
            f"{self.label} changed during exact scrub",
        )
        self.admitted = after
        self.sha256 = "sha256:" + hashlib.sha256(b"").hexdigest()
        self.size_bytes = 0

    def close(self) -> None:
        descriptor, self.descriptor = self.descriptor, -1
        if descriptor >= 0:
            os.close(descriptor)
        for _path, directory_descriptor, _admitted in reversed(
            self.ancestor_directories
        ):
            os.close(directory_descriptor)
        self.ancestor_directories = ()

    @classmethod
    def create_in(
        cls,
        directory: RetainedDirectory,
        name: str,
        label: str,
        value: bytes,
        *,
        maximum_size: int,
    ) -> RetainedFile:
        require(
            0 < len(value) <= maximum_size and "/" not in name and "\\" not in name,
            f"{label} has an invalid byte length or name",
        )
        path = directory / name
        try:
            if os.name == "nt":
                import ctypes
                import msvcrt
                from ctypes import wintypes

                create_file = ctypes.windll.kernel32.CreateFileW
                create_file.argtypes = (
                    wintypes.LPCWSTR,
                    wintypes.DWORD,
                    wintypes.DWORD,
                    wintypes.LPVOID,
                    wintypes.DWORD,
                    wintypes.DWORD,
                    wintypes.HANDLE,
                )
                create_file.restype = wintypes.HANDLE
                handle = create_file(
                    os.fspath(path),
                    0xC0000000,  # GENERIC_READ | GENERIC_WRITE
                    0x00000001,  # FILE_SHARE_READ: deny write and delete sharing
                    None,
                    1,  # CREATE_NEW
                    0x00000080 | 0x00200000,  # NORMAL | OPEN_REPARSE_POINT
                    None,
                )
                invalid_handle = wintypes.HANDLE(-1).value
                if handle == invalid_handle:
                    raise OSError(ctypes.get_last_error(), "CreateFileW failed")
                try:
                    descriptor = msvcrt.open_osfhandle(
                        handle, os.O_RDWR | getattr(os, "O_BINARY", 0)
                    )
                except BaseException:
                    ctypes.windll.kernel32.CloseHandle(handle)
                    raise
            else:
                flags = os.O_RDWR | os.O_CREAT | os.O_EXCL
                flags |= getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
                descriptor = os.open(name, flags, 0o600, dir_fd=directory.descriptor)
        except OSError as error:
            raise HostedReportError(
                f"cannot exclusively create {label}: {path}"
            ) from error
        try:
            offset = 0
            while offset < len(value):
                written = os.write(descriptor, value[offset:])
                require(written > 0, f"{label} write made no progress")
                offset += written
            os.fsync(descriptor)
            admitted = os.fstat(descriptor)
            require(
                stat.S_ISREG(admitted.st_mode) and admitted.st_size == len(value),
                f"{label} did not retain the created regular file",
            )
            digest = "sha256:" + hashlib.sha256(value).hexdigest()
            _verify_named_identity(path, label, descriptor, admitted)
            directory.retain_child(name, descriptor=descriptor)
        except BaseException:
            try:
                os.ftruncate(descriptor, 0)
                os.fsync(descriptor)
            except OSError:
                pass
            os.close(descriptor)
            raise
        return cls(
            path=path,
            label=label,
            descriptor=descriptor,
            admitted=admitted,
            sha256=digest,
            size_bytes=len(value),
            maximum_size=maximum_size,
            writable=True,
        )


def _open_exact_directory(path: Path, label: str) -> tuple[int, os.stat_result]:
    try:
        admitted = path.lstat()
    except OSError as error:
        raise HostedReportError(f"cannot inspect {label}: {path}") from error
    require(
        _safe_directory_metadata(admitted), f"{label} must be a non-reparse directory"
    )
    try:
        if os.name == "nt":
            import ctypes
            import msvcrt
            from ctypes import wintypes

            create_file = ctypes.windll.kernel32.CreateFileW
            create_file.argtypes = (
                wintypes.LPCWSTR,
                wintypes.DWORD,
                wintypes.DWORD,
                wintypes.LPVOID,
                wintypes.DWORD,
                wintypes.DWORD,
                wintypes.HANDLE,
            )
            create_file.restype = wintypes.HANDLE
            handle = create_file(
                os.fspath(path),
                0x80000000,  # GENERIC_READ
                0x00000001 | 0x00000002,  # share read/write, deny delete/rename
                None,
                3,  # OPEN_EXISTING
                0x02000000 | 0x00200000,  # BACKUP_SEMANTICS | OPEN_REPARSE_POINT
                None,
            )
            invalid_handle = wintypes.HANDLE(-1).value
            if handle == invalid_handle:
                raise OSError(ctypes.get_last_error(), "CreateFileW failed")
            try:
                descriptor = msvcrt.open_osfhandle(
                    handle, os.O_RDONLY | getattr(os, "O_BINARY", 0)
                )
            except BaseException:
                ctypes.windll.kernel32.CloseHandle(handle)
                raise
        else:
            flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0)
            flags |= getattr(os, "O_NOFOLLOW", 0)
            descriptor = os.open(path, flags)
    except OSError as error:
        raise HostedReportError(f"cannot retain {label}: {path}") from error
    try:
        opened = os.fstat(descriptor)
        require(
            _safe_directory_metadata(opened) and _same_identity(admitted, opened),
            f"{label} changed during admission",
        )
    except BaseException:
        os.close(descriptor)
        raise
    return descriptor, opened


def _retained_directory_chain(
    path: Path, label: str
) -> list[tuple[Path, int, os.stat_result]]:
    """Retain every Windows path component that absolute child APIs traverse."""

    absolute = Path(os.path.abspath(path))
    if os.name == "nt":
        require(
            bool(absolute.anchor) and not absolute.anchor.startswith("\\\\"),
            f"{label} must use a local Windows volume",
        )
        current = Path(absolute.anchor)
        directory_paths = [current]
        for component in absolute.relative_to(current).parts:
            current /= component
            directory_paths.append(current)
    else:
        directory_paths = [absolute]

    retained: list[tuple[Path, int, os.stat_result]] = []
    try:
        for index, directory_path in enumerate(directory_paths):
            descriptor, admitted = _open_exact_directory(
                directory_path, f"{label} component {index}"
            )
            retained.append((directory_path, descriptor, admitted))
    except BaseException:
        for _path, descriptor, _admitted in reversed(retained):
            os.close(descriptor)
        raise
    return retained


def _open_exact_child_directory(
    parent: Path,
    parent_descriptor: int,
    name: str,
    label: str,
) -> tuple[int, os.stat_result]:
    require(
        name not in {"", ".", ".."} and "/" not in name and "\\" not in name,
        f"{label} name is invalid",
    )
    if os.name == "nt":
        return _open_exact_directory(parent / name, label)
    try:
        admitted = os.stat(name, dir_fd=parent_descriptor, follow_symlinks=False)
        require(_safe_directory_metadata(admitted), f"{label} must be a directory")
        flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0)
        flags |= getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(name, flags, dir_fd=parent_descriptor)
    except OSError as error:
        raise HostedReportError(f"cannot retain {label}: {parent / name}") from error
    try:
        opened = os.fstat(descriptor)
        require(
            _safe_directory_metadata(opened) and _same_identity(admitted, opened),
            f"{label} changed during admission",
        )
    except BaseException:
        os.close(descriptor)
        raise
    return descriptor, opened


@dataclass
class RetainedDirectory:
    """A private work root and its parent held by exact directory handles."""

    path: Path
    name: str
    descriptor: int
    admitted: os.stat_result
    parent_path: Path
    parent_descriptor: int
    parent_admitted: os.stat_result
    ancestor_directories: tuple[tuple[Path, int, os.stat_result], ...] = ()
    retained_children: dict[str, tuple[int, os.stat_result, dict[str, str]]] = field(
        default_factory=dict
    )

    def __truediv__(self, name: str) -> Path:
        require("/" not in name and "\\" not in name, "private child name is invalid")
        return Path(self.execution_path()) / name

    def named_child(self, name: str) -> Path:
        require("/" not in name and "\\" not in name, "private child name is invalid")
        return self.path / name

    def current_named_path(self) -> Path:
        if sys.platform.startswith("linux"):
            try:
                current = Path(os.readlink(f"/proc/self/fd/{self.descriptor}"))
            except OSError as error:
                raise HostedReportError(
                    "cannot resolve retained private root"
                ) from error
            require(
                current.is_absolute(), "retained private root resolved non-absolutely"
            )
            return current
        return self.path

    def execution_path(self) -> str:
        if sys.platform.startswith("linux"):
            return f"/proc/self/fd/{self.descriptor}"
        return os.fspath(self.path)

    def verify(self) -> None:
        for (
            ancestor_path,
            ancestor_descriptor,
            ancestor_admitted,
        ) in self.ancestor_directories:
            ancestor_after = os.fstat(ancestor_descriptor)
            try:
                ancestor_named = ancestor_path.lstat()
            except OSError as error:
                raise HostedReportError(
                    "private work root ancestor changed while retained"
                ) from error
            require(
                _safe_directory_metadata(ancestor_after)
                and _safe_directory_metadata(ancestor_named)
                and _same_identity(ancestor_admitted, ancestor_after)
                and _same_identity(ancestor_admitted, ancestor_named),
                "private work root ancestor changed while retained",
            )
        parent_after = os.fstat(self.parent_descriptor)
        root_after = os.fstat(self.descriptor)
        try:
            parent_named = self.parent_path.lstat()
            if os.name == "nt":
                root_named = self.path.lstat()
            else:
                root_named = os.stat(
                    self.name,
                    dir_fd=self.parent_descriptor,
                    follow_symlinks=False,
                )
        except OSError as error:
            raise HostedReportError(
                "private work root changed while retained"
            ) from error
        require(
            _same_identity(self.parent_admitted, parent_after)
            and _same_identity(self.parent_admitted, parent_named)
            and _same_identity(self.admitted, root_after)
            and _same_identity(self.admitted, root_named),
            "private work root changed while retained",
        )

    def list_names(self) -> list[str]:
        self.verify()
        if os.name == "nt":
            return sorted(os.listdir(self.path))
        return sorted(os.listdir(self.descriptor))

    def retain_child(
        self,
        name: str,
        *,
        expected_identity: dict[str, str] | None = None,
        descriptor: int | None = None,
        writable: bool = True,
        share_write: bool = False,
    ) -> None:
        """Retain one expected child before any later destructive cleanup."""

        require(
            name not in self.retained_children
            and name not in {"", ".", ".."}
            and "/" not in name
            and "\\" not in name,
            "private artifact retention name is invalid or duplicated",
        )
        self.verify()
        retained = -1
        try:
            if descriptor is not None:
                retained = os.dup(descriptor)
                admitted = os.fstat(retained)
            elif os.name == "nt":
                retained, admitted = _open_exact(
                    self.path / name,
                    "committed private artifact",
                    writable=writable,
                    share_write=share_write,
                )
            else:
                admitted = os.stat(
                    name,
                    dir_fd=self.descriptor,
                    follow_symlinks=False,
                )
                flags = (os.O_RDWR if writable else os.O_RDONLY) | getattr(
                    os, "O_NOFOLLOW", 0
                )
                retained = os.open(name, flags, dir_fd=self.descriptor)
                opened = os.fstat(retained)
                require(
                    _same_identity(admitted, opened),
                    "private artifact changed during retained admission",
                )
                admitted = opened
            if os.name == "nt":
                named = (self.path / name).lstat()
            else:
                named = os.stat(
                    name,
                    dir_fd=self.descriptor,
                    follow_symlinks=False,
                )
            identity = _canonical_file_identity(retained)
            require(
                stat.S_ISREG(admitted.st_mode)
                and admitted.st_nlink == 1
                and _same_identity(admitted, named)
                and (
                    expected_identity is None
                    or (
                        _canonical_identity_is_valid(expected_identity)
                        and identity == expected_identity
                    )
                ),
                "private artifact differs from its committed retained identity",
            )
            self.retained_children[name] = (retained, admitted, identity)
            retained = -1
        finally:
            if retained >= 0:
                os.close(retained)

    def upgrade_retained_child_for_scrub(self, name: str) -> None:
        """Replace a shared-read admission with an exact writable handle."""

        require(name in self.retained_children, "private artifact was not retained")
        descriptor, admitted, identity = self.retained_children[name]
        self.verify()
        replacement = -1
        try:
            if os.name == "nt":
                replacement, opened = _open_exact(
                    self.path / name,
                    "recovered private artifact",
                    writable=True,
                )
            else:
                flags = os.O_RDWR | getattr(os, "O_NOFOLLOW", 0)
                replacement = os.open(name, flags, dir_fd=self.descriptor)
                opened = os.fstat(replacement)
            require(
                stat.S_ISREG(opened.st_mode)
                and opened.st_nlink == 1
                and _same_identity(admitted, opened)
                and _canonical_file_identity(replacement) == identity,
                "recovered private artifact changed before scrub retention",
            )
            self.retained_children[name] = (replacement, opened, identity)
            replacement = -1
            os.close(descriptor)
        finally:
            if replacement >= 0:
                os.close(replacement)

    def read_retained_child(self, name: str, *, maximum_size: int, label: str) -> bytes:
        require(name in self.retained_children, f"{label} was not retained")
        descriptor, admitted, identity = self.retained_children[name]
        first = _read_pass(descriptor, maximum_size, label)
        second = _read_pass(descriptor, maximum_size, label)
        after = os.fstat(descriptor)
        if os.name == "nt":
            named = (self.path / name).lstat()
        else:
            named = os.stat(
                name,
                dir_fd=self.descriptor,
                follow_symlinks=False,
            )
        require(
            first == second
            and len(first) == admitted.st_size
            and _same_identity(admitted, after)
            and _same_identity(admitted, named)
            and _canonical_file_identity(descriptor) == identity,
            f"{label} changed while retained",
        )
        return first

    def scrub_exact(self) -> int:
        """Zero only creation-bound children; unknown zero placeholders are untouched."""

        names = self.list_names()
        require(len(names) <= 16, "private work root contains unbounded artifacts")
        require(
            set(self.retained_children).issubset(names),
            "retained private artifact disappeared before scrub",
        )
        for name in names:
            require(
                name not in {".", ".."} and "/" not in name and "\\" not in name,
                "private work root contains an invalid entry",
            )
            if os.name == "nt":
                metadata = (self.path / name).lstat()
            else:
                metadata = os.stat(
                    name,
                    dir_fd=self.descriptor,
                    follow_symlinks=False,
                )
            require(
                stat.S_ISREG(metadata.st_mode)
                and metadata.st_nlink == 1
                and (name in self.retained_children or metadata.st_size == 0),
                "private work root contains an unknown or unsafe artifact",
            )
        # Complete every identity/link-count preflight before mutating even one
        # retained artifact. A substituted child therefore preserves the whole
        # recovery set and cannot become a cleanup victim.
        for name, (descriptor, admitted, identity) in self.retained_children.items():
            opened = os.fstat(descriptor)
            if os.name == "nt":
                named = (self.path / name).lstat()
            else:
                named = os.stat(
                    name,
                    dir_fd=self.descriptor,
                    follow_symlinks=False,
                )
            require(
                stat.S_ISREG(opened.st_mode)
                and opened.st_nlink == 1
                and _same_identity(admitted, opened)
                and _same_identity(admitted, named)
                and _canonical_file_identity(descriptor) == identity,
                "retained private artifact changed before exact scrub",
            )
        for descriptor, admitted, identity in self.retained_children.values():
            before = os.fstat(descriptor)
            require(
                before.st_nlink == 1
                and _same_identity(admitted, before)
                and _canonical_file_identity(descriptor) == identity,
                "retained private artifact changed at exact scrub",
            )
            os.ftruncate(descriptor, 0)
            os.fsync(descriptor)
            after = os.fstat(descriptor)
            require(
                after.st_size == 0
                and after.st_nlink == 1
                and _same_identity(admitted, after)
                and _canonical_file_identity(descriptor) == identity,
                "private artifact exact scrub failed",
            )
        if os.name != "nt":
            os.fsync(self.descriptor)
        require(self.list_names() == names, "private artifact set changed during scrub")
        for name in names:
            if os.name == "nt":
                after = (self.path / name).lstat()
            else:
                after = os.stat(
                    name,
                    dir_fd=self.descriptor,
                    follow_symlinks=False,
                )
            require(after.st_size == 0, "private artifact changed after scrub")
        self.verify()
        return len(names)

    def close(self) -> None:
        for descriptor, _admitted, _identity in self.retained_children.values():
            os.close(descriptor)
        self.retained_children.clear()
        for attribute in ("descriptor", "parent_descriptor"):
            descriptor = getattr(self, attribute)
            setattr(self, attribute, -1)
            if descriptor >= 0:
                os.close(descriptor)
        for _path, descriptor, _admitted in reversed(self.ancestor_directories):
            os.close(descriptor)
        self.ancestor_directories = ()


@dataclass(frozen=True)
class CommandEvidence:
    operation: str
    exit_code: int
    stdout_sha256: str
    stdout_size_bytes: int
    stderr_sha256: str
    stderr_size_bytes: int

    def as_dict(self) -> dict[str, Any]:
        return {
            "operation": self.operation,
            "status": "pass" if self.exit_code == 0 else "failed",
            "exit_code": self.exit_code,
            "stdout_sha256": self.stdout_sha256,
            "stdout_size_bytes": self.stdout_size_bytes,
            "stderr_sha256": self.stderr_sha256,
            "stderr_size_bytes": self.stderr_size_bytes,
            "environment": "sanitized_no_ambient_pg",
        }


def _sanitized_environment(
    *, passfile: str | None = None, tls_mode: str | None = None
) -> dict[str, str]:
    environment = {"LANG": "C", "LC_ALL": "C", "TZ": "UTC"}
    if os.name == "nt":
        for name in ("SYSTEMROOT", "WINDIR"):
            value = os.environ.get(name)
            if value:
                environment[name] = value
    if passfile is not None:
        environment["PGPASSFILE"] = passfile
        environment["PGCONNECT_TIMEOUT"] = "10"
    if tls_mode is not None:
        require(tls_mode in {"disable", "require"}, "provider TLS mode is invalid")
        environment["PGSSLMODE"] = "disable" if tls_mode == "disable" else "verify-full"
        environment["PGGSSENCMODE"] = "disable"
        if tls_mode == "require":
            environment["PGSSLROOTCERT"] = "system"
    allowed_pg = {
        "PGPASSFILE",
        "PGCONNECT_TIMEOUT",
        "PGSSLMODE",
        "PGGSSENCMODE",
        "PGSSLROOTCERT",
    }
    require(
        all(
            not key.upper().startswith("PG") or key in allowed_pg for key in environment
        ),
        "ambient PostgreSQL environment reached a provider command",
    )
    return environment


class _WindowsJob:
    """One kill-on-close Windows Job assigned before the child can execute."""

    def __init__(self) -> None:
        import ctypes
        from ctypes import wintypes

        class BasicLimitInformation(ctypes.Structure):
            _fields_ = [
                ("PerProcessUserTimeLimit", ctypes.c_longlong),
                ("PerJobUserTimeLimit", ctypes.c_longlong),
                ("LimitFlags", wintypes.DWORD),
                ("MinimumWorkingSetSize", ctypes.c_size_t),
                ("MaximumWorkingSetSize", ctypes.c_size_t),
                ("ActiveProcessLimit", wintypes.DWORD),
                ("Affinity", ctypes.c_size_t),
                ("PriorityClass", wintypes.DWORD),
                ("SchedulingClass", wintypes.DWORD),
            ]

        class IoCounters(ctypes.Structure):
            _fields_ = [
                ("ReadOperationCount", ctypes.c_ulonglong),
                ("WriteOperationCount", ctypes.c_ulonglong),
                ("OtherOperationCount", ctypes.c_ulonglong),
                ("ReadTransferCount", ctypes.c_ulonglong),
                ("WriteTransferCount", ctypes.c_ulonglong),
                ("OtherTransferCount", ctypes.c_ulonglong),
            ]

        class ExtendedLimitInformation(ctypes.Structure):
            _fields_ = [
                ("BasicLimitInformation", BasicLimitInformation),
                ("IoInfo", IoCounters),
                ("ProcessMemoryLimit", ctypes.c_size_t),
                ("JobMemoryLimit", ctypes.c_size_t),
                ("PeakProcessMemoryUsed", ctypes.c_size_t),
                ("PeakJobMemoryUsed", ctypes.c_size_t),
            ]

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        create_job = kernel32.CreateJobObjectW
        create_job.argtypes = (wintypes.LPVOID, wintypes.LPCWSTR)
        create_job.restype = wintypes.HANDLE
        handle = create_job(None, None)
        if not handle:
            raise HostedReportError("cannot create the Windows command Job")
        information = ExtendedLimitInformation()
        information.BasicLimitInformation.LimitFlags = 0x00002000
        set_information = kernel32.SetInformationJobObject
        set_information.argtypes = (
            wintypes.HANDLE,
            ctypes.c_int,
            wintypes.LPVOID,
            wintypes.DWORD,
        )
        set_information.restype = wintypes.BOOL
        if not set_information(
            handle, 9, ctypes.byref(information), ctypes.sizeof(information)
        ):
            kernel32.CloseHandle(handle)
            raise HostedReportError("cannot configure the Windows command Job")
        self._ctypes = ctypes
        self._wintypes = wintypes
        self._kernel32 = kernel32
        self.handle = handle
        self.assigned = False

    def assign_and_resume(self, process: subprocess.Popen[bytes]) -> None:
        assign = self._kernel32.AssignProcessToJobObject
        assign.argtypes = (self._wintypes.HANDLE, self._wintypes.HANDLE)
        assign.restype = self._wintypes.BOOL
        if not assign(self.handle, int(process._handle)):  # type: ignore[attr-defined]
            raise HostedReportError("cannot assign the suspended command to its Job")
        self.assigned = True
        ntdll = self._ctypes.WinDLL("ntdll", use_last_error=True)
        resume = ntdll.NtResumeProcess
        resume.argtypes = (self._wintypes.HANDLE,)
        resume.restype = self._ctypes.c_long
        require(
            resume(int(process._handle)) == 0,  # type: ignore[attr-defined]
            "cannot resume the Job-contained command",
        )

    def terminate(self) -> None:
        if not self.handle:
            return
        terminate = self._kernel32.TerminateJobObject
        terminate.argtypes = (self._wintypes.HANDLE, self._wintypes.UINT)
        terminate.restype = self._wintypes.BOOL
        if not terminate(self.handle, 137):
            raise HostedReportError("Windows command Job did not terminate")

    def wait_empty(self, timeout_seconds: float) -> None:
        class BasicAccountingInformation(self._ctypes.Structure):
            _fields_ = (
                ("TotalUserTime", self._ctypes.c_longlong),
                ("TotalKernelTime", self._ctypes.c_longlong),
                ("ThisPeriodTotalUserTime", self._ctypes.c_longlong),
                ("ThisPeriodTotalKernelTime", self._ctypes.c_longlong),
                ("TotalPageFaultCount", self._wintypes.DWORD),
                ("TotalProcesses", self._wintypes.DWORD),
                ("ActiveProcesses", self._wintypes.DWORD),
                ("TotalTerminatedProcesses", self._wintypes.DWORD),
            )

        query = self._kernel32.QueryInformationJobObject
        query.argtypes = (
            self._wintypes.HANDLE,
            self._ctypes.c_int,
            self._wintypes.LPVOID,
            self._wintypes.DWORD,
            self._wintypes.LPVOID,
        )
        query.restype = self._wintypes.BOOL
        deadline = time.monotonic() + timeout_seconds
        while True:
            information = BasicAccountingInformation()
            require(
                bool(
                    query(
                        self.handle,
                        1,
                        self._ctypes.byref(information),
                        self._ctypes.sizeof(information),
                        None,
                    )
                ),
                "cannot query Windows command Job containment",
            )
            if information.ActiveProcesses == 0:
                return
            require(
                time.monotonic() < deadline,
                "Windows command Job did not quiesce within its bound",
            )
            time.sleep(0.01)

    def close(self) -> None:
        handle, self.handle = self.handle, None
        if handle:
            self._kernel32.CloseHandle(handle)


def _leader_exited_without_reap(process: subprocess.Popen[bytes]) -> bool:
    if os.name == "nt":
        return process.poll() is not None
    observed = os.waitid(
        os.P_PID,
        process.pid,
        os.WEXITED | os.WNOHANG | os.WNOWAIT,
    )
    return observed is not None


def _terminate_process_tree(
    process: subprocess.Popen[bytes], job: _WindowsJob | None
) -> None:
    termination_error: ContainmentUncertainError | None = None
    try:
        if os.name == "nt":
            require(job is not None, "Windows command has no Job authority")
            job.terminate()
        else:
            os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    except HostedReportError as error:
        termination_error = ContainmentUncertainError(
            "hosted command tree could not be terminated"
        )
        termination_error.__cause__ = error
    except OSError as error:
        if error.errno == errno.ESRCH:
            pass
        elif not (sys.platform == "darwin" and error.errno == errno.EPERM):
            termination_error = ContainmentUncertainError(
                "hosted command tree could not be terminated"
            )
            termination_error.__cause__ = error
    try:
        process.wait(timeout=5)
    except BaseException as error:
        raise ContainmentUncertainError(
            "hosted command tree did not terminate within its bound"
        ) from error
    if termination_error is not None:
        raise termination_error
    if job is not None:
        try:
            job.wait_empty(5)
        except HostedReportError as error:
            raise ContainmentUncertainError(
                "Windows command Job absence could not be proven"
            ) from error
        return
    # Never signal after reaping the leader: the numeric PGID could be reused.
    # Observation-only killpg(0) reaches ESRCH only after every original group
    # member, including zombies, has disappeared.
    deadline = time.monotonic() + 5
    while True:
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            return
        except OSError as error:
            if error.errno == errno.ESRCH:
                return
            if sys.platform == "darwin" and error.errno == errno.EPERM:
                # macOS reports EPERM for a dead process group containing only
                # unreaped or adopted zombies. Hosted execution is Linux/Windows;
                # this branch keeps local macOS boundary tests deterministic.
                return
            raise ContainmentUncertainError(
                "cannot prove hosted command group absence"
            ) from error
        if time.monotonic() >= deadline:
            raise ContainmentUncertainError(
                "hosted command group did not quiesce within its bound"
            )
        time.sleep(0.01)


def _validate_command_output(
    output: bytes, label: str, secrets: tuple[bytes, ...]
) -> tuple[str, int]:
    require(
        len(output) <= MAX_COMMAND_OUTPUT_BYTES, f"{label} exceeded its bounded output"
    )
    lowered = output.lower()
    require(
        b"password=" not in lowered
        and b"postgres://" not in lowered
        and b"postgresql://" not in lowered
        and all(not secret or secret not in output for secret in secrets),
        f"{label} emitted credential material",
    )
    return "sha256:" + hashlib.sha256(output).hexdigest(), len(output)


def _drain_bounded_stream(
    stream: Any,
    result: dict[str, Any],
    key: str,
    overflow: threading.Event,
) -> None:
    captured = bytearray()
    error: OSError | None = None
    try:
        while True:
            chunk = stream.read(64 * 1024)
            if not chunk:
                break
            remaining = MAX_COMMAND_OUTPUT_BYTES + 1 - len(captured)
            if remaining > 0:
                captured.extend(chunk[:remaining])
            if len(captured) > MAX_COMMAND_OUTPUT_BYTES:
                overflow.set()
    except OSError as observed:
        error = observed
    finally:
        try:
            stream.close()
        except OSError:
            pass
    result[key] = bytes(captured)
    result[f"{key}_error"] = error


def _write_bounded_input(stream: Any, value: bytes, result: dict[str, Any]) -> None:
    error: BaseException | None = None
    try:
        offset = 0
        while offset < len(value):
            written = stream.write(value[offset : offset + 64 * 1024])
            if written is None:
                written = 0
            if written <= 0:
                error = HostedReportError("hosted command input made no progress")
                break
            offset += written
        if error is None:
            stream.flush()
    except OSError as observed:
        error = observed
    finally:
        try:
            stream.close()
        except OSError:
            pass
    result["stdin_error"] = error


def run_exact(
    executable: RetainedFile,
    operation: str,
    argv: list[str],
    *,
    inherited: tuple[RetainedFile, ...],
    environment: dict[str, str],
    directory: Path,
    directory_authority: RetainedDirectory | None = None,
    timeout_seconds: int,
    secrets: tuple[bytes, ...],
    stdin_bytes: bytes | None = None,
) -> tuple[CommandEvidence, bytes, bytes]:
    require(
        stdin_bytes is None or len(stdin_bytes) <= MAX_COMMAND_INPUT_BYTES,
        f"{operation} input exceeds its byte bound",
    )
    executable.verify()
    for retained in inherited:
        retained.verify()
    process: subprocess.Popen[bytes] | None = None
    job: _WindowsJob | None = None
    tree_terminated = False
    threads: list[threading.Thread] = []
    captured: dict[str, Any] = {}
    overflow = threading.Event()
    try:
        pass_descriptors = tuple(
            retained.descriptor for retained in (executable, *inherited)
        )
        if directory_authority is not None:
            directory_authority.verify()
            pass_descriptors += (
                directory_authority.descriptor,
                directory_authority.parent_descriptor,
            )
        kwargs: dict[str, Any] = {
            "stdin": subprocess.PIPE if stdin_bytes is not None else subprocess.DEVNULL,
            "stdout": subprocess.PIPE,
            "stderr": subprocess.PIPE,
            "cwd": (
                directory_authority.execution_path()
                if directory_authority is not None
                else directory
            ),
            "env": environment,
        }
        if os.name == "nt":
            job = _WindowsJob()
            kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP | 0x00000004
        else:
            kwargs["pass_fds"] = pass_descriptors
            kwargs["start_new_session"] = True
        process = subprocess.Popen(
            [executable.execution_path(), *argv],
            executable=executable.execution_path(),
            **kwargs,
        )
        if job is not None:
            try:
                job.assign_and_resume(process)
            except BaseException:
                tree_terminated = True
                if job.assigned:
                    _terminate_process_tree(process, job)
                else:
                    try:
                        process.kill()
                        process.wait(timeout=5)
                    except BaseException as error:
                        raise ContainmentUncertainError(
                            "unassigned suspended Windows command could not be contained"
                        ) from error
                raise
        require(
            process.stdout is not None and process.stderr is not None,
            f"{operation} output pipes were not created",
        )
        for key, stream in (("stdout", process.stdout), ("stderr", process.stderr)):
            thread = threading.Thread(
                target=_drain_bounded_stream,
                args=(stream, captured, key, overflow),
                daemon=True,
            )
            thread.start()
            threads.append(thread)
        if stdin_bytes is not None:
            require(
                process.stdin is not None, f"{operation} input pipe was not created"
            )
            input_thread = threading.Thread(
                target=_write_bounded_input,
                args=(process.stdin, stdin_bytes, captured),
                daemon=True,
            )
            input_thread.start()
            threads.append(input_thread)
        deadline = time.monotonic() + timeout_seconds
        while not _leader_exited_without_reap(process) and not overflow.is_set():
            if time.monotonic() >= deadline:
                tree_terminated = True
                _terminate_process_tree(process, job)
                raise HostedReportError(f"{operation} exceeded its absolute deadline")
            time.sleep(0.01)
        if overflow.is_set():
            tree_terminated = True
            _terminate_process_tree(process, job)
            raise HostedReportError(f"{operation} exceeded its bounded output")
        # Even a successful leader may leave descendants holding inherited
        # descriptors, credentials, or provider authority. Kill the still-pinned
        # process group/Job before accepting output or starting cleanup.
        tree_terminated = True
        _terminate_process_tree(process, job)
        for thread in threads:
            thread.join(timeout=5)
        if any(thread.is_alive() for thread in threads):
            raise ContainmentUncertainError(
                f"{operation} output readers did not quiesce"
            )
        require(
            captured.get("stdout_error") is None
            and captured.get("stderr_error") is None,
            f"{operation} output capture failed",
        )
        require(
            captured.get("stdin_error") is None, f"{operation} input delivery failed"
        )
        stdout = captured.get("stdout", b"")
        stderr = captured.get("stderr", b"")
        require(
            isinstance(stdout, bytes) and isinstance(stderr, bytes),
            f"{operation} output capture is incomplete",
        )
        stdout_sha256, stdout_size = _validate_command_output(
            stdout, f"{operation} stdout", secrets
        )
        stderr_sha256, stderr_size = _validate_command_output(
            stderr, f"{operation} stderr", secrets
        )
        executable.verify()
        for retained in inherited:
            retained.verify()
        if directory_authority is not None:
            directory_authority.verify()
        evidence = CommandEvidence(
            operation=operation,
            exit_code=process.returncode,
            stdout_sha256=stdout_sha256,
            stdout_size_bytes=stdout_size,
            stderr_sha256=stderr_sha256,
            stderr_size_bytes=stderr_size,
        )
        return evidence, stdout, stderr
    finally:
        if process is not None and not tree_terminated:
            _terminate_process_tree(process, job)
        for thread in threads:
            thread.join(timeout=1)
        if job is not None:
            job.close()


def fingerprint(
    path: Path, label: str, *, expected_size: int, maximum_size: int
) -> tuple[str, int]:
    descriptor, admitted = _open_exact(path, label)
    try:
        require(
            0 < admitted.st_size == expected_size <= maximum_size,
            f"{label} has an invalid or unexpected byte length",
        )
        observations: list[tuple[bytes, int]] = []
        for _ in range(2):
            os.lseek(descriptor, 0, os.SEEK_SET)
            digest = hashlib.sha256()
            observed = 0
            while True:
                chunk = os.read(descriptor, 1024 * 1024)
                if not chunk:
                    break
                observed += len(chunk)
                require(
                    observed <= admitted.st_size,
                    f"{label} changed while it was read",
                )
                digest.update(chunk)
            after_pass = os.fstat(descriptor)
            require(
                observed == admitted.st_size
                and after_pass.st_size == admitted.st_size
                and _same_identity(admitted, after_pass),
                f"{label} changed while it was read",
            )
            observations.append((digest.digest(), observed))
        after = os.fstat(descriptor)
        named = path.lstat()
        require(
            observations[0] == observations[1]
            and after.st_size == admitted.st_size
            and _same_identity(admitted, after)
            and _same_identity(admitted, named),
            f"{label} changed while it was read",
        )
    except OSError as error:
        raise HostedReportError(f"cannot read {label}: {path}") from error
    finally:
        os.close(descriptor)
    return "sha256:" + observations[0][0].hex(), observations[0][1]


def read_json(path: Path, label: str) -> tuple[dict[str, Any], str, int]:
    descriptor, admitted = _open_exact(path, label)
    try:
        require(
            0 < admitted.st_size <= MAX_JSON_BYTES,
            f"{label} exceeds its byte bound",
        )
        observations: list[bytes] = []
        for _ in range(2):
            os.lseek(descriptor, 0, os.SEEK_SET)
            chunks: list[bytes] = []
            observed = 0
            while True:
                chunk = os.read(
                    descriptor, min(1024 * 1024, MAX_JSON_BYTES + 1 - observed)
                )
                if not chunk:
                    break
                chunks.append(chunk)
                observed += len(chunk)
                require(observed <= MAX_JSON_BYTES, f"{label} exceeds its byte bound")
            after_pass = os.fstat(descriptor)
            require(
                observed == admitted.st_size
                and after_pass.st_size == admitted.st_size
                and _same_identity(admitted, after_pass),
                f"{label} changed while it was read",
            )
            observations.append(b"".join(chunks))
        after = os.fstat(descriptor)
        named = path.lstat()
        require(
            observations[0] == observations[1]
            and after.st_size == admitted.st_size
            and _same_identity(admitted, after)
            and _same_identity(admitted, named),
            f"{label} changed while it was read",
        )
        value = json.loads(observations[0])
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise HostedReportError(f"cannot decode {label}: {path}") from error
    finally:
        os.close(descriptor)
    require(isinstance(value, dict), f"{label} must be a JSON object")
    return (
        value,
        "sha256:" + hashlib.sha256(observations[0]).hexdigest(),
        len(observations[0]),
    )


def read_stable_bytes(path: Path, label: str, maximum_size: int) -> bytes:
    retained = RetainedFile.open(path, label, maximum_size=maximum_size)
    try:
        first = _read_pass(retained.descriptor, maximum_size, label)
        second = _read_pass(retained.descriptor, maximum_size, label)
        require(first == second, f"{label} changed while it was read")
        retained.verify()
        return first
    finally:
        retained.close()


def write_exclusive(
    path: Path, value: dict[str, Any], *, secrets: tuple[bytes, ...] = ()
) -> None:
    encoded = (
        json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
        + "\n"
    ).encode()
    require(0 < len(encoded) <= MAX_JSON_BYTES, "hosted restore output is unbounded")
    _validate_command_output(encoded, "hosted restore output", secrets)
    output = Path(os.path.abspath(path))
    require(
        output.is_absolute()
        and output.name not in {"", ".", ".."}
        and "/" not in output.name
        and "\\" not in output.name,
        "hosted restore output name is invalid",
    )
    parent = output.parent
    retained_directories: list[tuple[Path, int, os.stat_result]] = []
    descriptor = -1
    try:
        retained_directories = _retained_directory_chain(
            parent, "hosted output directory"
        )
        parent_descriptor = retained_directories[-1][1]
        if os.name == "nt":
            import ctypes
            import msvcrt
            from ctypes import wintypes

            create_file = ctypes.windll.kernel32.CreateFileW
            create_file.argtypes = (
                wintypes.LPCWSTR,
                wintypes.DWORD,
                wintypes.DWORD,
                wintypes.LPVOID,
                wintypes.DWORD,
                wintypes.DWORD,
                wintypes.HANDLE,
            )
            create_file.restype = wintypes.HANDLE
            handle = create_file(
                os.fspath(output),
                0xC0000000,  # GENERIC_READ | GENERIC_WRITE
                0x00000001,  # share read; deny write/delete/rename
                None,
                1,  # CREATE_NEW
                0x00000080 | 0x00200000,  # NORMAL | OPEN_REPARSE_POINT
                None,
            )
            if handle == wintypes.HANDLE(-1).value:
                raise OSError(ctypes.get_last_error(), "CreateFileW failed")
            try:
                descriptor = msvcrt.open_osfhandle(
                    handle, os.O_RDWR | getattr(os, "O_BINARY", 0)
                )
            except BaseException:
                ctypes.windll.kernel32.CloseHandle(handle)
                raise
        else:
            flags = os.O_RDWR | os.O_CREAT | os.O_EXCL
            flags |= getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
            descriptor = os.open(output.name, flags, 0o600, dir_fd=parent_descriptor)
        created = os.fstat(descriptor)
        require(
            stat.S_ISREG(created.st_mode) and created.st_size == 0,
            "hosted restore output creation was not exclusive",
        )
        offset = 0
        while offset < len(encoded):
            written = os.write(descriptor, encoded[offset:])
            require(written > 0, "hosted report write made no progress")
            offset += written
        os.fsync(descriptor)
        observations = [
            _read_pass(descriptor, MAX_JSON_BYTES, "hosted restore output"),
            _read_pass(descriptor, MAX_JSON_BYTES, "hosted restore output"),
        ]
        after = os.fstat(descriptor)
        if os.name == "nt":
            named = output.lstat()
        else:
            named = os.stat(
                output.name,
                dir_fd=parent_descriptor,
                follow_symlinks=False,
            )
        require(
            observations == [encoded, encoded]
            and after.st_size == len(encoded)
            and _same_identity(created, after)
            and _same_identity(created, named),
            "hosted restore output changed during publication",
        )
        require(
            json.loads(observations[0]) == value,
            "hosted restore output changed after serialization",
        )
        if os.name != "nt":
            os.fsync(parent_descriptor)
        for (
            directory_path,
            directory_descriptor,
            directory_admitted,
        ) in retained_directories:
            directory_after = os.fstat(directory_descriptor)
            directory_named = directory_path.lstat()
            require(
                _safe_directory_metadata(directory_after)
                and _safe_directory_metadata(directory_named)
                and _same_identity(directory_admitted, directory_after)
                and _same_identity(directory_admitted, directory_named),
                "hosted restore output directory changed during publication",
            )
    except OSError as error:
        if descriptor >= 0:
            try:
                os.ftruncate(descriptor, 0)
                os.fsync(descriptor)
            except OSError:
                pass
        raise HostedReportError(
            f"cannot exclusively publish output: {output}"
        ) from error
    except BaseException:
        try:
            if descriptor >= 0:
                os.ftruncate(descriptor, 0)
                os.fsync(descriptor)
        except OSError:
            pass
        raise
    finally:
        if descriptor >= 0:
            os.close(descriptor)
        for _directory_path, directory_descriptor, _directory_admitted in reversed(
            retained_directories
        ):
            os.close(directory_descriptor)


def _validate_endpoint(host: str, port: int, database: str, username: str) -> None:
    require(host == "127.0.0.1", "hosted restore endpoints must use loopback IPv4")
    require(0 < port <= 65_535, "hosted restore endpoint port is invalid")
    require(
        SAFE_SQL_IDENTIFIER.fullmatch(database) is not None
        and SAFE_SQL_IDENTIFIER.fullmatch(username) is not None,
        "hosted restore database or username is not a safe SQL identifier",
    )


def _passfile_secrets(passfile: RetainedFile) -> tuple[bytes, ...]:
    require(
        passfile.size_bytes <= MAX_PASSFILE_BYTES,
        "operator passfile exceeds its byte bound",
    )
    first = _read_pass(passfile.descriptor, MAX_PASSFILE_BYTES, passfile.label)
    second = _read_pass(passfile.descriptor, MAX_PASSFILE_BYTES, passfile.label)
    require(first == second, "operator passfile changed while it was read")
    passfile.verify()
    secrets: list[bytes] = []
    entries = 0
    for line in first.splitlines():
        if not line or line.startswith(b"#"):
            continue
        entries += 1
        require(entries <= 16 and len(line) <= 4096, "operator passfile is unbounded")
        decoded_fields: list[bytes] = []
        encoded_fields: list[bytes] = []
        decoded = bytearray()
        encoded = bytearray()
        escaped = False
        for byte in line:
            if escaped:
                encoded.append(byte)
                decoded.append(byte)
                escaped = False
            elif byte == ord("\\"):
                encoded.append(byte)
                escaped = True
            elif byte == ord(":") and len(decoded_fields) < 4:
                decoded_fields.append(bytes(decoded))
                encoded_fields.append(bytes(encoded))
                decoded.clear()
                encoded.clear()
            else:
                encoded.append(byte)
                decoded.append(byte)
        require(not escaped, "operator passfile has a trailing escape")
        decoded_fields.append(bytes(decoded))
        encoded_fields.append(bytes(encoded))
        require(
            len(decoded_fields) == 5
            and all(decoded_fields)
            and len(encoded_fields) == 5,
            "operator passfile entry is malformed",
        )
        secrets.extend((decoded_fields[4], encoded_fields[4]))
    require(secrets, "operator passfile contains no bounded credential entries")
    return tuple(dict.fromkeys(secrets))


def _private_work_root(
    parent: Path,
    *,
    expected_identity: dict[str, str] | None = None,
    fixed_name: str | None = None,
) -> RetainedDirectory:
    parent = Path(os.path.abspath(parent))
    retained_chain = _retained_directory_chain(parent, "hosted work parent")
    parent_path, parent_descriptor, parent_admitted = retained_chain[-1]
    ancestor_directories = tuple(retained_chain[:-1])
    descriptor = -1
    try:
        require(
            expected_identity is None
            or (
                _canonical_identity_is_valid(expected_identity)
                and _canonical_file_identity(parent_descriptor) == expected_identity
            ),
            "hosted work parent differs from the wrapper-retained identity",
        )
        if os.name != "nt":
            require(
                parent_admitted.st_uid == os.geteuid()
                and stat.S_IMODE(parent_admitted.st_mode) & 0o077 == 0,
                "hosted work parent must be owner-only",
            )
        require(
            fixed_name is None
            or (
                fixed_name == PRIVATE_BINDING_ROOT_NAME
                and "/" not in fixed_name
                and "\\" not in fixed_name
            ),
            "fixed private work root name is invalid",
        )
        attempts = 1 if fixed_name is not None else 128
        for _ in range(attempts):
            name = fixed_name or (
                "worldstream-native-hosted-" + secrets_module.token_hex(16)
            )
            path = parent / name
            try:
                if os.name == "nt":
                    os.mkdir(path, 0o700)
                else:
                    os.mkdir(name, 0o700, dir_fd=parent_descriptor)
                break
            except FileExistsError:
                if fixed_name is not None:
                    raise HostedReportError(
                        "private platform binding root already exists"
                    )
                continue
        else:  # pragma: no cover - cryptographically implausible exhaustion
            raise HostedReportError("cannot allocate a unique private work root")
        try:
            descriptor, admitted = _open_exact_child_directory(
                parent_path,
                parent_descriptor,
                name,
                "private work root",
            )
        except BaseException:
            try:
                if os.name == "nt":
                    os.rmdir(path)
                else:
                    os.rmdir(name, dir_fd=parent_descriptor)
            except OSError:
                pass
            raise
        retained = RetainedDirectory(
            path=path,
            name=name,
            descriptor=descriptor,
            admitted=admitted,
            parent_path=parent_path,
            parent_descriptor=parent_descriptor,
            parent_admitted=parent_admitted,
            ancestor_directories=ancestor_directories,
        )
        retained.verify()
        return retained
    except BaseException:
        if descriptor >= 0:
            os.close(descriptor)
        os.close(parent_descriptor)
        for _path, ancestor_descriptor, _admitted in reversed(ancestor_directories):
            os.close(ancestor_descriptor)
        raise


def _stream_retained_child_copy(
    source_root: RetainedDirectory,
    source_name: str,
    destination_root: RetainedDirectory,
    destination_name: str,
    *,
    expected_size: int,
    maximum_size: int,
) -> RetainedFile:
    """Copy one exact retained child without materializing its bytes in memory."""

    require(
        source_name in source_root.retained_children
        and destination_name not in destination_root.retained_children
        and 0 < expected_size <= maximum_size,
        "private dump binding copy arguments are invalid",
    )
    source_root.verify()
    destination_root.verify()
    source_descriptor, source_admitted, source_identity = source_root.retained_children[
        source_name
    ]
    require(
        source_admitted.st_size == expected_size,
        "retained native dump has an unexpected byte length",
    )
    destination_path = destination_root / destination_name
    descriptor = -1
    try:
        if os.name == "nt":
            import ctypes
            import msvcrt
            from ctypes import wintypes

            create_file = ctypes.windll.kernel32.CreateFileW
            create_file.argtypes = (
                wintypes.LPCWSTR,
                wintypes.DWORD,
                wintypes.DWORD,
                wintypes.LPVOID,
                wintypes.DWORD,
                wintypes.DWORD,
                wintypes.HANDLE,
            )
            create_file.restype = wintypes.HANDLE
            handle = create_file(
                os.fspath(destination_path),
                0xC0000000,  # GENERIC_READ | GENERIC_WRITE
                0x00000001,  # share read; deny write/delete/rename
                None,
                1,  # CREATE_NEW
                0x00000080 | 0x00200000,  # NORMAL | OPEN_REPARSE_POINT
                None,
            )
            if handle == wintypes.HANDLE(-1).value:
                raise OSError(ctypes.get_last_error(), "CreateFileW failed")
            try:
                descriptor = msvcrt.open_osfhandle(
                    handle, os.O_RDWR | getattr(os, "O_BINARY", 0)
                )
            except BaseException:
                ctypes.windll.kernel32.CloseHandle(handle)
                raise
        else:
            flags = os.O_RDWR | os.O_CREAT | os.O_EXCL
            flags |= getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
            descriptor = os.open(
                destination_name,
                flags,
                0o600,
                dir_fd=destination_root.descriptor,
            )
        created = os.fstat(descriptor)
        require(
            stat.S_ISREG(created.st_mode)
            and created.st_nlink == 1
            and created.st_size == 0,
            "private dump binding was not exclusively created",
        )
        os.lseek(source_descriptor, 0, os.SEEK_SET)
        copied = 0
        digest = hashlib.sha256()
        while True:
            chunk = os.read(source_descriptor, 1024 * 1024)
            if not chunk:
                break
            copied += len(chunk)
            require(
                copied <= expected_size and copied <= maximum_size,
                "retained native dump grew while it was copied",
            )
            digest.update(chunk)
            offset = 0
            while offset < len(chunk):
                written = os.write(descriptor, chunk[offset:])
                require(written > 0, "private dump binding write made no progress")
                offset += written
        os.fsync(descriptor)
        source_second_digest, source_second_size = _digest_pass(
            source_descriptor, maximum_size, "retained native dump"
        )
        destination_digest, destination_size = _digest_pass(
            descriptor, maximum_size, "private dump binding"
        )
        source_after = os.fstat(source_descriptor)
        destination_after = os.fstat(descriptor)
        if os.name == "nt":
            source_named = (source_root.path / source_name).lstat()
        else:
            source_named = os.stat(
                source_name,
                dir_fd=source_root.descriptor,
                follow_symlinks=False,
            )
        require(
            copied == expected_size
            and source_second_size == expected_size
            and destination_size == expected_size
            and digest.digest() == source_second_digest == destination_digest
            and _same_identity(source_admitted, source_after)
            and _same_identity(source_admitted, source_named)
            and _canonical_file_identity(source_descriptor) == source_identity
            and _same_identity(created, destination_after),
            "retained native dump changed while its private binding was copied",
        )
        destination_root.retain_child(destination_name, descriptor=descriptor)
        retained = RetainedFile(
            path=destination_root.named_child(destination_name),
            label="private native dump binding",
            descriptor=descriptor,
            admitted=destination_after,
            sha256="sha256:" + destination_digest.hex(),
            size_bytes=destination_size,
            maximum_size=maximum_size,
            writable=True,
        )
        descriptor = -1
        return retained
    except OSError as error:
        raise HostedReportError("cannot create private native dump binding") from error
    finally:
        if descriptor >= 0:
            try:
                os.ftruncate(descriptor, 0)
                os.fsync(descriptor)
            except OSError:
                pass
            os.close(descriptor)


def _private_binding_file_record(retained: RetainedFile) -> dict[str, Any]:
    retained.verify()
    return {
        "name": retained.path.name,
        "identity": _canonical_file_identity(retained.descriptor),
        "sha256": retained.sha256,
        "size_bytes": retained.size_bytes,
    }


def _provider_tool_binding(retained: RetainedFile) -> dict[str, Any]:
    return {"sha256": retained.sha256, "size_bytes": retained.size_bytes}


def _command_success(
    evidence: CommandEvidence, stdout: bytes, stderr: bytes, label: str
) -> None:
    require(evidence.exit_code == 0, f"{label} failed closed")
    require(not stderr.strip(), f"{label} emitted unexpected stderr")
    require(stdout.strip(), f"{label} emitted no completion receipt")


def _json_receipt(stdout: bytes, label: str) -> dict[str, Any]:
    try:
        receipt = json.loads(stdout)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise HostedReportError(f"{label} completion receipt is not JSON") from error
    require(isinstance(receipt, dict), f"{label} completion receipt is not an object")
    return receipt


def _expected_directory_identity(root: RetainedDirectory) -> dict[str, str]:
    return _canonical_file_identity(root.descriptor)


def _verify_directory_receipt(stdout: bytes, root: RetainedDirectory) -> dict[str, Any]:
    receipt = _json_receipt(stdout, "artifact directory identity")
    identity = receipt.get("artifact_directory_identity")
    require(
        set(receipt)
        == {
            "schema",
            "status",
            "artifact_directory_identity",
            "secrets_emitted",
        }
        and receipt.get("schema") == DIRECTORY_RECEIPT_SCHEMA
        and receipt.get("status") == "observed"
        and _canonical_identity_is_valid(identity)
        and receipt.get("secrets_emitted") is False,
        "artifact directory identity receipt is incomplete",
    )
    require(
        identity == _expected_directory_identity(root),
        "packaged control observed a substituted artifact directory",
    )
    root.verify()
    return receipt


def _verify_snapshot_receipt(stdout: bytes) -> dict[str, Any]:
    receipt = _json_receipt(stdout, "snapshot rebuild")
    require(
        set(receipt)
        == {
            "schema",
            "status",
            "rebuilt_snapshot_count",
            "source_provider_identity",
            "secrets_emitted",
        }
        and receipt.get("schema") == SNAPSHOT_RECEIPT_SCHEMA
        and receipt.get("status") == "complete"
        and type(receipt.get("rebuilt_snapshot_count")) is int
        and receipt["rebuilt_snapshot_count"] > 0
        and _provider_identity_is_canonical(receipt.get("source_provider_identity"))
        and receipt.get("secrets_emitted") is False,
        "snapshot rebuild completion receipt is incomplete",
    )
    return receipt


def _provider_identity_is_canonical(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and set(value) == {"system_identifier", "database_oid", "database_name"}
        and isinstance(value.get("system_identifier"), str)
        and value["system_identifier"].isdigit()
        and int(value["system_identifier"]) > 0
        and isinstance(value.get("database_oid"), str)
        and value["database_oid"].isdigit()
        and int(value["database_oid"]) > 0
        and isinstance(value.get("database_name"), str)
        and SAFE_SQL_IDENTIFIER.fullmatch(value["database_name"]) is not None
    )


def _preflight_restore_receipt(stdout: bytes) -> dict[str, Any]:
    receipt = _json_receipt(stdout, "native restore")
    require(
        set(receipt)
        == {
            "schema",
            "status",
            "report_digest",
            "report_size_bytes",
            "native_dump_digest",
            "native_dump_size_bytes",
            "native_dump_identity",
            "report_identity",
            "recovery_record_identity",
            "recovery_record_name",
            "source_provider_identity",
            "target_provider_identity",
            "secrets_emitted",
        }
        and receipt.get("schema") == RESTORE_RECEIPT_SCHEMA
        and receipt.get("status") == "committed"
        and isinstance(receipt.get("report_digest"), str)
        and re.fullmatch(r"blake3:[0-9a-f]{64}", receipt["report_digest"]) is not None
        and type(receipt.get("report_size_bytes")) is int
        and receipt["report_size_bytes"] > 0
        and isinstance(receipt.get("native_dump_digest"), str)
        and re.fullmatch(r"[0-9a-f]{64}", receipt["native_dump_digest"]) is not None
        and type(receipt.get("native_dump_size_bytes")) is int
        and receipt["native_dump_size_bytes"] > 0
        and _canonical_identity_is_valid(receipt.get("native_dump_identity"))
        and _canonical_identity_is_valid(receipt.get("report_identity"))
        and _canonical_identity_is_valid(receipt.get("recovery_record_identity"))
        and isinstance(receipt.get("recovery_record_name"), str)
        and RECOVERY_JOURNAL.fullmatch(receipt["recovery_record_name"]) is not None
        and _provider_identity_is_canonical(receipt.get("source_provider_identity"))
        and _provider_identity_is_canonical(receipt.get("target_provider_identity"))
        and receipt.get("secrets_emitted") is False,
        "native restore completion receipt is incomplete",
    )
    return receipt


def _verify_restore_receipt(
    receipt: dict[str, Any], native: dict[str, Any], raw_report: bytes
) -> dict[str, Any]:
    require(
        receipt.get("report_digest") == "blake3:" + BLAKE3(raw_report).hex()
        and receipt.get("report_size_bytes") == len(raw_report)
        and receipt.get("native_dump_digest") == native.get("native_dump_digest")
        and receipt.get("native_dump_size_bytes")
        == native.get("native_dump_size_bytes")
        and receipt.get("source_provider_identity")
        == native.get("source_provider_identity")
        and receipt.get("target_provider_identity")
        == native.get("target_provider_identity")
        and receipt.get("secrets_emitted") is False,
        "native restore completion receipt does not bind the durable report",
    )
    return receipt


def _psql_argv(
    args: argparse.Namespace, sql: str, *, endpoint: str = "target"
) -> list[str]:
    require(endpoint in {"source", "target"}, "provider endpoint is invalid")
    return [
        "--no-password",
        "--no-psqlrc",
        "--host",
        getattr(args, f"{endpoint}_host"),
        "--port",
        str(getattr(args, f"{endpoint}_port")),
        "--username",
        getattr(args, f"{endpoint}_username"),
        "--dbname",
        "postgres",
        "--quiet",
        "--tuples-only",
        "--no-align",
        "--set",
        "ON_ERROR_STOP=1",
        "--command",
        sql,
    ]


def _run_psql(
    args: argparse.Namespace,
    psql: RetainedFile,
    passfile: RetainedFile,
    root: RetainedDirectory,
    operation: str,
    sql: str,
    secrets: tuple[bytes, ...],
    *,
    endpoint: str = "target",
) -> tuple[CommandEvidence, str]:
    evidence, stdout, stderr = run_exact(
        psql,
        operation,
        _psql_argv(args, sql, endpoint=endpoint),
        inherited=(passfile,),
        environment=_sanitized_environment(
            passfile=passfile.execution_path(),
            tls_mode=getattr(args, f"{endpoint}_tls_mode"),
        ),
        directory=root.path,
        directory_authority=root,
        timeout_seconds=30,
        secrets=secrets,
    )
    _command_success(evidence, stdout, stderr, operation)
    try:
        value = stdout.decode("utf-8").strip()
    except UnicodeDecodeError as error:
        raise HostedReportError(f"{operation} output is not UTF-8") from error
    return evidence, value


def observe_source_identity(
    args: argparse.Namespace,
    psql: RetainedFile,
    passfile: RetainedFile,
    root: RetainedDirectory,
    secrets: tuple[bytes, ...],
) -> dict[str, str]:
    database_literal = args.source_database.replace("'", "''")
    _, observed = _run_psql(
        args,
        psql,
        passfile,
        root,
        "admit_source_provider_identity",
        (
            "SELECT control.system_identifier::text || E'\\t' || COALESCE(database.oid::text,'') "
            "FROM pg_control_system() AS control CROSS JOIN pg_database AS database "
            f"WHERE database.datname='{database_literal}'"
        ),
        secrets,
        endpoint="source",
    )
    parts = observed.split("\t", 1)
    identity = {
        "system_identifier": parts[0] if len(parts) == 2 else "",
        "database_oid": parts[1] if len(parts) == 2 else "",
        "database_name": args.source_database,
    }
    require(
        _provider_identity_is_canonical(identity),
        "hosted source admission did not observe an exact provider identity",
    )
    return identity


def observe_target_identity(
    args: argparse.Namespace,
    psql: RetainedFile,
    passfile: RetainedFile,
    root: RetainedDirectory,
    secrets: tuple[bytes, ...],
    operation: str,
    *,
    expected_system_identifier: str | None = None,
) -> dict[str, str | int] | None:
    database_literal = args.target_database.replace("'", "''")
    _, observed = _run_psql(
        args,
        psql,
        passfile,
        root,
        operation,
        (
            "SELECT control.system_identifier::text || E'\\t' || COALESCE(database.oid::text,'') "
            "|| E'\\t' || COALESCE(database.datconnlimit::text,'') || E'\\t' || "
            "COALESCE(shobj_description(database.oid,'pg_database'),'') "
            "FROM pg_control_system() AS control LEFT JOIN pg_database AS database "
            f"ON database.datname='{database_literal}'"
        ),
        secrets,
    )
    parts = observed.split("\t", 3)
    require(
        len(parts) == 4 and parts[0].isdigit(),
        "hosted target admission did not observe an exact provider cluster",
    )
    if expected_system_identifier is not None:
        require(
            parts[0] == expected_system_identifier,
            "hosted target provider cluster changed after admission",
        )
    if not parts[1]:
        require(
            parts[2] == "" and parts[3] == "",
            "hosted absent target observation was malformed",
        )
        return None
    require(
        parts[1].isdigit() and parts[2] == "0" and parts[3] == DISPOSABLE_TARGET_MARKER,
        "hosted target admission did not observe the exact sealed disposable identity",
    )
    return {
        "system_identifier": parts[0],
        "database_oid": parts[1],
        "database_name": args.target_database,
    }


def cleanup_target(
    args: argparse.Namespace,
    psql: RetainedFile,
    passfile: RetainedFile,
    root: RetainedDirectory,
    secrets: tuple[bytes, ...],
    *,
    require_target: bool,
    expected_identity: dict[str, str | int],
) -> dict[str, Any]:
    require(
        expected_identity.get("database_name") == args.target_database
        and isinstance(expected_identity.get("system_identifier"), str)
        and str(expected_identity["system_identifier"]).isdigit()
        and isinstance(expected_identity.get("database_oid"), str)
        and str(expected_identity["database_oid"]).isdigit()
        and int(expected_identity["database_oid"]) > 0,
        "cleanup has no exact admitted target identity",
    )
    observed_identity = observe_target_identity(
        args,
        psql,
        passfile,
        root,
        secrets,
        "cleanup_target_observe",
        expected_system_identifier=str(expected_identity["system_identifier"]),
    )
    if observed_identity is None:
        require(not require_target, "restored target disappeared before cleanup")
        database_literal = args.target_database.replace("'", "''")
        _, absence = _run_psql(
            args,
            psql,
            passfile,
            root,
            "cleanup_bound_absent_target",
            (
                "SELECT control.system_identifier::text || E'\\t' || "
                f"(SELECT count(*)::text FROM pg_database WHERE datname='{database_literal}') "
                "|| E'\\t' || (SELECT count(*)::text FROM pg_roles "
                "WHERE rolname LIKE 'worldstream_restore_%') "
                "FROM pg_control_system() AS control"
            ),
            secrets,
        )
        absence_parts = absence.split("\t", 2)
        require(
            absence_parts == [str(expected_identity["system_identifier"]), "0", "0"],
            "absent target cleanup proof was not bound to the admitted provider",
        )
        return {
            "status": "pass",
            "admitted_target_identity": expected_identity,
            "target_marker_before_drop": "absent_after_admission",
            "target_connection_limit_before_drop": 0,
            "target_backends_before_drop": 0,
            "generated_restore_roles_before_drop": 0,
            "target_database_after_drop": "absent",
            "provider_cleanup_transcript_sha256": "sha256:"
            + hashlib.sha256(b"admitted-target-already-absent").hexdigest(),
        }
    require(
        observed_identity == expected_identity,
        "cleanup refused a target whose provider identity changed after admission",
    )
    database_literal = args.target_database.replace("'", "''")
    target_identifier = f'"{args.target_database}"'
    _, backends = _run_psql(
        args,
        psql,
        passfile,
        root,
        "cleanup_target_backends",
        (
            "SELECT count(*)::text FROM pg_stat_activity "
            f"WHERE datname='{database_literal}' AND pid<>pg_backend_pid()"
        ),
        secrets,
    )
    require(backends == "0", "restored target still has provider sessions")
    _, roles = _run_psql(
        args,
        psql,
        passfile,
        root,
        "cleanup_restore_roles",
        "SELECT count(*)::text FROM pg_roles WHERE rolname LIKE 'worldstream_restore_%'",
        secrets,
    )
    require(
        roles == "0", "generated native restore roles remain after product execution"
    )
    cleanup_script_bytes = (
        "\\set ON_ERROR_STOP on\n"
        "SELECT EXISTS (SELECT 1 FROM pg_control_system() AS control "
        "CROSS JOIN pg_database AS database "
        f"WHERE control.system_identifier::text='{expected_identity['system_identifier']}' "
        f"AND database.oid={expected_identity['database_oid']} "
        f"AND database.datname='{database_literal}' "
        "AND database.datconnlimit=0 "
        "AND COALESCE(shobj_description(database.oid,'pg_database'),'')="
        f"'{DISPOSABLE_TARGET_MARKER}') AS admitted \\gset\n"
        "\\if :admitted\n"
        f"DROP DATABASE {target_identifier};\n"
        "\\else\n"
        "\\quit 3\n"
        "\\endif\n"
    ).encode()
    cleanup_script = RetainedFile.create_in(
        root,
        "cleanup-target.psql",
        "conditional cleanup script",
        cleanup_script_bytes,
        maximum_size=64 * 1024,
    )
    try:
        drop_evidence, _, drop_stderr = run_exact(
            psql,
            "cleanup_target_drop",
            [
                "--no-password",
                "--no-psqlrc",
                "--host",
                args.target_host,
                "--port",
                str(args.target_port),
                "--username",
                args.target_username,
                "--dbname",
                "postgres",
                "--quiet",
                "--tuples-only",
                "--no-align",
                "--file",
                "-",
            ],
            inherited=(passfile, cleanup_script),
            environment=_sanitized_environment(
                passfile=passfile.execution_path(), tls_mode=args.target_tls_mode
            ),
            directory=root.path,
            directory_authority=root,
            timeout_seconds=30,
            secrets=secrets,
            stdin_bytes=cleanup_script_bytes,
        )
    finally:
        cleanup_script.close()
    require(drop_evidence.exit_code == 0, "disposable target drop failed closed")
    require(not drop_stderr.strip(), "disposable target drop emitted unexpected stderr")
    _, absent = _run_psql(
        args,
        psql,
        passfile,
        root,
        "cleanup_target_absent",
        (
            "SELECT control.system_identifier::text || E'\\t' || "
            f"(SELECT count(*)::text FROM pg_database WHERE datname='{database_literal}') "
            "|| E'\\t' || (SELECT count(*)::text FROM pg_roles "
            "WHERE rolname LIKE 'worldstream_restore_%') "
            "FROM pg_control_system() AS control"
        ),
        secrets,
    )
    require(
        absent == f"{expected_identity['system_identifier']}\t0\t0",
        "disposable target cleanup was not proven on the admitted provider",
    )
    transcript = {
        "observe": "sealed_disposable_target",
        "backends": int(backends),
        "restore_roles": int(roles),
        "drop_stdout_sha256": drop_evidence.stdout_sha256,
        "drop_stdout_size_bytes": drop_evidence.stdout_size_bytes,
    }
    transcript_bytes = json.dumps(
        transcript, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    return {
        "status": "pass",
        "admitted_target_identity": expected_identity,
        "target_marker_before_drop": DISPOSABLE_TARGET_MARKER,
        "target_connection_limit_before_drop": 0,
        "target_backends_before_drop": 0,
        "generated_restore_roles_before_drop": 0,
        "target_database_after_drop": "absent",
        "provider_cleanup_transcript_sha256": "sha256:"
        + hashlib.sha256(transcript_bytes).hexdigest(),
    }


def _recover_if_needed(
    args: argparse.Namespace,
    control: RetainedFile,
    passfile: RetainedFile,
    root: RetainedDirectory,
    secrets: tuple[bytes, ...],
    artifact_directory_identity: dict[str, str],
) -> None:
    journal_names = [
        name
        for name in root.list_names()
        if RECOVERY_JOURNAL.fullmatch(name) is not None
    ]
    require(
        len(journal_names) <= 1,
        "native restore produced multiple recovery authorities",
    )
    if not journal_names:
        return
    journal_name = journal_names[0]
    root.retain_child(
        journal_name,
        writable=False,
        share_write=os.name == "nt",
    )
    journal_identity = root.retained_children[journal_name][2]
    recovery_root = root.current_named_path()
    evidence, stdout, stderr = run_exact(
        control,
        "native_recover",
        [
            "postgres",
            "native",
            "recover",
            "--target-host",
            args.target_host,
            "--target-port",
            str(args.target_port),
            "--target-database",
            args.target_database,
            "--target-username",
            args.target_username,
            "--target-tls-mode",
            args.target_tls_mode,
            "--passfile",
            passfile.execution_path(),
            "--recovery",
            os.fspath(recovery_root / journal_name),
            "--recovery-storage-id",
            journal_identity["storage_id"],
            "--recovery-file-id",
            journal_identity["file_id"],
            "--artifact-directory-storage-id",
            artifact_directory_identity["storage_id"],
            "--artifact-directory-file-id",
            artifact_directory_identity["file_id"],
            "--timeout-seconds",
            str(args.timeout_seconds),
        ],
        inherited=(passfile,),
        environment=_sanitized_environment(),
        directory=root.path,
        directory_authority=root,
        timeout_seconds=args.timeout_seconds + 30,
        secrets=secrets,
    )
    _command_success(evidence, stdout, stderr, "native recovery")
    root.upgrade_retained_child_for_scrub(journal_name)


def emit(args: argparse.Namespace) -> None:
    expected_system, expected_machines = PLATFORMS[args.source]
    observed_system = platform.system()
    observed_machine = platform.machine().lower()
    require(
        observed_system == expected_system and observed_machine in expected_machines,
        "hosted platform identity does not match the requested source",
    )
    _validate_endpoint(
        args.source_host,
        args.source_port,
        args.source_database,
        args.source_username,
    )
    _validate_endpoint(
        args.target_host,
        args.target_port,
        args.target_database,
        args.target_username,
    )
    require(
        (args.source_host, args.source_port, args.source_database)
        != (args.target_host, args.target_port, args.target_database),
        "native restore source and target must be distinct databases",
    )
    require(
        MIN_TIMEOUT_SECONDS <= args.timeout_seconds <= MAX_TIMEOUT_SECONDS,
        "native restore timeout is outside the reviewed bound",
    )
    require(
        args.scrub_passfile_on_success is True,
        "hosted restore requires explicit exact passfile scrub on success",
    )
    paths = [
        args.package_report,
        args.runtime_report,
        args.artifact,
        args.packaged_control,
        args.fixture_report,
        args.passfile,
        args.pg_dump,
        args.pg_restore,
        args.psql,
        args.work_parent,
        args.output,
    ]
    require(
        len({path.resolve(strict=False) for path in paths}) == len(paths),
        "hosted restore inputs and output must be distinct",
    )

    _, package_sha256, package_size = read_json(args.package_report, "package report")
    runtime, _, _ = read_json(args.runtime_report, "packaged runtime report")
    fixture, fixture_sha256, fixture_size = read_json(
        args.fixture_report, "source fixture report"
    )
    require(
        runtime.get("schema") == RUNTIME_SCHEMA
        and runtime.get("status") == "pass"
        and runtime.get("release_evidence") is False
        and runtime.get("secrets_emitted") is False,
        "packaged runtime report is incomplete",
    )
    binding = runtime.get("package_binding")
    require(isinstance(binding, dict), "packaged runtime binding is missing")
    package_binding = {field: binding.get(field) for field in PACKAGE_BINDING_FIELDS}
    require(
        all(
            type(package_binding[field]) is int and package_binding[field] > 0
            for field in (
                "archive_size_bytes",
                "package_report_size_bytes",
                "worldstreamctl_size_bytes",
            )
        )
        and isinstance(package_binding["source_revision"], str)
        and GIT_REVISION.fullmatch(package_binding["source_revision"]) is not None
        and all(
            isinstance(package_binding[field], str)
            and SHA256_REF.fullmatch(package_binding[field]) is not None
            for field in (
                "archive_sha256",
                "package_report_sha256",
                "worldstreamctl_sha256",
            )
        ),
        "packaged runtime binding has malformed identities",
    )
    archive = RetainedFile.open(
        args.artifact,
        "release archive",
        expected_size=package_binding["archive_size_bytes"],
        maximum_size=MAX_ARCHIVE_BYTES,
    )
    control = RetainedFile.open(
        args.packaged_control,
        "retained packaged control binary",
        expected_size=package_binding["worldstreamctl_size_bytes"],
        maximum_size=MAX_CONTROL_BYTES,
    )
    passfile: RetainedFile | None = None
    pg_dump: RetainedFile | None = None
    pg_restore: RetainedFile | None = None
    psql: RetainedFile | None = None
    root: RetainedDirectory | None = None
    binding_root: RetainedDirectory | None = None
    binding_files: dict[str, RetainedFile] = {}
    binding_published = False
    cleanup_complete = False
    provider_cleanup_complete = False
    restore_committed = False
    admitted_target: dict[str, str | int] | None = None
    admitted_source: dict[str, str] | None = None
    artifact_directory_identity: dict[str, str] | None = None
    target_cleanup: dict[str, Any] | None = None
    primary_error: BaseException | None = None
    try:
        require(
            package_binding["archive_sha256"] == archive.sha256
            and package_binding["archive_size_bytes"] == archive.size_bytes
            and package_binding["package_report_sha256"] == package_sha256
            and package_binding["package_report_size_bytes"] == package_size
            and package_binding["worldstreamctl_sha256"] == control.sha256
            and package_binding["worldstreamctl_size_bytes"] == control.size_bytes,
            "packaged runtime binding does not identify the exact inputs",
        )
        passfile = RetainedFile.open(
            args.passfile,
            "operator passfile",
            maximum_size=MAX_PASSFILE_BYTES,
            writable=os.name != "nt",
        )
        expected_passfile_identity = {
            "storage_id": getattr(args, "passfile_storage_id", None),
            "file_id": getattr(args, "passfile_file_id", None),
        }
        expected_passfile_size = getattr(args, "passfile_size", None)
        expected_passfile_sha256 = getattr(args, "passfile_sha256", None)
        require(
            _canonical_identity_is_valid(expected_passfile_identity)
            and type(expected_passfile_size) is int
            and expected_passfile_size > 0
            and isinstance(expected_passfile_sha256, str)
            and SHA256_REF.fullmatch(expected_passfile_sha256) is not None
            and _canonical_file_identity(passfile.descriptor)
            == expected_passfile_identity
            and passfile.size_bytes == expected_passfile_size
            and passfile.sha256 == expected_passfile_sha256,
            "operator passfile differs from the wrapper-retained identity or content",
        )
        require(
            passfile.admitted.st_nlink == 1,
            "operator passfile retained for scrub must not have hard links",
        )
        if os.name != "nt":
            require(
                stat.S_IMODE(passfile.admitted.st_mode) & 0o077 == 0
                and passfile.admitted.st_uid == os.geteuid(),
                "operator passfile must be owner-only",
            )
        pg_dump = RetainedFile.open(
            args.pg_dump, "retained pg_dump", maximum_size=MAX_PROVIDER_TOOL_BYTES
        )
        pg_restore = RetainedFile.open(
            args.pg_restore,
            "retained pg_restore",
            maximum_size=MAX_PROVIDER_TOOL_BYTES,
        )
        psql = RetainedFile.open(
            args.psql, "retained psql", maximum_size=MAX_PROVIDER_TOOL_BYTES
        )
        secrets = _passfile_secrets(passfile)
        source_revision = package_binding["source_revision"]
        require(
            fixture.get("schema") == FIXTURE_SCHEMA
            and fixture.get("status") == "pass"
            and fixture.get("release_evidence") is False
            and fixture.get("secrets_emitted") is False
            and fixture.get("source_construction")
            == {
                "classification": FIXTURE_CLASSIFICATION,
                "source_revision": source_revision,
                "trusted_product_execution": False,
            },
            "source fixture report is not explicitly untrusted and source-bound",
        )
        root = _private_work_root(
            args.work_parent,
            expected_identity={
                "storage_id": getattr(args, "work_parent_storage_id", None),
                "file_id": getattr(args, "work_parent_file_id", None),
            },
        )
        raw_report_argument = root.named_child("native-restore-report.json")
        dump_path = root.named_child("native-restore.dump")
        common_inherited = (passfile, pg_dump, pg_restore, psql)
        directory_evidence, directory_stdout, directory_stderr = run_exact(
            control,
            "artifact_directory_identity",
            [
                "postgres",
                "native",
                "directory-identity",
                "--path",
                os.fspath(root.path),
            ],
            inherited=common_inherited,
            environment=_sanitized_environment(),
            directory=root.path,
            directory_authority=root,
            timeout_seconds=30,
            secrets=secrets,
        )
        _command_success(
            directory_evidence,
            directory_stdout,
            directory_stderr,
            "packaged artifact directory identity",
        )
        directory_receipt = _verify_directory_receipt(directory_stdout, root)
        artifact_directory_identity = directory_receipt["artifact_directory_identity"]
        admitted_source = observe_source_identity(
            args,
            psql,
            passfile,
            root,
            secrets,
        )
        admitted_target = observe_target_identity(
            args,
            psql,
            passfile,
            root,
            secrets,
            "admit_cleanup_target",
        )
        require(admitted_target is not None, "hosted disposable target is absent")
        rebuild_evidence, rebuild_stdout, rebuild_stderr = run_exact(
            control,
            "snapshot_rebuild",
            [
                "postgres",
                "snapshots",
                "rebuild",
                "--host",
                args.source_host,
                "--port",
                str(args.source_port),
                "--database",
                args.source_database,
                "--username",
                args.source_username,
                "--tls-mode",
                args.source_tls_mode,
                "--passfile",
                passfile.execution_path(),
            ],
            inherited=common_inherited,
            environment=_sanitized_environment(),
            directory=root.path,
            directory_authority=root,
            timeout_seconds=args.timeout_seconds,
            secrets=secrets,
        )
        _command_success(
            rebuild_evidence,
            rebuild_stdout,
            rebuild_stderr,
            "packaged snapshot rebuild",
        )
        rebuild_receipt = _verify_snapshot_receipt(rebuild_stdout)
        require(
            rebuild_receipt.get("source_provider_identity") == admitted_source,
            "snapshot rebuild receipt differs from hosted source admission",
        )
        restore_evidence, restore_stdout, restore_stderr = run_exact(
            control,
            "native_restore",
            [
                "postgres",
                "native",
                "restore",
                "--source-host",
                args.source_host,
                "--source-port",
                str(args.source_port),
                "--source-database",
                args.source_database,
                "--source-username",
                args.source_username,
                "--source-tls-mode",
                args.source_tls_mode,
                "--target-host",
                args.target_host,
                "--target-port",
                str(args.target_port),
                "--target-database",
                args.target_database,
                "--target-username",
                args.target_username,
                "--target-tls-mode",
                args.target_tls_mode,
                "--passfile",
                passfile.execution_path(),
                "--pg-dump",
                pg_dump.execution_path(),
                "--pg-restore",
                pg_restore.execution_path(),
                "--psql",
                psql.execution_path(),
                "--dump",
                os.fspath(dump_path),
                "--report",
                os.fspath(raw_report_argument),
                "--artifact-directory-storage-id",
                artifact_directory_identity["storage_id"],
                "--artifact-directory-file-id",
                artifact_directory_identity["file_id"],
                "--timeout-seconds",
                str(args.timeout_seconds),
            ],
            inherited=common_inherited,
            environment=_sanitized_environment(),
            directory=root.path,
            directory_authority=root,
            timeout_seconds=args.timeout_seconds + 30,
            secrets=secrets,
        )
        _command_success(
            restore_evidence,
            restore_stdout,
            restore_stderr,
            "packaged native restore",
        )
        restore_receipt = _preflight_restore_receipt(restore_stdout)
        # A committed receipt is emitted only after the contained commit tree
        # is drained. From this point target cleanup is authorized even when a
        # later artifact identity check fails; automatic recovery is not.
        restore_committed = True
        root.retain_child(
            "native-restore.dump",
            expected_identity=restore_receipt["native_dump_identity"],
        )
        root.retain_child(
            "native-restore-report.json",
            expected_identity=restore_receipt["report_identity"],
        )
        recovery_record_name = restore_receipt["recovery_record_name"]
        root.retain_child(
            recovery_record_name,
            expected_identity=restore_receipt["recovery_record_identity"],
        )
        raw_report = root.read_retained_child(
            "native-restore-report.json",
            maximum_size=MAX_JSON_BYTES,
            label="committed native restore report",
        )
        try:
            native = json.loads(raw_report)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise HostedReportError(
                "cannot decode committed native restore report"
            ) from error
        require(
            isinstance(native, dict),
            "committed native restore report must be a JSON object",
        )
        native_sha256 = "sha256:" + hashlib.sha256(raw_report).hexdigest()
        native_size = len(raw_report)
        require(
            root.retained_children["native-restore.dump"][1].st_size
            == restore_receipt["native_dump_size_bytes"]
            and root.retained_children["native-restore-report.json"][1].st_size
            == restore_receipt["report_size_bytes"],
            "native restore report bytes changed after commit",
        )
        require(
            native.get("schema") == NATIVE_SCHEMA
            and native.get("status") == "ready"
            and native.get("reason")
            == "postgres_native_restore_verified_by_unified_verifier"
            and native.get("release_evidence") is False
            and native.get("secrets_emitted") is False
            and native.get("native_witness_minted") is True,
            "native restore report is not committed Ready evidence",
        )
        restore_receipt = _verify_restore_receipt(restore_receipt, native, raw_report)
        require(
            native.get("source_provider_identity") == admitted_source,
            "native restore source identity differs from hosted admission",
        )
        require(
            native.get("target_provider_identity") == admitted_target,
            "native restore target identity differs from hosted admission",
        )
        target_cleanup = cleanup_target(
            args,
            psql,
            passfile,
            root,
            secrets,
            require_target=True,
            expected_identity=admitted_target,
        )
        provider_cleanup_complete = True
        archive.verify()
        control.verify()
        for retained in common_inherited:
            retained.verify()
        binding_root = _private_work_root(
            args.work_parent,
            expected_identity={
                "storage_id": getattr(args, "work_parent_storage_id", None),
                "file_id": getattr(args, "work_parent_file_id", None),
            },
            fixed_name=PRIVATE_BINDING_ROOT_NAME,
        )
        for key, raw_value, maximum_size in (
            (
                "artifact_directory_stdout",
                directory_stdout,
                MAX_COMMAND_OUTPUT_BYTES,
            ),
            ("snapshot_rebuild_stdout", rebuild_stdout, MAX_COMMAND_OUTPUT_BYTES),
            ("native_restore_stdout", restore_stdout, MAX_COMMAND_OUTPUT_BYTES),
            ("native_report", raw_report, MAX_JSON_BYTES),
        ):
            binding_files[key] = RetainedFile.create_in(
                binding_root,
                PRIVATE_BINDING_FILES[key],
                f"private {key.replace('_', ' ')} binding",
                raw_value,
                maximum_size=maximum_size,
            )
        binding_files["native_dump"] = _stream_retained_child_copy(
            root,
            "native-restore.dump",
            binding_root,
            PRIVATE_BINDING_FILES["native_dump"],
            expected_size=restore_receipt["native_dump_size_bytes"],
            maximum_size=MAX_ARCHIVE_BYTES,
        )
        for evidence, key in (
            (directory_evidence, "artifact_directory_stdout"),
            (rebuild_evidence, "snapshot_rebuild_stdout"),
            (restore_evidence, "native_restore_stdout"),
        ):
            retained_output = binding_files[key]
            require(
                evidence.stdout_sha256 == retained_output.sha256
                and evidence.stdout_size_bytes == retained_output.size_bytes,
                f"{evidence.operation} retained stdout binding mismatch",
            )
        require(
            binding_files["native_report"].sha256 == native_sha256
            and binding_files["native_report"].size_bytes == native_size,
            "retained native report private binding mismatch",
        )
        private_binding = {
            "schema": PRIVATE_BINDING_SCHEMA,
            "parent_identity": _canonical_file_identity(binding_root.parent_descriptor),
            "root_name": PRIVATE_BINDING_ROOT_NAME,
            "root_identity": _canonical_file_identity(binding_root.descriptor),
            "files": {
                key: _private_binding_file_record(binding_files[key])
                for key in PRIVATE_BINDING_FILES
            },
        }
        report = {
            "schema": SCHEMA,
            "status": "pass",
            "release_evidence": False,
            "secrets_emitted": False,
            "platform": {"system": observed_system, "machine": observed_machine},
            "package_binding": package_binding,
            "source_fixture": {
                "classification": FIXTURE_CLASSIFICATION,
                "fixture_report_sha256": fixture_sha256,
                "fixture_report_size_bytes": fixture_size,
                "source_revision": source_revision,
            },
            "product_execution": {
                "controller": {
                    "execution": "retained_exact_packaged_binary",
                    "sha256": control.sha256,
                    "size_bytes": control.size_bytes,
                },
                "provider_tools": {
                    "pg_dump": _provider_tool_binding(pg_dump),
                    "pg_restore": _provider_tool_binding(pg_restore),
                    "psql": _provider_tool_binding(psql),
                },
                "source": {
                    "host": args.source_host,
                    "port": args.source_port,
                    "database": args.source_database,
                    "username": args.source_username,
                    "tls_mode": args.source_tls_mode,
                },
                "target": {
                    "host": args.target_host,
                    "port": args.target_port,
                    "database": args.target_database,
                    "username": args.target_username,
                    "tls_mode": args.target_tls_mode,
                },
                "actions": [
                    {
                        **directory_evidence.as_dict(),
                        "receipt": directory_receipt,
                    },
                    {**rebuild_evidence.as_dict(), "receipt": rebuild_receipt},
                    {**restore_evidence.as_dict(), "receipt": restore_receipt},
                ],
                "native_report_sha256": native_sha256,
                "native_report_size_bytes": native_size,
                "native_report_canonical_sha256": "sha256:"
                + hashlib.sha256(
                    json.dumps(
                        native,
                        ensure_ascii=False,
                        sort_keys=True,
                        separators=(",", ":"),
                    ).encode("utf-8")
                ).hexdigest(),
            },
            "native_restore": native,
            "cleanup": target_cleanup,
            "private_binding": private_binding,
        }
        # Keep the exact directory handles until every file created below that
        # authority is zeroed. The empty/zero-length retained root is deliberate:
        # removing a caller-visible pathname would reintroduce a substitution
        # victim at the final destructive step.
        scrubbed_count = root.scrub_exact()
        passfile.scrub_exact()
        target_cleanup["private_artifacts_disposition"] = PRIVATE_ARTIFACT_DISPOSITION
        target_cleanup["private_artifact_placeholder_count"] = scrubbed_count
        target_cleanup["operator_passfile_disposition"] = (
            "exact_retained_file_scrubbed_to_zero_length"
        )
        cleanup_complete = True
        write_exclusive(args.output, report, secrets=secrets)
        binding_published = True
    except BaseException as error:
        primary_error = error
        nested_uncertainty: ContainmentUncertainError | None = None
        if (
            not isinstance(error, ContainmentUncertainError)
            and not provider_cleanup_complete
            and root is not None
            and passfile is not None
            and psql is not None
            and admitted_target is not None
        ):
            recovery_clean = restore_committed
            try:
                if not restore_committed:
                    require(
                        artifact_directory_identity is not None,
                        "hosted recovery has no artifact-directory identity",
                    )
                    _recover_if_needed(
                        args,
                        control,
                        passfile,
                        root,
                        _passfile_secrets(passfile),
                        artifact_directory_identity,
                    )
                recovery_clean = True
            except ContainmentUncertainError as uncertainty:
                nested_uncertainty = uncertainty
                recovery_clean = False
            except (HostedReportError, OSError, ValueError, subprocess.SubprocessError):
                recovery_clean = False
            if recovery_clean:
                try:
                    cleanup_target(
                        args,
                        psql,
                        passfile,
                        root,
                        _passfile_secrets(passfile),
                        require_target=False,
                        expected_identity=admitted_target,
                    )
                    provider_cleanup_complete = True
                    root.scrub_exact()
                    passfile.scrub_exact()
                    cleanup_complete = True
                except ContainmentUncertainError as uncertainty:
                    nested_uncertainty = uncertainty
                    cleanup_complete = False
                except (
                    HostedReportError,
                    OSError,
                    ValueError,
                    subprocess.SubprocessError,
                ):
                    cleanup_complete = False
        if nested_uncertainty is not None:
            raise nested_uncertainty from error
        if isinstance(error, ContainmentUncertainError):
            raise
        if binding_root is not None and not binding_published:
            try:
                binding_root.scrub_exact()
            except (HostedReportError, OSError, ValueError) as binding_error:
                raise RecoveryPreservedError(
                    "hosted native restore could not scrub its private platform binding"
                ) from binding_error
        if root is not None and not cleanup_complete:
            raise RecoveryPreservedError(
                f"hosted native restore preserved recovery authority after: {error}"
            ) from error
        raise
    finally:
        for retained in binding_files.values():
            retained.close()
        if binding_root is not None:
            binding_root.close()
        for retained in (psql, pg_restore, pg_dump, passfile, control, archive):
            if retained is not None:
                retained.close()
        if root is not None:
            root.close()
        if primary_error is not None and not cleanup_complete and root is not None:
            print(
                f"hosted native restore preserved recovery root: {root.path}",
                file=sys.stderr,
            )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", choices=tuple(PLATFORMS), required=True)
    parser.add_argument("--package-report", type=Path, required=True)
    parser.add_argument("--runtime-report", type=Path, required=True)
    parser.add_argument("--artifact", type=Path, required=True)
    parser.add_argument("--packaged-control", type=Path, required=True)
    parser.add_argument("--fixture-report", type=Path, required=True)
    parser.add_argument("--source-host", required=True)
    parser.add_argument("--source-port", type=int, required=True)
    parser.add_argument("--source-database", required=True)
    parser.add_argument("--source-username", required=True)
    parser.add_argument(
        "--source-tls-mode", choices=("require", "disable"), required=True
    )
    parser.add_argument("--target-host", required=True)
    parser.add_argument("--target-port", type=int, required=True)
    parser.add_argument("--target-database", required=True)
    parser.add_argument("--target-username", required=True)
    parser.add_argument(
        "--target-tls-mode", choices=("require", "disable"), required=True
    )
    parser.add_argument("--passfile", type=Path, required=True)
    parser.add_argument("--passfile-storage-id", required=True)
    parser.add_argument("--passfile-file-id", required=True)
    parser.add_argument("--passfile-size", type=int, required=True)
    parser.add_argument("--passfile-sha256", required=True)
    parser.add_argument("--scrub-passfile-on-success", action="store_true")
    parser.add_argument("--pg-dump", type=Path, required=True)
    parser.add_argument("--pg-restore", type=Path, required=True)
    parser.add_argument("--psql", type=Path, required=True)
    parser.add_argument("--work-parent", type=Path, required=True)
    parser.add_argument("--work-parent-storage-id", required=True)
    parser.add_argument("--work-parent-file-id", required=True)
    parser.add_argument("--timeout-seconds", type=int, default=300)
    parser.add_argument("--output", type=Path, required=True)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    try:
        emit(parse_args(argv))
    except (ContainmentUncertainError, RecoveryPreservedError) as error:
        print(f"hosted native restore report failed: {error}", file=sys.stderr)
        return PRESERVED_RECOVERY_EXIT_CODE
    except (HostedReportError, OSError, ValueError) as error:
        print(f"hosted native restore report failed: {error}", file=sys.stderr)
        return SAFE_FAILURE_EXIT_CODE
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

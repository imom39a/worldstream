#!/usr/bin/env python3
"""Small fail-closed browser adapter for the release Heist DOM story.

The command surface intentionally matches only the browser operations used by
``web/console/live-browser-story.sh``.  It launches an explicitly identified
Chrome-for-Testing headless shell and drives it over the Chrome DevTools
Protocol; it is not a general browser automation wrapper.
"""

from __future__ import annotations

import contextlib
import hashlib
import json
import os
import pathlib
import re
import secrets
import signal
import socket
import stat
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

from websockets.sync.client import connect

MAX_CONTROL_BYTES = 8 * 1024 * 1024
SHA256 = re.compile(r"^[0-9a-f]{64}$")
VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$")
SURFACE = re.compile(r"^surface:[0-9a-f]{32}$")
VERIFIED_EXECUTABLE = re.compile(r"^\.worldstream-verified-[0-9a-f]{32}$")
STATE_FIELDS = {
    "schema",
    "pid",
    "port",
    "surface_ref",
    "websocket_url",
    "browser",
    "executable",
}


class BrowserFailure(RuntimeError):
    """The pinned browser or requested CDP operation failed closed."""


def _file_identity(metadata: os.stat_result) -> tuple[int, int, int, int, int, int]:
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_mode,
        metadata.st_size,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
    )


def _sha256(path: pathlib.Path, expected_size: int) -> str:
    try:
        before = path.lstat()
    except OSError as error:
        raise BrowserFailure("browser_binary_unavailable") from error
    if (
        stat.S_ISLNK(before.st_mode)
        or not stat.S_ISREG(before.st_mode)
        or before.st_size != expected_size
    ):
        raise BrowserFailure("browser_binary_size_mismatch")
    digest = hashlib.sha256()
    size = 0
    try:
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(path, flags)
        with os.fdopen(descriptor, "rb") as source:
            opened = os.fstat(source.fileno())
            while chunk := source.read(1024 * 1024):
                size += len(chunk)
                if size > expected_size:
                    raise BrowserFailure("browser_binary_changed_during_hash")
                digest.update(chunk)
            after = os.fstat(source.fileno())
        final_path = path.lstat()
    except OSError as error:
        raise BrowserFailure("browser_binary_unavailable") from error
    identity = _file_identity(before)
    if (
        size != expected_size
        or not stat.S_ISREG(opened.st_mode)
        or _file_identity(opened) != identity
        or _file_identity(after) != identity
        or _file_identity(final_path) != identity
    ):
        raise BrowserFailure("browser_binary_changed_during_hash")
    return digest.hexdigest()


def _verify_browser_version(
    path: pathlib.Path,
    identity: dict[str, Any],
    *,
    descriptor: int | None = None,
    record: dict[str, Any] | None = None,
) -> None:
    if (descriptor is None) != (record is None):
        raise BrowserFailure("browser_version_descriptor_invalid")
    try:
        descriptor_path = (
            _descriptor_executable_path(descriptor, record)
            if descriptor is not None and record is not None
            else None
        )
        if descriptor_path is None:
            completed = subprocess.run(
                [str(path), "--version"],
                stdin=subprocess.DEVNULL,
                capture_output=True,
                text=True,
                check=False,
                timeout=10,
            )
        else:
            completed = subprocess.run(
                [str(path), "--version"],
                executable=descriptor_path,
                pass_fds=(descriptor,),
                stdin=subprocess.DEVNULL,
                capture_output=True,
                text=True,
                check=False,
                timeout=10,
            )
    except (OSError, subprocess.SubprocessError) as error:
        raise BrowserFailure("browser_version_unavailable") from error
    output = (completed.stdout + completed.stderr).strip()
    if (
        completed.returncode != 0
        or "\n" in output
        or output != identity["version_output"]
    ):
        raise BrowserFailure("browser_version_mismatch")


def _regular_executable(path: pathlib.Path) -> pathlib.Path:
    try:
        mode = path.lstat().st_mode
    except OSError as error:
        raise BrowserFailure("browser_binary_unavailable") from error
    if stat.S_ISLNK(mode) or not stat.S_ISREG(mode) or mode & 0o111 == 0:
        raise BrowserFailure("browser_binary_not_regular_executable")
    return path


def _browser_configuration() -> tuple[pathlib.Path, dict[str, Any]]:
    raw_path = os.environ.get("WORLDSTREAM_BROWSER_BINARY", "")
    version = os.environ.get("WORLDSTREAM_BROWSER_VERSION", "")
    expected_digest = os.environ.get("WORLDSTREAM_BROWSER_SHA256", "")
    archive_url = os.environ.get("WORLDSTREAM_BROWSER_ARCHIVE_URL", "")
    archive_digest = os.environ.get("WORLDSTREAM_BROWSER_ARCHIVE_SHA256", "")
    try:
        expected_size = int(os.environ.get("WORLDSTREAM_BROWSER_SIZE_BYTES", ""))
        archive_size = int(os.environ.get("WORLDSTREAM_BROWSER_ARCHIVE_SIZE_BYTES", ""))
    except ValueError as error:
        raise BrowserFailure("browser_identity_environment_invalid") from error
    parsed_archive = urllib.parse.urlsplit(archive_url)
    if (
        not raw_path
        or not VERSION.fullmatch(version)
        or not SHA256.fullmatch(expected_digest)
        or not SHA256.fullmatch(archive_digest)
        or not (1 <= expected_size <= 1024 * 1024 * 1024)
        or not (1 <= archive_size <= 1024 * 1024 * 1024)
        or parsed_archive.scheme != "https"
        or parsed_archive.netloc != "storage.googleapis.com"
        or parsed_archive.query
        or parsed_archive.fragment
        or not parsed_archive.path.startswith(f"/chrome-for-testing-public/{version}/")
        or not parsed_archive.path.endswith(".zip")
    ):
        raise BrowserFailure("browser_identity_environment_invalid")
    path = _regular_executable(pathlib.Path(raw_path))
    try:
        if path.lstat().st_size != expected_size:
            raise BrowserFailure("browser_binary_size_mismatch")
    except OSError as error:
        raise BrowserFailure("browser_binary_unavailable") from error
    return path, {
        "product": "chrome-for-testing-headless-shell",
        "version": version,
        "sha256": "sha256:" + expected_digest,
        "size_bytes": expected_size,
        "version_output": f"Google Chrome for Testing {version}",
        "distribution": {
            "url": archive_url,
            "sha256": "sha256:" + archive_digest,
            "size_bytes": archive_size,
        },
    }


def browser_identity() -> tuple[pathlib.Path, dict[str, Any]]:
    path, identity = _browser_configuration()
    expected_digest = identity["sha256"].removeprefix("sha256:")
    observed_digest = _sha256(path, identity["size_bytes"])
    if observed_digest != expected_digest:
        raise BrowserFailure("browser_binary_digest_mismatch")
    _verify_browser_version(path, identity)
    return path, identity


def _prepared_executable_record(
    path: pathlib.Path, metadata: os.stat_result, identity: dict[str, Any]
) -> dict[str, Any]:
    return {
        "path": str(path),
        "device": metadata.st_dev,
        "inode": metadata.st_ino,
        "mode": stat.S_IMODE(metadata.st_mode),
        "size_bytes": metadata.st_size,
        "mtime_ns": metadata.st_mtime_ns,
        "ctime_ns": metadata.st_ctime_ns,
        "sha256": identity["sha256"],
    }


def _validate_prepared_executable(
    record: object,
    configured_path: pathlib.Path,
    identity: dict[str, Any],
) -> pathlib.Path:
    if not isinstance(record, dict) or set(record) != {
        "path",
        "device",
        "inode",
        "mode",
        "size_bytes",
        "mtime_ns",
        "ctime_ns",
        "sha256",
    }:
        raise BrowserFailure("browser_executable_identity_invalid")
    candidate = pathlib.Path(str(record.get("path", "")))
    if (
        not candidate.is_absolute()
        or candidate.parent != configured_path.parent
        or VERIFIED_EXECUTABLE.fullmatch(candidate.name) is None
        or type(record.get("device")) is not int
        or type(record.get("inode")) is not int
        or type(record.get("mode")) is not int
        or type(record.get("size_bytes")) is not int
        or type(record.get("mtime_ns")) is not int
        or type(record.get("ctime_ns")) is not int
        or record.get("sha256") != identity["sha256"]
        or record.get("size_bytes") != identity["size_bytes"]
        or record.get("mode") != 0o500
    ):
        raise BrowserFailure("browser_executable_identity_invalid")
    try:
        observed = candidate.lstat()
    except OSError as error:
        raise BrowserFailure("browser_executable_identity_invalid") from error
    if (
        stat.S_ISLNK(observed.st_mode)
        or not stat.S_ISREG(observed.st_mode)
        or observed.st_dev != record["device"]
        or observed.st_ino != record["inode"]
        or stat.S_IMODE(observed.st_mode) != record["mode"]
        or observed.st_size != record["size_bytes"]
        or observed.st_mtime_ns != record["mtime_ns"]
        or observed.st_ctime_ns != record["ctime_ns"]
    ):
        raise BrowserFailure("browser_executable_identity_changed")
    return candidate


def _remove_prepared_executable(record: object) -> None:
    if not isinstance(record, dict):
        return
    candidate = pathlib.Path(str(record.get("path", "")))
    if VERIFIED_EXECUTABLE.fullmatch(candidate.name) is None:
        raise BrowserFailure("browser_executable_cleanup_refused")
    try:
        observed = candidate.lstat()
    except FileNotFoundError:
        return
    except OSError as error:
        raise BrowserFailure("browser_executable_cleanup_refused") from error
    if (
        not stat.S_ISREG(observed.st_mode)
        or observed.st_dev != record.get("device")
        or observed.st_ino != record.get("inode")
    ):
        raise BrowserFailure("browser_executable_cleanup_refused")
    try:
        candidate.unlink()
    except OSError as error:
        raise BrowserFailure("browser_executable_cleanup_failed") from error


def _prepare_browser_executable(
    path: pathlib.Path, identity: dict[str, Any]
) -> tuple[pathlib.Path, dict[str, Any]]:
    """Copy the verified executable beside its resources and bind the new inode."""

    try:
        parent = path.parent.lstat()
    except OSError as error:
        raise BrowserFailure("browser_executable_parent_unsafe") from error
    if (
        stat.S_ISLNK(parent.st_mode)
        or not stat.S_ISDIR(parent.st_mode)
        or parent.st_uid != os.geteuid()
        or stat.S_IMODE(parent.st_mode) & 0o077
    ):
        raise BrowserFailure("browser_executable_parent_unsafe")
    target = path.parent / f".worldstream-verified-{secrets.token_hex(16)}"
    target_created: tuple[int, int] | None = None
    success = False
    expected_size = identity["size_bytes"]
    expected_digest = identity["sha256"].removeprefix("sha256:")
    try:
        before = path.lstat()
        if (
            stat.S_ISLNK(before.st_mode)
            or not stat.S_ISREG(before.st_mode)
            or before.st_size != expected_size
            or before.st_mode & 0o111 == 0
        ):
            raise BrowserFailure("browser_binary_size_mismatch")
        source_flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        digest = hashlib.sha256()
        copied = 0
        with contextlib.ExitStack() as stack:
            source_descriptor = os.open(path, source_flags)
            source = stack.enter_context(os.fdopen(source_descriptor, "rb"))
            target_flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
            target_flags |= getattr(os, "O_NOFOLLOW", 0)
            target_descriptor = os.open(target, target_flags, 0o500)
            target_metadata = os.fstat(target_descriptor)
            target_created = (target_metadata.st_dev, target_metadata.st_ino)
            destination = stack.enter_context(os.fdopen(target_descriptor, "wb"))
            opened = os.fstat(source.fileno())
            source_identity = _file_identity(before)
            if (
                not stat.S_ISREG(opened.st_mode)
                or _file_identity(opened) != source_identity
            ):
                raise BrowserFailure("browser_binary_changed_before_copy")
            while chunk := source.read(1024 * 1024):
                copied += len(chunk)
                if copied > expected_size:
                    raise BrowserFailure("browser_binary_changed_during_copy")
                digest.update(chunk)
                destination.write(chunk)
            destination.flush()
            os.fsync(destination.fileno())
            os.fchmod(destination.fileno(), 0o500)
            finished_source = os.fstat(source.fileno())
            finished_target = os.fstat(destination.fileno())
        final_source = path.lstat()
        if (
            copied != expected_size
            or digest.hexdigest() != expected_digest
            or _file_identity(finished_source) != source_identity
            or _file_identity(final_source) != source_identity
            or not stat.S_ISREG(finished_target.st_mode)
            or finished_target.st_size != expected_size
            or stat.S_IMODE(finished_target.st_mode) != 0o500
        ):
            raise BrowserFailure("browser_binary_changed_during_copy")
        if _sha256(target, expected_size) != expected_digest:
            raise BrowserFailure("browser_executable_digest_mismatch")
        prepared = target.lstat()
        record = _prepared_executable_record(target, prepared, identity)
        _validate_prepared_executable(record, path, identity)
        version_descriptor = _open_prepared_descriptor(record, path, identity)
        with os.fdopen(version_descriptor, "rb") as version_source:
            _verify_browser_version(
                target,
                identity,
                descriptor=version_source.fileno(),
                record=record,
            )
        _validate_prepared_executable(record, path, identity)
        success = True
        return target, record
    except BrowserFailure:
        raise
    except OSError as error:
        raise BrowserFailure("browser_executable_preparation_failed") from error
    finally:
        if target_created is not None:
            try:
                current = target.lstat()
            except FileNotFoundError:
                pass
            except OSError as error:
                raise BrowserFailure("browser_executable_cleanup_refused") from error
            else:
                if not success:
                    if (current.st_dev, current.st_ino) != target_created:
                        raise BrowserFailure("browser_executable_cleanup_refused")
                    target.unlink()


def _open_prepared_descriptor(
    record: dict[str, Any], configured_path: pathlib.Path, identity: dict[str, Any]
) -> int:
    candidate = _validate_prepared_executable(record, configured_path, identity)
    try:
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(candidate, flags)
        opened = os.fstat(descriptor)
    except OSError as error:
        raise BrowserFailure("browser_executable_identity_changed") from error
    if (
        opened.st_dev != record["device"]
        or opened.st_ino != record["inode"]
        or stat.S_IMODE(opened.st_mode) != record["mode"]
        or opened.st_size != record["size_bytes"]
        or opened.st_mtime_ns != record["mtime_ns"]
        or opened.st_ctime_ns != record["ctime_ns"]
    ):
        os.close(descriptor)
        raise BrowserFailure("browser_executable_identity_changed")
    return descriptor


def _descriptor_executable_path(descriptor: int, record: dict[str, Any]) -> str | None:
    """Return Linux's exact open-file exec path; private-copy exec is the fallback."""

    if not sys.platform.startswith("linux"):
        return None
    candidate = pathlib.Path(f"/proc/self/fd/{descriptor}")
    try:
        observed = candidate.stat()
        opened = os.fstat(descriptor)
    except OSError as error:
        raise BrowserFailure("browser_descriptor_exec_unavailable") from error
    if (
        observed.st_dev != record["device"]
        or observed.st_ino != record["inode"]
        or _file_identity(observed) != _file_identity(opened)
    ):
        raise BrowserFailure("browser_descriptor_exec_identity_mismatch")
    return str(candidate)


def _state_dir() -> pathlib.Path:
    raw = os.environ.get("WORLDSTREAM_CDP_STATE_DIR", "")
    if not raw:
        raise BrowserFailure("browser_state_directory_missing")
    path = pathlib.Path(raw)
    try:
        mode = path.lstat().st_mode
    except OSError as error:
        raise BrowserFailure("browser_state_directory_unavailable") from error
    if stat.S_ISLNK(mode) or not stat.S_ISDIR(mode) or stat.S_IMODE(mode) & 0o077:
        raise BrowserFailure("browser_state_directory_unsafe")
    return path


def _state_path() -> pathlib.Path:
    return _state_dir() / "browser-state.json"


def _init_script_path() -> pathlib.Path:
    return _state_dir() / "browser-init.js"


def _write_init_script(script: str) -> None:
    content = script.encode("utf-8")
    if not content or len(content) > MAX_CONTROL_BYTES or b"\x00" in content:
        raise BrowserFailure("browser_init_script_invalid")
    path = _init_script_path()
    try:
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(path, flags, 0o600)
        with os.fdopen(descriptor, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
    except OSError as error:
        path.unlink(missing_ok=True)
        raise BrowserFailure("browser_init_script_unavailable") from error


def _load_init_script() -> str:
    path = _init_script_path()
    try:
        before = path.lstat()
        if (
            stat.S_ISLNK(before.st_mode)
            or not stat.S_ISREG(before.st_mode)
            or stat.S_IMODE(before.st_mode) != 0o600
            or before.st_size <= 0
            or before.st_size > MAX_CONTROL_BYTES
        ):
            raise BrowserFailure("browser_init_script_invalid")
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(path, flags)
        with os.fdopen(descriptor, "rb") as source:
            opened = os.fstat(source.fileno())
            content = source.read(MAX_CONTROL_BYTES + 1)
            after = os.fstat(source.fileno())
        identity = (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
        if (
            not stat.S_ISREG(opened.st_mode)
            or identity
            != (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns)
            or identity
            != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
            or len(content) != before.st_size
            or len(content) > MAX_CONTROL_BYTES
        ):
            raise BrowserFailure("browser_init_script_changed_during_read")
        return content.decode("utf-8")
    except (OSError, UnicodeError) as error:
        raise BrowserFailure("browser_init_script_invalid") from error


def _write_state(value: dict[str, Any]) -> None:
    path = _state_path()
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump(value, output, sort_keys=True, separators=(",", ":"))
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
    except (BrowserFailure, OSError, TimeoutError):
        path.unlink(missing_ok=True)
        raise


def _load_state() -> dict[str, Any]:
    path = _state_path()
    try:
        mode = path.lstat().st_mode
        before = path.stat()
        if before.st_size <= 0 or before.st_size > MAX_CONTROL_BYTES:
            raise BrowserFailure("browser_state_invalid")
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(path, flags)
        with os.fdopen(descriptor, "rb") as source:
            opened = os.fstat(source.fileno())
            content = source.read(MAX_CONTROL_BYTES + 1)
            after = os.fstat(source.fileno())
        value = json.loads(content)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        raise BrowserFailure("browser_state_invalid") from error
    if (
        stat.S_ISLNK(mode)
        or not stat.S_ISREG(mode)
        or stat.S_IMODE(mode) != 0o600
        or not stat.S_ISREG(opened.st_mode)
        or (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
        != (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns)
        or (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns)
        != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
        or len(content) != before.st_size
        or len(content) > MAX_CONTROL_BYTES
        or not isinstance(value, dict)
        or set(value) != STATE_FIELDS
        or value.get("schema") != "worldstream/cdp-browser-state/v1"
        or type(value.get("pid")) is not int
        or value["pid"] <= 1
        or type(value.get("port")) is not int
        or not (1024 <= value["port"] <= 65535)
        or not isinstance(value.get("surface_ref"), str)
        or SURFACE.fullmatch(value["surface_ref"]) is None
        or not isinstance(value.get("websocket_url"), str)
        or not value["websocket_url"].startswith(
            f"ws://127.0.0.1:{value['port']}/devtools/page/"
        )
        or not isinstance(value.get("browser"), dict)
    ):
        raise BrowserFailure("browser_state_invalid")
    configured_path, observed = _browser_configuration()
    if value["browser"] != observed:
        raise BrowserFailure("browser_state_identity_mismatch")
    _validate_prepared_executable(value["executable"], configured_path, observed)
    try:
        os.kill(value["pid"], 0)
    except OSError as error:
        raise BrowserFailure("browser_process_unavailable") from error
    return value


def _http_json(url: str, *, method: str = "GET") -> Any:
    request = urllib.request.Request(url, method=method)
    try:
        with urllib.request.urlopen(request, timeout=2) as response:
            content = response.read(MAX_CONTROL_BYTES + 1)
    except (OSError, urllib.error.URLError) as error:
        raise BrowserFailure("browser_debug_endpoint_unavailable") from error
    if not content or len(content) > MAX_CONTROL_BYTES:
        raise BrowserFailure("browser_debug_response_invalid")
    try:
        return json.loads(content)
    except (UnicodeError, ValueError, json.JSONDecodeError) as error:
        raise BrowserFailure("browser_debug_response_invalid") from error


def _free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def _open() -> dict[str, Any]:
    configured_path, identity = _browser_configuration()
    state_dir = _state_dir()
    if _state_path().exists():
        raise BrowserFailure("browser_state_already_exists")
    user_data = state_dir / "user-data"
    user_data.mkdir(mode=0o700)
    port = _free_port()
    executable_path, executable = _prepare_browser_executable(configured_path, identity)
    process: subprocess.Popen[bytes] | None = None
    try:
        with contextlib.ExitStack() as resources:
            stdout = resources.enter_context((state_dir / "browser.stdout").open("xb"))
            stderr = resources.enter_context((state_dir / "browser.stderr").open("xb"))
            executable_descriptor = _open_prepared_descriptor(
                executable, configured_path, identity
            )
            executable_source = resources.enter_context(
                os.fdopen(executable_descriptor, "rb")
            )
            command = [
                str(executable_path),
                "--headless",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-default-apps",
                "--disable-extensions",
                "--disable-features=Translate,MediaRouter,OptimizationHints",
                "--disable-sync",
                "--metrics-recording-only",
                "--no-first-run",
                f"--remote-debugging-port={port}",
                "--remote-allow-origins=*",
                f"--user-data-dir={user_data}",
                "about:blank",
            ]
            descriptor_path = _descriptor_executable_path(
                executable_source.fileno(), executable
            )
            try:
                if descriptor_path is None:
                    process = subprocess.Popen(
                        command,
                        stdin=subprocess.DEVNULL,
                        stdout=stdout,
                        stderr=stderr,
                        start_new_session=True,
                    )
                else:
                    process = subprocess.Popen(
                        command,
                        executable=descriptor_path,
                        pass_fds=(executable_source.fileno(),),
                        stdin=subprocess.DEVNULL,
                        stdout=stdout,
                        stderr=stderr,
                        start_new_session=True,
                    )
            except OSError as error:
                raise BrowserFailure("browser_launch_failed") from error
            after_launch = os.fstat(executable_source.fileno())
            if (
                after_launch.st_dev != executable["device"]
                or after_launch.st_ino != executable["inode"]
                or stat.S_IMODE(after_launch.st_mode) != executable["mode"]
                or after_launch.st_size != executable["size_bytes"]
                or after_launch.st_mtime_ns != executable["mtime_ns"]
                or after_launch.st_ctime_ns != executable["ctime_ns"]
            ):
                raise BrowserFailure("browser_executable_changed_during_launch")
            _validate_prepared_executable(executable, configured_path, identity)
        deadline = time.monotonic() + 20
        version_body: Any = None
        targets: Any = None
        while time.monotonic() < deadline:
            assert process is not None
            if process.poll() is not None:
                raise BrowserFailure("browser_exited_before_debug_ready")
            try:
                version_body = _http_json(f"http://127.0.0.1:{port}/json/version")
                targets = _http_json(f"http://127.0.0.1:{port}/json/list")
                break
            except BrowserFailure:
                time.sleep(0.1)
        if not isinstance(version_body, dict) or not isinstance(targets, list):
            raise BrowserFailure("browser_debug_ready_timeout")
        expected_product = "HeadlessChrome/" + identity["version"]
        if version_body.get("Browser") != expected_product:
            raise BrowserFailure("browser_debug_product_mismatch")
        pages = [
            item
            for item in targets
            if isinstance(item, dict)
            and item.get("type") == "page"
            and isinstance(item.get("webSocketDebuggerUrl"), str)
        ]
        if len(pages) != 1:
            raise BrowserFailure("browser_page_target_invalid")
        _validate_prepared_executable(executable, configured_path, identity)
        surface = (
            "surface:"
            + hashlib.sha256(pages[0]["webSocketDebuggerUrl"].encode()).hexdigest()[:32]
        )
        state = {
            "schema": "worldstream/cdp-browser-state/v1",
            "pid": process.pid,
            "port": port,
            "surface_ref": surface,
            "websocket_url": pages[0]["webSocketDebuggerUrl"],
            "browser": identity,
            "executable": executable,
        }
        _write_state(state)
        return {"surface_ref": surface, "browser": identity}
    except BaseException:
        if process is not None:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except OSError:
                pass
        _remove_prepared_executable(executable)
        raise


class Cdp:
    def __init__(self, websocket_url: str) -> None:
        self.socket = connect(
            websocket_url,
            open_timeout=5,
            close_timeout=1,
            max_size=MAX_CONTROL_BYTES,
        )
        self.sequence = 0

    def close(self) -> None:
        self.socket.close()

    def request(
        self, method: str, params: dict[str, Any] | None = None, *, timeout: float = 20
    ) -> dict[str, Any]:
        self.sequence += 1
        request_id = self.sequence
        self.socket.send(
            json.dumps(
                {"id": request_id, "method": method, "params": params or {}},
                separators=(",", ":"),
            )
        )
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            message = self.socket.recv(timeout=max(0.01, deadline - time.monotonic()))
            if (
                not isinstance(message, str)
                or len(message.encode()) > MAX_CONTROL_BYTES
            ):
                raise BrowserFailure("browser_cdp_response_invalid")
            try:
                value = json.loads(message)
            except (ValueError, json.JSONDecodeError) as error:
                raise BrowserFailure("browser_cdp_response_invalid") from error
            if isinstance(value, dict) and value.get("id") == request_id:
                if "error" in value or not isinstance(value.get("result"), dict):
                    raise BrowserFailure("browser_cdp_command_failed")
                return value["result"]
        raise BrowserFailure("browser_cdp_command_timeout")

    def evaluate(self, expression: str) -> Any:
        result = self.request(
            "Runtime.evaluate",
            {
                "expression": expression,
                "awaitPromise": True,
                "returnByValue": True,
                "userGesture": True,
            },
        )
        if "exceptionDetails" in result:
            raise BrowserFailure("browser_javascript_failed")
        remote = result.get("result")
        if not isinstance(remote, dict):
            raise BrowserFailure("browser_javascript_result_invalid")
        return remote.get("value")


def _with_cdp() -> tuple[dict[str, Any], Cdp]:
    state = _load_state()
    try:
        return state, Cdp(state["websocket_url"])
    except OSError as error:
        raise BrowserFailure("browser_cdp_connection_failed") from error


def _surface(arguments: list[str], state: dict[str, Any]) -> list[str]:
    if not arguments or arguments[0] != state["surface_ref"]:
        raise BrowserFailure("browser_surface_identity_mismatch")
    return arguments[1:]


def _print(value: Any) -> None:
    if isinstance(value, str):
        print(value)
    else:
        print(json.dumps(value, sort_keys=True, separators=(",", ":")))


def _selector_expression(selector: str, action: str) -> str:
    encoded = json.dumps(selector)
    return f"(() => {{ const element=document.querySelector({encoded}); if (!element) throw new Error('selector'); {action} }})()"


def _browser(arguments: list[str]) -> int:
    state, cdp = _with_cdp()
    try:
        arguments = _surface(arguments, state)
        if not arguments:
            raise BrowserFailure("browser_operation_missing")
        operation = arguments.pop(0)
        if operation == "addinitscript" and arguments[:1] == ["--script"]:
            script = arguments[1] if len(arguments) == 2 else ""
            diagnostics = (
                "(()=>{const diagnostics=window.__WORLDSTREAM_BROWSER_DIAGNOSTICS__=[];"
                "const record=(entry)=>{diagnostics.push(entry);if(diagnostics.length>100)diagnostics.shift();};"
                "for(const method of ['warn','error']){const original=console[method].bind(console);console[method]=(...values)=>{record({kind:'console.'+method,message:values.map((value)=>String(value)).join(' ').slice(0,1024)});return original(...values);};}"
                "addEventListener('error',(event)=>record({kind:'error',message:String(event.message||'').slice(0,1024),source:String(event.filename||'').slice(0,1024),line:Number(event.lineno||0),column:Number(event.colno||0),stack:String(event.error&&event.error.stack||'').slice(0,4096)}));"
                "addEventListener('unhandledrejection',(event)=>record({kind:'rejection',message:String(event.reason||'').slice(0,1024),stack:String(event.reason&&event.reason.stack||'').slice(0,4096)}));})();"
            )
            _write_init_script(diagnostics + script)
            print("ok")
        elif operation == "navigate" and len(arguments) == 1:
            script = _load_init_script()
            cdp.request("Page.enable")
            installed = cdp.request(
                "Page.addScriptToEvaluateOnNewDocument", {"source": script}
            )
            if not isinstance(installed.get("identifier"), str):
                raise BrowserFailure("browser_init_script_install_failed")
            result = cdp.request("Page.navigate", {"url": arguments[0]})
            if result.get("errorText"):
                raise BrowserFailure("browser_navigation_failed")
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                if cdp.evaluate("document.readyState === 'complete'") is True:
                    break
                time.sleep(0.05)
            else:
                raise BrowserFailure("browser_navigation_load_timeout")
            _init_script_path().unlink(missing_ok=True)
            print("ok")
        elif operation == "wait":
            timeout_ms = 30_000
            if "--timeout-ms" in arguments:
                index = arguments.index("--timeout-ms")
                try:
                    timeout_ms = int(arguments[index + 1])
                except (IndexError, ValueError) as error:
                    raise BrowserFailure("browser_wait_timeout_invalid") from error
                del arguments[index : index + 2]
            if not (1 <= timeout_ms <= 300_000):
                raise BrowserFailure("browser_wait_timeout_invalid")
            if arguments[:1] == ["--load-state"] and arguments[1:] == ["complete"]:
                expression = "document.readyState === 'complete'"
            elif arguments[:1] == ["--text"] and len(arguments) == 2:
                expression = (
                    "Boolean(document.body && document.body.innerText.includes("
                    + json.dumps(arguments[1])
                    + "))"
                )
            elif arguments[:1] == ["--selector"] and len(arguments) == 2:
                expression = (
                    "document.querySelector(" + json.dumps(arguments[1]) + ") !== null"
                )
            else:
                raise BrowserFailure("browser_wait_contract_invalid")
            deadline = time.monotonic() + timeout_ms / 1000
            while time.monotonic() < deadline:
                if cdp.evaluate(expression) is True:
                    print("ok")
                    return 0
                time.sleep(0.1)
            raise BrowserFailure("browser_wait_timeout")
        elif (
            operation == "eval"
            and arguments[:1] == ["--script"]
            and len(arguments) == 2
        ):
            _print(cdp.evaluate(arguments[1]))
        elif operation == "fill" and len(arguments) == 2:
            selector, value = arguments
            action = (
                "const prototype=Object.getPrototypeOf(element);"
                "const setter=Object.getOwnPropertyDescriptor(prototype,'value')?.set;"
                "if (!setter) throw new Error('not-fillable');"
                f"setter.call(element,{json.dumps(value)});"
                "element.dispatchEvent(new Event('input',{bubbles:true}));"
                "element.dispatchEvent(new Event('change',{bubbles:true}));"
                "return true;"
            )
            _print(cdp.evaluate(_selector_expression(selector, action)))
        elif operation == "click" and len(arguments) == 1:
            _print(
                cdp.evaluate(
                    _selector_expression(arguments[0], "element.click(); return true;")
                )
            )
        elif operation == "get" and arguments == ["text", "body"]:
            _print(cdp.evaluate("document.body?.innerText || ''"))
        elif operation == "get" and arguments == ["url"]:
            _print(cdp.evaluate("location.href"))
        elif operation == "storage" and len(arguments) == 2 and arguments[1] == "get":
            storage = {"local": "localStorage", "session": "sessionStorage"}.get(
                arguments[0]
            )
            if storage is None:
                raise BrowserFailure("browser_storage_contract_invalid")
            _print(
                cdp.evaluate(
                    f"JSON.stringify(Object.fromEntries(Object.entries({storage})))"
                )
            )
        elif operation in {"errors", "console"} and arguments == ["list"]:
            _print(
                cdp.evaluate(
                    "JSON.stringify(window.__WORLDSTREAM_BROWSER_DIAGNOSTICS__ || [])"
                )
            )
        else:
            raise BrowserFailure("browser_operation_unsupported")
    finally:
        cdp.close()
    return 0


def _close(arguments: list[str]) -> int:
    state = _load_state()
    surface = ""
    for index, argument in enumerate(arguments):
        if argument == "--surface" and index + 1 < len(arguments):
            surface = arguments[index + 1]
    if surface != state["surface_ref"]:
        raise BrowserFailure("browser_surface_identity_mismatch")
    try:
        cdp = Cdp(state["websocket_url"])
        try:
            cdp.request("Browser.close", timeout=5)
        finally:
            cdp.close()
    except (BrowserFailure, OSError, TimeoutError):
        try:
            os.killpg(state["pid"], signal.SIGTERM)
        except OSError:
            pass
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        try:
            os.kill(state["pid"], 0)
        except OSError:
            break
        time.sleep(0.05)
    else:
        try:
            os.killpg(state["pid"], signal.SIGKILL)
        except OSError:
            pass
    _remove_prepared_executable(state["executable"])
    _state_path().unlink(missing_ok=True)
    _init_script_path().unlink(missing_ok=True)
    print("ok")
    return 0


def main() -> int:
    arguments = sys.argv[1:]
    json_output = False
    if arguments[:1] == ["--json"]:
        json_output = True
        arguments.pop(0)
    try:
        if arguments[:2] == ["browser", "open"]:
            if len(arguments) < 3 or arguments[2] != "about:blank":
                raise BrowserFailure("browser_open_contract_invalid")
            result = _open()
            _print(result if json_output else result["surface_ref"])
            return 0
        if arguments[:1] == ["browser"]:
            return _browser(arguments[1:])
        if arguments[:1] == ["close-surface"]:
            return _close(arguments[1:])
        raise BrowserFailure("browser_command_unsupported")
    except BrowserFailure as error:
        print(f"blocked:{error}", file=sys.stderr)
        return 2
    except Exception as error:  # noqa: BLE001 - fail closed without leaking values
        print(
            f"blocked:browser_adapter_internal:{type(error).__name__}", file=sys.stderr
        )
        return 2


if __name__ == "__main__":
    raise SystemExit(main())

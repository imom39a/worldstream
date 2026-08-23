"""Adversarial boundaries for pinned browser installation and CDP identity."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import stat
import subprocess
import sys
import zipfile
from pathlib import Path
from unittest import mock

import pytest

ROOT = Path(__file__).resolve().parents[1]


def load(name: str, relative: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


INSTALLER = load(
    "worldstream_pinned_browser_installer_test", "scripts/install-pinned-browser.py"
)
CDP = load("worldstream_cdp_browser_test", "scripts/cdp-browser.py")


def browser_story_control_helper() -> str:
    source = (ROOT / "web/console/live-browser-story.sh").read_text(encoding="utf-8")
    marker = 'bounded_control_read() {\n  "$python_bin" - "$1" "$2" <<\'PY\'\n'
    return source.split(marker, 1)[1].split("\nPY\n}\n", 1)[0]


def archive_args(tmp_path: Path, members: list[tuple[zipfile.ZipInfo, bytes]]):
    archive = tmp_path / "browser.zip"
    with zipfile.ZipFile(archive, "w") as output:
        for info, content in members:
            output.writestr(info, content)
    binary = members[0][1]
    return argparse.Namespace(
        archive=str(archive),
        archive_size=archive.stat().st_size,
        archive_sha256=hashlib.sha256(archive.read_bytes()).hexdigest(),
        output=str(tmp_path / "installed"),
        binary_relative="browser/chrome-headless-shell",
        binary_size=len(binary),
        binary_sha256=hashlib.sha256(binary).hexdigest(),
    )


def regular_info(name: str) -> zipfile.ZipInfo:
    info = zipfile.ZipInfo(name)
    info.create_system = 3
    info.external_attr = (stat.S_IFREG | 0o755) << 16
    return info


def test_installer_verifies_exact_archive_and_binary(tmp_path: Path):
    binary = b"pinned-browser-fixture"
    args = archive_args(
        tmp_path,
        [
            (regular_info("browser/chrome-headless-shell"), binary),
            (regular_info("browser/resources.pak"), b"resource"),
        ],
    )
    result = INSTALLER.install(args)

    assert result["status"] == "pass"
    installed = tmp_path / "installed/browser/chrome-headless-shell"
    assert installed.read_bytes() == binary
    assert installed.stat().st_mode & stat.S_IXUSR


def test_installer_rejects_archive_path_replacement_after_verification(
    tmp_path: Path, monkeypatch
):
    binary = b"pinned-browser-fixture"
    args = archive_args(
        tmp_path,
        [(regular_info("browser/chrome-headless-shell"), binary)],
    )
    replacement = tmp_path / "replacement.zip"
    with zipfile.ZipFile(replacement, "w") as output:
        output.writestr(
            regular_info("browser/chrome-headless-shell"), b"substituted-browser"
        )
    original_zipfile = INSTALLER.zipfile.ZipFile
    swapped = False

    def replace_archive(file, *zip_args, **zip_kwargs):
        nonlocal swapped
        if not swapped:
            swapped = True
            replacement.replace(Path(args.archive))
        return original_zipfile(file, *zip_args, **zip_kwargs)

    monkeypatch.setattr(INSTALLER.zipfile, "ZipFile", replace_archive)
    with pytest.raises(
        INSTALLER.InstallFailure, match="browser_archive_identity_mismatch"
    ):
        INSTALLER.install(args)
    assert swapped


@pytest.mark.parametrize("kind", ["traversal", "symlink", "wrong-digest", "wrong-size"])
def test_installer_rejects_adversarial_archives(tmp_path: Path, kind: str):
    binary = b"pinned-browser-fixture"
    first = regular_info("browser/chrome-headless-shell")
    second = regular_info("browser/resource")
    if kind == "traversal":
        second = regular_info("browser/../escape")
    elif kind == "symlink":
        second = zipfile.ZipInfo("browser/link")
        second.create_system = 3
        second.external_attr = (stat.S_IFLNK | 0o777) << 16
    args = archive_args(tmp_path, [(first, binary), (second, b"value")])
    if kind == "wrong-digest":
        args.archive_sha256 = "0" * 64
    elif kind == "wrong-size":
        args.archive_size += 1

    with pytest.raises(INSTALLER.InstallFailure):
        INSTALLER.install(args)


def browser_environment(monkeypatch, binary: Path, *, size: int) -> None:
    values = {
        "WORLDSTREAM_BROWSER_BINARY": str(binary),
        "WORLDSTREAM_BROWSER_VERSION": "152.0.7977.54",
        "WORLDSTREAM_BROWSER_SHA256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "WORLDSTREAM_BROWSER_SIZE_BYTES": str(size),
        "WORLDSTREAM_BROWSER_ARCHIVE_URL": "https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.54/linux64/browser.zip",
        "WORLDSTREAM_BROWSER_ARCHIVE_SHA256": "1" * 64,
        "WORLDSTREAM_BROWSER_ARCHIVE_SIZE_BYTES": "1234",
    }
    for key, value in values.items():
        monkeypatch.setenv(key, value)


def test_cdp_binary_hash_is_size_bounded_and_fd_stable(tmp_path: Path, monkeypatch):
    binary = tmp_path / "browser"
    binary.write_bytes(b"fixture")
    binary.chmod(0o700)
    browser_environment(monkeypatch, binary, size=binary.stat().st_size + 1)
    with (
        mock.patch.object(CDP.subprocess, "run") as run,
        pytest.raises(CDP.BrowserFailure, match="browser_binary_size_mismatch"),
    ):
        CDP.browser_identity()
    run.assert_not_called()


def test_cdp_binary_growth_is_stopped_at_the_expected_size(tmp_path: Path):
    binary = tmp_path / "browser"
    binary.write_bytes(b"oversized")
    observed = binary.lstat()
    fields = list(observed)
    fields[6] = observed.st_size - 1
    stale_metadata = CDP.os.stat_result(fields)
    with (
        mock.patch.object(CDP.pathlib.Path, "lstat", return_value=stale_metadata),
        pytest.raises(CDP.BrowserFailure, match="browser_binary_changed_during_hash"),
    ):
        CDP._sha256(binary, observed.st_size - 1)


def test_cdp_identity_binds_exact_observed_version(tmp_path: Path, monkeypatch):
    binary = tmp_path / "browser"
    binary.write_bytes(b"fixture")
    binary.chmod(0o700)
    browser_environment(monkeypatch, binary, size=binary.stat().st_size)
    completed = subprocess.CompletedProcess(
        [str(binary), "--version"],
        0,
        stdout="Google Chrome for Testing 152.0.7977.54\n",
        stderr="",
    )
    with mock.patch.object(CDP.subprocess, "run", return_value=completed):
        _path, identity = CDP.browser_identity()
    assert identity["version_output"] == "Google Chrome for Testing 152.0.7977.54"
    assert identity["size_bytes"] == len(b"fixture")
    assert identity["distribution"]["size_bytes"] == 1234


def test_cdp_prepares_a_private_exact_executable_copy(tmp_path: Path, monkeypatch):
    binary = tmp_path / "browser"
    binary.write_text(
        "#!/bin/sh\nprintf 'Google Chrome for Testing 152.0.7977.54\\n'\n",
        encoding="utf-8",
    )
    binary.chmod(0o700)
    browser_environment(monkeypatch, binary, size=binary.stat().st_size)
    configured, identity = CDP._browser_configuration()
    prepared, record = CDP._prepare_browser_executable(configured, identity)

    replacement = tmp_path / "replacement"
    replacement.write_bytes(b"hostile")
    replacement.chmod(0o700)
    replacement.replace(binary)
    assert prepared.read_text(encoding="utf-8").startswith("#!/bin/sh")
    assert CDP._validate_prepared_executable(record, configured, identity) == prepared
    assert prepared.stat().st_ino != binary.stat().st_ino
    CDP._remove_prepared_executable(record)
    assert not prepared.exists()


@pytest.mark.skipif(not sys.platform.startswith("linux"), reason="Linux /proc fd exec")
def test_cdp_linux_exec_uses_the_verified_open_descriptor(tmp_path: Path, monkeypatch):
    binary = tmp_path / "browser"
    binary.write_text(
        "#!/bin/sh\nprintf 'Google Chrome for Testing 152.0.7977.54\\n'\n",
        encoding="utf-8",
    )
    binary.chmod(0o700)
    browser_environment(monkeypatch, binary, size=binary.stat().st_size)
    configured, identity = CDP._browser_configuration()
    prepared, record = CDP._prepare_browser_executable(configured, identity)
    descriptor = CDP._open_prepared_descriptor(record, configured, identity)
    try:
        descriptor_path = CDP._descriptor_executable_path(descriptor, record)
        assert descriptor_path is not None
        substitute = tmp_path / "substitute"
        substitute.write_text("#!/bin/sh\nprintf 'hostile\\n'\n", encoding="utf-8")
        substitute.chmod(0o500)
        substitute.replace(prepared)
        completed = subprocess.run(
            [str(prepared), "--version"],
            executable=descriptor_path,
            pass_fds=(descriptor,),
            capture_output=True,
            text=True,
            check=False,
        )
    finally:
        CDP.os.close(descriptor)
        prepared.unlink(missing_ok=True)
    assert completed.returncode == 0
    assert completed.stdout == "Google Chrome for Testing 152.0.7977.54\n"


def test_cdp_rejects_source_path_replacement_during_stable_copy(
    tmp_path: Path, monkeypatch
):
    binary = tmp_path / "browser"
    binary.write_bytes(b"fixture")
    binary.chmod(0o700)
    replacement = tmp_path / "replacement"
    replacement.write_bytes(b"hostile")
    replacement.chmod(0o700)
    browser_environment(monkeypatch, binary, size=binary.stat().st_size)
    configured, identity = CDP._browser_configuration()
    real_fsync = CDP.os.fsync
    replaced = False

    def replace_during_copy(descriptor: int) -> None:
        nonlocal replaced
        real_fsync(descriptor)
        if not replaced:
            replaced = True
            replacement.replace(binary)

    with (
        mock.patch.object(CDP.os, "fsync", side_effect=replace_during_copy),
        mock.patch.object(CDP.subprocess, "run") as version,
        pytest.raises(CDP.BrowserFailure, match="browser_binary_changed_during_copy"),
    ):
        CDP._prepare_browser_executable(configured, identity)
    version.assert_not_called()
    assert replaced
    assert not list(tmp_path.glob(".worldstream-verified-*"))


def test_cdp_rejects_prepared_executable_inode_replacement(tmp_path: Path, monkeypatch):
    binary = tmp_path / "browser"
    binary.write_bytes(b"fixture")
    binary.chmod(0o700)
    browser_environment(monkeypatch, binary, size=binary.stat().st_size)
    configured, identity = CDP._browser_configuration()
    completed = subprocess.CompletedProcess(
        [str(binary), "--version"],
        0,
        stdout="Google Chrome for Testing 152.0.7977.54\n",
        stderr="",
    )
    with mock.patch.object(CDP.subprocess, "run", return_value=completed):
        prepared, record = CDP._prepare_browser_executable(configured, identity)

    substitute = tmp_path / "substitute"
    substitute.write_bytes(b"fixture")
    substitute.chmod(0o500)
    substitute.replace(prepared)
    with pytest.raises(CDP.BrowserFailure, match="browser_executable_identity_changed"):
        CDP._validate_prepared_executable(record, configured, identity)


def test_cdp_state_is_rejected_before_unbounded_read(tmp_path: Path, monkeypatch):
    state = tmp_path / "state"
    state.mkdir(mode=0o700)
    (state / "browser-state.json").write_bytes(b"x" * (CDP.MAX_CONTROL_BYTES + 1))
    monkeypatch.setenv("WORLDSTREAM_CDP_STATE_DIR", str(state))
    with pytest.raises(CDP.BrowserFailure, match="browser_state_invalid"):
        CDP._load_state()


def test_browser_story_control_reader_is_bounded_and_nofollow(tmp_path: Path):
    helper = browser_story_control_helper()
    valid = tmp_path / "valid.json"
    valid.write_text('{"ok":true}\n', encoding="utf-8")
    completed = subprocess.run(
        [sys.executable, "-c", helper, "compact-json", str(valid)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0
    assert completed.stdout == '{"ok":true}\n'

    oversized = tmp_path / "oversized.json"
    with oversized.open("wb") as output:
        output.truncate(8 * 1024 * 1024 + 1)
    linked = tmp_path / "linked.json"
    linked.symlink_to(valid)
    for path in (oversized, linked):
        rejected = subprocess.run(
            [sys.executable, "-c", helper, "compact-json", str(path)],
            capture_output=True,
            text=True,
            check=False,
        )
        assert rejected.returncode != 0
        assert rejected.stdout == ""


def test_release_browser_never_disables_sandbox_and_records_console_channels():
    source = (ROOT / "scripts/cdp-browser.py").read_text(encoding="utf-8")
    story = (ROOT / "web/console/live-browser-story.sh").read_text(encoding="utf-8")
    assert "--no-sandbox" not in source
    assert "console[method]" in source
    assert "if(diagnostics.length>100)diagnostics.shift()" in source
    assert "browser-diagnostics.txt" in story
    assert "browser_diagnostics_not_clean" in story
    assert "Warnings are release-blocking too" in story
    assert "errors list" not in story
    assert "Path(path).read_text" not in story
    assert "json.load(open" not in story
    assert 'result="$(<"' not in story
    assert "def identity(metadata)" in story
    assert "bounded_control_read" in story
    assert "MAX_CONTROL_BYTES + 1" in story
    assert "package_python_identity_mismatch" in story
    assert "def stable_regular_file" in story
    assert 'getattr(os, "O_NOFOLLOW", 0)' in story
    assert "source.read(MAX_CONTROL_BYTES + 1)" in story
    assert "_prepare_browser_executable" in source
    assert "_open_prepared_descriptor" in source
    assert "_descriptor_executable_path" in source
    assert 'root / "bin/worldstreamd"' in story
    assert 'root / "ui"' in story
    assert 'root / "sdk/python/src"' in story
    assert 'root / "examples/heist"' in story

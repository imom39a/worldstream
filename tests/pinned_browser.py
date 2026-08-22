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


def test_cdp_state_is_rejected_before_unbounded_read(tmp_path: Path, monkeypatch):
    state = tmp_path / "state"
    state.mkdir(mode=0o700)
    (state / "browser-state.json").write_bytes(b"x" * (CDP.MAX_CONTROL_BYTES + 1))
    monkeypatch.setenv("WORLDSTREAM_CDP_STATE_DIR", str(state))
    with pytest.raises(CDP.BrowserFailure, match="browser_state_invalid"):
        CDP._load_state()


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
    assert 'root / "bin/worldstreamd"' in story
    assert 'root / "ui"' in story
    assert 'root / "sdk/python/src"' in story
    assert 'root / "examples/heist"' in story

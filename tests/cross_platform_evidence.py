#!/usr/bin/env python3
"""Regression checks for the fail-closed cross-platform evidence lane."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/cross-platform-evidence.sh"
DOC = ROOT / "docs/agents/imo-59-cross-platform-luna.md"


def run(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["bash", str(SCRIPT), *args],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
        env=os.environ.copy(),
    )


def main() -> None:
    text = SCRIPT.read_text(encoding="utf-8")
    assert "linux/amd64" in text
    assert "windows_native_status=INCOMPLETE" in text
    assert "cross_compilation_is_not_native_evidence=true" in text
    assert "finished_digests=release-manifest.json" in text
    assert "digest_location_release-manifest.json" in text
    assert "release_evidence=false" in text
    assert "docker buildx build --platform linux/amd64" in text
    assert "native_windows_host_required" in text
    assert "native_linux_x86_64_host_required" in text

    help_result = run("--help")
    assert help_result.returncode == 0, help_result
    assert "--no-docker" in help_result.stdout

    with_report = Path("/tmp/cross-platform-evidence-test-report.txt")
    result = run("--no-docker", "--report", str(with_report))
    assert result.returncode == 13, result
    report = with_report.read_text(encoding="utf-8")
    assert "status=INCOMPLETE" in report
    assert "docker_reason=explicitly_disabled" in report
    assert "windows_native_status=INCOMPLETE" in report
    assert "native_linux_status=INCOMPLETE" in report
    assert "release_evidence=false" in report
    assert "finished_digests=release-manifest.json" in report
    assert "digest_location_release-manifest.json" in report

    # The report is line-oriented by design so exact commands and outcomes can
    # be retained without pretending this diagnostic is a compatibility row.
    assert "command[package_linux_dry_run]=" in report
    assert "command[package_oci_dry_run]=" in report
    assert DOC.exists()
    document = DOC.read_text(encoding="utf-8")
    assert "cross-compilation" in document.lower()
    assert "Windows" in document
    assert "linux/amd64" in document
    print("cross-platform evidence contract: PASS")


if __name__ == "__main__":
    main()

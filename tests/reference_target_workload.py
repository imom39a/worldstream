"""Focused unit boundaries for the genuine frozen reference-target runner."""

from __future__ import annotations

import asyncio
import hashlib
import importlib.util
import os
import re
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/reference-target-workload.py"
NOFOLLOW_AVAILABLE = (
    isinstance(getattr(os, "O_NOFOLLOW", None), int) and os.O_NOFOLLOW != 0
)


def load_module():
    spec = importlib.util.spec_from_file_location(
        "worldstream_reference_target_workload_test", SCRIPT
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_recovery_fixture_count_is_exact_for_frozen_and_bounded_for_reduced():
    module = load_module()

    assert module.fixture_transition_count(module.frozen_profile()) == 100_000
    assert module.fixture_transition_count(module.reduced_profile()) == 20


def test_frozen_timeout_arithmetic_is_exact_and_cli_drift_fails_closed():
    module = load_module()
    arguments = SimpleNamespace(
        startup_timeout_seconds=module.FROZEN_STARTUP_TIMEOUT_SECONDS,
        shutdown_timeout_seconds=module.FROZEN_SHUTDOWN_TIMEOUT_SECONDS,
        fixture_setup_timeout_seconds=module.FROZEN_FIXTURE_SETUP_TIMEOUT_SECONDS,
    )

    module.validate_timeout_contract(arguments, module.frozen_profile())
    assert (
        module.FROZEN_FIXTURE_SETUP_TIMEOUT_SECONDS
        + module.frozen_profile()["sustained_transition_window_seconds"]
        + module.REFERENCE_JOB_MINIMUM_RESERVE_SECONDS
        == module.REFERENCE_JOB_HARD_SECONDS
    )

    arguments.fixture_setup_timeout_seconds += 1
    with pytest.raises(module.TargetFailure, match="timeout arguments drifted"):
        module.validate_timeout_contract(arguments, module.frozen_profile())


def test_reference_workflow_binds_four_hour_job_and_exact_source_revision():
    module = load_module()
    workflow = (ROOT / ".github/workflows/compatibility-gates.yml").read_text(
        encoding="utf-8"
    )

    def job_block(name: str) -> str:
        start = workflow.index(f"  {name}:\n")
        following = re.search(r"\n  [a-z][a-z0-9-]+:\n", workflow[start + 1 :])
        end = -1 if following is None else start + 1 + following.start()
        return workflow[start:] if end == -1 else workflow[start:end]

    reference_job = job_block("reference-performance-release")
    assert (
        f"    timeout-minutes: {module.REFERENCE_JOB_HARD_SECONDS // 60}\n"
        in reference_job
    )
    # The workflow wrapper includes checkout and tool installation. The gate
    # processes enforce their frozen execution deadlines independently, so the
    # wrapper must leave setup/teardown headroom instead of racing the contract.
    assert "    timeout-minutes: 20\n" in job_block("fast")
    assert "    timeout-minutes: 90\n" in job_block("native")
    assert "    timeout-minutes: 30\n" in job_block("macos-source")
    assert "    timeout-minutes: 300\n" in job_block("release-verify")
    assert workflow.count("  WORLDSTREAM_BUILD_REVISION: ${{ github.sha }}\n") == 1
    assert "--example reference_snapshot_tail_fixture" in reference_job
    assert '--snapshot-fixture-bin "$snapshot_fixture"' in reference_job
    assert "--fixture-setup-timeout-seconds" not in reference_job


@pytest.mark.skipif(
    not NOFOLLOW_AVAILABLE, reason="the frozen reference workload is Linux-only"
)
def test_fixture_report_reader_is_single_descriptor_and_bounded(
    tmp_path: Path, monkeypatch
):
    module = load_module()
    report = tmp_path / "fixture.json"
    original = b'{"schema":"fixture"}\n'
    report.write_bytes(original)

    assert module.stable_fixture_report_bytes(report) == original

    original_read = module.os.read
    mutated = False

    def mutate_after_read(descriptor: int, count: int) -> bytes:
        nonlocal mutated
        value = original_read(descriptor, count)
        if not mutated:
            mutated = True
            with report.open("ab") as output:
                output.write(b" ")
        return value

    monkeypatch.setattr(module.os, "read", mutate_after_read)
    with pytest.raises(module.TargetFailure, match="changed while being read"):
        module.stable_fixture_report_bytes(report)


@pytest.mark.skipif(
    not NOFOLLOW_AVAILABLE, reason="the frozen reference workload is Linux-only"
)
def test_fixture_report_reader_rejects_oversize_and_symlink(tmp_path: Path):
    module = load_module()
    report = tmp_path / "oversize.json"
    report.write_bytes(b"x" * (module.MAX_FIXTURE_REPORT_BYTES + 1))
    with pytest.raises(module.TargetFailure, match="unavailable or unbounded"):
        module.stable_fixture_report_bytes(report)

    if os.name != "nt":
        target = tmp_path / "target.json"
        target.write_bytes(b"{}\n")
        link = tmp_path / "link.json"
        link.symlink_to(target)
        with pytest.raises(module.TargetFailure, match="unavailable or unsafe"):
            module.stable_fixture_report_bytes(link)


def test_nearest_rank_retains_no_unbounded_sample_inventory():
    module = load_module()

    result = module.latency_triplet([9.0, 1.0, 5.0, 3.0])

    assert result == {
        "definition": "nearest-rank",
        "sample_count": 4,
        "p50_ms": 3.0,
        "p95_ms": 9.0,
        "p99_ms": 9.0,
    }
    assert "samples_ms" not in result


def test_process_tree_rss_sums_root_and_children(tmp_path: Path):
    module = load_module()
    for pid, rss_kib, children in ((10, 100, "11 12"), (11, 20, ""), (12, 30, "")):
        root = tmp_path / str(pid)
        (root / "task" / str(pid)).mkdir(parents=True)
        (root / "status").write_text(f"Name:\tfixture\nVmRSS:\t{rss_kib} kB\n")
        (root / "task" / str(pid) / "children").write_text(children)

    assert module.process_tree_rss_bytes(10, tmp_path) == 150 * 1024


def test_idle_session_normal_completion_before_deliberate_close_fails():
    module = load_module()

    async def exercise() -> None:
        task = asyncio.create_task(asyncio.sleep(0))
        await task
        with pytest.raises(module.TargetFailure, match="ended before"):
            await module.close_idle_sessions([], [task])

    asyncio.run(exercise())


def test_idle_session_close_failure_fails_the_measurement():
    module = load_module()

    class BrokenRoom:
        async def close(self) -> None:
            raise OSError("fixture close failure")

    async def exercise() -> None:
        task = asyncio.create_task(asyncio.sleep(60))
        with pytest.raises(module.TargetFailure, match="ended before"):
            await module.close_idle_sessions([BrokenRoom()], [task])

    asyncio.run(exercise())


def test_runtime_sdk_must_be_imported_from_bounded_packaged_root(
    tmp_path: Path, monkeypatch
):
    module = load_module()
    sdk = tmp_path / "sdk/python"
    files = {
        "pyproject_sha256": sdk / "pyproject.toml",
        "lock_sha256": sdk / "uv.lock",
        "module_init_sha256": sdk / "src/worldstream_sdk/__init__.py",
        "client_module_sha256": sdk / "src/worldstream_sdk/client.py",
        "compatibility_identity_sha256": (
            sdk / "src/worldstream_sdk/compatibility_identity.json"
        ),
    }
    for index, path in enumerate(files.values(), start=1):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(f"fixture-{index}\n".encode())
    expected = {
        "source": "verified_native_archive",
        **{
            field: "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()
            for field, path in files.items()
        },
        **{
            field.replace("_sha256", "_size_bytes"): path.stat().st_size
            for field, path in files.items()
        },
        "runtime_module_under_packaged_sdk_root": True,
    }
    monkeypatch.setattr(
        module.worldstream_sdk, "__file__", str(files["module_init_sha256"])
    )
    monkeypatch.setattr(
        sys.modules[module.Client.__module__],
        "__file__",
        str(files["client_module_sha256"]),
    )
    assert module.verify_runtime_packaged_sdk(sdk, expected) == expected

    monkeypatch.setattr(
        module.worldstream_sdk, "__file__", str(tmp_path / "checkout.py")
    )
    with pytest.raises(module.TargetFailure, match="not imported"):
        module.verify_runtime_packaged_sdk(sdk, expected)


def test_reduced_profile_is_explicitly_nonpublishable():
    module = load_module()

    profile = module.reduced_profile()

    assert profile["name"] == "reduced_test_nonpublishable"
    assert profile["publishable_candidate"] is False
    assert profile != module.frozen_profile()


def test_owned_root_cleanup_is_bounded_and_fail_closed(tmp_path: Path):
    module = load_module()
    root = module.COMMON.private_root("worldstream-reference-target-")

    module.remove_owned_root(root)

    assert not root.exists()
    with pytest.raises(module.TargetFailure, match="bounded ownership"):
        module.remove_owned_root(tmp_path)
    assert tmp_path.exists()

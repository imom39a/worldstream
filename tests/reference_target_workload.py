"""Focused unit boundaries for the genuine frozen reference-target runner."""

from __future__ import annotations

import asyncio
import hashlib
import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/reference-target-workload.py"


def load_module():
    spec = importlib.util.spec_from_file_location(
        "worldstream_reference_target_workload_test", SCRIPT
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_frozen_history_attempt_names_exact_public_semantic_ceiling():
    module = load_module()

    observed = module.history_limit_observation()

    assert observed["requested_transition_count"] == 100_000
    assert observed["action_attempt_count"] == 33
    assert observed["accepted_transition_count"] == 32
    assert observed["pack"]["accepted_action_sequence"] == {
        "increment": 16,
        "private_ack": 16,
    }
    assert observed["terminal_rejection_code"] == "counter_limit_reached"
    assert observed["recovery_measurement_status"] == (
        "not_reachable_due_to_frozen_pack_semantics"
    )


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

"""Focused live-resource sampling tests for the daemon transition soak."""

from __future__ import annotations

import importlib.util
import inspect
import pathlib
import sys
import time

import pytest

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "sdk/python/src"))


def load_soak():
    spec = importlib.util.spec_from_file_location(
        "worldstream_daemon_transition_soak_resources",
        ROOT / "scripts/daemon-transition-soak.py",
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def wait_for_peak(sampler, resource: str, minimum: int) -> None:
    deadline = time.monotonic() + 1.0
    while time.monotonic() < deadline:
        if sampler.observed_peak(resource) >= minimum:
            return
        time.sleep(0.005)
    raise AssertionError(f"live sampler did not observe {resource} peak")


def test_live_storage_sampler_retains_transient_peaks(tmp_path: pathlib.Path) -> None:
    soak = load_soak()
    data_dir = tmp_path / "data"
    data_dir.mkdir()
    main = data_dir / "worldstream.sqlite3"
    wal = pathlib.Path(f"{main}-wal")
    shm = pathlib.Path(f"{main}-shm")
    authority = tmp_path / "authority.secret"
    log = tmp_path / "worldstreamd-test.log"
    temporary = tmp_path / "transient.bin"
    main.write_bytes(b"m" * 16)
    wal.write_bytes(b"w" * 8)
    log.write_bytes(b"l" * 4)
    authority.write_bytes(b"a" * 32)
    excluded = {main, wal, shm, authority}
    baseline = soak.storage_resource_values(tmp_path, data_dir, excluded)
    sampler = soak.LiveStorageSampler(
        tmp_path,
        data_dir,
        excluded,
        baseline,
        interval_seconds=0.01,
        maximum_gap_seconds=0.2,
    )
    sampler.start()
    try:
        wal.write_bytes(b"w" * 2048)
        temporary.write_bytes(b"t" * 1024)
        log.write_bytes(b"l" * 512)
        wait_for_peak(sampler, "wal_bytes", 2048)
        wait_for_peak(sampler, "temporary_bytes", 1024)
        wait_for_peak(sampler, "artifact_bytes", 1536)

        wal.write_bytes(b"w" * 4)
        temporary.write_bytes(b"t" * 2)
        log.write_bytes(b"l" * 3)
        result = sampler.finish()
    finally:
        sampler.cancel()

    final = soak.storage_resource_values(tmp_path, data_dir, excluded)
    assert final["wal_bytes"] == 4
    assert final["temporary_bytes"] == 2
    assert result["peaks"]["wal_bytes"] >= 2048
    assert result["peaks"]["temporary_bytes"] >= 1024
    assert result["peaks"]["artifact_bytes"] >= 1536
    assert result["sample_count"] >= 2
    assert 0 <= result["observed_max_gap_ms"] <= result["maximum_gap_ms"]


def resource_summaries(soak):
    rss = {
        "sampling_interval_ms": 50,
        "maximum_gap_ms": 1000,
        "observed_max_gap_ms": 50,
        "coverage_duration_ms": 2000,
        "sample_count": 40,
        "peak_bytes": 80,
    }
    storage = {
        "sampling_interval_ms": 250,
        "maximum_gap_ms": 1000,
        "observed_max_gap_ms": 250,
        "coverage_duration_ms": 2250,
        "sample_count": 10,
        "initial": {name: 0 for name in soak.STORAGE_RESOURCE_NAMES},
        "peaks": {name: 40 for name in soak.STORAGE_RESOURCE_NAMES},
    }
    limits = {
        "process_tree_rss_bytes": 100,
        **{name: 100 for name in soak.STORAGE_RESOURCE_NAMES},
    }
    return rss, storage, limits


def test_live_resource_contract_fails_closed_on_peak_or_coverage() -> None:
    soak = load_soak()
    rss, storage, limits = resource_summaries(soak)
    evidence = soak.live_resource_sampling(
        rss,
        storage,
        workload_elapsed_seconds=2,
        hard_limits=limits,
    )
    assert evidence["sampling_complete"] is True
    assert evidence["resources"]["temporary_bytes"]["observed_peak_growth_bytes"] == 40

    storage["peaks"]["temporary_bytes"] = 101
    with pytest.raises(soak.SoakFailure, match="temporary_bytes exceeded"):
        soak.live_resource_sampling(
            rss,
            storage,
            workload_elapsed_seconds=2,
            hard_limits=limits,
        )

    storage["peaks"]["temporary_bytes"] = 40
    rss["coverage_duration_ms"] = 0
    with pytest.raises(soak.SoakFailure, match="did not cover"):
        soak.live_resource_sampling(
            rss,
            storage,
            workload_elapsed_seconds=2,
            hard_limits=limits,
        )


def queue_samples(soak):
    samples = {}
    for spec in soak.QUEUE_SPECS:
        capacity = spec["hard_limit"]
        samples[spec["name"]] = [
            {
                "capacity": capacity,
                "process_current": 0,
                "process_high_water": 1,
                "unit_high_water": 1,
                "activity_total": 10,
                "completion_total": 9,
                "backpressure_total": 0,
            },
            {
                "capacity": capacity,
                "process_current": 1,
                "process_high_water": 2,
                "unit_high_water": 2,
                "activity_total": 20,
                "completion_total": 19,
                "backpressure_total": 0,
            },
        ]
    return samples


def test_internal_queue_summary_requires_every_active_bounded_queue() -> None:
    soak = load_soak()
    sampler = soak.InternalQueueSampler("http://127.0.0.1:1")
    sampler.samples = queue_samples(soak)
    observed = sampler.summarize()
    assert [row["name"] for row in observed] == [
        spec["name"] for spec in soak.QUEUE_SPECS
    ]
    assert all(row["activity_total_delta"] > 0 for row in observed)
    assert all(row["completion_total_delta"] > 0 for row in observed)
    assert all(row["backpressure_total_delta"] == 0 for row in observed)

    sampler.samples["telemetry_dns_resolver_queue"][1]["unit_high_water"] = 3
    with pytest.raises(soak.SoakFailure, match="configured unit hard limit"):
        sampler.summarize()


def test_fixed_label_metric_parser_rejects_missing_or_extra_labels() -> None:
    soak = load_soak()
    expected = (
        'worldstream_internal_queue_process_current{queue="room_admission_lane"} 2\n'
    )
    assert (
        soak.metric_value(
            expected,
            "worldstream_internal_queue_process_current",
            {"queue": "room_admission_lane"},
        )
        == 2
    )
    with pytest.raises(soak.SoakFailure, match="fixed labels"):
        soak.metric_value(
            expected.replace("}", ',room_id="private"}'),
            "worldstream_internal_queue_process_current",
            {"queue": "room_admission_lane"},
        )


@pytest.mark.parametrize(
    "raw",
    [
        b'{"status":"wrong","status":"ready"}',
        b'{"status":"ready","ignored":NaN}',
        b'{"status":"ready","ignored":Infinity}',
    ],
)
def test_public_json_rejects_duplicate_and_nonfinite_values(
    monkeypatch: pytest.MonkeyPatch, raw: bytes
) -> None:
    soak = load_soak()

    class Response:
        def __enter__(self):
            return self

        def __exit__(self, *_args):
            return False

        def read(self, _maximum: int) -> bytes:
            return raw

    monkeypatch.setattr(
        soak.urllib.request, "urlopen", lambda *_args, **_kwargs: Response()
    )

    with pytest.raises(soak.SoakFailure, match="disclosure was unavailable"):
        soak.public_json("http://127.0.0.1:1", "/version")


def test_queue_boundary_runner_executes_only_the_exact_production_tests(
    tmp_path: pathlib.Path,
) -> None:
    soak = load_soak()
    cargo = tmp_path / "cargo"
    cargo.write_text(
        """#!/usr/bin/env python3
import sys
test_name = sys.argv[sys.argv.index("--lib") + 1]
print("running 1 test")
print(f"test {test_name} ... ok")
print("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out")
""",
        encoding="utf-8",
    )
    cargo.chmod(0o700)

    evidence = soak.run_queue_boundary_tests(10.0, 4096, cargo=str(cargo))

    assert evidence["schema"] == soak.QUEUE_BOUNDARY_TEST_SCHEMA
    assert evidence["status"] == "pass"
    assert [
        (row["queue_class"], row["package"], row["test_name"])
        for row in evidence["tests"]
    ] == list(soak.QUEUE_BOUNDARY_TESTS)
    assert all(row["exact_test_count"] == 1 for row in evidence["tests"])
    assert all(row["output_sha256"].startswith("sha256:") for row in evidence["tests"])


def test_queue_boundary_runner_rejects_a_non_exact_or_failed_result(
    tmp_path: pathlib.Path,
) -> None:
    soak = load_soak()
    cargo = tmp_path / "cargo"
    cargo.write_text(
        """#!/usr/bin/env python3
print("running 0 tests")
print("test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out")
""",
        encoding="utf-8",
    )
    cargo.chmod(0o700)

    with pytest.raises(soak.SoakFailure, match="exact production queue boundary"):
        soak.run_queue_boundary_tests(10.0, 4096, cargo=str(cargo))


def test_production_soak_samples_storage_through_recovery() -> None:
    soak = load_soak()
    source = inspect.getsource(soak.build_report)
    start = source.index("storage_sampler.start()")
    workload = source.index("await run_transition_workload")
    queue_start = source.index("queue_sampler.start()")
    queue_finish = source.index("internal_queues = queue_sampler.finish()")
    recovery = source.index("externally_kill")
    finish = source.index("storage_summary = storage_sampler.finish()")
    evidence = source.index("resource_sampling = live_resource_sampling")
    assert start < workload < recovery < finish < evidence
    assert queue_start < workload < queue_finish
    assert '"resource_sampling": resource_sampling' in source
    assert '"queue_observation": queue_observation' in source

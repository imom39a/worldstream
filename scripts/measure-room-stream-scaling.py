#!/usr/bin/env python3
"""Run isolated paired Core stream cells. Other measurement modes are separate."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import subprocess
import sys
import time


ROOT = Path(__file__).resolve().parent.parent


def command(*args):
    result = subprocess.run(args, cwd=ROOT, capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else "unavailable"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def positive_list(value):
    values = [int(item) for item in value.split(",")]
    if not values or any(item <= 0 for item in values):
        raise argparse.ArgumentTypeError("values must be positive integers")
    return values


def metadata():
    diff = subprocess.run(["git", "diff", "HEAD", "--binary"], cwd=ROOT,
                          capture_output=True, check=True).stdout
    return {
        "git_revision": command("git", "rev-parse", "HEAD"),
        "tracked_dirty_diff_sha256": hashlib.sha256(diff).hexdigest(),
        "untracked_paths": command("git", "ls-files", "--others", "--exclude-standard").splitlines(),
        "os": platform.platform(), "kernel": platform.release(),
        "cpu_model": command("sysctl", "-n", "machdep.cpu.brand_string") if sys.platform == "darwin" else platform.processor(),
        "logical_cores": os.cpu_count(),
        "physical_ram_bytes": int(command("sysctl", "-n", "hw.memsize")) if sys.platform == "darwin" else os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE"),
        "scratch_free_bytes": shutil.disk_usage(ROOT).free,
        "rust": command("rustc", "--version"), "cargo": command("cargo", "--version"),
        "cargo_lock_sha256": digest(ROOT / "Cargo.lock"),
        "driver_sha256": digest(Path(__file__)),
        "core_and_protocol_source_sha256": {
            str(path.relative_to(ROOT)): digest(path)
            for crate in ("worldstream-core", "worldstream-protocol")
            for path in sorted((ROOT / "crates" / crate).rglob("*.rs"))
        },
        "python": sys.version, "codec": "uncompressed canonical JSON",
        "rss_sampling_interval_seconds": 0.05,
        "rss_limitations": "Sampled RSS can miss short spikes. time maximum measures the Rust child; driver and database RSS are separate.",
    }


def run_cell(binary, count, size, output):
    report = output / f"paired-{count}-{size}.json"
    resource_log = output / f"paired-{count}-{size}.time.txt"
    stdout = output / f"paired-{count}-{size}.stdout.txt"
    argv = [str(binary), "--transitions", str(count), "--state-bytes", str(size), "--output", str(report)]
    time_tool = Path("/usr/bin/time")
    timed = time_tool.exists()
    if timed:
        argv = [str(time_tool), "-l" if sys.platform == "darwin" else "-v", *argv]
    peak = 0
    baseline = None
    samples = 0
    began = time.monotonic()
    with stdout.open("w") as out, resource_log.open("w") as err:
        child = subprocess.Popen(argv, cwd=ROOT, stdout=out, stderr=err, start_new_session=True)
        try:
            while child.poll() is None:
                # time is a wrapper. Sample it and its immediate Rust child separately.
                rows = command("ps", "-axo", "pid=,ppid=,rss=").splitlines()
                rust_rss = []
                for row in rows:
                    fields = row.split()
                    if len(fields) == 3:
                        pid, ppid, rss = map(int, fields)
                        if (timed and ppid == child.pid) or (not timed and pid == child.pid):
                            rust_rss.append(rss * 1024)
                if rust_rss:
                    current = sum(rust_rss)
                    baseline = current if baseline is None else baseline
                    peak = max(peak, current)
                    samples += 1
                time.sleep(0.05)
        except BaseException:
            try:
                os.killpg(child.pid, signal.SIGTERM)
                child.wait(timeout=5)
            except ProcessLookupError:
                pass
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
            raise
        status = child.wait()
    maximum = None
    for line in resource_log.read_text().splitlines():
        if sys.platform == "darwin" and "maximum resident set size" in line:
            maximum = int(line.split()[0])
        elif "Maximum resident set size (kbytes):" in line:
            maximum = int(line.rsplit(":", 1)[1]) * 1024
    result = {
        "transitions_per_format": count, "requested_state_bytes": size,
        "exit_code": status, "elapsed_seconds": time.monotonic() - began,
        "rust_rss_sampled_baseline_bytes": baseline,
        "rust_rss_sampled_peak_bytes": peak if samples else None,
        "rust_rss_samples": samples, "rust_time_maximum_rss_bytes": maximum,
        "time_log": resource_log.name, "stdout_log": stdout.name,
    }
    if status == 0 and report.exists():
        result["measurement"] = json.loads(report.read_text())
    else:
        result["status"] = "failed"
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=["core-stream"], required=True)
    parser.add_argument("--formats", default="v1,v2", choices=["v1,v2"])
    parser.add_argument("--transitions", type=positive_list, default=[100, 10000, 100000])
    parser.add_argument("--state-bytes", type=positive_list, default=[1024, 16384, 65536, 262144])
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/examples/measure_room_stream_scaling")
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error("build the Core example with --features conformance-tracer first, or supply --binary")
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {"schema": "worldstream/core-stream-measurement-driver/v1", "metadata": metadata(), "cells": []}
    report["metadata"]["example_source_sha256"] = digest(ROOT / "crates/worldstream-core/examples/measure_room_stream_scaling.rs")
    report["metadata"]["benchmark_source_sha256"] = digest(ROOT / "crates/worldstream-core/src/benchmark_state.rs")
    report["metadata"]["binary_sha256"] = digest(binary)
    for count in args.transitions:
        for size in args.state_bytes:
            print(f"paired core-stream transitions={count} state-bytes={size}", flush=True)
            report["cells"].append(run_cell(binary, count, size, output))
            (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
            if report["cells"][-1]["exit_code"] != 0:
                return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

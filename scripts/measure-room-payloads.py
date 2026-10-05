#!/usr/bin/env python3
"""Measure retained JSON bytes and labeled synthetic payloads; never write fixtures.

This is a byte/codec workload, not a live kernel throughput benchmark. Python's
encoding is used only for synthetic safe-integer JSON and extracted subvalues.
Retained *_bytes strings are measured and compressed without re-encoding.
"""
import argparse
import copy
import hashlib
import json
import math
import platform
import random
import shutil
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DECODER_MEMORY_BYTES = 8 * 1024 * 1024

def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode("utf-8")

def distribution(values):
    values = sorted(values)
    return {"count": len(values), "min": values[0], "p50": values[math.ceil(len(values)*.5)-1], "p95": values[math.ceil(len(values)*.95)-1], "p99": values[math.ceil(len(values)*.99)-1], "max": values[-1], "total": sum(values)}

def retained_strings(value, path=""):
    if isinstance(value, dict):
        for key, child in value.items():
            position = path + "/" + key
            if key.endswith("bytes") and isinstance(child, str) and child.startswith(("{", "[")):
                yield position, child.encode("utf-8")
            else:
                yield from retained_strings(child, position)
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from retained_strings(child, path + "/" + str(index))

def bounded_decode(zstd, encoded, compressed_limit, decoded_limit, memory_bytes=DECODER_MEMORY_BYTES):
    if len(encoded) > compressed_limit:
        raise ValueError("compressed bound")
    # Input sizes in this standalone workload are small. Use a file for stdin
    # so a full pipe cannot deadlock while the bounded output is read.
    import tempfile
    with tempfile.TemporaryFile() as source:
        source.write(encoded)
        source.seek(0)
        process = subprocess.Popen([zstd, "-q", "-d", "-M" + str(memory_bytes), "-c"], stdin=source, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        try:
            decoded = process.stdout.read(decoded_limit + 1)
            if len(decoded) > decoded_limit:
                raise ValueError("decoded bound")
            if process.wait() != 0:
                raise ValueError("invalid zstd")
            return decoded
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()
            process.stdout.close()

def compression(zstd, raw, repeats):
    enc_times, dec_times = [], []
    for _ in range(repeats):
        start = time.perf_counter_ns()
        encoded = subprocess.run([zstd, "-q", "-1", "-c"], input=raw, stdout=subprocess.PIPE, check=True).stdout
        enc_times.append(time.perf_counter_ns()-start)
        start = time.perf_counter_ns()
        decoded = bounded_decode(zstd, encoded, len(encoded), len(raw))
        dec_times.append(time.perf_counter_ns()-start)
        assert decoded == raw
    corrupt = b"\x00" + encoded[1:]
    failures = {}
    for label, data, c_limit, d_limit in (("corrupt", corrupt, len(encoded), len(raw)), ("truncated", encoded[:-1], len(encoded), len(raw)), ("compressed_bound", encoded, len(encoded)-1, len(raw)), ("decoded_bound", encoded, len(encoded), len(raw)-1)):
        try:
            bounded_decode(zstd, data, c_limit, d_limit)
        except ValueError as error:
            failures[label] = str(error)
        else:
            raise AssertionError(label + " was accepted")
    return {"raw_bytes": len(raw), "zstd1_bytes": len(encoded), "ratio": round(len(encoded)/len(raw), 6), "encode_median_ms_including_process": round(sorted(enc_times)[len(enc_times)//2]/1e6, 4), "decode_median_ms_including_process": round(sorted(dec_times)[len(dec_times)//2]/1e6, 4), "round_trip_exact": True, "negative_checks": failures}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--no-compression", action="store_true")
    args = parser.parse_args()
    if args.repeats < 1:
        parser.error("--repeats must be positive")
    fixture_path = ROOT / "tests/fixtures/core_v1_golden.json"
    fixture_raw = fixture_path.read_bytes()
    fixture = json.loads(fixture_raw)
    retained = list(retained_strings(fixture))
    records = [(p, b) for p, b in retained if p.endswith("/record_bytes") or p in ("/genesis_record_bytes", "/transition_record_bytes")]
    categories = {}
    samples = []
    def add(category, name, raw):
        categories.setdefault(category, []).append(len(raw))
        samples.append({"category": category, "name": name, "bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()})
    for path, raw in retained:
        add("retained_exact_json", path, raw)
    for path, raw in records:
        value = json.loads(raw)
        for key in ("recorded_stimulus", "ordered_domain_events", "ordered_timer_changes", "ordered_attention_signals", "resulting_core_state", "resulting_activity_state", "initial_core_state", "initial_activity_state"):
            if key in value:
                add("fixture_"+key, path, canonical(value[key]))
        for event in value.get("ordered_domain_events", []):
            add("fixture_event_item", path, canonical(event))
    base = json.loads(fixture["transition_record_bytes"])
    workloads = []
    rng = random.Random(3444)
    alphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
    for profile in ("embedded-shape", "portable-shape"):
        for pattern in ("repeated", "seeded-ascii"):
            for size in (1024, 16384, 262144):
                text_size = size-len(canonical({"blob": ""}))
                text = "a"*text_size if pattern == "repeated" else "".join(rng.choice(alphabet) for _ in range(text_size))
                state = {"blob": text}
                assert len(canonical(state)) == size
                v1 = copy.deepcopy(base)
                v1["resulting_activity_state"] = state
                compact = {k: v for k, v in v1.items() if k not in ("resulting_activity_state", "resulting_core_state")}
                # Shape comparison only: copied commitments are not recomputed.
                name = f"{profile}/{pattern}/{size}"
                full, small = canonical(v1), canonical(compact)
                projection = canonical({"projection_schema": "synthetic.projection.v1", "projection": {"state": state}, "action_offers": []})
                observation = canonical({"observation_schema": "synthetic.observation.v1", "observation": {"changed": True}, "action_offers": []})
                add("synthetic_complete_projection", name, projection)
                add("synthetic_complete_observation", name, observation)
                workloads.append({"name": name, "activity_bytes": size, "v1_shape_bytes": len(full), "compact_shape_bytes": len(small), "complete_projection_bytes": len(projection), "complete_observation_bytes": len(observation), "canonical_1000_v1_shape_bytes": len(full)*1000, "canonical_1000_compact_plus_five_activity_only_checkpoints_bytes": len(small)*1000+size*5})
                records.append(("synthetic/"+name, full))
    membership = next(iter(base["resulting_core_state"]["memberships"].values()))
    membership_workloads = []
    for count in (1, 10, 100, 1000):
        members = {}
        for i in range(count):
            key = f"{i:026d}"
            members[key] = {**membership, "member_id": key, "principal_id": key}
        core = canonical({"memberships": members, "room_status": "active"})
        membership_workloads.append({"memberships": count, "canonical_core_bytes": len(core), "within_proposed_131072_core_bound": len(core) <= 131072})
    zstd = shutil.which("zstd")
    codec = None
    if not args.no_compression:
        if not zstd:
            parser.error("zstd CLI not installed; use --no-compression for byte inventory")
        memory_probe_raw = b"a" * (3 * 1024 * 1024)
        memory_probe = subprocess.run([zstd, "-q", "-1", "--long=23", "-c"], input=memory_probe_raw, stdout=subprocess.PIPE, check=True).stdout
        try:
            bounded_decode(zstd, memory_probe, len(memory_probe), len(memory_probe_raw), 1024 * 1024)
        except ValueError:
            memory_negative = True
        else:
            raise AssertionError("large decoder window was accepted")
        assert bounded_decode(zstd, memory_probe, len(memory_probe), len(memory_probe_raw)) == memory_probe_raw
        codec = {"decoder_memory_limit_bytes": DECODER_MEMORY_BYTES, "memory_negative_probe": {"compressed_bytes": len(memory_probe), "decoded_bytes": len(memory_probe_raw), "advertised_window_bytes": 8 * 1024 * 1024, "rejecting_memory_limit_bytes": 1024 * 1024, "rejected": memory_negative, "normal_memory_limit_round_trip_exact": True}, "version": subprocess.run([zstd, "--version"], capture_output=True, text=True, check=True).stdout.strip(), "level": 1, "repeats": args.repeats, "records": [{"name": name, **compression(zstd, raw, args.repeats)} for name, raw in records]}
    report = {"schema": "worldstream/payload-measurements/v1", "environment": {"platform": platform.platform(), "machine": platform.machine(), "python": platform.python_version()}, "fixture": {"path": str(fixture_path.relative_to(ROOT)), "file_bytes": len(fixture_raw), "sha256": hashlib.sha256(fixture_raw).hexdigest()}, "distributions": {key: distribution(value) for key, value in sorted(categories.items())}, "samples": samples, "synthetic_workloads": workloads, "synthetic_membership_workloads": membership_workloads, "compression": codec, "limitations": ["No executor or live Session benchmark. Profiles are labeled JSON shapes, not executable Activity Packs.", "Copied Transition commitments are intentionally not recomputed; synthetic records are not conformance vectors.", "CLI timings include process startup and IPC. They are not isolated codec CPU time.", "Whole fixture-file SHA256 is audit metadata; it is not a WorldStream canonical digest.", "Synthetic full projection includes schema and offers; it can exceed the policy when its Activity body alone equals that policy.", "History totals omit full Genesis, Core checkpoints, operational witnesses, indexes and backup framing. They are byte-shape illustrations only."]}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True)+"\n")
    print(json.dumps({"output": str(args.output), "retained_exact_samples": len(retained), "codec_records": len(records) if codec else 0, "synthetic_workloads": len(workloads)}))

if __name__ == "__main__":
    main()

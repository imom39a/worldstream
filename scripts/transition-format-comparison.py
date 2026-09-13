#!/usr/bin/env python3
"""Repeatable wire-size comparison for the IMO-224 Transition proposal.

This is deliberately a design benchmark.  It does not read or rewrite a Room
and it does not claim that the proposed record is a production codec.  The
payloads model the bytes that a real codec would carry so that the decision is
repeatable without a live Pack or database.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import platform
import sys
import time
import zlib
from dataclasses import dataclass


CHECKPOINT_INTERVAL = 250
OBSERVATION_BYTES = 1024
SAMPLE_RECORDS = 100
STATE_SIZES_KIB = (1, 16, 256, 1024)
HISTORY_LENGTHS = (1_000, 10_000, 100_000)


def canonical(value: object) -> bytes:
    """The benchmark's canonical envelope: UTF-8 JSON, sorted keys, no spaces."""

    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()


def digest(value: bytes) -> str:
    return hashlib.blake2b(value, digest_size=32).hexdigest()


def state_bytes(size: int) -> bytes:
    # A deterministic non-repeating stream prevents compression from making
    # the comparison depend on an unrealistically repetitive fixture.
    return bytes((index * 131 + 17) % 251 for index in range(size))


def state_text(size: int) -> str:
    alphabet = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
    return "".join(chr(alphabet[value % len(alphabet)]) for value in state_bytes(size))


def full_record(state_size: int, seq: int) -> bytes:
    activity = state_text(state_size)
    state_hash = digest(activity.encode())
    return canonical(
        {
            "transition_version": "worldstream/transition/v1",
            "codec_id": "worldstream/canonical-json/v1",
            "hash_suite": "blake3-canonical-json-v1",
            "room_id": "room-fixture",
            "room_seq": seq,
            "core_schema_version": "worldstream.core-room-state.v1",
            "pack_digest": digest(b"pack-fixture"),
            "previous_transition_or_genesis_hash": digest(("transition:" + str(seq - 1)).encode()),
            "core_state": {"room_status": "active", "membership_digest": digest(b"members")},
            "activity_state": activity,
            "recorded_stimulus": {"kind": "pack_input", "seq": seq, "input": "fixture"},
            "ordered_domain_events": [
                {"kind": "fixture.event", "seq": seq, "observation": state_text(OBSERVATION_BYTES)}
            ],
            "ordered_timer_changes": [],
            "ordered_attention_signals": [{"kind": "fixture.attention", "seq": seq}],
            "resulting_core_state_hash": digest(b"core"),
            "resulting_activity_state_hash": state_hash,
            "resulting_authoritative_state_hash": digest(("authoritative:" + state_hash).encode()),
            "transition_hash": digest(("transition:" + str(seq)).encode()),
        }
    )


def compact_record(seq: int) -> bytes:
    # The proposed v2 record intentionally contains no resulting state.  The
    # exact Pack re-executes the recorded input and checks the output hashes.
    activity_hash = digest(state_text(1024).encode())
    return canonical(
        {
            "transition_version": "worldstream/transition/v2",
            "codec_id": "worldstream/transition-record/v2",
            "hash_suite": "blake3-canonical-json-v2",
            "room_id": "room-fixture",
            "room_seq": seq,
            "core_schema_version": "worldstream.core-room-state.v1",
            "pack_digest": digest(b"pack-fixture"),
            "previous_lineage_hash": digest(("transition:" + str(seq - 1)).encode()),
            "recorded_stimulus": {"kind": "pack_input", "seq": seq, "input": "fixture"},
            "ordered_domain_events": [
                {"kind": "fixture.event", "seq": seq, "observation": state_text(OBSERVATION_BYTES)}
            ],
            "ordered_timer_changes": [],
            "ordered_attention_signals": [{"kind": "fixture.attention", "seq": seq}],
            "resulting_core_state_hash": digest(b"core"),
            "resulting_activity_state_hash": activity_hash,
            "resulting_authoritative_state_hash": digest(("authoritative:" + activity_hash).encode()),
            "transition_hash": digest(("transition:" + str(seq)).encode()),
        }
    )


def checkpoint(state_size: int, seq: int) -> bytes:
    activity_hash = digest(state_text(state_size).encode())
    return canonical(
        {
            "checkpoint_version": "worldstream/checkpoint/v2",
            "room_seq": seq,
            "core_state": {"room_status": "active", "membership_digest": digest(b"members")},
            "activity_state": state_text(state_size),
            "core_state_hash": digest(b"core"),
            "activity_state_hash": activity_hash,
            "authoritative_state_hash": digest(("authoritative:" + activity_hash).encode()),
            "lineage_hash": digest(("transition:" + str(seq)).encode()),
        }
    )


@dataclass(frozen=True)
class Row:
    state_kib: int
    history: int
    full_bytes: int
    compact_bytes: int
    full_zlib_bytes: int
    compact_zlib_bytes: int
    compact_recovery_callbacks: int


def measure(state_kib: int, history: int) -> Row:
    full = full_record(state_kib * 1024, 1)
    compact = compact_record(1)
    checkpoint_bytes = checkpoint(state_kib * 1024, 0)
    checkpoint_count = (history + CHECKPOINT_INTERVAL - 1) // CHECKPOINT_INTERVAL
    return Row(
        state_kib,
        history,
        len(full) * history,
        len(compact) * history + len(checkpoint_bytes) * checkpoint_count,
        len(zlib.compress(full, 9)) * history,
        len(zlib.compress(compact, 9)) * history + len(zlib.compress(checkpoint_bytes, 9)) * checkpoint_count,
        min(CHECKPOINT_INTERVAL, history),
    )


def timed_encoding(state_kib: int) -> tuple[float, float]:
    state_size = state_kib * 1024
    start = time.perf_counter_ns()
    for seq in range(1, SAMPLE_RECORDS + 1):
        full_record(state_size, seq)
    full_ms = (time.perf_counter_ns() - start) / 1_000_000
    start = time.perf_counter_ns()
    for seq in range(1, SAMPLE_RECORDS + 1):
        compact_record(seq)
    compact_ms = (time.perf_counter_ns() - start) / 1_000_000
    return full_ms, compact_ms


def fmt_bytes(value: int) -> str:
    units = ("B", "KiB", "MiB", "GiB", "TiB")
    amount = float(value)
    unit = 0
    while amount >= 1024 and unit < len(units) - 1:
        amount /= 1024
        unit += 1
    return f"{amount:.2f} {units[unit]}"


def strict_decode(encoded: bytes) -> dict[str, object]:
    value = json.loads(encoded)
    assert canonical(value) == encoded
    return value


def recovery_decision(*, checkpoint_present: bool, executor_present: bool, archived: bool) -> str:
    if not executor_present:
        return "fail_closed"
    if archived:
        return "read_only_replay"
    if checkpoint_present:
        return "bounded_tail"
    return "genesis_replay"


def render() -> str:
    lines = [
        "# IMO-224 Transition format comparison",
        "",
        "This report is generated by `scripts/transition-format-comparison.py`.",
        "The fixture uses incompressible deterministic state bytes, a 1 KiB observation payload, and a 250-transition checkpoint interval.",
        "The totals are wire-byte estimates; timing is measured for 100 representative records on the host running the script.",
        "",
        "## Measurements",
        "",
        "| state | history | V1 full | V2 records + checkpoints | V1 zlib-9 | V2 zlib-9 | bounded recovery callbacks |",
        "|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for state_kib in STATE_SIZES_KIB:
        for history in HISTORY_LENGTHS:
            row = measure(state_kib, history)
            lines.append(
                f"| {state_kib} KiB | {history:,} | {fmt_bytes(row.full_bytes)} | {fmt_bytes(row.compact_bytes)} | "
                f"{fmt_bytes(row.full_zlib_bytes)} | {fmt_bytes(row.compact_zlib_bytes)} | {row.compact_recovery_callbacks} |"
            )
    lines += [
        "",
        "| state | V1 encode 100 records | V2 encode 100 records |",
        "|---:|---:|---:|",
    ]
    for state_kib in STATE_SIZES_KIB:
        full_ms, compact_ms = timed_encoding(state_kib)
        lines.append(f"| {state_kib} KiB | {full_ms:.2f} ms | {compact_ms:.2f} ms |")
    lines += [
        "",
        "## Interpretation",
        "",
        "V1 repeats the complete resulting Activity value in every Transition. The proposed V2 record retains the exact recorded input, ordered outputs, and all three resulting hashes; a verified checkpoint supplies the materialized Core/Activity values every 250 transitions.",
        "Random historical views replay from the nearest checkpoint and therefore have a bounded tail. Full forensic verification still replays every record and remains available for skipped-prefix corruption detection.",
        "zlib is included only as a physical-compression reference. It preserves V1's logical shape and does not remove state-copy, random-view, or corruption-localization tradeoffs.",
        "",
    ]
    return "\n".join(lines)


def check() -> None:
    for state_kib in STATE_SIZES_KIB:
        for history in HISTORY_LENGTHS:
            row = measure(state_kib, history)
            assert row.full_bytes > row.compact_bytes > 0
            assert row.compact_recovery_callbacks <= CHECKPOINT_INTERVAL
            assert row.full_zlib_bytes > 0 and row.compact_zlib_bytes > 0
    old = strict_decode(full_record(1024, 1))
    new = strict_decode(compact_record(1))
    for key in (
        "recorded_stimulus",
        "ordered_domain_events",
        "ordered_timer_changes",
        "ordered_attention_signals",
        "resulting_core_state_hash",
        "resulting_activity_state_hash",
        "resulting_authoritative_state_hash",
        "transition_hash",
    ):
        assert old[key] == new[key]
    assert "core_state" not in new and "activity_state" not in new
    tampered = bytearray(compact_record(1))
    tampered[-2] = ord("0") if tampered[-2] != ord("0") else ord("1")
    assert bytes(tampered) != compact_record(1)
    assert recovery_decision(checkpoint_present=True, executor_present=True, archived=False) == "bounded_tail"
    assert recovery_decision(checkpoint_present=False, executor_present=True, archived=False) == "genesis_replay"
    assert recovery_decision(checkpoint_present=True, executor_present=False, archived=False) == "fail_closed"
    assert recovery_decision(checkpoint_present=True, executor_present=True, archived=True) == "read_only_replay"
    assert checkpoint(1024, 0) == checkpoint(1024, 0)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="run deterministic benchmark invariants")
    parser.add_argument("--output", help="write the Markdown report to this path")
    args = parser.parse_args()
    if args.check:
        check()
    report = render()
    if args.output:
        with open(args.output, "w", encoding="utf-8") as handle:
            handle.write(report)
    else:
        print(report)
    print(
        f"source=IMO-224 fixture engine=CPython {platform.python_version()} "
        f"platform={platform.platform()} zlib={zlib.ZLIB_VERSION} python={sys.version.split()[0]}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

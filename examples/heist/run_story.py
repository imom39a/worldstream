#!/usr/bin/env python3
"""Run the offline IMO-57 absent-Broker story."""

from __future__ import annotations

import argparse
import json

from story import self_test


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--self-test", action="store_true", help="verify the deterministic corpus"
    )
    args = parser.parse_args()
    transcript = self_test()
    if args.self_test:
        print(
            json.dumps(
                {
                    "status": "ok",
                    "transcript_digest": transcript["transcript_digest"],
                    "room_seq": transcript["final_head"]["room_seq"],
                    "phase_path": transcript["phase_path"],
                    "expected_fixture_outcome": transcript["expected_fixture_outcome"],
                    "replay": transcript["final_read_only_replay"],
                },
                ensure_ascii=False,
                sort_keys=True,
                separators=(",", ":"),
            )
        )
    else:
        print(
            json.dumps(
                transcript, ensure_ascii=False, sort_keys=True, separators=(",", ":")
            )
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

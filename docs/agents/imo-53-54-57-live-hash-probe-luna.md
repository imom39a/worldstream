# IMO-53 / IMO-54 / IMO-57 live Replay hash probe

Status: bounded live-hash evidence improvement; no release claim.

The public protocol currently exposes the exact `room_head` fields needed for
the live claim on both current Projection and historical Replay:

- `core_state_hash` (Core);
- `activity_state_hash` (Activity);
- `authoritative_state_hash` (aggregate Authoritative state); and
- `genesis_or_transition_hash` (Transition lineage).

`examples/heist/wave10_live/run_absent_broker_live.py` now compares those four
opaque server-provided strings, plus `room_id` and `room_seq`, between the
final current Projection and the final Replay response. It does not calculate
or normalize a digest. A missing field returns
`replay_hash_parity_fields_unexposed`; a differing field returns
`replay_hash_parity_mismatch`. Thus a payload regression cannot silently
preserve a weaker `verification=verified`-only claim.

## Focused evidence

```text
uv run --project sdk/python --locked python -m unittest -v \
  examples/heist/wave10_live/test_absent_broker_live.py
Ran 7 tests ... OK

python3 -m py_compile examples/heist/wave10_live/run_absent_broker_live.py
git diff --check -- examples/heist/wave10_live
```

The bounded disposable live attempt was stopped at the requested limit while
the existing timer loop was retrying public HTTP `429 room_busy`; it did not
reach final Replay. Therefore this lane does not claim live parity evidence.
The probe and its offline pass are the strongest reproducible result without
waiting on the long timer service. No reducer/core/server/UI, compatibility
manifest, or release fields were changed, and no commit was created.

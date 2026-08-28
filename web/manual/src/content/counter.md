# Counter: the smallest working pack

Counter is the best first exercise because it exposes the complete kernel path
without domain privacy or a phase machine: create a Room, attach a Membership,
receive an offer, submit an increment, observe a Transition, and Replay it.

## What to inspect

| Concern | Repository source |
| --- | --- |
| pack implementations and registry | `crates/worldstream-core/src/counter_registry.rs` |
| golden transcript tests | counter registry tests in `worldstream-core` |
| live service acceptance | `examples/counter/run_live_acceptance.py` |
| acceptance tests | `examples/counter/test_acceptance.py` |
| storage-neutral conformance | `crates/worldstream-conformance` |

Counter v1 and v2 are both retained for exact Replay; only the selected current
revision is available for new conformance Rooms. This demonstrates why a pack
revision is executable history rather than a mutable label.

## Run the live acceptance

Prepare the normal local authority state, then run:

```sh
cargo build --locked -p worldstream-server --bins
uv run --project sdk/python --python 3.14.7 \
  python examples/counter/run_live_acceptance.py
```

The harness starts a temporary real daemon boundary and validates the live
Counter workflow. Run its focused tests with:

```sh
uv run --project sdk/python --python 3.14.7 \
  python -m unittest examples.counter.test_acceptance
```

## Use Counter as a template

When creating a new small pack, copy the *shape*, not the domain names:

1. define one compact canonical Activity State;
2. define one or two closed Action types and strict payload schemas;
3. initialize deterministically from recorded configuration;
4. reduce only exact offers against exact Heads;
5. project state and offers per viewer;
6. freeze a golden transcript;
7. prove both current execution and retained Replay.

Then add complexity one axis at a time: Roles, private views, timers, attention,
Membership changes, and Outcome. This makes it clear which invariant a failing
test belongs to.

## Minimal client loop

```text
attach Membership
  → install Projection Reset
  → read current Action Offers
  → submit exact offer + payload + Head precondition
  → retain accepted/rejected receipt
  → process Observation Frame durably
  → ACK contiguous frame sequence
```

Do not convert this into “send increment command.” The offer and Head
precondition are the stale-action safety boundary.

Source: [Counter registry](https://github.com/imom39a/worldstream/blob/main/crates/worldstream-core/src/counter_registry.rs),
[live acceptance](https://github.com/imom39a/worldstream/blob/main/examples/counter/run_live_acceptance.py),
and [protocol conformance requirements](https://github.com/imom39a/worldstream/blob/main/docs/protocol.md#required-conformance-scenarios).

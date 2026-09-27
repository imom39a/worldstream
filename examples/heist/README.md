# Agent Heist example

`run_story.py` is a dependency-free, offline evidence harness for the public
Agent Heist behavior. It does not test a live server, database, browser, SDK, or model.

From a fresh checkout:

```sh
python3 examples/heist/run_story.py --self-test
python3 -m unittest discover -s examples/heist -p 'test_*.py'
```

The selected public fixture is `service_window`. Navigator and Insider commit
the same correct plan; Broker remains an immutable enabled seat with no
commitment. The expected public reducer result is therefore a strict 2-of-3
majority, `success`, score `5`, and `missing_roles: ["broker"]`. The harness uses labeled SHA-256 hashes over canonical UTF-8 JSON. These hashes are separate from the BLAKE3 Pack digest in the Rust implementation.

## Retained exact-executor fixture

`story.py` also exercises the retained Heist executor locally. The fixture loads the Room from Genesis and produces the initial and final public views. It applies 15 retained Transitions sequentially and runs read-only Replay. Replay compares the exact Core, Activity, aggregate Authoritative,
lineage, and complete Transition-hash set. The retained executor is pinned to
the public Heist digest recorded in `parity_fixture.json`; a missing or changed
digest fails closed.

The parity fixture remains separate from the transcript so the existing
console-consumed transcript digest does not drift. It records the six-phase
path and the exact retained hash set.

## Runner control harness

`run_runner.py --self-test` prints a deterministic, fixture-only Runner wire
plan. It does not claim a server, capability, or Activation. Live polling is
explicitly fail-closed and requires all of `--base-url`, `--bearer`,
`--runner-id`, `--room-id`, and `--member-id`:

```sh
uv run --project sdk/python python examples/heist/run_runner.py --self-test
uv run --project sdk/python python examples/heist/run_runner.py \
  --base-url http://127.0.0.1:8080 \
  --bearer 'wsb1:<64 lowercase hex characters>' \
  --runner-id runner-a --room-id room-a --member-id member-a
```

The live command uses the dedicated `/v1/runner/stream` endpoint and reports
only typed protocol outcomes and offer metadata. It does not mint
capabilities, inject an inbound update, or treat a fixture run as live
evidence. Add `--claim-first` (and optionally `--complete-disposition
success`) only when a real authorized Runner capability and Agent Membership
are available.

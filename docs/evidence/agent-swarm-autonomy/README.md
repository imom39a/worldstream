# Bounded adaptive-planning protocol evidence

The final managed behavior run passed on 2026-09-22 in 194.653 seconds, including
startup and cleanup. Controller, Runtime, execution daemon and owned processes
stopped afterward. All five runtime executable hashes and all three shared
harness hashes were unchanged before and after the scenarios.

[Final sanitized report](results.json). The earlier intermediate-build run also
passed in 214.137 seconds; its [initial report](results-initial.json) is retained
separately.

This is a controlled, state-sensitive provider fixture. It exercises the real
planning-decision protocol, coordinator, Room and native processes. It does not
measure a live model's reasoning quality. The harness supplies a goal, roster,
working area and bounded autonomy policy, and confirms setup. It supplies no
WorkerActionPlans and no ordinary work Actions. The provider fixture chooses
two independent tasks from the available options; that choice is fixture
behavior, not a task count forced by the production planner.

| Final scenario | Work Items | Contributions | Planning + worker Invocations | Observed native overlap | Scenario seconds | Outcome |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Parallel | 2 | 2 | 6 + 6 | 2 | 52.389 | Provider chose Wait after contributions |
| Failure recovery | 2 | 2 | 7 + 7 | 1 | 71.537 | One failed worker, revised instruction, accepted retry |
| Budget of one | 0 | 0 | 1 + 0 | 0 | 8.148 | Entire selected batch refused after planning exhausted budget |

The final coordinator journals contain six planning Invocations plus six
worker Invocations for the healthy flow, and seven planning plus seven worker
Invocations for recovery. This makes the cost of separating model decisions
from action authoring visible: this fixture used 12 provider Invocations for
two completed Contributions. These counts do not establish live-model cost or
latency. Reducing model turns is a future optimization that must preserve the
selected member's authority and the validation of its proposed Action.

The service's separate unit coverage starts with a model choosing Wait while
proposal options remain available and verifies that no Work Item is forced.
The host supplies safe choices and validates decisions; it does not prescribe
the fixture's propose/claim/contribute sequence for a real model.

The recovery fixture deliberately fails its first contribution. Retrying its
unchanged instruction fails again. Only a new planning decision that has seen
the retained `process_failed` outcome supplies the revised instruction that
allows the same owned work to finish. The report verifies the original failure
and the accepted attempt with the changed instruction against the coordinator
journal. Independent accepted work remains retained.

Each scenario proves that a paused cycle launches nothing and that reopening
the coordinator command process after its stopping decision creates no extra
Invocations or Room transitions. The daemon stays alive during these reopenings;
this is not a daemon crash-recovery test. Each completed contribution comes from
a different roster member and retains its own artifact digest. The ordinary
successful fixture's native process overlap is observed using non-consuming
Collect results, excluding retained terminal completions.

The fixture contributes with 1500 ms of artificial delay plus the controlled
adapter's existing 250 ms WorkAttempt delay. These are concurrency checks, not
live-provider throughput measurements. All scenarios retain zero accepted
Results. Checks, independent review and result-acceptance faults are covered by
the separate [CSV challenge](../agent-swarm-parallel-challenge/README.md).

Run from the repository root after building the managed native binaries and
the Agent Swarm Pack. The root directory must exist and be empty:

```sh
uv run --project sdk/python --python 3.14.7 python \
  scripts/verify-agent-swarm-managed.py \
  --workspace "$PWD" --root /path/to/empty/test-root \
  --pack packs/agent-swarm/releases/0.2.0/worldstream-agent-swarm-candidate.wspack \
  --autonomy-report .scratch/swarm-autonomy-validation/results.json
```

The final harness captures executable and script hashes before running, checks
them again afterward, and publishes a successful report only after cleanup is
proven. Bounded failure snapshots retain execution state before cleanup without
copying provider prompts, credential values or raw output into the report.

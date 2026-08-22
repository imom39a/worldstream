# IMO-55 / IMO-57 Activation claim-loss verification

Date: 2026-08-21

The parent orchestrator ran the public Python SDK and disposable daemon proof directly:

```text
uv run --project sdk/python --locked python \
  examples/heist/wave10_live/run_absent_broker_live.py \
  --spawn-daemon \
  --binary target/debug/worldstreamd \
  --timer-timeout 180 \
  --offer-timeout 20 \
  --exercise-lost-claim-reply
```

Result: exit code `0`, `status=completed`, `live_evidence=true`.

The daemon log recorded the opt-in termination immediately after the durable
Activation claim and before its reply. The runner then restarted that daemon
against the same data directory with the seam disabled, reconnected, and
retried the exact retained request twice. Both replies were the same durable
`granted` receipt with the same claim identity and lease generation. The runner
completed the lease and observed an idempotent duplicate completion.

The same run also completed the six Heist phases, preserved the absent Broker
seat, emitted no private context to participants, and verified exact replay hash
parity for core, activity, aggregate-authoritative, transition, room identity,
and room sequence.

This supersedes the earlier wave-11 note that described the claim-loss proof as
not exercised. The public SDK already retains the original request in
`LostRunnerReply`; no product code change was required for this proof.

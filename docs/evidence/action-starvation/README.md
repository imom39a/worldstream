# IMO-227 action-starvation evidence

Evidence date: 2026-09-14

This directory records the deterministic delayed-Runner matrix and the focused
production Core admission matrix for IMO-227. Both use virtual time and require
no model provider or network. A stale proposal is discarded; each retry uses a
new Action identity after refresh and re-evaluation. The implementation does
not weaken exact-Head admission or silently rebase an Action.

## Commands

```sh
uv run --python 3.14.7 --no-project python \
  scripts/action-starvation-probe.py \
  --output docs/evidence/action-starvation/deterministic-matrix.json

uv run --python 3.14.7 --no-project python \
  scripts/action-starvation-production-matrix.py \
  --output docs/evidence/action-starvation/production-matrix.json
```

The production wrapper ran the exact focused test through
`CoreTraceV1::assess_stable_action_disposition` with `cargo test --locked`.

## Results

The 99-scenario deterministic report covers 792 trials across update rate,
decision delay, participant count, visibility, relation, and the separate
hidden-head-lag baseline. It recorded 11,578 attempts, 448 accepted useful
Actions, 11,130 stale rejections/model-equivalent wasted decisions, and 344
starved trials. At most one attempt was pending. The declared envelope of at
most 250 ms decision time and two Room updates per second passed within eight
attempts. Outside that envelope, sustained whole-Head changes can starve the
strict contract, so a broader product envelope requires a versioned successor
ADR before changing admission.

The production matrix covers 240 schedules: delays of 0/100/250/500/1000 ms,
rates of 0/2/5 updates per second, 1/2/4/8 participants, and visible/hidden plus
related/unrelated dimensions. It recorded 160 accepted trials, 80 starved
trials, and 240 exact stale rejections, with at most three attempts and 2 ms
backoff. Each participant-count lane ran 60 trials and accepted 40, so the
controlled schedule did not favor a participant-count lane.

The production-Core fairness schedule then gave each of eight participants 32
round-robin opportunities. All 256 first attempts raced a real private update
and rejected stale; all 256 refreshed second attempts used new Action
identities and committed. The minimum and maximum accepted contribution counts
were both 32.

IMO-217 is intentionally separate. Its hidden-head synchronization defect
starts before reasoning, while this ticket's primary matrix starts from a
synchronized Head and introduces updates during reasoning. The deterministic
report includes the current hidden-head-lag baseline. A post-fix comparison can
be appended when IMO-217 has an implementation; no IMO-217 payload or delivery
change is part of this measurement ticket.

## Integrity

```text
deterministic-matrix.json sha256 2a6e65f78fbb3cb2853608e492d892fb683b58bcdb24f82e4e53eb1f2033d2a8
production-matrix.json    sha256 4dd1fc5f413bec57d81c113fc93e0e9cb699fc47df659d4a89fd7bf5a6412d9f
```

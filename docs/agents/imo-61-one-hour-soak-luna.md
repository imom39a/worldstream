# IMO-61 one-hour SQLite soak lane

Date: 2026-08-21
Lane: Luna bounded SQLite soak and harness-boundary verification
Repository: `/Users/vinothshanmugam/code/agent-streamer`

## Parent-reproducible commands

```text
bash scripts/soak-smoke.sh --one-hour --output /tmp/worldstream-one-hour-parent.json
python3 -m unittest -v tests.soak_smoke
```

The parent run completed with `status=pass`,
`one_hour_window_completed=true`, and `elapsed_seconds=3600.015`.

## Results

- 462 complete `cargo test -p worldstream-sqlite --locked` matrix runs.
- Every matrix run reported all 90 listed tests passed.
- Command duration percentiles: p50 `7096.465 ms`, p95 `9129.828 ms`,
  p99 `26164.545 ms`.
- Peak RSS was measured per child run; observed first-to-last delta was
  `573440` bytes and the maximum observed value was `160317440` bytes.
- Resource, fault, and corruption fixture hooks passed.

The soak runner was corrected after the first attempt began a final Cargo
process with only a few seconds remaining and incorrectly reported a timeout.
Full-window runs now stop starting new commands when the configured command
timeout no longer fits, wait through the bounded tail, and then report the
window result. The nine-case harness boundary suite passes after the fix.

## Evidence boundary

This is bounded local in-process SQLite fixture evidence. It does not claim
process-kill or power-loss recovery (`kill_point=not_exposed`), hosted
PostgreSQL, native Windows/Linux release profiles, or signed release
artifacts. The report intentionally remains `release_evidence=false`; the
compatibility manifest remains `release_ready=false`.

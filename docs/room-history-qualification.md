# Room history qualification

`scripts/room-history-qualification.py` is the deterministic qualification
harness for the 1k, 10k, 100k, and 1m Room history tiers. It models the
production boundaries that need qualification while retaining only counters
and one bounded replaceable Runner context. It does not allocate a list of
transitions, retain model transcripts, or make a claim about a particular
database host.

Run the complete deterministic matrix with:

```sh
python3 scripts/room-history-qualification.py --compact --output qualification-room-history.json
```

The `tiers[*].counters` object keeps Models, Invocations, Attempts,
Transitions, and Frames separate. The same tier records warm path history
reads, reducer callback count, modeled recovery and oldest-work latency,
bounded Runner and context bytes, modeled database/WAL growth, exact
`based_on_room_seq` action behavior, Runner replacement and failure scenarios,
and backup/transfer outcomes. `--state-bytes`, `--fanout`, `--burst-size`,
`--decision-delay-ms`, and `--update-rate-per-second` vary the workload while
remaining bounded.

Use `--checkpoint PATH` after each tier and `--resume PATH` to reuse completed
tiers. Checkpoints are atomically replaced and contain the same machine
readable schema as the final report. The report marks deterministic values as
`source: deterministic_model`; the modeled RSS number is a bounded estimate,
and observed host RSS/database/WAL values require a backend driver and a real
environment.

The result is `smoke_pass` when the deterministic required portions pass. It is
`qualified` only when every required evidence item is complete, including a
wall clock 72-hour soak. With no soak flag the report is explicitly
`pass: false` and lists `seventy_two_hour_soak` in
`skipped_required_evidence`. `--soak-hours` below 72 records an incomplete
wall-clock observation and cannot pass. A 72-hour request waits for 72 hours;
the flag cannot manufacture elapsed time. Optional `--quality-samples N`
produces at most 32 deterministic proxy samples and is never part of the
durability gate; it is not an LLM provider measurement.

The harness establishes counter arithmetic, bounded state, exact action basis,
repeatability, restart/offline/credential/timer scenario coverage, and
fail-closed evidence accounting. It does not establish real PostgreSQL or
SQLite latency, RSS, WAL growth, crash recovery, or export/restore/transfer
performance. Those still require an environment-specific backend driver and
the actual 72-hour run.

## Real backend evidence

The SQLite driver uses the production `SqliteRoomStore`, Core authorization,
Room creation, commit, and recovery APIs:

```sh
python3 scripts/room-history-qualification.py \
  --tier 1000 --real-sqlite --sqlite-transition-count 1000 \
  --sqlite-soak-seconds 30 --checkpoint qualification-room-history.json \
  --output qualification-room-history-real.json
```

It emits measured database, WAL, shared-memory, and process RSS bytes, durable
transition/frame row counts, recovery time, reducer callback count, and Pack
projection context bytes. A real reopen after commit is the crash/restart
exercise. The current driver marks offline Runner, timer, backup, and transfer
scenarios as `not_exercised`; its `qualification_eligible` flag therefore
remains false until those production scenarios are driven as well. This is
intentional fail-closed behavior.

`--sqlite-transition-count 100000` and `1000000` are explicit long-run driver
commands. They are not run by the smoke command. PostgreSQL is represented by
`--postgres-dsn` as a fail-closed hook: until a provider driver is available,
an absent DSN is `skipped` and a supplied DSN is `failed`.

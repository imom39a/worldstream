# PROTOTYPE / THROWAWAY — Counter Room Commit Contract

This is not production code. It is a tiny, disposable probe for one design question: **can `commit(PreparedRoomCommit) -> CommitOutcome`, plus `resolve(OperationIdentity, CanonicalRequestHash) -> StoredResolution`, remain one deep semantic seam over SQLite and local PostgreSQL?**

Run both isolated adapters (Python 3.12, `psycopg2`, and PostgreSQL `initdb`/`pg_ctl` on `PATH`):

```sh
python3.12 prototypes/room-commit-contract/run.py
```

The PostgreSQL half creates a brand-new cluster below a safe `tempfile.TemporaryDirectory`, selects an unused loopback port, stops it, and removes it. It never opens, modifies, or relies on an existing database. If local binaries are unavailable, explicitly run the SQLite fallback:

```sh
python3.12 prototypes/room-commit-contract/run.py --sqlite-only
```

Open [walkthrough.html](walkthrough.html) directly in a browser for a no-install, click-through model. It uses in-memory state only; the page is deliberately separate from the database probe.

What the runner exercises: resolver-first submission prevents a retry from being reinterpreted against newer state; pure `prepare()` then constructs an opaque canonical-byte bundle; `commit()` locks the Room root, re-resolves, validates the complete Head and exact operational witnesses, conditionally consumes timer witnesses, and installs typed rows. New v2 operation results include a codec version and basis Head.

The controlled cases include real multi-connection SQLite `BEGIN IMMEDIATE` and scratch PostgreSQL Read Committed Room-root `FOR UPDATE` contention; same-ID Existing/Conflict behavior; hidden committed and hidden absent replies; exact timer identity after a fresh connection, unhealthy state, and archive; missing timer rows after preparation; timer generations starting at 1 and increasing exactly; strictly-forward schedules; atomic cancel-plus-reschedule; stable overdue ordering; Activity-owned half-open deadlines; generation-witness races; archive timer cancellation and activation fencing; same-sequence Head corruption; and replay from Genesis plus transitions without snapshots. Every injected bundle-write rollback compares the complete durable Room bundle—Head/current state, transitions, timers, frames, activations, and operation results—before and after.

The migration probe seeds literal frozen v1 bytes for a receipt, canonical transition, and retained timer. It interrupts the v2 DDL/data-copy transaction, proves the store remains version 1 with no partial v2 table, then upgrades successfully, freshly renders the legacy result, retains changed-hash Conflict behavior, and reaches the same replayed Head.

`--sqlite-only` deliberately reports a SQLite self-check, never a cross-adapter equality claim.

Proof limits: this is not a SQLite WAL/FULL, power-loss, or crash-consistency proof. It does not test managed failover, a real driver connection loss during `COMMIT`, fairness/starvation, scheduler or Host Clock correctness, performance, consensus, or production schema constraints and rollout. The rollback and indeterminate-result branches are injected control flow, not real SQLite busy/locked or PostgreSQL lock-not-available/deadlock classification; generic backend-error taxonomy remains unproven. The codec is deliberately small ad-hoc canonical JSON + SHA-256, not the production canonical codec/hash suite. Concurrent tests normalize either permitted winner instead of claiming identical lock-acquisition order.

The atomic prototype bundle is visible in `run.py`: Transition/hash chain; Room Head/current Core and Activity state; timer state; frame; activation intent/decision; and durable operation result. It intentionally excludes snapshots, delivery, telemetry, cursors, and claims. Host time is injected; there is no database/native-client time call. Structural SQL constraints are used only for shape—the semantic guards stay at the seam.

Three adapter findings stay below the seam: PostgreSQL and SQLite use different Room-lock mechanics; portable `INSERT ... SELECT ... ON CONFLICT` needs an explicit `WHERE TRUE` to avoid parser ambiguity; and physical result order differs, so every hash-relevant collection is explicitly ordered before encoding. None becomes a domain outcome.

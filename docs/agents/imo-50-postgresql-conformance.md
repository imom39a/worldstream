# IMO-50 PostgreSQL conformance evidence

This increment audits the PostgreSQL adapter against the SQLite kernel seams
without claiming a live PostgreSQL provider run. `FixturePostgresStore` remains
local evidence only; `probe_from_environment()` continues to report
`UnavailableEnvironment` unless an explicitly configured PostgreSQL 17 service
is reachable.

## Implemented evidence

- `src/conformance.rs` defines a provider-neutral black-box transcript over
  `RoomCommitStorageV1`. It records only resolution class, duplicate status,
  exact canonical receipt bytes, and guarded resolution. The test vector covers
  Create, same-identity duplicate, changed-request conflict, and receipt-first
  resolution.
- PostgreSQL migration `0003-kernel-conformance-v1` adds byte-preserving
  semantic receipts, observation reset/visibility consequences, activation
  intents and operation-receipt storage, paired snapshot cache shape, integrity
  incident shape, membership delivery positions, and the unique scheduled-timer
  witness index. Its checksum and the resulting catalog fingerprint are recorded
  in `compatibility.toml` and the generated JSON mirror.
- The live adapter now conditionally verifies a TimerFired generation and exact
  scheduled time/payload, locks and verifies frame-head witnesses, persists
  reset/visibility consequences, materializes Intent activation decisions, and
  stores the exact Core semantic-input/time/receipt bytes in the semantic receipt
  table. Any mismatch is a typed `NotApplicable`, `Fenced`, or `Fault` outcome;
  it is never silently rebased.
- The fixture tracks timer generations/states, frame heads and payload hashes,
  reset/visibility consequences, activation decision bytes, integrity generation,
  and (with the conformance feature) authority-fence drift. Receipt corruption
  is exposed only as a test seam and resolves as `ResolutionUnavailable`.
- Existing contention, rollback/lost-reply, restart-after-interrupted-migration,
  migration identity/checksum drift, downgraded/mixed history, and transaction
  pooler profile tests remain passing. New tests cover the provider-neutral
  transcript, receipt corruption, authority drift (feature-enabled), and the
  third migration contract.

## Explicit gaps / fail-closed boundaries

- PostgreSQL does not yet implement the public observation attach/ack/prune API,
  Activation offer/claim/renew/release/complete API, authorized receipt reads,
  recovery inspection/install, replay, snapshot post-commit writing, or
  integrity incident/repair operations. Adding their tables does not claim those
  operations exist. Production capability witnesses without a wired authority
  snapshot return `Fenced`.
- The SQLite conformance authority seeding and deeper fixture seams are private
  `cfg(test)` methods in `worldstream-sqlite`; the PostgreSQL integration target
  cannot legally drive them without changing the forbidden SQLite file. The
  shared transcript is therefore exercised against the PostgreSQL fixture in
  this increment, while the cross-adapter execution of richer SQLite-only
  operations remains an explicit blocker.
- No live PostgreSQL, direct-admin, session-pool, or transaction-pooler run is
  claimed by this document. The manifest remains release-incomplete for live
  provider patches and artifacts.

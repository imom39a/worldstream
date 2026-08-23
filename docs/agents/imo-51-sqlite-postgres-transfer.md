# IMO-51 SQLite-to-PostgreSQL transfer foundation

## Scope

This increment adds `crates/worldstream-transfer`, a small provider-neutral
logical-transfer contract plus a bounded durable-destination import seam. It
does not open a database connection, call a provider API, or claim that live
SQLite-to-PostgreSQL transfer has been proven.

The crate models the reviewed one-way direction from the bundled SQLite
profile to a PostgreSQL 17 primary. Its wire format is a versioned bounded
binary bundle (`WSTRANS1`) containing:

- exact BLAKE3-hashed pack, resource, and logical-record identities;
- canonical records separated from rebuildable derived records;
- an explicit session-state `InvalidateAndRebuild` policy;
- exact record bytes, including embedded zero bytes, without decode/re-encode;
- a source deployment epoch and lineage identity; and
- deterministic ordering and size/count limits.

The logical record vocabulary covers the canonical lineage, room, authority,
timer, frame/cursor, operation/receipt, activation/context, artifact, and
integrity evidence named by the transfer requirements. Derived snapshots,
indexes, delivery attempts, and telemetry are separately tagged and cannot be
mistaken for canonical truth. Unknown wire versions, classes, kinds, resource
tags, session policies, malformed lengths, trailing bytes, duplicate identities,
hash mismatches, and unsupported directions fail closed.

## Resumption and authority fence

`TransferCheckpointV1` chains contiguous chunk digests. Applying the exact
already-committed chunk is idempotent; a different replay or a gap is rejected.
The checkpoint itself has a versioned `WSCHECK1` encoding for a durable retry
seam.

`TransferLifecycleV1` expresses the offline fence:

1. `TransferPending` blocks source writes while export/import are in progress.
2. `TargetVerified` requires the next epoch, target profile, lineage, pack,
   resource identities, and complete bundle hash to match.
3. `finalize` creates the explicit retirement boundary.
4. Only the matching first target write in the next epoch can make the target
   authoritative; after that, abort and SQLite writes are rejected.
5. `abort` is safe only before the finalization record and restores the source
   authoritative state.

The new destination contract adds resumable serialized import state, idempotent
chunk replay handling, canonical record ordering and per-Room byte/digest
parity, exact source/target backend fingerprints, destination verification,
verification-gated finalization, target-authority commit, and abort cleanup.
Adapter operations still need to connect it to source quiescing, recoverable
SQLite backup, a real PostgreSQL target, full semantic verification,
nonterminal Activation lease fencing, native PostgreSQL checks, and
provider-backed finalization records.

## Reopened retirement-order defect closure

The live PostgreSQL adapter now closes the authority-ordering gap that remained
after the original foundation increment:

1. The first staged chunk installs an exact bundle/target importing fence on an
   atomically verified empty PostgreSQL target.
2. `verify_complete` hydrates canonical and operational rows, runs the existing
   PostgreSQL Room verifier plus executable replay, and persists the exact
   provider-derived `verified` marker in one transaction. The transaction
   temporarily removes the importing fence only from its own MVCC view and
   restores the same fence before commit, so the hydrated target never becomes
   serving-visible.
3. `record_finalization` rechecks staged chunks, exact native rows, deployment
   identity/resource bytes, Room semantics, and executable replay without
   repairing the target. Only that reconfirmed provider state can authorize
   SQLite retirement.
4. After SQLite retirement, `accept_target_write` is publication-only: it
   removes the exact importing fence and advances the already hydrated target
   to `authoritative`; its verification is non-repairing and cannot defer
   hydration until after source retirement.
5. Before the retirement boundary, abort atomically discards every PostgreSQL
   user-truth domain and writes an exact durable tombstone before the source is
   restored. Missing-state abort is also tombstoned, preventing a stale
   publisher from racing restored SQLite authority.

Deterministic destination fault injection covers hydration, executable replay,
verified-marker persistence, provider reconfirmation, finalization-marker
persistence, and rejection of a stale legacy `verified` marker. Every injected
pre-retirement failure leaves source retirement uncalled and proves that the
same import can still be coordinately aborted. A separate ordering witness
asserts `hydrate and verify target` → `persist verified target` → `retire
source` → `publish target authority`.

The native SQLite bridge deliberately does not fill absent source metadata,
pack/resource bytes, or hosted/native restore witnesses. The smoke lane must
remain incomplete until an adapter supplies those exact facts. The transfer
adapter also rejects canonical records whose kind and Room identity disagree,
and rejects canonical records for an explicitly isolated Room. Isolated
operational rows remain byte-preserved evidence, but this lane does not claim
that the current PostgreSQL publication seam can install them without
promoting them to healthy canonical state.

## Deterministic evidence

The in-crate fixture uses fixed identifiers, epoch `7`, fixed bytes, and a
fixed pack/resource digest. Focused tests prove:

- deterministic bundle encoding and byte-preserving round-trip;
- fail-closed unknown wire values;
- idempotent checkpoint replay, serialized checkpoint resume, and mismatch
  rejection;
- destination verification gates, restart after target commit, finalization,
  and abort cleanup;
- exact target identity/hash mismatch rejection;
- canonical deployment/Room parity and repair/resume after a missing
  destination chunk; and
- pre-retirement abort plus source/target epoch fencing.

Commands run from the repository root:

```text
cargo test --manifest-path crates/worldstream-transfer/Cargo.toml
cargo fmt --manifest-path crates/worldstream-transfer/Cargo.toml -- --check
cargo clippy --manifest-path crates/worldstream-transfer/Cargo.toml --all-targets -- -D warnings
```

The focused transfer crate has 40 passing tests. The reopened defect was also
verified from the repository root with:

```text
cargo test --manifest-path crates/worldstream-postgres/Cargo.toml --lib --locked transfer::tests::
uv run --python 3.14.7 --no-project python -m unittest -v tests.postgres_transfer_smoke
bash -n scripts/postgres-transfer-smoke.sh
scripts/postgres-transfer-smoke.sh --build-source --evidence /tmp/imo-51-transfer-evidence.json
```

The PostgreSQL transfer filter has 21 passing tests, the Python boundary suite
has 17 passing tests, and the disposable digest-pinned PostgreSQL 17.11 smoke
completed with exit `0`. Its evidence records passing whole-deployment,
provider-derived authority coordinator, exact Room/operational readback,
restart replay, source transfer lifecycle, empty-target preflight, and isolated
provider-abort witnesses. The JSON remains bounded operational evidence with
`release_evidence: false`; it does not claim production deployment evidence.

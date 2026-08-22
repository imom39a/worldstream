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

No live SQLite, PostgreSQL 17, provider, full WorldStream semantic verifier,
or production deployment evidence is claimed by these commands; the focused
crate suite has 24 passing tests.

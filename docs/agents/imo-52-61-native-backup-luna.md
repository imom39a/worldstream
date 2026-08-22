# IMO-52/IMO-61 native SQLite backup/restore continuation

Updated 2026-08-21.

## Implemented

This scoped continuation uses the explicit SQLite `canonical_export_metadata`
record initialized by the runtime deployment-identity lane. The native backup
bridge now:

- extracts and validates the operator-supplied deployment lineage and nonzero
  storage epoch from the single metadata row using the runtime's exact
  bounds: lineage at most 128 bytes and epoch `1..=9_007_199_254_740_991`;
- binds deployment lineage into `BackupManifestV1` and requires the restored
  target witness to match it exactly;
- compares source and isolated-target lineage and epoch in the native restore
  smoke runner without printing the raw lineage;
- keeps exact native SQLite BLOBs, canonical Genesis/Transition bytes,
  materializations, paired-snapshot evidence, integrity membership, and all
  modeled operational rows byte-preserved and read-only;
- rejects malformed or overlong lineage, out-of-range epochs, missing metadata,
  mismatched target metadata,
  tampered canonical evidence, incomplete healthy Room evidence, existing
  destinations, interrupted copies, and publication races without replacing
  the source or an existing target; and
- retains the provider-neutral verifier's typed fail-closed checks for native
  point identity, migration contract/checksums, exact Pack identities, exact
  resource identities/bytes, and complete Room membership.

The smoke runner still reports `release_evidence: false` and returns an
incomplete result when a complete manifest-backed image cannot be constructed.
It does not turn sidecar presence into synthetic SQLite rows.

The native envelope lane is now executable, not only a data model. A real
test performs bundled SQLite online backup, a second native restore into an
isolated path, exact source/target extraction, envelope sealing, construction
of `BackupImageV1`, pure replay through the production registry, and the
existing native verifier's `Ready` result. The same test lane rejects missing
resource/authoritative evidence, altered resource bytes, stale global/origin/
target facts, and changed source rows.

## IMO-52 criterion report

- Versioned native envelope/adapter: complete for bundled SQLite; schema,
  bounds, canonical JSON, and digest checks are explicit and fail closed.
- Product/compatibility/schema/backend point/storage epoch: complete for the
  supported product version and checked-in compatibility root; SQLite engine,
  native point, deployment lineage, epoch, and exact migration IDs/checksums
  are bound and re-read.
- Retained packs/resources: complete for compatibility-registered executor,
  schema, codec, and resource digests plus exact companion bytes. Pure replay
  uses only the production registry and never invokes pack side effects.
- Room evidence: complete for canonical Genesis-to-Head bytes, domain-correct
  SQLite hashes, materializations, authoritative companion bytes, integrity,
  complete membership, receipts, timers, frames, cursors, Activations, and
  causal/integrity relations.
- Ready barrier: complete for clean SQLite evidence only after source/target
  equality, native capture-witness verification, pure healthy-Room replay,
  Complete Head/materialization hash checks, and `verify_native_restore`.
  Pre-existing Faulted/Quarantined Rooms remain isolated.
- Negative coverage: complete for missing companion/resource/authoritative
  facts, resource tamper, stale global digest, caller-asserted origin, stale
  target epoch, changed source canonical bytes, migration tamper/order/gap,
  invalid operational relations, interrupted transfer, existing destination,
  and publication race. Diagnostics remain bounded/redacted.
- Side effects/release claim: complete for the local lane; no pack, runner,
  frame publication, policy, timer, source-history, or canonical mutation is
  invoked. `release_evidence` remains `false`.
- Remaining IMO-52 criteria: PostgreSQL provider-native snapshot/PITR/dump,
  provider durability, and crash/power-loss/disk-full/platform evidence remain
  unimplemented and are not claimed.

The envelope's JSON self-digest is only a corruption check. Authority comes
from two independent boundaries: (1) `backup_file`/`restore_file` mint a
non-serializable capture witness only after native copy plus exact bounded
source/target extraction, and (2) pack executor/schema/codec/resource
identities are checked against the hard-coded digest of the checked-in
compatibility manifest. `seal` and verification require the operation-minted
witness, bind the source/target evidence digests and content-derived native
point, and reject caller-asserted producer/capture text. Resealing JSON alone
cannot create a valid native point or trusted pack/resource evidence.

## Focused evidence

From `/Users/vinothshanmugam/code/agent-streamer`:

```text
cargo test --locked --manifest-path crates/worldstream-backup/Cargo.toml --lib       # 55 passed
cargo clippy --locked --manifest-path crates/worldstream-backup/Cargo.toml --all-targets -- -D warnings  # pass
cargo fmt --manifest-path crates/worldstream-backup/Cargo.toml -- --check             # pass
python3 tests/native_restore_smoke.py                                                   # 5 passed
bash -n scripts/native-restore-smoke.sh                                                 # pass
```

The smoke driver was also compiled through the harness against an invalid
SQLite source. It returned the typed incomplete result
`native_restore_or_source_verification_failed` and did not claim release
evidence.

The boundary/tamper tests accept exactly 128 bytes and the maximum safe epoch,
then reject 129 bytes and maximum-plus-one in both manifest validation and
native SQLite extraction/verification.

## Precise remaining blockers

The current SQLite schema and allowed APIs do not persist or expose:

- authoritative-state materialization bytes beyond the exact Core/Activity
  materializations available in the current SQLite schema;
- a provider/platform durability witness for filesystem, disk-full,
  process-kill, power-loss, or filesystem-corruption scenarios.

These omissions remain outside this local SQLite lane and are reported as
incomplete rather than being synthesized. PostgreSQL native
snapshot/PITR/dump evidence remains an operator/provider responsibility and is
not claimed by this SQLite lane.

## Changed files

- `crates/worldstream-backup/src/lib.rs`
- `crates/worldstream-backup/src/native_restore.rs`
- `crates/worldstream-backup/src/native_sqlite.rs`
- `crates/worldstream-backup/src/native_envelope.rs`
- `scripts/native-restore-smoke.sh`
- `tests/native_restore_smoke.py`
- `docs/agents/imo-52-61-native-backup-luna.md`

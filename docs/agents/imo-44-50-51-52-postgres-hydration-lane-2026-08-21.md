# IMO-44/50/51/52 PostgreSQL canonical hydration lane

Date: 2026-08-21

## Disposition

Implemented the PostgreSQL destination hydration lane in
`crates/worldstream-postgres/**`. The destination now accepts native SQLite
bundles whose healthy Rooms have complete canonical evidence while allowing
explicitly isolated source Rooms to remain non-promoted. An isolated Room is
accepted only when the target already has the matching faulted/quarantined
integrity root; its native operational rows remain exact-comparison evidence.

The publication transaction preserves exact Genesis, Head, Core/Activity,
pack-revision-lock, Transition, Membership, native operational, deployment
lineage, and storage-epoch bytes. Chunk coverage, target fence, metadata, and
existing-row comparisons remain fail-closed. Authority is not recorded until
the existing PostgreSQL semantic Room verifier reads the newly hydrated rows
through the same transaction. A failed semantic check therefore rolls back
the publication and leaves the import non-authoritative.

The verifier refactor is internal: the public read-only provider verifier is
unchanged, with an additional transaction-backed form used only by transfer
publication.

## Verification

From `/Users/vinothshanmugam/code/agent-streamer`:

```text
cargo fmt --manifest-path crates/worldstream-postgres/Cargo.toml -- --check  # pass
cargo test --manifest-path crates/worldstream-postgres/Cargo.toml --all-targets --locked --no-fail-fast  # 23 unit + 13 integration passed
cargo clippy --manifest-path crates/worldstream-postgres/Cargo.toml --all-targets --locked -- -D warnings  # pass
```

The existing live PostgreSQL environment was also run:

```text
scripts/postgres-live-evidence.sh --evidence /tmp/imo-44-50-51-postgres-live.json  # exit 13: transfer source not supplied
```

Live PostgreSQL adapter, migration/read-only harness, and direct/transaction
pool conformance passed. Transfer parity and target authority were not run
because no verified SQLite source file was available; this lane does not claim
full transfer acceptance.

No changes were made outside the requested PostgreSQL crate and this report.

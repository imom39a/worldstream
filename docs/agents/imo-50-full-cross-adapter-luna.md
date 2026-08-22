# IMO-50 full SQLite/PostgreSQL kernel conformance — Luna evidence

Date: 2026-08-21
Lane: PostgreSQL adapter and shared cross-adapter conformance
Status: shared IMO-50 conformance gate passed; not Linear Done; not `release_ready`

## Scope and implementation

The provider-neutral seam is `crates/worldstream-conformance/src/lib.rs`.
`ScenarioPlan`, `KernelConformanceAdapter`, `ScenarioEvidence`, and
`run_catalog` define one unchanged seven-row catalog:

| Scenario | Shared definition |
|---|---|
| create | create, duplicate, conflict, receipt resolution |
| Core admin | Core administration transition and receipt |
| Action | Participant Action transition and receipt |
| timer | conditional Timer generation and payload witness |
| frame/Cursor/reset | frames, Cursor, prune, reset, and visibility |
| Activation | offer, claim, renew, release, complete, and leases |
| recovery | restart, replay, snapshot/cache, and integrity recovery |

The runner rejects any adapter result with empty outcomes, projections, or
canonical facts, and rejects byte-less/non-pass projections. It does not emit
`incomplete` rows. `ConformanceArtifact::compare_adapters` compares the exact
canonical bytes and hashes after removing only the provider label.

The PostgreSQL adapter now provides the missing implementation surfaces behind
the feature-gated conformance seam:

- exact retained Genesis/Transition trace hydration through
  `CoreTraceV1::replay`;
- Core-authorized administration, Participant Action, and Timer commit
  preparation using Core’s public opaque sealers;
- transaction-locked observation suffix, Cursor acknowledgement, prefix
  pruning, and reset delivery with exact frame bytes/hashes;
- direct-admin snapshot cache deletion and exact Genesis/Transition rebuild;
- generation-fenced integrity incident recording and verification evidence;
- exact integrity incident rows in `PostgresRoomVerification`.

These methods keep reducer, timer-generation, authority-purpose, receipt, and
projection semantics in Core. PostgreSQL only hydrates retained bytes, locks
rows, and persists prepared writes. The conformance capability mode is
feature-gated and used only by the typed conformance methods; it is absent from
the default production build.

The shared live harness now projects both real observation deliveries into the
same typed frame/Cursor shape, including the actual prune reset witness. It
also compares the typed Activation offer receipt while separately retaining
the full durable receipt bytes. PostgreSQL offer receipt readback does not
apply claim-context retirement to the offer operation; claim/renew/complete
receipt retirement remains unchanged.

## Current acceptance state

The real SQLite and PostgreSQL adapters execute the unchanged seven-row
`SCENARIOS` catalog. The latest live run reports all seven rows `Pass`, exact
provider-neutral canonical bytes/hashes, receipts, projections, and semantic
outcomes equal on both direct PostgreSQL and PgBouncer transaction-pooler
paths.

Latest redacted evidence:

```text
/tmp/imo50-final-evidence-3.json
/tmp/imo50-final-evidence-3.json.imo-50-shared-comparison.json
```

The independent shared gate in the first file is:

```text
all_scenarios_pass=true
direct=pass
transaction_pooler=pass
scenario_count=7
```

The comparison artifact has `redacted=true`, `scenario_count=7`, and
`canonical_equal=true` for both `direct` and `transaction_pooler`; both
provider hashes are the identical
`321035549fb6bf918eb5cf9567bc7727ecd79d4e9793c420555f957f5e7640d3`.

## Local verification

Passing:

- `cargo test --locked -p worldstream-conformance --all-targets` — 2 shared
  contract tests pass; missing provider evidence fails closed.
- `cargo test --locked -p worldstream-sqlite --lib` — 95 tests pass after the
  overlapping SQLite lane stabilized.
- `cargo test --locked -p worldstream-postgres --all-targets --no-fail-fast`
  — 24 unit tests and 13 adapter tests pass without the conformance feature.
- `cargo test --locked -p worldstream-postgres --features conformance-tracer
  --all-targets --no-fail-fast` — 24 unit tests and 17 adapter tests compile
  and pass; the environment-gated live test is included in this target.
- `cargo check --locked -p worldstream-conformance --features
  live-conformance --test shared_live` passes.
- `cargo clippy --locked --no-deps -p worldstream-postgres --lib --features
  conformance-tracer -- -D warnings` passes.
- `cargo clippy --locked --no-deps -p worldstream-conformance --test
  shared_live --features live-conformance -- -D warnings` passes with only
  test-harness panic/manual-let/unused-self/map-unwrapping lints relaxed.
- `cargo fmt --all -- --check` passes.

The strict clippy invocation is currently blocked by unrelated dirty-worktree
errors in `worldstream-backup` (missing serde implementations and invalid `?`
use in its native SQLite hydration). The conformance crate’s own clippy
documentation lints have been fixed; no backup files were edited.

## Live PostgreSQL 17.11/PgBouncer evidence

Command:

```text
scripts/postgres-live-evidence.sh --evidence /tmp/imo-50-postgres-live-evidence-implementation.json
```

Latest result: exit code `13`, cleanup `pass`, `release_evidence=false`, and
`secrets_emitted=false`. Exit code `13` is caused only by the separate transfer
lane: its canonical-export mechanics pass, but global `pack_identities` and
`resource_identities` evidence is missing, so whole-deployment transfer
acceptance remains incomplete. It does not suppress or alter the independent
IMO-50 shared result above.

Pinned images observed:

- PostgreSQL `17.11`: `postgres@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73`
- PgBouncer: `edoburu/pgbouncer@sha256:4c1ca296ef525f108f5d3552cc337c0c09587cf8dae7f0067fd93349e47dc1cd`

The direct live adapter and shared runner were rerun after the receipt/frame
projection fixes; both direct and transaction-pooler catalogs pass. No release
or Linear state was changed.

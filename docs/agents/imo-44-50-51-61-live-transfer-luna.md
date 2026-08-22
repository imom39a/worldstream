# IMO-44/50/51/61 live SQLite to PostgreSQL transfer lane

Date: 2026-08-21

## Result

The parent reran the exact live command against pinned PostgreSQL 17.11 and
PgBouncer transaction pooling after the shared SQLite lifecycle edit compiled.
The aggregate result is intentionally **incomplete**, with exit code `13`, at:

`/tmp/imo-44-50-51-61-live-transfer-parent-rerun.json`

```text
scripts/postgres-live-evidence.sh --evidence /tmp/imo-44-50-51-61-live-transfer-parent-rerun.json
exit 13
```

Direct admin/runtime, transaction-pooler, live adapter, redacted harness, and
owned-container cleanup passed. Canonical staging mechanics also passed. The
aggregate remains incomplete because the source does not persist the complete
deployment pack-identity set or an explicit complete resource-identity set (or
empty-resource witness). The evidence contains no DSNs, passwords, lineage
values, or Room IDs.

## Safety correction

Canonical-export evidence is not a deployment authority transfer. The public
session API now fails closed for `CanonicalExport` bundles:

- `verify_canonical_export` performs complete staged-record verification only;
- `finalize` cannot record source retirement/finalization;
- `accept_target_write` cannot record target authority;
- serialized/restarted sessions reject forged `Finalized` or
  `TargetAuthoritative` states for this scope.

The existing native SQLite constructor remains whole-deployment strict and was
not weakened. The canonical bundle carries no source backend fingerprint, pack
identity, or resource identities. It retains exact lineage/epoch and canonical
Room records, plus isolated-source Room identities, without decoding or
rewriting payload bytes.

## Corrected live transfer evidence

The corrected redacted transfer invocation reported:

- scope: `canonical_export_only`;
- first chunk: `applied`;
- partial interruption chunk: `applied`, followed by real target abort: `pass`;
- same-range conflicting bytes: rejected;
- checkpoint replay: `alreadyapplied`;
- target epoch fence: `verified`;
- canonical staged verification: `pass`;
- staged target state: `verified`;
- finalization: `not_allowed_canonical_export`;
- target authority: `not_allowed_canonical_export`;
- target Room publication/readback: not performed for this non-authoritative
  scope;
- direct and transaction-pooler profiles: `pass`;
- cleanup: `pass`.

The parent rerun reproduced these transfer rows after the shared compile seam
cleared.

The source had no isolated/faulted Room in this disposable fixture, so no live
isolated-Room preservation claim is made. The native transfer unit suite still
covers isolated-corrupt byte preservation and non-promotion.

## Whole-deployment identity review

The source contains an exact per-Room pack revision lock and the Room head
pack digest. Those facts do not prove the complete retained deployment pack
identity set. The SQLite canonical export schema also has no reviewed,
explicit empty-resource witness or persisted resource-identity relation.
Therefore the lane does not elevate those observations into
`PackIdentityV1`/`ResourceIdentityV1` evidence and does not synthesize either.

The full IMO-51 acceptance row remains incomplete:

```text
canonical_export_mechanics = pass
whole_deployment = incomplete
blocking_rows = pack_identities, resource_identities
```

Missing IMO-52 native restore metadata is not used as a blocker for the
narrower canonical transfer mechanics. Native restore verification remains
strict for its own contract.

## Focused validation

- `cargo test --manifest-path crates/worldstream-transfer/Cargo.toml --locked`:
  **25 passed**.
- `cargo clippy --manifest-path crates/worldstream-transfer/Cargo.toml
  --all-targets --locked -- -D warnings`: **passed**.
- `cargo fmt --manifest-path crates/worldstream-transfer/Cargo.toml -- --check`:
  **passed**.
- `bash -n scripts/postgres-live-evidence.sh scripts/postgres-transfer-smoke.sh`:
  **passed**.
- `ruff check` and Python bytecode compilation for the two owned test files:
  **passed**.
- Python boundary suite: **16 tests OK**.
- Exact parent live command above: **exit 13**; direct/pooler/provider setup and
  canonical transfer mechanics passed, while whole-deployment acceptance
  remained incomplete by design.
- `cargo check -p worldstream-sqlite --locked`: **passed** before the parent
  live rerun.

## Owned files changed

- `crates/worldstream-transfer/src/lib.rs` — canonical-export scope, strict
  non-authoritative session behavior, serialization/restart guards,
  safe chunk construction, and lifecycle tests.
- `scripts/postgres-transfer-smoke.sh` — canonical staging/verification,
  partial abort, conflict, replay, and fencing evidence without finalization or
  authority claims.
- `scripts/postgres-live-evidence.sh` — redacted canonical mechanics in the
  parent direct/pooler report.
- `docs/agents/imo-44-50-51-61-live-transfer-luna.md` — this report.

No SQLite lifecycle/recovery sections, PostgreSQL adapter files, or release
manifests were edited.

## Remaining blockers

1. Persist and review the complete deployment pack-identity set.
2. Persist an explicit complete empty-resource witness or exact resource
   identity set; do not infer emptiness from absent rows.
3. Add an isolated/faulted Room to a reviewed live fixture before claiming its
   provider-level preservation path; this run observed none.

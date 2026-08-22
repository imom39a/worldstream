# IMO-51 PostgreSQL transfer lane

Date: 2026-08-21

## Scope

This lane owns only the PostgreSQL adapter, the PostgreSQL transfer smoke
runner, its boundary tests, and this report. SQLite, backup, workflow, gate,
browser, and packaging changes remain outside this lane.

## Completion

The PostgreSQL destination now requires and verifies the authoritative
`DeploymentIdentityV1` source record for whole-deployment transfers. Migration
`0008-deployment-identities-v1` persists the identity metadata, normalized pack
identities, and normalized resource identities in the same transaction as
canonical Room hydration and target authority. Empty resources are represented
by an explicit identity witness and zero resource rows; they are not treated as
missing evidence.

`persist_deployment_identity` is split into private metadata, normalized-row,
load, and reconstruction-verification helpers. The refactor preserves
fail-closed mismatch handling and transaction rollback without a broad Clippy
allow.

The smoke report now makes the authority boundary explicit:

- `source.transfer_contract_gate.name` is
  `authoritative_deployment_identity_v1`.
- `source.backup_diagnostic` is an independent native backup/restore readiness
  diagnostic with `authoritative_for_transfer: false` and
  `required_for_transfer: false`.
- Transfer pass requires source and target identity witnesses plus source and
  target membership witnesses. The pass wrapper rejects evidence that omits
  these fields, so a false backup-readiness result cannot mask missing transfer
  identity or membership.

## Parent live evidence

`/tmp/parent-imo51-transfer-final.json` is the parent’s fresh Docker live
result after the evidence-contract update:

- status: `pass`
- exit code: `0`
- provider: Docker PostgreSQL 17.11
- scope: `whole_deployment`
- record count: `12`
- operational row count: `2`
- identity metadata: present
- pack identity count: `1`
- resource identity count: `0` with explicit empty-resource witness
- pre-authority verification: `pass`
- finalization: `pass`
- target authority: `pass`
- target Room readback: `exact_bytes_and_hashes_verified`
- all acceptance rows: `pass`
- source transfer gate: `authoritative_deployment_identity_v1`
- source/target deployment identity and membership witnesses: `pass`
- independent backup diagnostic: non-authoritative and not required for this
  logical-transfer gate

## Verification

```text
cargo test --locked -p worldstream-postgres --all-targets --no-fail-fast
24 unit tests passed; 13 integration tests passed

cargo clippy --locked -p worldstream-postgres --all-targets -- -D warnings
pass

cargo fmt --manifest-path crates/worldstream-postgres/Cargo.toml -- --check
pass

bash -n scripts/postgres-transfer-smoke.sh
pass

python3 -m unittest tests/postgres_transfer_smoke.py
12 tests passed
```

The targeted regression test now supplies a canonical deployment identity
record, and migration-history assertions expect all 8 migrations. No Linear
state was changed.

## Parent disposition

All IMO-51 logical-transfer acceptance gates are satisfied. PostgreSQL-native
backup/restore remains independently tracked by IMO-52 and cannot mask or
weaken this transfer result.

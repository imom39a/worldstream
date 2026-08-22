# IMO-48/49/50/51/52 transfer and restore continuation

Date: 2026-08-21

## Scope

This Luna lane was limited to the permitted backup/transfer crates, the two
smoke runners, focused smoke tests, and this report. The dirty shared
checkout was preserved; no Rust Core, server, SQLite adapter, manifest,
release gate, or Linear state was changed.

## Confirmed defects fixed

### Native restore registry selection

`scripts/native-restore-smoke.sh` used `builtin_counter_registry()` for every
healthy Room. The daemon also emits the retained Agent Heist revision, so a
valid Heist Genesis was rejected during the smoke driver's replay check as
`healthy_room_executor_replay_failed`. The driver now uses the existing
`builtin_worldstream_registry()`, which contains the exact embedded Counter
and Agent Heist revisions.

### Empty operational relations during bundle validation

`NativeSqliteTransferAdapterV1::from_operational_rows` preserved all nine
modeled relations, including zero-row relations. `validate_bundle` rebuilt its
map only from emitted rows and therefore reported a valid empty relation such
as `activation_decisions` as missing. Validation now seeds every modeled
relation before decoding row records. A regression test covers a source with
empty timers, frames, consequences, and Activation relations.

The transfer smoke runner also records a stable redacted validation class if a
future adapter contract failure occurs; it never emits provider error text,
credentials, Room IDs, or source paths.

## Source and live commands

The live source was a disposable public-daemon Counter Room created through
the existing `POST /v1/rooms` route, then stopped before source inspection.
For this transfer run, explicit deployment metadata was persisted with the
existing `SqliteRoomStore::initialize_canonical_metadata` API in an ephemeral
driver:

```text
target/debug/worldstreamd --data-dir <temporary-data-dir>/data \
  --bind 127.0.0.1:<port>
POST /v1/rooms                       # public operator bearer, Counter v2
SqliteRoomStore::initialize_canonical_metadata(
  "deployment/live-transfer-test", 1
)

bash -n scripts/native-restore-smoke.sh scripts/postgres-transfer-smoke.sh
cargo fmt --manifest-path crates/worldstream-transfer/Cargo.toml -- --check
bash scripts/native-restore-smoke.sh \
  --source <temporary-data-dir>/data/worldstream.sqlite3
WORLDSTREAM_PG_TRANSFER_TIMEOUT_SECONDS=300 \
  bash scripts/postgres-transfer-smoke.sh \
  --sqlite <temporary-data-dir>/data/worldstream.sqlite3
```

The source had one healthy Counter Room, zero Transitions, one Genesis, one
paired snapshot, one semantic receipt, and empty timer/frame/Activation
relations. Its explicit metadata makes it complete for the reviewed native
canonical/operational transfer seam. It is not evidence that normal daemon
startup currently initializes deployment metadata, nor is it a full
`BackupImageV1` or provider-native PostgreSQL fixture.

## Verification results

Focused repository checks:

```text
cargo test --locked -p worldstream-transfer --lib                  # 22 passed
cargo clippy --locked -p worldstream-transfer --all-targets -- -D warnings # pass
cargo fmt --manifest-path crates/worldstream-transfer/Cargo.toml -- --check # pass
python3 -m unittest -v tests/native_restore_smoke.py tests/postgres_transfer_smoke.py # 15 passed
bash -n scripts/native-restore-smoke.sh scripts/postgres-transfer-smoke.sh # pass
```

Native restore after the registry fix:

```text
exit 13, status=incomplete, reason=native_restore_metadata_incomplete
source/destination canonical_ready=true
round_trip exact operational/canonical/materialization/snapshot/integrity=true
healthy_rooms_replayed=1, isolated_rooms_preserved=0
```

The remaining native result is intentional: the bridge still lacks the full
provider-neutral metadata image required for a release or full
`BackupImageV1` claim.

The original disposable Agent Heist source was also rerun directly:

```text
bash scripts/native-restore-smoke.sh \
  --source <heist-daemon-data-dir>/worldstream.sqlite3
# exit 13, reason=native_restore_metadata_incomplete
# healthy_rooms_replayed=1, isolated_rooms_preserved=0
```

That run is the direct regression proof that the prior
`healthy_room_executor_replay_failed` classification was removed without
weakening the metadata gate.

The PostgreSQL 17.11 live transfer advanced beyond the original
`native_bundle_validation_failed` result:

```text
postgres version 17.11: verified
exit 14, status=incomplete
reason=target_room_semantic_verification_failed
release_evidence=false, secrets_emitted=false
```

## Remaining blockers

The permitted transfer/restore files no longer contain the two reported
defects. The live target now fails at the next truthful boundary: the current
`worldstream-postgres` destination stages and verifies transfer chunks but
does not hydrate the canonical Genesis, Head, Core/Activity materializations,
pack lock, and Transition rows into provider-native Room tables. Fixing that
requires changes outside this lane's allowed paths (`crates/worldstream-postgres/**`)
and a corresponding live target verifier/fixture. No target semantic parity,
full PostgreSQL transfer finalization, or release evidence is claimed.

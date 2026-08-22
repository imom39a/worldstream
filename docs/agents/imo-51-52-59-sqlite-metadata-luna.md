# IMO-51/52/59 SQLite deployment-identity implementation lane

Updated 2026-08-21.

## Decision

`worldstreamd` now accepts an explicit, operator-supplied deployment lineage and
storage epoch through the versioned runtime configuration contract. The daemon
never derives either fact from a path, Room, secret, timestamp, schema state,
SQLite engine identity, or migration state.

The inputs are optional but must be supplied together:

- versioned TOML: `[storage] deployment_lineage = "..."` and
  `storage_epoch = <integer>`;
- environment overrides:
  `WORLDSTREAM__STORAGE__DEPLOYMENT_LINEAGE` and
  `WORLDSTREAM__STORAGE__STORAGE_EPOCH`.

Lineage is bounded to a canonical ASCII identity shape and storage epoch is a
nonzero integer no greater than `9007199254740991`. The typed values redact
their contents from `Debug` and effective-config output. Partial, malformed,
or out-of-range input is rejected before startup.

## Verified startup facts

For the bundled SQLite profile, startup currently verifies:

- the exact bundled SQLite version and source ID;
- the forward migration/schema startup path and database integrity checks;
- the authority bootstrap receipt; and
- the built-in Activity Pack registry and its executable/artifact checks.

The store also preserves exact canonical Room records when a Room is healthy.
The independent metadata is now initialized, when explicitly configured,
immediately after verified SQLite open and before the daemon can serve. The
existing serialized immediate transaction returns first initialization,
identical replay, or a conflict; a conflict fails startup.

## Precise blocker

`canonical_export_metadata` remains an independent witness. The daemon writes
it only from the paired typed configuration values through the explicit,
atomic, idempotent `SqliteRoomStore::initialize_canonical_metadata` call. An
absent pair leaves the row absent and does not manufacture evidence.

The remaining native-restore inputs are also not established by daemon
startup: a native consistent restore-point identity, persisted migration
checksums, exact pack/resource identity and bytes, and complete target Room
membership. The built-in registry is not a substitute for those source-side
or target-side witnesses, and the current daemon has no approved publication
API for them.

## Preserved fail-closed behavior

A freshly daemon-created SQLite database with no configured pair may continue
serving, but remains transfer-incomplete: `export_canonical_evidence` returns
`MetadataAbsent`, and the native bridge reports missing metadata rather than
synthesizing values. An explicitly configured pair is initialized on first
startup and an exact restart is accepted. Existing explicitly initialized
sources retain their atomic/idempotent conflict checks.

## Acceptance boundary

This lane unlocks source deployment-lineage and storage-epoch publication for
the SQLite daemon, including first initialization, exact restart, conflict
rejection, absence behavior, and redaction-safe config inspection. It does not
claim live SQLite-to-PostgreSQL transfer, native restore readiness, or IMO-59
release evidence. Native restore and transfer remain fail-closed until the
independent restore point, migration checksums, exact pack/resource bytes,
target membership, and provider-backed/live verification witnesses exist.

## Focused evidence

- `worldstream-runtime` library tests: 37 passed, including TOML/environment
  layering, invalid/partial rejection, and redacted/debug safety.
- `worldstream-sqlite` library tests: 90 passed, including the three canonical
  metadata tests for first initialization, exact replay, conflict
  serialization, and invalid-value rejection.
- `worldstream-runtime` clippy with `-D warnings`: passed.
- `cargo fmt` and `git diff --check` for the scoped implementation files:
  passed.

The `worldstreamd` binary test/lint could not compile because unrelated dirty
changes in the forbidden `crates/worldstream-server/src/lib.rs` reference the
undefined `browser_ticket_preflight` and `issue_browser_ticket` symbols. This
lane did not modify that file.

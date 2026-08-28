# Storage, backup, restore, and transfer

Use bundled SQLite for local development. Storage maintenance is explicit,
offline where required, identity-bound, and verified before authority moves.

## SQLite native operations

```sh
target/debug/worldstreamctl sqlite verify \
  --database <SOURCE.sqlite3>
```

Native backup publishes both exact backup bytes and a sealed full-verifier
envelope. Destinations must not already exist:

```sh
target/debug/worldstreamctl sqlite backup \
  --database <SOURCE.sqlite3> \
  --output <NEW_BACKUP.sqlite3> \
  --companion <OWNER_ONLY_COMPANION.json> \
  --envelope <NEW_ENVELOPE.json>
```

Restore requires the exact matching backup/envelope and a new destination:

```sh
target/debug/worldstreamctl sqlite restore \
  --backup <BACKUP.sqlite3> \
  --envelope <ENVELOPE.json> \
  --database <NEW_RESTORED.sqlite3>
```

`sqlite verify` is read-only native verification; it does not by itself assert
complete restore readiness.

## Studio live SQLite backup

Studio's live path uses a stable operation ID and a daemon-derived destination
under the fixed shared backup root. It performs SQLite online backup, reopens
the artifact, and compares deterministic source/destination exports. The
pathless result includes name, size, and checksum.

This is intentionally narrower than the offline full semantic restore verifier.
Studio reports that full semantic verification was not run.

## SQLite to PostgreSQL transfer

The supported logical migration is one-way and offline:

```text
begin → resume bounded chunks → finalize authority handoff
                         └────→ abort before handoff
```

`begin` freezes source authority and retains a verified backup/bundle/checkpoint.
`resume` is restartable while both ends remain fenced. `finalize` verifies the
target and performs the irreversible authority handoff. `abort` is allowed only
before target authority and discards incomplete target truth before restoring
SQLite authority.

Inspect exact arguments before any operation:

```sh
target/debug/worldstreamctl postgres transfer --help
target/debug/worldstreamctl postgres transfer begin --help
target/debug/worldstreamctl postgres transfer resume --help
target/debug/worldstreamctl postgres transfer finalize --help
target/debug/worldstreamctl postgres transfer abort --help
```

There is no reverse transfer, live dual write, automatic failover, or runtime
profile switch.

## PostgreSQL native restore

Provider-native rebuild/restore uses a protected artifact directory, one-use
restore identity, bounded worker processes, exact target fencing, durable
recovery journal, semantic verification, and explicit containment when cleanup
certainty is lost.

Use the full checked-in runbook rather than composing commands from memory:

```sh
target/debug/worldstreamctl postgres native --help
```

## Safety rules

- stop or fence writers exactly as the selected workflow requires;
- use new explicit artifact paths and owner-only directories;
- never overwrite an existing destination;
- retain the original backup/envelope/journal until post-restore readiness is
  proven;
- do not treat provider backup completion as WorldStream semantic readiness;
- never move authority based on a partial or path-substituted artifact.

Primary source: [operator storage commands](https://github.com/imom39a/worldstream/blob/main/docs/operator-storage.md),
[backup/restore ADR](https://github.com/imom39a/worldstream/blob/main/docs/adr/0004-supported-storage-profiles-and-offline-portability.md),
and [backup crate](https://github.com/imom39a/worldstream/tree/main/crates/worldstream-backup).

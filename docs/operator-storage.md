# Offline storage commands

The shipped `worldstreamctl` binary provides offline bundled-SQLite backup,
restore, and verification, plus the resumable whole-deployment
SQLite-to-PostgreSQL transfer. Stop `worldstreamd` before using these commands.
All artifact directories and input files must be owner-only; existing output
paths are refused.

Native SQLite evidence is admitted only as one standalone DELETE-journal main
database (`read_version=1`, `write_version=1`) with no `-journal`, `-wal`, or
`-shm` sidecar. Stopping the daemon is necessary but does not rewrite a WAL-mode
database header. The deployment's reviewed offline shutdown procedure must
checkpoint and transition the database to DELETE mode before invoking these
commands. WorldStream rejects WAL-header or sidecar-bearing inputs rather than
rewriting source evidence during backup.

## SQLite backup and restore

A complete SQLite backup is a pair: the native database image and its exact
sealed companion envelope. The command does not report success until both have
been published, reopened, and independently verified by the native SQLite
checks and the full WorldStream semantic verifier.

```text
worldstreamctl sqlite backup \
  --database /srv/worldstream/worldstream.sqlite3 \
  --output /srv/worldstream-backups/backup.sqlite3 \
  --companion /srv/worldstream-backups/companion.template.json \
  --envelope /srv/worldstream-backups/backup.envelope.json

worldstreamctl sqlite restore \
  --backup /srv/worldstream-backups/backup.sqlite3 \
  --envelope /srv/worldstream-backups/backup.envelope.json \
  --database /srv/worldstream-restore/worldstream.sqlite3
```

The canonical companion template supplies facts that SQLite intentionally does
not persist: exact trusted pack/resource bytes, canonical request witnesses,
authoritative materializations, and fired-timer relations. A deployment
integration must collect those facts at the same offline boundary. The backup
command binds the template to the operation-minted native capture witness,
seals it, and rejects missing, extra, substituted, noncanonical, or untrusted
facts. A native database file without the successfully published envelope is
an incomplete/orphaned artifact, not a complete WorldStream backup.

Restore admits the backup through an owner-only open handle, copies those exact
bytes to a private owner-only snapshot, and performs every native reopen and
source-evidence pass against that snapshot. It restores only to a new path and
reports `semantic_verifier: "pass"` only after rebuilding a full verifier
`Ready` result from the exact published envelope. Keep the backup and envelope
together and immutable.

`sqlite verify` is deliberately narrower:

```text
worldstreamctl sqlite verify \
  --database /srv/worldstream-restore/worldstream.sqlite3
```

It runs bounded native SQLite checks and reports
`semantic_verifier: "not_invoked"`. It is useful diagnostics, but it is never a
restore-readiness assertion and cannot replace `sqlite restore` with the sealed
envelope.

## SQLite-to-PostgreSQL transfer

The only supported transfer direction is one offline, whole deployment from
authoritative SQLite to an empty PostgreSQL 17 target. Use one dedicated
owner-only state directory. The backup and bundle must be direct children of
that directory. `transfer-id` is a stable non-secret identifier; use the same
value and paths when retrying `begin`.

```text
worldstreamctl postgres transfer begin \
  --sqlite /srv/worldstream/worldstream.sqlite3 \
  --backup /srv/worldstream-transfer/source.backup.sqlite3 \
  --bundle /srv/worldstream-transfer/deployment.bundle \
  --state-dir /srv/worldstream-transfer \
  --transfer-id deployment-move-2026-08
```

`begin` fsyncs an immutable intent before freezing the source, creates and
verifies the exact SQLite backup, rechecks the complete transfer point under an
immediate write transaction, enters `transfer_pending`, builds the deterministic
bundle, and publishes the first immutable checkpoint generation. A retry must
match the persisted intent. If the source changed between capture and freeze,
the operation rolls back, removes the verified backup, and leaves SQLite
authoritative.

Provider commands accept credentials only through an owner-only DSN file. Do
not place a DSN or password on the command line or in an environment variable.

```text
worldstreamctl postgres transfer resume \
  --sqlite /srv/worldstream/worldstream.sqlite3 \
  --bundle /srv/worldstream-transfer/deployment.bundle \
  --state-dir /srv/worldstream-transfer \
  --dsn-file /run/secrets/worldstream-postgres-admin.dsn \
  --chunk-records 64

worldstreamctl postgres transfer finalize \
  --sqlite /srv/worldstream/worldstream.sqlite3 \
  --bundle /srv/worldstream-transfer/deployment.bundle \
  --state-dir /srv/worldstream-transfer \
  --dsn-file /run/secrets/worldstream-postgres-admin.dsn
```

Each `resume` invocation applies at most one bounded, idempotent chunk and
publishes an immutable, fsynced checkpoint after that provider commit. An
already-complete resume is a no-op that creates no new generation. Repeat the
command until `chunks_complete` is true. `finalize` first persists the
pre-handoff state, reconfirms the durable provider and source witnesses,
performs the whole-deployment authority handoff, and then persists the terminal
state. After PostgreSQL is authoritative, SQLite cannot be resumed as
continuity.

Before finalization, abort discards/tombstones the matching provider import and
restores SQLite authority:

```text
worldstreamctl postgres transfer abort \
  --sqlite /srv/worldstream/worldstream.sqlite3 \
  --bundle /srv/worldstream-transfer/deployment.bundle \
  --state-dir /srv/worldstream-transfer \
  --dsn-file /run/secrets/worldstream-postgres-admin.dsn
```

Each operation holds an exclusive state-directory lock for its entire run.
Serialized generations are retry hints only: every retry revalidates the exact
bundle, retained backup path and bytes, SQLite authority fence, and PostgreSQL
provider state. Retry the same command after a crash. Do not edit, rename, or
add files in the state directory.

Successful output is bounded, redacted JSON containing only phases, counts,
epochs, and digests. It never contains a DSN, password, source path, state path,
or plaintext transfer ID.

## PostgreSQL native snapshot rebuild, restore, and recovery

PostgreSQL native restore is a direct-administration operation for PostgreSQL
17.11 on Linux and Windows. The source and target coordinates must identify
different databases. `require` is authenticated, hostname-verified TLS and is
valid only for TCP endpoints. `disable` is accepted only for a numeric
loopback address or, on Unix, an absolute local socket directory. Credentials
are read only from an owner-owned mode-0600 libpq passfile; never put a
password or credential-bearing DSN in argv or the environment.

The target is deliberately disposable and non-serving. Before restore, a
direct superuser must prepare an empty database with exactly this database
comment and a connection limit of zero:

```sql
COMMENT ON DATABASE worldstream_restore IS
  'worldstream/native-postgres-disposable-target/v1';
ALTER DATABASE worldstream_restore CONNECTION LIMIT 0;
```

The native admission probe also requires the exact database to contain no
user objects or other client sessions. Restore temporarily admits only the
retained superuser keeper plus one random, short-lived non-superuser restore
role. It seals the database back to limit zero, terminates every session for
that role across the cluster, and removes the role before accepting semantic
verification.

Use one new owner-only artifact directory for the dump, canonical report, and
crash-recovery journal. First obtain its retained filesystem identity:

```text
worldstreamctl postgres native directory-identity \
  --path /srv/worldstream-native-restore
```

Record the fixed-width `storage_id` and `file_id` from that bounded receipt.
Pass both values to restore and recovery. This causes the command to reject a
renamed or substituted directory before creating a child or changing the
target.

If the source fixture requires rebuilding disposable snapshot caches, run the
reviewed packaged command first. Its receipt includes the exact source
provider identity used by the rebuild:

```text
worldstreamctl postgres snapshots rebuild \
  --host source.example \
  --port 5432 \
  --database worldstream \
  --username postgres \
  --tls-mode require \
  --passfile /run/secrets/worldstream-postgres.pgpass
```

Run restore with exact provider binaries and new dump/report names:

```text
worldstreamctl postgres native restore \
  --source-host source.example \
  --source-port 5432 \
  --source-database worldstream \
  --source-username postgres \
  --source-tls-mode require \
  --target-host target.example \
  --target-port 5432 \
  --target-database worldstream_restore \
  --target-username postgres \
  --target-tls-mode require \
  --passfile /run/secrets/worldstream-postgres.pgpass \
  --pg-dump /usr/lib/postgresql/17/bin/pg_dump \
  --pg-restore /usr/lib/postgresql/17/bin/pg_restore \
  --psql /usr/lib/postgresql/17/bin/psql \
  --dump /srv/worldstream-native-restore/source.dump \
  --report /srv/worldstream-native-restore/restore-report.json \
  --artifact-directory-storage-id 0123456789abcdef \
  --artifact-directory-file-id 0123456789abcdef0123456789abcdef \
  --timeout-seconds 1200
```

The command returns success only after its contained worker tree is drained,
target repair and credential retirement are proven, the exact dump and report
are durably published without replacement, and the recovery journal records
acknowledgement. Stdout is only a bounded completion receipt; the authoritative
redacted Ready report is the file named by `--report`. It binds the source and
target provider identities, dump digest and size, native backup point, every
durable-domain digest, and the unified verifier result. The completion receipt
also binds the exact retained-handle identities of the final dump, report, and
acknowledged recovery journal, plus the journal's validated basename. Cleanup
automation must match those identities before touching any nonempty artifact.

If the command is interrupted after destructive admission, do not delete or
edit `.worldstream_native_recovery_*.json` or any associated artifact. Keep the
target non-serving and retry exact recovery with fresh administrator
credentials:

```text
worldstreamctl postgres native recover \
  --target-host target.example \
  --target-port 5432 \
  --target-database worldstream_restore \
  --target-username postgres \
  --target-tls-mode require \
  --passfile /run/secrets/worldstream-postgres-recovery.pgpass \
  --recovery /srv/worldstream-native-restore/.worldstream_native_recovery_ID.json \
  --recovery-storage-id 0123456789abcdef \
  --recovery-file-id fedcba98765432100123456789abcdef \
  --artifact-directory-storage-id 0123456789abcdef \
  --artifact-directory-file-id 0123456789abcdef0123456789abcdef \
  --timeout-seconds 1200
```

Use the recovery-journal identity from the original committed receipt, or
observe and retain that exact journal before invoking recovery after an
interrupted run. Recovery rejects a pathname replacement before it reads the
journal or starts target repair.

Recovery first rebinds to the journaled target cluster/database identity,
restores connection limit zero, retires the exact one-use role and all of its
sessions, and only then validates or scrubs the journaled private/public
artifact identity. It appends a durable completion tombstone; it does not mint
a Ready witness. A containment-uncertain failure deliberately preserves the
journal and performs no concurrent target or artifact cleanup. Escalate that
record for operator recovery rather than deleting its directory by pathname.

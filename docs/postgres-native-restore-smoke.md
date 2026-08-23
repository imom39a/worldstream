# PostgreSQL native restore smoke

`scripts/postgres-native-restore-smoke.sh` exercises the PostgreSQL 17.11
custom-format dump and restore path. Its result is operational smoke evidence,
not release evidence: every output has `release_evidence=false`, and the lane
never publishes the restored target.

## Running the lane

On a Linux Docker host, run:

```text
scripts/postgres-native-restore-smoke.sh [--evidence /absolute/result.json]
```

The default fixture uses the digest-pinned PostgreSQL 17.11 image. The script
creates two disposable containers, seeds the source through the PostgreSQL
transfer fixture, restores into the isolated target, observes the restore
credential lifecycle, and removes only the exact container IDs returned by
successful `docker run` calls.

External source and target endpoints may instead be supplied through the
`WORLDSTREAM_NATIVE_PG_SOURCE_*` and `WORLDSTREAM_NATIVE_PG_TARGET_*`
variables. External targets must be non-serving, empty, and marked with the
database comment `worldstream/native-postgres-disposable-target/v1`. Passwords
are supplied only through an owner-only `WORLDSTREAM_NATIVE_PG_PGPASSFILE`.
Both endpoints require an explicit TLS mode. Remote TCP endpoints accept only
authenticated TLS; plaintext is restricted to numeric loopback and Unix
socket endpoints. Provider commands receive a cleared environment containing
only the reviewed PostgreSQL settings and required Windows runtime variables.

The standalone Docker lane is Linux-only because it binds mounts and generated
client wrappers through `/proc/<runner-pid>/fd/<descriptor>` paths backed by
retained descriptors. Windows release coverage uses
`scripts/postgres-native-restore-windows-live.ps1`, whose native handle identity
and cleanup rules do not rely on Unix `dir_fd` or procfs behavior.

## Evidence contract

Successful stdout and `--evidence` output are one canonical JSON line with
schema `worldstream/native-postgres-restore-smoke-evidence/v1`. The object
contains:

- `native_restore`: the complete, independently validated
  `worldstream/native-postgres-restore-evidence/v2` report;
- `live_restore_concurrency`: the exact keeper and one-use restore-role
  observation, including distinct backend PIDs and post-restore role cleanup;
- projected target isolation/publication fields; and
- `secrets_emitted=false` and `release_evidence=false`.

The driver receipt is not evidence. The smoke accepts only the exact committed
receipt field set, retains the reported dump, report, and recovery-record
identities, streams the exact retained dump bytes through the receipt's BLAKE3
digest under a fixed size bound with before/after identity and size checks,
verifies the report digest and canonical bytes, binds provider and dump
identities into the full report, and runs the shared native-report validator
before constructing the smoke wrapper. Any malformed, missing, additional,
mismatched, growing, mutated, or non-ready content fails closed.

The worker performs semantic replay before it can produce a checkpoint. The
room evidence compares Membership, Timer, Activation-decision, frame/cursor,
and reset/visibility witnesses and binds every typed field in the source and
restored fingerprints. Before returning either success or failure after target
admission, it durably writes connection limit zero, retires the one-use role,
and re-observes the target as marked, identity-bound, and isolated.

## Local authority and cleanup

Every smoke-owned temporary child is created once with `O_EXCL`. A keeper
retains a verified read-only handle for each creation identity before releasing
the creation writer. Generated `pg_dump`, `pg_restore`, and `psql` wrappers are
passed to the Linux driver through separately identity-checked read-only
descriptors, so replacing their pathnames cannot select replacement code.
This is pathname and inode identity authority, not immutable-byte sealing. A
process already running under the smoke runner's OS identity with an existing
writable handle to an admitted wrapper inode could still mutate that inode in
place. The standalone lane therefore requires isolation from untrusted
same-identity processes and does not claim sealed executable bytes; hosted
release lanes bind their packaged client tools independently.

The keeper emits bounded-rate heartbeats on its retained output pipe. Before
and after cleanup mutation, the parent drains old heartbeats and requires a new
one from the exact worker. Closing the parent pipe is the only shutdown signal:
the worker observes the broken pipe, exits, and its direct process-substitution
wrapper waits for that exact child. The parent stores no keeper PID and cannot
signal a recycled numeric process ID.

Cleanup first opens and validates the complete expected manifest. A narrowly
named unmanifested `.worldstream_*` placeholder is accepted only when its exact
retained handle is one owner-only, single-link, empty regular file; its handle,
pathname identity, link count, and size are rechecked before any mutation.
Cleanup then truncates only expected files through the exact preflighted
handles. Unknown placeholders are never scrubbed. A substituted root, child,
wrapper, or placeholder makes cleanup fail without mutating the replacement.

The protected temporary directory and scrubbed placeholders are intentionally
left for the supervising platform runner to remove. The standalone script does
not recursively delete a temporary pathname.

Exit codes are `0` for pass, `10` for unavailable prerequisites, `12` for
configuration errors, `13` for incomplete evidence, and `14` for cleanup
failure. Diagnostic mode must not be used to print passfile contents; normal
stdout and stderr remain secret-free.

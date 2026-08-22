# IMO-44/49/50/51 PostgreSQL live-evidence Luna lane

This lane adds `scripts/postgres-live-evidence.sh`, an evidence-only
orchestrator around the reviewed PostgreSQL harness and SQLite-to-PostgreSQL
transfer runner. It owns a temporary Docker network, a pinned PostgreSQL
17.11 Alpine image, and—when the pinned image is available—a static-auth
PgBouncer transaction pool. Credentials remain process-local and the final
JSON always reports `secrets_emitted: false` and `release_evidence: false`.

## What the runner attempts

1. Start PostgreSQL 17.11 at the recorded image digest with separate admin and
   least-privileged runtime roles.
2. Start PgBouncer at the recorded digest with `pool_mode=transaction`, no
   `auth_query`, and a temporary static auth file. The runner verifies the
   pooler admin console and runtime connection before treating it as usable.
3. Run the feature-gated `worldstream-postgres` live vector for direct-admin
   migration/restart, runtime DDL denial, operation resolution, same-identity
   concurrency, stale-head/authority-fence behavior, rollback/unknown-commit
   resolution, normalized transcript, and pooler duplicate resolution.
4. Reset only the disposable database's durable test rows, revoke runtime
   writes on `worldstream_schema_migrations`, and run the existing redacted
   `scripts/postgres-harness.sh` from a clean logical state.
5. If `--sqlite PATH` is supplied, run the existing transfer verifier against
   a separate disposable PostgreSQL database. Canonical bytes, source
   migration identity, target epoch fencing, checkpoint replay, read-back
   parity, and authority promotion remain governed by that reviewed adapter;
   incomplete source evidence is not upgraded to a pass.

The aggregate can return `pass` only when direct/provider conformance, the
transaction pool, the redacted harness, and the supplied transfer source all
pass. A local pass is still operational evidence, not a release or hosted
provider claim.

## Commands

Boundary checks:

```text
bash -n scripts/postgres-live-evidence.sh
python3 -m py_compile tests/postgres_live_evidence.py
python3 -m unittest -v tests/postgres_live_evidence.py
```

Live direct/pooler attempt:

```text
scripts/postgres-live-evidence.sh --evidence /tmp/worldstream-postgres-live.json
```

With a verified native SQLite source:

```text
scripts/postgres-live-evidence.sh \
  --sqlite /path/to/worldstream.sqlite3 \
  --evidence /tmp/worldstream-postgres-live.json
```

The command prints one compact JSON object. Nonzero results are expected when
Docker, the exact image digest, PgBouncer, or canonical SQLite source
evidence is unavailable. The wrapper does not print raw provider logs, DSNs,
passwords, Room IDs, or temporary paths.

## Evidence boundary

This lane does not edit compatibility manifests, production crates, release
evidence rows, or Linear state. It does not claim hosted PostgreSQL, remote
TLS, cross-platform artifacts, power-loss recovery, or full Core hydration
when the source/target adapters report those boundaries as incomplete.

# ADR 0004: Support Two Storage Profiles and One-Way Offline Portability

Date: 2026-08-15

Status: Accepted

## Context

WorldStream needs a zero-dependency default and an ordinary hosted-database option without creating two semantic products. Operators also need a supported path away from a local deployment, but live switching, dual writes, and distributed ownership would expand the correctness boundary.

## Decision

The frozen releases support exactly two startup-selected durable storage profiles behind one backend-neutral contract:

- the default release-bundled SQLite profile, with the exact linked build and required pragmas pinned by the Storage Compatibility Manifest; and
- `postgres-primary`, using one writable hosted or self-managed PostgreSQL 17 primary through direct or bounded transaction-scoped connections.

Both profiles run one WorldStream process and preserve identical canonical bytes, hashes, receipts, timers, frames/cursors, Activation fencing, Room Commit resolution classes, failure classes, recovery, and replay. PostgreSQL provider APIs, extensions, session state, named prepared statements, replicas, or HA services are never correctness dependencies.

Migrations follow one logical, checksummed, forward-only history with backend-specific execution. Production SQLite migration is an exclusive startup operation after a verified recoverable backup. Production PostgreSQL migration is an explicit offline direct-admin operation; the daemon verifies schema and uses a least-privilege runtime role.

WorldStream supports one resumable whole-deployment transfer direction: offline SQLite to an empty PostgreSQL target. The transfer is two-phase and Storage-Epoch-fenced, copies canonical serialized bytes without decode/re-encode, verifies the full target while neither store serves, and makes PostgreSQL authoritative only at explicit finalization. The source then becomes a read-only recovery artifact.

## Consequences

The default remains simple, while a remote PostgreSQL primary does not impose a process-location limit. Transfer may abort safely before source retirement; after the PostgreSQL epoch accepts its first write, returning to SQLite is not continuity.

Live switching, dual writes, reverse transfer, authoritative replica reads, rolling mixed versions, automatic failover, consensus, multi-process serving, and provider-specific HA or durability claims are excluded.

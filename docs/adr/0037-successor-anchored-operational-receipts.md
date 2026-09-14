---
status: accepted
date: 2026-09-14
---

# ADR 0037: Merkle proofs for bounded operational reads

## Context

ADR 0036 makes V2 checkpoint recovery independent of retained Frame,
consequence, and Activation-decision rows. Its durable V2 root receipt is
trusted after guarded checkpoint/tail verification, but it does not yet give a
serving read an authenticated inclusion proof for one retained row. A reader
can verify a Frame payload hash and canonical encoding, but not a coordinated
replacement of both payload and hash.

## Decision

Each operational domain receives an append-only Merkle-mountain-range sidecar.
The commit transaction derives a domain-separated canonical leaf from the exact
prepared Frame, consequence, or Activation decision, appends its logarithmic
node set, and writes the resulting MMR root to the existing durable V2 root
receipt. Sidecar nodes are not trusted: a bounded reader fetches the requested
leaf and sibling path, recomputes the root, and compares it to the root guarded
recovery already trusts. A changed row, adjacent hash, omitted, reordered,
duplicate, stale, or malformed node cannot verify without a BLAKE3 collision.

The migration is additive. Rooms lacking a complete MMR inventory keep the V2
root-only contract and use reset/full-verifier fallback for proof-required
reads; no serving path scans legacy history to synthesize a proof. New writes
atomically update retained rows, MMR nodes, and durable roots. SQLite and
PostgreSQL use identical canonical leaves and node hashes; transfer verifies
node inventory and roots before a Room serves.

## Consequences

- Proof work is `O(log n)` sidecar nodes plus one bounded retained row.
- The public Room wire and frozen V1 lineage remain unchanged.
- Both providers need matching node schemas, commit ordering, proof queries,
  transfer checks, and coordinated payload-plus-hash tamper tests.
- The design preserves the existing trust boundary: arbitrary rewrite of
  canonical history, checkpoints, and their durable roots is out of scope for
  an internal proof and requires an external authenticated store.

## Rejected alternatives

- Recomputing a retained ledger root on every serving read violates the bound.
- A mutable sidecar root alone is not an anchor; a mutable sidecar path is
  sufficient when verified against the trusted durable V2 root.
- Binding same-Transition operational output into immutable Transition bytes
  creates a fixed point because Pack observations may depend on `head_after`.

---
status: accepted
date: 2026-09-14
---

# ADR 0037: MMR proofs for bounded operational reads

## Context

ADR 0036 makes V2 checkpoint recovery independent of retained Frame,
consequence, and Activation-decision rows. Its frozen rolling roots let recovery
check a bounded checkpoint plus tail, but they do not provide an inclusion path
for one retained row. A reader can verify canonical encoding and an adjacent
payload hash, but coordinated replacement of both still passes that local
check.

A proof path is useful only when its root has already crossed the recovery
trust boundary. A mutable database sidecar root cannot authenticate nodes kept
in that same database. The V2 root algorithm and witness encoding are frozen,
so replacing their meaning with a Merkle root would also break compatibility.

## Decision

Each operational domain receives a separate append-only binary-forest Merkle
mountain range (MMR). The V2 rolling root remains unchanged. A V3 checkpoint
witness carries both the V2 receipts and the MMR receipt for each domain. The
checkpoint witness is content-hashed and tied to the exact checkpoint Head;
Core verifies the checkpoint record, replays the bounded immutable tail, and
advances both receipt families before storage admits them. Serving reads compare
their proof with that admitted MMR root.

The domains are `frames`, `consequences`, and `activation_decisions`. Each row
gets a zero-based `mmr_leaf_index` in commit order. The leaf bytes use the same
length-prefixed canonical entry already consumed by the V2 rolling root:

* Frame: member ID, frame sequence, cause Room sequence, payload hash.
* Consequence: member ID, cause Room sequence, consequence kind, and projection
  hash when present.
* Activation decision: cause Room sequence, decision ID, target member ID or an
  empty value, and canonical decision bytes.

The leaf hash binds the algorithm tag, domain, leaf index, and canonical entry.
Parent hashes bind the algorithm tag, height, start index, and ordered child
hashes. The receipt root binds the domain, leaf count, and ordered current
peaks. These rules are implemented once in `worldstream-core`; adapters only
persist nodes and execute the coordinates produced by Core.

The operational row, its leaf index, every new immutable MMR node, and the
receipt count/root update occur in one database transaction. Receipt updates
compare the previous count and root. Existing node coordinates are immutable;
a conflicting value faults the commit. SQLite and PostgreSQL therefore produce
the same nodes and roots for the same ordered entries.

A proof-required read loads the row and its leaf index, loads the receipt
admitted during recovery, asks Core for the exact logarithmic node coordinates,
and fetches only those nodes. It reconstructs the canonical entry from the row
and verifies the proof before releasing bytes. Missing, extra, duplicate,
misordered, wrong-domain, wrong-index, wrong-cause, stale, or altered proof
material fails closed. Observation catch-up returns Reset when continuity or a
proof is unavailable. Internal Activation history returns an integrity error.

Rows may be pruned while their immutable MMR nodes and receipt remain. A pruned
Frame range therefore causes the existing explicit Reset behavior; a retained
row remains independently provable. Node compaction may happen only if it
preserves every proof needed by retained rows. This version keeps the nodes.

Pre-V3 Rooms have no trusted MMR receipt. They remain readable through the
existing V1/V2 full-verification and reset fallback. No serving request scans
legacy history to synthesize a proof. A later verified rebuild may install a
new V3 checkpoint and MMR inventory atomically.

Backup, native restore, and streamed SQLite-to-PostgreSQL transfer include leaf
indices, MMR receipts, and nodes. Admission verifies uniqueness, coordinate
alignment, node hashes, receipt roots, and exact source/destination equality
before the destination serves the Room. The full verifier recomputes both the
frozen V2 roots and the MMR from ordered retained rows when the complete row set
is available.

## Consequences

Proof work is one bounded row plus at most logarithmically many nodes. The
public Room wire and frozen V1/V2 formats stay unchanged. Storage grows by one
leaf node plus carry nodes per operational entry; total node count remains
linear and each proof remains logarithmic.

The database protects atomicity and detects accidental or partial corruption.
An attacker able to rewrite canonical transitions, checkpoints, witness hashes,
and all roots remains outside this internal-storage threat model; resisting that
attacker requires an independently authenticated external anchor.

## Alternatives considered

Paged segment roots reduce node count but still require an authenticated path
from a segment to a trusted root and introduce page-sealing and partial-page
rules. Recomputing a complete retained root on each read violates the bound.
The canonical cause Transition does not always commit the exact same-transition
operational output because Pack observations can depend on the resulting Head.
An adjacent payload hash or an unanchored sidecar root authenticates no more
than the mutable row beside it.

---
status: proposed
date: 2026-09-14
---

# ADR 0037: Successor-anchored operational receipts

## Context

ADR 0036 deliberately makes V2 checkpoint recovery independent of retained
Frame, consequence, and Activation-decision rows. Its compact roots bind a
checkpoint to the current storage receipt, but do not give a bounded serving
read an authenticated inclusion proof for a retained row. A reader can verify
a Frame payload hash and canonical encoding, but a coordinated replacement of
both payload and hash remains indistinguishable from the original row.

The obvious design, putting the operational root produced by a Transition
inside that same Transition hash, is not valid. A Pack observation receives the
post-Transition Complete Head, including the Transition hash. Its Frame bytes
may depend on that Head. Hashing those Frame bytes into the same Transition
therefore creates a cryptographic fixed-point requirement.

## Proposed decision

New Rooms opt into a new immutable Transition lineage version. A V2
Transition hashes the *previous* operational receipt hash. After its semantic
Transition hash is known, the commit coordinator obtains the exact operational
outputs and stores one immutable per-Transition receipt. The receipt commits
separate canonical leaf lists for Frames, consequences, and Activation
decisions, including their address, cause sequence, canonical bytes or payload
hash, and prior receipt hash.

The successor Transition binds that receipt hash in its own immutable semantic
lineage. A bounded serving read fetches the row's cause Transition, its
per-cause receipt batch, and the successor anchor. It verifies the requested
row against the batch and returns it only when the receipt has a successor
anchor. The live tail with no successor is reset-required rather than served
incrementally. This avoids a fixed point and keeps validation bounded by one
Transition's output, which is capped by the V2 membership and activation
limits.

V1 Rooms remain explicit full-replay/forensic-verifier rooms. A database
migration creates V2 receipt and leaf tables only; it must not synthesize
anchors for existing V1 lineage. Transfers copy V2 receipts and verify their
exact canonical bytes and successor bindings.

## Consequences

- New V2 Rooms have one-transition delivery latency for an incremental Frame.
  A reset is safe during that tail; a later Transition anchors the row.
- The transition decoder and hash input need a versioned V2 path, while V1
  canonical bytes and hashes remain frozen. Genesis records select the lineage
  version for a Room; adapters must persist and transfer that selection.
- SQLite and PostgreSQL need the same transaction ordering, receipt schema,
  bounded proof query, successor fence, and coordinated payload-plus-hash
  tamper tests.
- This design relies on the existing trust boundary that canonical Genesis and
  Transition records are immutable. Protecting a database adversary that can
  rewrite canonical transitions as well requires an external signature or
  independently authenticated checkpoint service.

## Rejected alternatives

- A mutable sidecar Merkle root alone is not an anchor: an attacker able to
  replace a row can replace that root and its proof.
- Recomputing a retained ledger root on every serving read violates the bounded
  read contract.
- Binding same-Transition operational output directly in its Transition hash
  is rejected because Pack observations may depend on `head_after`.

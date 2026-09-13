---
status: accepted
date: 2026-09-12
---

# ADR 0033: Scoped external-input ingress

WorldStream admits one narrowly allowlisted generic external input through a
host-authorized HTTP envelope. The envelope carries an immutable source and
input identity, exact `based_on_room_seq`, exact pinned Pack digest, canonical
payload, and an optional original recorded time. The source/type allowlist is
closed in v1; there is no connector catalog, network capability, arbitrary Pack
dispatch, or Activity Start special case.

The existing Core `ExternalInputV1` commit path remains authoritative. The
gateway canonicalizes and bounds the payload, checks the exact current Pack
pin, authorizes the host ExternalInput operation, reserves the first semantic
recorded time by source/input identity and request hash, and then commits with
the existing current Head, integrity, and authority fences. Retries and
restarts resolve the durable receipt first and reuse the first recorded time;
changed payload, source, type, Pack pin, or basis conflicts rather than
rebasing. Pack reducers own the meaning of out-of-order revisions and
corrections.

SQLite and PostgreSQL expose the same endpoint and durable preparation schema.
Admission uses the existing bounded host-stimulus lane, so overload is
reported as busy and does not create an unbounded ingress queue.

## Consequences

- Existing Activity Start semantics and route compatibility remain unchanged.
- A future Pack may explicitly accept the generic typed input; the host does
  not infer Pack behavior or provide a connector integration.
- The durable preparation row stores only identity, request hash, and semantic
  time. Payload bytes remain in the normal canonical receipt/Transition path.

## Rejected alternatives

- Treating every external input as Activity Start was rejected because start is
  a lifecycle contract with its own phase and approval rules.
- Accepting arbitrary source/type strings or connector metadata was rejected
  because it would create an unreviewed integration catalog and weaken Pack
  pinning.

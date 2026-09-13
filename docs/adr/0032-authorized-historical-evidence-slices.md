---
status: accepted
date: 2026-09-12
---

# ADR 0032: Authorized historical evidence slices

WorldStream exposes a narrow evidence adapter for Runner and controller
diagnostics. A request is authorized through the caller's current Membership
Capability and the requested historical Role at one exact Room sequence. The
adapter returns only durable Transition identity metadata and an addressable
reference; canonical Genesis/Transition bytes remain behind the verified
Replay/storage boundary, and model summaries are never authoritative history.

Each response is fenced to the requested sequence and integrity generation.
Pages use a strict keyset cursor (`after_room_seq`) and fixed limits of 128
references, 256 KiB encoded reference metadata, and 250 ms capture time. A
caller must use the returned cut and last sequence for the next page; the
adapter never silently moves the cut or auto-replays a stale cursor.

Missing, pruned, and retired evidence are explicit outcomes. An empty page at
the cut is `exhausted`, not an implicit absence. SQLite and PostgreSQL expose
the same protocol shape and use immutable retained lineage, so this adapter is
not a second history store or a generic Pack-storage escape hatch.

The HTTP contract is
`GET /v1/rooms/{room_id}/evidence?at_room_seq=N&after_room_seq=M`.
`at_room_seq` is both the authorization point and the stable page cut;
`after_room_seq` is an exclusive keyset cursor. A successful reference has the
following bounded shape:

```json
{
  "room_seq": 418,
  "transition_id": "01K4EXAMPLETRANSITION0000000",
  "transition_hash": "blake3:…",
  "previous_lineage_hash": "blake3:…",
  "evidence_reference": "worldstream://room/01K4EXAMPLE/transition/418"
}
```

The reference is an opaque address. WorldStream does not currently expose a
network dereference endpoint. A future resolver must repeat present authority,
historical Membership, Room integrity, and exact-reference checks before it
returns any payload. Possession of the reference never grants payload access.

## Privacy and authority matrix

| Present authority and state | Historical state at `at_room_seq` | Result |
| --- | --- | --- |
| Current enabled Room Member Capability with `room:replay` | Same Member existed and was eligible for the requested historical Role | Bounded metadata references |
| Current enabled Room Member Capability with `room:replay` | Member had not joined, had departed, was suspended, or had another Role | Denied by the Replay facade |
| Expired, revoked, wrong-Room, or wrong-Member Capability | Any | Denied before storage access |
| Current Capability without `room:replay` | Any | Denied before storage access |
| Runner identity without a delegated Room Member Capability | Any | Denied; Runner identity alone conveys no Room history authority |
| Any caller | Quarantined Room or changed integrity fence | Denied or retried; no references are released |
| Authorized caller | Retained prefix is unavailable | Explicit `missing`, `pruned`, or `retired` outcome with no references |

For example, an agent evaluating a disputed assessment at sequence 418 can
request references through its still-current Member Capability, cite the
returned transition identity in its bounded Pack context, and ask an authorized
operator workflow to resolve it later. An agent that joined at sequence 500
cannot use that same endpoint to discover sequence 418 under a Role it did not
hold. A leaked `worldstream://` string also yields no evidence because it
carries no authority.

## Consequences

- Present authority is rechecked by the existing Replay facade before a page
  is released, which also proves historical Membership eligibility.
- References are useful for an external evidence resolver while preserving
  privacy; the response cannot be used as current Projection state, and the
  resolver remains a separate future capability.
- The current stores retain complete Transition records, so `pruned` and
  `retired` are modeled outcomes for retention implementations and are not
  manufactured by dropping rows in this adapter.

## Rejected alternatives

- Returning canonical replay bytes was rejected because it would create a
  second unbounded export path and weaken the existing privacy boundary.
- A generic Pack or evidence query was rejected because it would bypass
  authorization, stable fences, and the retained exact executor.

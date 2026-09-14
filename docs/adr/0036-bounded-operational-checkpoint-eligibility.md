# ADR 0036: Bound checkpoint operational-witness eligibility

## Status

Accepted

## Context

ADR 0031 bounds the immutable Transition tail, but its V1 operational witness
eagerly reads every retained Timer, observation Frame and consequence,
Membership, and Activation decision. The 16 MiB encoded-witness ceiling is
checked only after that collection has been materialized. It therefore does
not bound checkpoint capture or the checkpoint recovery allocation.

The current Core checkpoint path reconstructs the complete Timer generation
map and passes all retained operational materializations to the guarded
installation check. Any exact implementation must therefore read every such
row that it uses. A partition manifest or incremental domain root can verify
one page, but cannot make the total reconstruction cost independent of the
number of rows under that Core contract.

## Decision

Checkpoint acceleration is an eligibility cache with a fixed V2 capture
contract. Each operational domain may contribute at most 1,024 rows. Timer,
Frame-payload, consequence-payload, and Activation-decision binary values may
each contribute at most 4 KiB. Adapters request at most the limit plus one
ordered row, and use a SQL conditional value expression so an oversized binary
value is never materialized in the adapter process. The existing 16 MiB
canonical witness limit remains an independent final bound.

A row or value beyond a limit means the paired snapshot receives no
operational witness. It is not truncation: checkpoint recovery is ineligible
and uses the complete Genesis-to-Head recovery path. This preserves exact
serving semantics and the existing fail-closed behavior for absent, malformed,
tampered, stale, or fence-mismatched witnesses.

SQLite and PostgreSQL use the same limits, ordering, limit-plus-one test, and
fallback result. The current canonical V1 witness bytes remain the stored
format because this change constrains cache admission rather than changing the
meaning of a witness. Full canonical history and the native operational rows
are retained for forensic replay and backup verification.

Newly prepared Rooms also admit at most 1,024 distinct Timer IDs. Reusing an
existing ID is allowed; introducing an additional ID fails preparation before
persistence. The limit bounds the Core Timer generation map and permits an
operational table to retain only the current generation/state per ID in a
future compatible migration. Existing V1 histories are replayed unchanged,
including those above this limit, and remain ineligible for that acceleration.

## Consequences

Eligible checkpoint capture has a fixed logical row and binary-value budget;
its Core recovery retains ADR 0031's bounded immutable tail. The Core
lower-bound remains explicit: a Room with more than the allowed retained
operational continuation is still recoverable exactly, but only through full
replay. Supporting bounded accelerated recovery for that Room requires a
future Core storage-continuation interface that pages and verifies exact
operational materializations, plus a domain-root/partition protocol. A bare
manifest is insufficient.

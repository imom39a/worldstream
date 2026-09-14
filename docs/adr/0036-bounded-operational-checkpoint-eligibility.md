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

New rooms likewise admit at most 1,024 membership identities, and a current
room at the cap cannot introduce another identity. The admission check does
not reinterpret stored V1 Genesis or Transition records. It bounds the
checkpoint frame-head and membership-generation maps for rooms eligible for
the V2 path.

The host also rejects new pack descriptors declaring more than 16 MiB of
Activity state. Together with the current 1,024 membership and Timer-ID
admission limits, this bounds the serving data that a V2 checkpoint decodes.
Legacy records outside these limits use the V1/full-replay path.

## Consequences

Eligible checkpoint capture has a fixed logical row and binary-value budget;
its Core recovery retains ADR 0031's bounded immutable tail. The Core
lower-bound remains explicit: a Room with more than the allowed retained
operational continuation is still recoverable exactly, but only through full
replay. Supporting bounded accelerated recovery for that Room requires a
future Core storage-continuation interface that pages and verifies exact
operational materializations, plus a domain-root/partition protocol. A bare
manifest is insufficient.

## Amendment: compact current-state witness

The V2 continuation interface is now implemented for Rooms created after the
root migration. It keeps the immutable Timer, Frame, consequence, and
Activation-decision ledgers as forensic truth, while a checkpoint carries only
the bounded current Timer materialization, member frame heads and membership
generations, and exactly three domain-separated incremental root receipts.

Each adapter updates the current-Timer cache and roots in the same transaction
as its ledger mutation. Capture reads no retained Frame, consequence, or
Activation-decision rows; it reads at most 1,025 current Timers, 1,025
members, and four root rows. Recovery advances the captured roots through the
immutable bounded tail, then compares them with the durable receipts and
compares current Timers and members with the recovered state. A missing,
duplicate, malformed, stale, reordered, or mismatched receipt is a checkpoint
cache miss followed by complete replay; it never permits a partial operational
history to become serving state.

The frozen V1 witness table and full-replay recovery remain available for
legacy Rooms, including databases upgraded after a Room already has history.

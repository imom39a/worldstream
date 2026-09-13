---
status: accepted
date: 2026-09-12
---

# Bound refresh Activation attention and fence supersession

## Context

An Attention Signal is canonical evidence, while an Activation Intent is
durable operational work. A Room that changes faster than a Runner can claim
work can otherwise retain one pending intent for every distinct cause
sequence. Cause based identity is required for auditability, so changing the
deduplication key cannot solve this growth.

## Decision

Policy revision `worldstream/activation-attention-policy/v1` bounds pending
refresh considerations per Room Membership to 64 intents and 1 MiB of
estimated attention bytes. A refresh consideration is an intent without a
semantic deadline; it asks a Runner to reconsider current state. When a new
refresh would exceed either limit, the oldest pending refreshes are marked
`cancelled` with terminal disposition `superseded_refresh`, the new Activation
identity is retained, and `superseded_by_activation_id` records the newer
identity. A refresh larger than the byte budget is retained as a cancelled
`refresh_capacity_exceeded` identity. Pending refreshes older than 24 hours
are terminally recorded as `refresh_age_exceeded` when the offer path runs.

An intent with a semantic deadline is an obligation. It is never superseded by
this refresh policy; the Pack's bounded Activity State remains the source of
any unresolved domain obligation. Timers remain governed by ADR 0007 and are
never coalesced or dropped. A Membership has at most one live lease. Lease
generation, claim identity, operation receipts, revocation and deadline checks
continue to fence old Runners. Completed executions are rate limited to 60 per
Membership per minute by the offer path; this is an operational admission
limit and does not alter canonical history.

The policy metadata is additive operational state. It is persisted by both
SQLite and PostgreSQL under migration 0016, survives restart, and is included
in transfer/restore parity where Activation rows are copied. Existing rows
without metadata are treated as old refresh considerations and are eligible
for bounded retirement. No workflow engine or new Pack callback is added.

## Consequences

Every superseded identity remains queryable with an explicit terminal reason,
so a newer refresh cannot silently erase an earlier promise. Refresh bursts
remain bounded in count and estimated bytes, while durable obligations retain
their own discoverable representation in Pack state. The policy does not claim
that an unbounded Pack obligation set is safe; Pack revisions must keep that
state bounded. An absent Runner leaves pending obligations and refreshes for a
replacement Runner, subject to expiry and policy limits.

The policy is intentionally versioned and measurable: status reports can expose
pending count, oldest creation time, supersession and age-retirement counts,
execution count, and timer progress. Backend-specific clock and SQL details
must produce the same state transitions and disposition vocabulary.

## Considered options

- Dropping older Activation identities was rejected because it hides retained
  operational promises and breaks audit correspondence.
- Coalescing canonical Attention Signals or timer firings was rejected because
  those records are part of the Room's authoritative history.
- Reusing one Activation identity for all causes was rejected because identity
  includes the causing Room sequence and operation receipts must remain exact.
- Giving leases or pending work priority over timers was rejected by ADR 0007;
  timer obligations remain in the shared bounded Room lane.

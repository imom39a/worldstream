---
status: accepted
date: 2026-09-10
---

# Close hosted launch lineages with durable Host evidence

## Context

A retained Room Setup Operation makes an ambiguous launch retry safe: the Host
will reconcile the same operation instead of creating a second Room. That same
safety becomes a deadlock if the user cannot terminally close the retained
lineage. Discarding only the platform row or setup key would be unsafe because
an earlier Host request could still complete, create a Room, bind a House
Runner, or consume capacity after the platform lets the user start again.

The existing pre-start abandonment path is intentionally narrow. It cannot be
the creator's general escape hatch once the Lobby launch commits or Genesis has
been observed, and a creator must not gain arbitrary Room administration merely
because they initiated a hosted Launch Request.

## Decision

Expose one creator-facing **Hosted Launch Closure** for every nonterminal hosted
launch lineage. Product copy may say `Cancel setup`, `Stop setup`, `End
activity`, or `Finish closing` according to observed state, but every label
invokes the same operation. The operation closes; it never deletes or rewinds.

The Hosted Activity Platform first authenticates the Launch Request creator,
retains one immutable closure intent, and moves the Launch Request to
`closing`. It must keep the original Room Setup Operation identity and capacity
reservation while closure is uncertain. A repeated request with the same
canonical input resumes the same operation; changed input conflicts.

The Host then performs these steps under the launch lineage's serialization
boundary:

1. Install a durable closure fence before inspecting setup or Runner state.
   New launch submissions, House Runner reservations, bindings, and starts for
   that lineage must conflict with the fence, including after Host restart.
2. Reconcile the retained Room Setup Operation. If no Room can exist, report
   `cancelled_before_genesis`. If an ambiguous creation request has already
   committed or must be resolved to discover its outcome, retain that exact
   operation and archive its resulting Room instead of guessing that creation
   failed.
3. When a canonical Room exists, archive it through the normal authenticated
   Core administration path. Archival preserves Canonical History and is
   idempotent; there is no hosted hard-delete authority. If an immutable Pack's
   incremental observer faults while delivering an otherwise state-neutral
   Archive, the Host records a validated full-projection reset instead. This
   narrow delivery fallback requires unchanged Activity state, no Domain
   Events, and the exact active-to-archived Core transition, so a retained Pack
   defect cannot veto creator closure or conceal an Activity-owned change.
4. Stop or terminally fence every House Runner reserved for the lineage. An
   already retired Runner is stronger terminal evidence and need not be
   restarted merely to stop it again.
5. Return exact closure evidence only after the applicable Room archival and
   Runner fencing are durable. Evidence binds the installation, Launch
   Request, Room Setup Operation, closure key and fence, plus the archived Room
   and Head when a Room exists.

The Hosted Gateway exposes only this narrow service-authenticated command. The
browser receives no Host credential, Room identifier, Principal, Membership,
or generic archive route. Creator authorization remains a platform decision;
the Host validates exact service evidence and lineage correspondence.

After validating Host evidence, the platform records it immutably, releases
the lineage's pre-Genesis and active-Run capacity reservations, and moves the
Launch Request to terminal `closed_by_creator`. A crash or timeout leaves it in
`closing`; reads and the bounded recovery sweep retry the same closure. The UI
derives its available action from server state and never offers creation or
entry while closure is pending.

The browser retains its create idempotency key across an ambiguous response.
It may replace that key only after a later create response proves the retained
Launch Request terminal, including `closed_by_creator`; it then submits one new
Launch Request in the same user action. It must never rotate a key merely
because a request timed out or closure is still pending.

```text
collecting_roster | provisioning | reconciling | run_created
                              |
                              v
                           closing  <--- exact retry / recovery
                              |
                Host fence + terminal evidence
                              |
                              v
                    closed_by_creator
```

The existing expiry and pre-start abandonment mechanisms remain valid for
their automatic, proven-before-start cases. Hosted Launch Closure is the
creator-facing recovery and stop primitive across the whole lineage.

## Consequences

- A retained setup key cannot permanently block its creator from starting a
  later game; capacity is freed once terminal Host evidence is recorded.
- Ambiguous creation is resolved safely even when that requires observing and
  immediately archiving the one retained Room.
- Post-Genesis closure is represented by an archived Room, not erased history
  or a fabricated Activity Outcome.
- Activity history identifies creator closure explicitly rather than claiming
  that every closed lineage ended before a Room existed.
- `closing` may remain visible during an outage, but it is recoverable and
  cannot race a late launch or Runner start.
- The implementation needs durable Host fences, exact closure evidence,
  platform intent/evidence records, and recovery tests across process restart.

This decision extends
[ADR 0021](0021-use-reviewed-pre-genesis-formation-for-hosted-activity-runs.md)
and preserves the authority boundary from
[ADR 0019](0019-separate-hosted-activity-platform-from-worldstream.md).

---
status: accepted
date: 2026-09-12
---

# ADR 0035: Durable external-effect requests and reconciliation

An external effect is an application-level request derived from one committed
Room transition. Its stable business operation ID and monotonically increasing
work revision identify the intended business work. The request also stores the
committed Room sequence and Head hash, Pack digest, transition identity, and
exact preconditions. Action identity, Activation identity, effect identity,
and attempt identity remain separate; an attempt ID is generated for each
delivery and is never used as the target's idempotency key.

The application journal has explicit `pending`, `applied`, `failed`, `unknown`,
and transient `reconciling` states. A target receives the operation/revision
key and must make that key idempotent. A lost reply, including a reply lost
after target application, is recorded as `unknown`. Reconciliation probes the
target and changes state only from an explicit answer. An unavailable or still
ambiguous probe leaves the effect `unknown`; it is never guessed or retried as
if it were known absent. Failed work is retried only by an explicit operation.

Dispatch requires exact equality of the saved Room sequence, Head hash,
integrity generation, work revision, owner identity, eligibility, and optional
Activation lease identity and generation. Reconciliation receives a fence
before probing and a second fence before installing the probe answer. A
cancellation, supersession, owner change, lease loss, or Room update at either
cut leaves the record `unknown`. The target identity is checked before
invocation. These fences prevent stale Room state, stale revisions, expired
leases, and lost claimants from causing effects. The in-memory prototype
exposes a snapshot/restore seam; durable server storage persists the same
journal record before dispatch and rehydrates it after restart.

## Consequences

- The system provides at-least-once delivery with target idempotency and an
  honest uncertain outcome. It does not claim exactly-once side effects.
- A target must provide a bounded, authorized probe for its operation key.
- Room commits remain the source of truth; replay and model summaries do not
  cause effects.

## Rejected alternatives

- Retrying an unknown request without probing was rejected because it can
  duplicate a target that already applied the request.
- Using an Action or Activation ID as the effect key was rejected because
  those identities have different lifecycles and fencing rules.
- A generic workflow engine or real connector was rejected; this ADR defines
  only the application pattern and a narrow target port.

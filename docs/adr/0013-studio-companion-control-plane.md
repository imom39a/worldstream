# ADR 0013: Add Studio as a Companion Control Plane

Date: 2026-08-23

Status: Accepted

## Context

ADR 0001 deferred broad task-management and general operator UI work until the
two reference releases and outside-adopter gates were complete. The WorldStream
Studio roadmap now makes a local operator companion part of the product plan.
That roadmap changes release ordering, but it does not require moving Room
authority or model execution into a new process.

## Decision

The Studio roadmap supersedes ADR 0001's release-order constraint for
companion-product and operator-portal work.

Studio is separately served and backed by a local Supervisor. The Supervisor
may expose small, versioned, typed operations designed for specific operator
workflows. It must not expose arbitrary command or shell execution.

`worldstreamd` remains the only authoritative Room runtime. Studio and its
Supervisor cannot directly mutate Authoritative Room State, Canonical History,
storage materializations, or Room lifecycle values. Any requested Room change
must use a supported, authorized daemon API and retain the daemon's existing
admission, authority, persistence, and Replay semantics.

Model execution and model-provider integration remain outside `worldstreamd`.
The Supervisor may later coordinate external Runners, but that cannot merge
Runner authority, participant Action authority, or daemon Room authority.

## Consequences

- Studio can evolve as an operator-facing product without becoming the
  Participant Console or a second Room runtime.
- The first Supervisor interface is a read-only daemon status snapshot derived
  from the daemon's health, readiness, and manifest-backed version endpoints.
- Existing Room behavior, storage semantics, Replay guarantees, and Participant
  Console behavior remain unchanged.
- ADR 0001 remains authoritative for all boundaries except the superseded
  release ordering for Studio companion work.

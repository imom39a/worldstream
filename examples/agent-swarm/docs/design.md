# Agent Swarm design

Agent Swarm is a preserved application experiment. It illustrates how a domain
Pack and an external coordinator can share durable state without moving model
execution into the kernel. The implementation and its deterministic tests remain;
there is no claim of a maintained autonomous-agent product.

## Authority boundaries

One goal occupies one Room. The immutable roster establishes participant
identities separately from short-lived provider processes. The Pack owns goal
confirmation, work ownership, contributions, candidate results, checks, review
findings, and acceptance. Core owns ordering, exact Room Heads, Memberships,
authorization, transcript durability, and Timer generations.

The application owns provider selection, process supervision, scheduling,
capacity, budgets, and local artifacts. Its execution journal records operational
recovery state, not a second authoritative copy of Pack state. Pause and stop
control local execution; they do not freeze Room Semantic Time.

A provider process cannot commit arbitrary shared state. A scoped participant
submits an offered Action with its observed Head; the Pack validates domain
preconditions. Entity revisions add a second fence for work and result changes.

## Result lifecycle

Work items have explicit owners, attempts, dependencies, and blockers.
Contributions refer to retained artifacts. Integration produces a versioned
candidate, which needs the required checks and independent review before result
acceptance. A narrative success report is not acceptance evidence.

Ownership handoff requires interruption and reconciliation of the prior attempt.
Pending writebacks and unresolved shared-resource conflicts prevent reassignment.
Late output remains attributable to its original attempt. Human directions and
review corrections become durable domain facts rather than hidden coordinator
instructions.

Progress Timers create review obligations. They do not launch provider processes;
the external coordinator decides whether and how to service an obligation.
Completion cancels the active Timer, and stale generations are suppressed.

## Application modules

| Area | Source in `app/src/` | Responsibility |
| --- | --- | --- |
| Domain and application | `domain.rs`, `application.rs` | Application requests and lifecycle |
| Room integration | `backend.rs`, `managed_local.rs` | Scoped Controller/Runtime operations |
| Coordination | `coordinator.rs`, `coordinator_service/`, `planning.rs` | Assignment and progress policies |
| Execution | `execution/` | Provider adapters, process ownership, and recovery journal |
| Artifacts and checks | `artifacts.rs`, `checks.rs`, `code_change*.rs` | Bounded artifact resolution and candidate evidence |
| Presentation | `tui/` | Terminal views and explicit human actions |

The default application runs deterministic fixtures. The `managed-local-runtime`
feature adds the real local Controller/Runtime integration. Provider adapters
were exploratory and depend on exact native CLI behavior and qualification
records; they are not maintained against current provider releases. Controlled
worker tests establish coordination behavior, not real-model quality.

## Running and inspecting

See the [application reference](../app/README.md) for commands and the
[Pack reference](../../packs/agent-swarm/README.md) for the retained revisions,
Action semantics, and check-evidence contract. Repository-wide authority and
Replay guarantees are described in [architecture](../../../docs/architecture.md).

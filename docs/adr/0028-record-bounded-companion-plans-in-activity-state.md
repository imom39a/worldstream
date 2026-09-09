---
status: accepted
date: 2026-09-09
---

# Record bounded companion plans in Activity State

Archive needs companions to continue useful work across player-committed
turns within a small model allowance. Record each **Companion Plan** as the
result of one authenticated, schema-valid companion Action, with at most three
typed steps. Apply at most one eligible step per later Activity Turn. This
keeps task progress recoverable and replayable while avoiding a model call for
every movement or routine operation.

The Pack owns the recorded Companion Task, its limits, accepted plan, progress
and resulting game facts. The Runner owns model execution. A plan records its
origin Membership and task revision; it is invalidated or blocked by task
changes, cancellation, loss of participant eligibility, newly unmet
preconditions or a decision requiring information not yet available. A selected
step is fenced to its current preparation and Activity Turn; advancing the turn
alone does not invalidate eligible remaining steps. Current
Role ownership is derived from Core; no parallel mutable Role-to-Membership
index is introduced. No plan confers permanent or broader participant
authority.

Steps are finite Pack operations with a closed vocabulary, not future
protocol requests, arbitrary scripts, loops or new callbacks. Each execution
rechecks current task limits, resources, knowledge/access and participant
eligibility. A resolving human Action can apply consequences of previously
accepted plans without impersonating the companions. History records the
original proposals and subsequent resolving Transitions; Replay never calls
the provider. Blocked work does not manufacture success or silently spend a
reserved resource.

Use bounded, serialized planning opportunities and strict current-Head
admission for initial proposals. A short plan does not remove stale-head
hazards. Late replies, cancelled opportunities and changed tasks are fenced;
they cannot authorize a new turn or resurrect an old task. The Pack cannot
inspect provider health or wait for a model inside reduction. The human can
explicitly continue without a contribution, or use disclosed Pack-defined
regroup/follow behavior under their own Role, without replacing an agent's
Principal or fabricating its Action.

Essential task facts and permitted discoveries are supplied through current
authorized Projections. Bounded dialogue can be retained as game facts with
the correct audience. For the new companion mode, this narrowly extends ADR
0022 to permit a bounded window of accepted, audience-scoped dialogue in the
current Projection. External hidden conversation memory and cross-run memory
are not added; execution caps remain unchanged. Longer conversations and
provider failures still consume the existing allowance; storing steps does
not increase that allowance. The current helper supports bounded
arrays/objects but not schema unions or references, so use compact
closed-schema steps and reducer-level relationship checks.

This chooses recorded bounded plans over a call on every turn, opaque
Runner-only task state, or an unbounded plan interpreter. The cost is explicit
planning, invalidation, privacy and conflict rules in the Pack. The five
callback boundary and separation of activation from Action authority remain
unchanged. Implementation and qualification remain outstanding.

References: [Activation and Action authority](0003-separate-activation-and-action-authority.md),
[Pack contract](../activity-packs.md),
[House execution](0022-use-bounded-openrouter-house-runners-for-exhibition-fill.md),
[Archive design](../midnight-archive-design.md).

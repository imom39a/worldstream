---
status: accepted
date: 2026-09-15
---

# Represent each Swarm goal with one Room

Each Swarm is one goal-directed activity backed by exactly one WorldStream
Room. Returning to the same goal retains the same Room and Participant
identities; starting a separate goal creates another Swarm. Reusable roster
configurations are separate from the lifetime of a Swarm. This keeps goal
history, membership, review, and accepted results attributable to one activity,
at the cost of explicitly carrying useful material into later goals rather
than treating an indefinitely reused team conversation as shared authority.

Human steering may amend the existing goal through authorized Pack Actions.
Directions stop new affected work, request interruption of affected Invocations,
and require affected results to be revalidated. Explicitly reopening a completed
goal retains its Room and produces a newly reviewed result version; starting a
separate goal creates another Swarm. Completion preserves the accepted result
and stops automatic agent work without archiving the Room. This decision adds
no cross-Room transaction, history fork, or new Core Room State field.

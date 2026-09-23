---
status: accepted
date: 2026-09-15
---

# Separate Swarm Directions from execution controls

Human steering distinguishes binding Swarm Directions, which amend the
accepted goal or constraints, from advisory Swarm Suggestions. An accepted
Direction stops new affected work, requests interruption of affected
Invocations, and requires affected results to be revalidated before acceptance.
Human direction uses participant authority; it does not expand tool access
or turn Host Operator authority into domain Action authority.

Directions can target the whole Swarm or specific Work Items and their
affected dependencies. An agent-focused shortcut identifies the agent's
affected current work, so reassignment does not lose the Direction. The TUI
shows the target before submission.

Agent-led corrections following a Swarm Progress Review may create corrective
Work Items and revise plans within the accepted goal and permissions. They do
not become Human Directions or authorize goal, constraint, or permission
changes. Repeated unsuccessful corrections must surface a blocker for human
attention. Reviews are due on a configurable five-minute interval and on
explicit blockers or goal changes, with one outstanding review per scope.
After the recovery limit is reached, affected work and its dependents are
blocked while independent work continues; a goal-wide blocker prevents all
goal progress. The default recovery limit is three unsuccessful corrective
attempts for the same unresolved problem and is configurable. Useful findings,
relevant artifact changes, resolved blockers, and verified checks provide
goal-specific progress evidence; messages or self-reported activity alone do
not reset the failure count, and routine reviews do not count as failures.

Multiple Swarms have equal scheduling priority by default, with explicit human
priority controls. Progress reviews take precedence within each Swarm's share,
subject to global provider caps. Viewing a Swarm does not change its priority.

Swarm Pause prevents new Invocations and lets current Invocations finish,
reporting Pausing until execution is quiescent. Swarm Stop interrupts owned
agent and tool execution and preserves work for explicit resumption. Closing
the client interface leaves execution running. These separate controls make graceful
draining and prompt interruption available without conflating either with goal
completion, abandonment, Room archive, or a frozen semantic clock. The trade-off
is explicit transitional state and reconciliation of interrupted work; neither
control promises to undo edits or cancel external operations already submitted.

After a supervisor crash or machine restart, the application recovers and
reconciles the existing Room and requires explicit Resume before starting new
Invocations. A worker failure during an otherwise active run may receive
bounded recovery after reconciliation. Optional configured time or Invocation
budgets request Pause when exhausted; there is no mandatory one-hour cutoff.

[ADR 0042](0042-use-a-tui-for-agent-swarm.md) replaces the initially planned
browser client with a TUI while preserving these execution-control semantics.

The execution adapter must establish ownership and verify descendant cleanup
on each supported operating system. This decision records required behavior,
not a claim that the current reference Runners implement it on every platform.

# Agent Swarm specification

Date: 2026-09-15.
Status: Product decisions Q1-Q38, the consolidated specification, and the
application-layer JEV advisor exception accepted by the user. This document
specifies the application to build. It does not claim that the application or
its provider adapters are implemented or qualified for release.

Decision provenance: [design interview](agent-swarm-design.md).
Domain vocabulary: [CONTEXT.md](../CONTEXT.md).

## 1. Product contract

Agent Swarm lets a person give a goal to a chosen group of local CLI agents,
observe their shared work in a TUI, and steer them toward a reviewed result.
Coding and non-coding goals are equally supported: research, writing, analysis,
planning, and software changes use the same coordination model.

- Windows and macOS are the first supported systems, including native Windows
  provider processes. WSL is an optional later path.
- The TUI, WorldStream, SQLite coordination storage, agent processes, and
  application artifacts run locally. No externally deployed application service
  is required. Provider CLIs may contact their cloud model services.
- Agents use installed, provider-supplied Codex, Claude Code, or Kiro tooling
  with the user's normal subscription sign-in. Direct model API integration
  and API-key fallback are outside agent execution scope.
- Agent Swarm may use the hosted JEV API as the sole direct-model-API
  exception, through a built-in application-layer advisor that is disabled by
  default and enabled explicitly for each Swarm. JEV is not a roster member or
  a fallback agent. Enablement discloses the remote recipient, exact model,
  evaluation policy, budget and fields eligible to leave the machine.
- The user chooses each member's provider, model, and supported reasoning
  effort. Settings persist and are visible; there are no random selections or
  silent substitutions. Unsupported effort controls are clearly identified.
- One Swarm pursues one goal in one Room. Separate goals create separate Rooms.
  Multiple Swarms can run concurrently, and saved roster configurations can be
  reused without reusing another goal's private provider conversations.
- The intended scale includes a user-selected roster of approximately ten
  agents. The example of two Claude, two Codex, and five Kiro members contains
  nine; actual concurrency remains subject to chosen caps and provider capacity.

## 2. Ownership and architecture

| Component | Responsibility |
| --- | --- |
| WorldStream and SQLite | Ordered authoritative Room changes, authorization, durable observations and attention, timer handling, integrity and recovery. |
| Swarm Activity Pack | Goal and constraints, work ownership and dependencies, contributions, review obligations and verdicts, result acceptance, visibility and legal participant Actions. |
| Local application supervisor and Runners | Owned CLI/process lifecycle, subscription-provider adapters, bounded Invocations, global capacity allocation, execution controls and operational health. |
| Provider agents | Read authorized context, reason about the work, use authorized tools, and submit participant Actions and attributed contributions. |
| Optional JEV advisor | Evaluate a minimized exact-Head Swarm projection and return a versioned Swarm Assessment for operational attention and authorized review context. It owns no Room or participant authority. |
| TUI | Show authorized Room facts and separate live execution activity; submit human Directions, Suggestions and execution controls through their respective authorities. |
| Local artifact handling | Preserve task-appropriate inputs, contributions and results in approved working areas; maintain version references and reconcile shared-resource changes. |

Model execution and generic Swarm workflow rules remain outside the frozen
Room Runtime. One Room uses one exact Activity Pack revision. Runner authority,
participant authority and Host Operator authority remain distinct; local
reachability does not itself grant Room or tool access.

A Work Item's single owner is enforced by Swarm Actions and state, independently
of Activation leases. A lease for one Participant does not prevent another
Participant from competing for the same Work Item. Agents receive bounded
authorized current context and relevant observations, not the entire history
in every prompt. Operational process output is not automatically a Room fact.

JEV runs beside the local supervisor and Runners, never inside WorldStream Core
or the deterministic Swarm Pack. Its one-purpose helper receives no Participant
credential, Room submission capability, provider-session token, working-area
path or general tool access. The TypeSafe API key is available only to that
helper and is not inherited by roster agents or their tool processes. A
Swarm Assessment is advisory operational evidence until an existing authority
submits an offered Action; replay never calls JEV again.

## 3. Starting and doing work

1. The user supplies a goal, selects the roster and explicit model/effort
   settings, and establishes the working area and authorized resources.
2. The application derives visible acceptance criteria from that goal before
   execution. Agents may propose refinements; relaxing accepted criteria or
   changing the goal's constraints requires a Human Direction.
3. Any eligible agent can propose Work Items. Each claimed item has one
   accountable owner; other agents can assist or review.
4. Agents produce separate, attributed contributions. These can be findings,
   citations, calculations, document drafts, files, or code patches. Git is
   optional and is never a prerequisite for non-coding work.
5. Integration is itself a claimable Work Item with one temporary owner. It
   combines contributions into a result and updates original resources within
   the goal's existing authorization.
6. Goal-specific checks and independent review determine whether that result
   can be accepted. Process exit or an agent claiming success is insufficient.

An approved working area belongs to each Swarm, with separate contributions
and a clear result location. If a human or another Swarm changes a shared
resource, preserve the newer version, refresh or reconcile affected work, and
repeat affected checks and review. Unresolved conflicts block affected work
and dependents while independent work continues. Integration must not silently
overwrite newer work.

## 4. Models, effort and provider conversations

- Prefer explicit versioned provider model identifiers. Moving aliases require
  deliberate human selection with their behavior disclosed. Auto-routing is
  not a default.
- Validate choices against the installed provider version and its available
  capabilities. Preserve requested values and provider-reported resolved values
  where available. An unreported effective setting must not be shown as verified.
- Unavailable choices or detected substitutions block the affected member until
  resolved. Do not silently accept output produced under mismatched settings.
- Human model/effort changes apply to the next Invocation and are recorded.
  Current work finishes unless the human explicitly requests interruption;
  the Participant's identity remains the same.
- Where supported, reuse one explicitly identified private provider conversation
  per member per Swarm. Refresh authorized Room context every turn. If a session
  cannot resume, use a fresh session with a recorded handoff and the same settings.
  Reuse must not retain superseded settings or override current Room facts.
- If work is reassigned to another eligible member, that member retains its own
  selected provider/model/effort. Reassignment is recorded after reconciling the
  previous attempt. It does not silently reconfigure the unavailable member.
- The roster is human-controlled. Agents may propose capacity changes but cannot
  admit extra members or add a provider automatically. Adapters must account for
  provider-native delegation so it cannot silently evade roster or capacity rules.

## 5. Review, completion and reopening

At least one different roster member reviews each final deliverable against
its criteria and evidence. Authors cannot self-certify; a one-agent Swarm
requires human review. Any eligible member can perform review work without a
permanent reviewer role.

Unresolved blocking findings prevent acceptance. A later passing review cannot
erase them. Another eligible member may examine a dispute; unresolved
disagreement is surfaced to the human. Corrections receive renewed review.

Acceptance records a Swarm Result identifying the exact deliverables and
relevant input and criteria versions, passing checks, and review evidence.
Relevant material changes require renewed
validation. Unresolved relevant conflicts or uncertain external effects block
the affected result.

Completion preserves that accepted result and stops automatic agent work,
including automatic progress reviews. Late contributions remain attributable
but cannot silently change the result. Explicitly reopening the same goal
assesses changed inputs and produces a newly reviewed result version in the
same Room. Completion does not archive the Room or erase prior evidence.

## 6. Progress supervision and limits

A Swarm Progress Review becomes due on a configurable five-minute interval
while the Swarm is executing, and after explicit blockers or accepted goal
changes. An eligible existing member claims the review and checks progress
and alignment with the current goal and constraints.

- Keep one outstanding review per scope. Due work remains visible in current
  Room state; downtime must not generate a backlog of duplicate review tasks.
  Preserve WorldStream's canonical timer catch-up rules.
- Host timers can make checks due even when agents are silent. Review execution
  still waits for eligible capacity; a due timer is not proof of model execution.
- Reviews take priority for the next available eligible member within the
  Swarm's share of capacity. Do not add agents or interrupt healthy turns to
  manufacture a reviewer. Show a blocker when no reviewer can run.
- Reviewers may create corrective tasks and revise plans within accepted goals
  and permissions. Goal, constraint, or permission changes return to the human.
- Useful findings, relevant artifact changes, resolved blockers and verified
  checks supply goal-specific progress evidence. Messages and self-reported
  activity alone do not reset a no-progress count.
- The default limit is three unsuccessful corrective attempts for the same
  unresolved problem, configurable by the user. Routine reviews are not failed
  attempts. At the limit, block affected work and its dependents and surface
  the issue; independent work continues. Goal-wide blockers prevent all progress.
- Per-provider concurrency caps apply across Swarms, initially based on the
  selected roster and adjustable by the human. The automatic daemon-wide cap
  is the maximum selected count for that provider in any registered Swarm, not
  the sum of independent rosters. A durable manual override wins over roster
  registrations; a provider missing from both sources has zero capacity.
  Swarms have equal default priority, with explicit priority controls. TUI
  focus does not change priority.
- Time and Invocation budgets are optional. Exhaustion requests Pause and
  preserves work. There is no mandatory one-hour cutoff.

Executing is an application condition, not merely an active Room status.
Paused, Stopped and completed Swarms do not launch automatic review Invocations.
Progress reviews cannot guarantee that every goal will eventually be solved.

When JEV is enabled, the application may evaluate goal alignment, evidence of
progress, unresolved risk, review urgency and likely missing context over the
same bounded current state. It records the exact Head, Pack and evaluation
policy revisions, requested and resolved model, input and question hashes,
typed answers, probabilities, confidence, usage, latency and attempt identity.
A stale, uncertain, failed or unavailable assessment cannot silently rebase,
close a review, suppress a due review or block normal Swarm work. Initial use
is shadow mode; later use may raise attention or request an ordinary review
only through existing authority. An eligible roster member or human still
performs every required review.

## 7. Steering, lifecycle and recovery

| Operation or event | Required behavior |
| --- | --- |
| Suggestion | Advisory human input that agents may evaluate and decline. |
| Direction | Binding change to the accepted goal or constraints, scoped to the Swarm or specified Work Items and affected dependencies. Stop new affected work, request interruption of affected Invocations, and revalidate affected results. |
| Agent-focused Direction | Identify the agent's affected current work and show the target before submission; the instruction follows that work across reassignment. |
| Pause | Start no new Invocations. Let existing turns finish, showing Pausing until quiescent. |
| Stop | Interrupt owned agent and tool processes, preserving resumable work. |
| Targeted cancellation | Interrupt only the exact named active Invocation, retain Terminated or Unknown conservatively, and leave the Swarm's desired Running state and unrelated active work unchanged. |
| TUI detach or close | Leave execution running; reconnect to the same Swarm later. |
| Supervisor crash or machine restart | Recover and reconcile durable state, then require explicit Resume. Preserve an existing Pause or Stop. |
| Worker failure during active execution | Permit bounded recovery after reconciliation, preserving Participant identity and selected settings. |
| Unknown external-action outcome | Reconcile before retry or reassignment; unresolved uncertainty blocks affected work. Room replay must not repeat external actions. |
| JEV timeout, rate limit, exhausted budget or unavailable service | Record bounded operational failure and continue without the optional assessment. Do not block ordinary Swarm execution or retry outside the configured attempt and budget limits. |
| Late JEV response | Preserve its receipt against the source Head but do not apply it to newer work. A fresh assessment is a new attempt, never a silent rebase. |

Directions do not expand tool access. Existing authorization remains effective;
broader access and external changes require authorization when it has not
already been granted for the goal. Stop and Directions do not promise to undo
edits or cancel external operations already submitted. Process cleanup must
cover owned descendants on both native Windows and macOS.

Provider-authored Actions use revision-bound semantic targets in addition to
the current Action offer. Dependency revisions bind the target Work Item and
the exact revisions of every proposed dependency; Suggestions bind the current
goal, Direction and any target Work revisions. A delayed contribution is
launched only from its original active Work Attempt and member. It may change
to `record_late_contribution` authority only after completion, when the same
Work/Attempt lineage has made the Pack's exact one-revision completion
transition. Recording it appends attribution and artifact evidence without
changing the accepted Result. An unrelated Head advance may rebind an active
Work Attempt only when that exact attempt and its Action schema remain current.

## 8. TUI contract

Use the supplied terminal dashboard as the visual reference: dark background,
thin borders, aligned monospace tables, restrained status colors, compact
summaries, freshness information, and visible keyboard help.

The interface provides a Swarm overview, work ownership and blockers,
contributions and results, human attention, and per-agent inspection. It shows
provider/model/effort choices, supervised activity and outputs, and keyboard
controls for Directions, Suggestions, Pause, Stop, Resume and detach.

For a JEV-enabled Swarm, the TUI shows that remote assessment is enabled, the
pinned and resolved model, question-policy revision, budget, source Head,
freshness, typed answers and uncertainty. It distinguishes a Swarm Assessment
from accepted Room facts and from an agent-authored Progress Review.

Accepted Room facts are distinguishable from transient Runner activity. Do not
invent completion percentages, usage telemetry or verified model resolution.
Rich artifacts remain local files with discoverable paths or an explicit open
action. Embedded interactive provider terminals are deferred; provider login
uses its normal CLI flow. A TUI framework is an implementation choice.

## 9. Existing foundation and remaining engineering work

WorldStream already supplies local SQLite operation, authorized observations,
durable attention, real timer scheduling and recovery machinery. The Hanoi
example demonstrates local agent coordination for a specific activity. The
[million-transition qualification](evidence/imo-232-local-docker-2026-09-14/README.md)
provides finite Linux Docker evidence for large history and bounded current
context; it is not a ten-agent subscription-CLI test, a native Windows/macOS
Swarm test, or the separate long-duration soak.

Implementation still needs the Swarm Pack and work model, general local process
supervision, qualified provider adapters, artifact/version handling, the TUI,
packaging, the isolated JEV advisor, and end-to-end evidence. Preserve useful
participant progress under concurrent work and timer traffic; existing
[Action starvation evidence](evidence/action-starvation/README.md) means that
large-history support alone does not settle concurrent commit behavior.

Provider qualification must establish strict configuration handling, normal
subscription authentication, output/cancellation behavior, and concurrency:

- Codex: explicit model/effort application, supported capabilities, session
  identity and native process/permission behavior.
- Claude Code: installed-version support, environment/configuration precedence,
  model substitutions, effort reduction and owned descendant cleanup.
- Kiro: signed-in CLI/ACP execution without an API-key dependency, model
  substitution detection, and independent concurrent effort settings. Kiro
  was absent from the local PATH during the design audit, and ACP effort
  negotiation/acknowledgement was not established by the reviewed documentation.
- JEV advisor: explicit per-Swarm opt-in, pinned and resolved model identity,
  minimized fields, credential isolation, bounded retries and budget, stale
  result rejection, shadow-mode accuracy/cost evidence, outage degradation and
  native helper behavior. JEV exposes no reasoning-effort setting and must not
  be presented as a roster agent.

See [provider findings and sources](agent-swarm-design.md#explicit-model-and-effort-selection).
These are qualification requirements, not claims of completed adapter support.

## 10. Release acceptance evidence

| Area | Required demonstration |
| --- | --- |
| General usefulness | Produce a sourced report from supplied material and a small code change with test evidence through the same coordination engine. |
| Intended scale | Exercise approximately ten roster members with mixed providers; record configured members, actual concurrency, machine limits and provider availability. |
| Settings | Run concurrent members of the same provider with different explicit model/effort selections; verify isolation, settings changes, unavailable choices and detected substitutions. Do not report unavailable resolution as verified. |
| Ownership and review | Competing Work Item claims yield one owner independently of Activation leases; preserve attribution, exact result/input/criteria evidence and blocking review findings. |
| Progress supervision | Cover silence, blockers, review priority, missing reviewer capacity, three failed corrections, duplicate avoidance and timer recovery. |
| Shared resources | Preserve concurrent human/other-Swarm edits; refresh affected work, repeat review and block unresolved conflicts. |
| Steering and lifecycle | Demonstrate task-scoped Directions across reassignment, Pause drain, owned descendant cleanup on Stop, TUI detach/reattach, and explicit Resume after crash/reboot. |
| Recovery and completion | Reconcile uncertain effects without blind retries; preserve accepted results against late contributions; stop automatic work at completion and reopen the same Room with a new reviewed result. |
| Multiple Swarms | Enforce global provider caps, equal default priority and explicit priority changes; TUI focus has no scheduling side effect. |
| Optional JEV advisor | With JEV disabled, preserve the ordinary review flow. With it enabled, compare shadow assessments against labeled healthy, stalled, drifting, blocked, stale and adversarial cases; record false negatives, false escalations, cost, latency, freshness and data minimization. Prove service/key/credit failures do not block the Swarm. |
| Native platforms | Run Windows/macOS automation with controlled fake workers and real subscription-CLI smoke tests on both systems, including login, permissions, paths, sleep/wake, keyboard input, resize and terminal restoration. |

Docker/Linux tests can support backend qualification but do not replace native
Windows evidence. The user's Windows machine is an agreed manual-test route.
Record CLI versions and observed results for every provider combination claimed
as supported. This interview ran no provider inference or platform acceptance
tests; documentation consistency checks do not establish release readiness.

## Decision references

- [ADR 0038: local subscription CLI execution](adr/0038-local-subscription-cli-execution-for-agent-swarm.md)
- [ADR 0039: one Room per goal](adr/0039-one-room-per-swarm-goal.md)
- [ADR 0040: contributions, integration and accepted results](adr/0040-separate-swarm-contributions-from-accepted-results.md)
- [ADR 0041: Directions and execution controls](adr/0041-separate-swarm-directions-and-execution-controls.md)
- [ADR 0042: TUI interface](adr/0042-use-a-tui-for-agent-swarm.md)
- [ADR 0043: application-layer JEV advisor](adr/0043-use-jev-as-an-application-layer-swarm-advisor.md)
- [JEV integration research](jev-agent-swarm-integration-research.md)

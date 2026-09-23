# Agent Swarm design interview

Status: All product decisions Q1-Q38 accepted, 2026-09-15. The design tree is
closed. The user confirmed the [consolidated specification](agent-swarm-spec.md)
and requested implementation tickets. This interview record is not a
release-readiness claim. The confirmed execution constraints are recorded in
[ADR 0038](adr/0038-local-subscription-cli-execution-for-agent-swarm.md).

## Confirmed requirements

- The proposed application is called Agent Swarm and uses WorldStream as its
  local coordination core.
- A user supplies a goal and chooses a roster of agents from installed,
  provider-supplied local agent CLIs. A representative roster is two Claude
  Code agents, two Codex agents, and five Kiro agents.
- Each roster member has an explicitly selected model and reasoning effort
  where supported. The application records and displays those selections;
  random choices, silent substitutions, and unnoticed inherited defaults do
  not satisfy the requirement. Unsupported effort controls are clearly marked.
- Coding and non-coding goals are first-class application scope. Research,
  analysis, writing, planning, and code changes share the coordination model;
  their tools, deliverables, and acceptance evidence differ. The use of
  coding-agent CLIs does not require every goal to involve source code.
- Execution uses the user's provider CLI subscriptions and normal provider
  sign-in. Direct model API integration and API-key fallback are outside the
  roster agent execution scope; the later accepted JEV advisor is the sole
  application-layer exception.
- WorldStream, coordination data, agent processes, and the application UI run
  on the user's computer. No externally deployed application infrastructure
  is required. Provider CLIs may contact their model services.
- The first interactive interface is a cross-platform TUI for inspecting
  running Swarms and providing human steering. This supersedes the earlier
  local web UI and desktop-wrapper proposals. The user supplied a dense,
  keyboard-driven terminal dashboard as the visual reference.
- Windows and macOS are the first supported operating systems. The user can
  perform manual tests on a Windows machine. Native Windows CI and other
  automation should reduce the manual burden; Docker is not assumed to provide
  native Windows qualification.
- The user requested a grill-with-docs interview before implementation.

## Existing domain and architecture constraints

The repository remains a single context governed by [CONTEXT.md](../CONTEXT.md).
Resolved domain terms belong there; this document is a design record rather
than a second glossary.

The application name **Agent Swarm** and the resolved terms **Swarm**, **Swarm
Goal**, **Swarm Roster**, **Swarm Direction**, **Swarm Suggestion**, **Swarm Work
Item**, **Swarm Contribution**, **Swarm Progress Review**, **Swarm Result**,
**Swarm Pause**, and **Swarm Stop** are recorded in CONTEXT.md.

- A Room is one authoritative situation governed by one exact Activity Pack
  Revision. Each Swarm maps to one Room, as recorded in
  [ADR 0039](adr/0039-one-room-per-swarm-goal.md).
- Agent Participants outlive individual Invocations. A provider conversation
  or CLI process is not the Participant's durable identity.
- Runner handling authority, participant Action authority, and Host Operator
  authority are distinct. See [ADR 0003](adr/0003-separate-activation-and-action-authority.md).
- Activity Clients may present a rich experience while Room state and legality
  remain with WorldStream and the Pack. See [ADR 0017](adr/0017-separate-activity-clients-from-packs-and-studio.md).
- [ADR 0018](adr/0018-cli-first-operator-surface.md) retired the Studio
  administration website. The Agent Swarm TUI is an application client with
  scoped participation and owned process controls; it does not silently
  replace the generic operator CLI or obtain Room authority from local
  reachability.
- The existing operator CLI has no generic domain pause. Swarm Pause and Swarm
  Stop are application execution controls with the semantics recorded below;
  they do not freeze Room Semantic Time or archive the Room.
- [ADR 0001](adr/0001-product-boundary.md) keeps model execution, coding
  harnesses, generic task management, and timeline branching outside the frozen
  Room Runtime. Application work should preserve that separation; any deliberate
  change requires an explicit decision.

## Accepted round 1 decisions

The user accepted all seven recommendations on 2026-09-15, then clarified that
non-coding work must be first-class. That clarification supersedes the earlier
coding-first scope in Q1. The user subsequently accepted round 2 with Windows
and macOS replacing the recommended macOS-only launch, then replaced Q7's
browser interface with a TUI. Q2-Q6 remain accepted. Following the peer-review,
same-Room resumption, and progress-supervision clarifications, the user accepted
all outstanding recommendations through Q28 and requested explicit per-agent
model and reasoning-effort selection. A subsequent "Accept all" settles
Q29-Q35, followed by acceptance of Q36-Q38 and final shared-understanding
confirmation. The next authorized task is to draft and publish implementation
tickets using the to-tickets workflow.

| ID | Accepted decision |
| --- | --- |
| Q1 | Revised by the user: Agent Swarm supports coding and non-coding goals as first-class work. Git repositories, source changes, and software tests are not universal prerequisites. Q23 selects the initial report and code-change proof scenarios. |
| Q2 | One Swarm pursues one goal in one WorldStream Room. Reopening resumes it; a separately started goal creates another Swarm. Roster configurations can be reused independently. |
| Q3 | The UI distinguishes binding Swarm Directions from advisory Swarm Suggestions. Directions amend accepted goals or constraints; Suggestions leave agent discretion. |
| Q4 | The human-selected roster is the limit. Agents may propose more capacity, but the human decides. Replacing a crashed execution process preserves the existing Agent Participant's identity. |
| Q5 | The UI leads with the goal, current work, ownership, produced results, and required human attention. Individual agent activity and transcripts are available on inspection. The later TUI choice adds a dense, keyboard-driven visual direction. |
| Q6 | Work continues locally when the interface closes or detaches. Reopening reconnects to the same work. Explicit Pause and Stop controls follow the semantics accepted in Q12. |
| Q7 | Revised by the user: the first release uses a TUI on Windows and macOS, with a Swarm overview and individual Swarm inspection. This replaces the localhost browser and desktop-wrapper plans. |

## Accepted round 2 decisions

The user accepted Q8-Q14 on 2026-09-15, including the generalized Q9 and
goal-specific interpretation of Q10/Q14. For Q15 the user explicitly selected
Windows and macOS together and offered manual Windows testing.

| ID | Accepted decision |
| --- | --- |
| Q8 | Any agent may propose work. Each Work Item has one accountable owner; other agents may assist or review. Shared plans and explicit claims support coordination. |
| Q9 | Shared coordination facts live in WorldStream. Agents submit separate, attributed contributions that are explicitly combined into the accepted result. The mechanism depends on the work; Git/worktrees are optional coding tools. |
| Q10 | Goal completion may be automatic when agreed, goal-specific acceptance checks and independent agent review pass. A reviewable result retains its evidence. Missing or failing checks leave the goal unresolved. |
| Q11 | Binding Directions stop new affected work, request interruption of affected Invocations, and require affected results to be revalidated before acceptance. Already-made edits and completed external actions are not undone by the Direction. |
| Q12 | Pause launches no new agent turns and lets current turns finish, showing Pausing until they finish. Stop interrupts owned agents and tool processes. Both preserve work for explicit resume and neither automatically undoes effects or abandons the goal. |
| Q13 | Multiple Swarms may run simultaneously with a global per-provider concurrency limit shared across Swarms. Q28 sets initial caps from the selected roster; Q35 establishes equal default priority with human-adjustable priorities. |
| Q14 | Agents may perform scoped work using the goal's authorized tools/resources. Broader access and external changes require explicit authorization unless the user already granted it for that work. Directions do not automatically expand tool permissions. |
| Q15 | Windows and macOS ship first. Native Windows automation and the user's Windows machine are available validation routes; Linux-container results are not native Windows evidence. |

The consequential integration and control choices are recorded in
[ADR 0040](adr/0040-separate-swarm-contributions-from-accepted-results.md) and
[ADR 0041](adr/0041-separate-swarm-directions-and-execution-controls.md).

## Accepted TUI direction

[ADR 0042](adr/0042-use-a-tui-for-agent-swarm.md) records the change in interface.
The user-supplied terminal dashboard is the visual reference: dark background,
thin panel borders, aligned monospace rows, restrained status colors, compact
summary panels, freshness timestamps, and a persistent keyboard-help footer.
Exact layout and key bindings are still design proposals.

The proposed layout translates that reference into:

- Swarm and provider summaries with reported activity and availability.
- A work table showing the item, owner, state, age, and relevant blocker.
- An inspect view for selected work, an agent, a contribution, or an event.
- Live Runner activity shown separately from accepted Room facts.
- Keyboard controls for navigation, Directions, Suggestions, Pause, Stop,
  Resume, and detaching the interface.

The interface must not infer task completion from process exit, invent progress
percentages, or display unavailable subscription telemetry as measured usage.
Rich non-text artifacts can remain local files with a path or explicit open
action rather than requiring a browser UI for the application itself.

Q24 now settles inspect mode: supervised agent activity and outputs, with
Directions and Suggestions entered through Agent Swarm. Embedded interactive
provider terminals are deferred. Framework choice and terminal qualification
are engineering follow-ups; no dependency has been added.

## Windows validation findings

- The repository already contains a native Windows x64 job in
  `.github/workflows/compatibility-gates.yml` and `scripts/verify-local.ps1`.
  Checked-in tests include Windows file protection and descendant cleanup.
  Their presence does not establish that the current branch passes them.
- The Hanoi reference Runner's Windows cleanup terminates its direct child;
  its POSIX path terminates a process group. The Swarm adapter needs native
  Windows descendant-cleanup implementation and tests before satisfying Q12.
- Docker Desktop on macOS runs Linux containers through a Linux VM. It can
  help test Linux/backend behavior, but cannot substitute for native Windows
  execution. WSL testing likewise qualifies a Linux worker path rather than
  the native Windows application.
- Proposed validation combines native Windows/macOS CI using fake CLI workers
  with real subscription-CLI smoke tests on both systems. The user's Windows
  machine can cover login, steering, pause/stop, TUI detach/reattach, terminal
  resizing and cleanup, keyboard input, sleep/wake, paths, and permissions
  without putting subscription credentials into CI.

Sources: [Docker VM architecture](https://docs.docker.com/desktop/features/vmm/),
[container platform constraints](https://docs.docker.com/build/building/multi-platform/),
[Microsoft WSL architecture](https://learn.microsoft.com/en-us/windows/wsl/compare-versions),
[GitHub hosted runners](https://docs.github.com/en/actions/concepts/runners/github-hosted-runners).

## Progress supervision and recovery clarifications

For Q21 the user proposed and then accepted scheduled checks, or work triggered by stream
activity, that an existing roster member can claim to assess whether agents
are stuck or deviating from the goal and identify corrections. This is modeled
as a Swarm Progress Review. It is ordinary shared work, not a requirement for
an extra permanent supervisor agent. The user accepted the recommendation
that reviewers can create corrective tasks and revise the plan within the
accepted goal and permissions; goal or constraint changes return to the human.
Repeated unsuccessful corrections must surface a blocker. The review policy
replaces the earlier blanket 60-minute cutoff as the primary supervision
mechanism. Q28 subsequently makes time and Invocation budgets optional, with
no mandatory one-hour cutoff. Budget exhaustion requests Pause.

The accepted loop is: a due check or relevant accepted work change creates a
review obligation; an eligible roster member claims it; that agent compares
the current goal and constraints with relevant work, evidence, and blockers;
it records findings and corrective-work proposals. Agents receive authorized
current views and relevant observations rather than the entire raw history.
Progress requires evidence appropriate to the goal, not simply a high event
count, a live process, or an agent reporting that it is still working.

The existing local SQLite runtime has an automatic timer scheduler in
[server/lib.rs](../crates/worldstream-server/src/lib.rs) and due-timer handling
in [sqlite_backend.rs](../crates/worldstream-server/src/sqlite_backend.rs).
The [Late Join reference Pack](../packs/late-join-reference/src/pack.ts)
already turns a due assessment reminder into a Domain Event and targeted
Attention. These are implementation foundations, not an implemented Swarm
review policy. The Swarm Pack still needs review obligations, explicit
one-owner claim Actions, and rules for accepting findings and corrections;
an Activation lease alone does not assign a Work Item to one roster member.

Engineering constraints and remaining details for the accepted loop:

- Host timers can make a review due while agents are silent. Operational
  process health and domain progress remain distinct; the local supervisor
  need not spend model calls detecting an exited or unresponsive worker.
- Keep outstanding review work bounded and make it visible in current Room
  state. Review work should receive scheduling priority within the selected
  roster and provider caps. Unavailable reviewer capacity must surface as a
  blocker, not create another agent automatically.
- Reviewers may create corrective tasks and revise plans within the accepted
  goal and permissions. These corrections are not Human Directions and do
  not relax constraints or grant new tool permissions. Q19 permits recorded
  reassignment to an eligible existing roster member after reconciliation;
  this uses the receiving member's selected model and effort. Progress reviews
  do not grant new process interruption powers.
- Repeated reviews without evidence of progress need bounded recovery and
  must surface a blocker after repeated unsuccessful corrections. Reviews
  alone cannot guarantee eventual completion. Q25 sets a configurable
  five-minute review interval plus explicit blockers or goal changes. Q26
  blocks affected work and dependents while independent work continues;
  goal-wide blockers prevent all progress. Q27 prioritizes reviews for the
  next available eligible member. Accepted Q34 uses useful findings, relevant
  artifact changes, resolved blockers and verified checks as goal-specific
  evidence. It sets a configurable default of three unsuccessful corrective
  attempts for the same problem; messages alone do not reset that count, and
  ordinary review runs do not count as failures.
- Pause and Stop still prohibit new Invocations. Review timers do not override
  those controls. Overdue timer handling must preserve ADR 0007; a one-shot
  reminder rearmed after handling is a candidate for avoiding a backlog of
  periodic reviews without dropping canonical timer obligations.
- Review execution applies while a Swarm is executing, not merely while its
  Room has active status. Paused, Stopped and completed Swarms do not launch
  automatic reviews. Q37 preserves completed results and requires explicit
  reopening for further agent work on the same goal.

For Q20 the user clarified that an existing Room should make the same Swarm
resumable. This agrees with Q2 and Q12: recover the saved goal, work,
contributions, review state, and artifact references, then reconcile incomplete
attempts before continuing. Room recovery does not preserve live CLI processes
or guarantee provider-private session availability; replacement Invocations
retain the same Agent Participant identity. Interrupted external actions may
need reconciliation before retry. Q20 is now accepted: recover and reconcile
state after a supervisor crash or reboot, then require explicit Resume.
Explicitly Paused or Stopped Swarms retain their state. Worker failures during
an otherwise active run may receive bounded recovery after reconciliation.

## Explicit model and effort selection

The user requires model and supported effort selection for individual agents.
These choices belong to the human-selected roster, persist across replacement
Invocations, and are visible in the TUI. Human changes apply to the next
Invocation unless the human explicitly interrupts the current one. A reused
provider conversation must use the updated settings and current Room context;
it cannot silently preserve superseded settings. Reusable roster presets can copy
explicit choices; the application must not infer an unrequested model or
reasoning level. Work reassignment preserves the receiving agent's own choices.

Adapter qualification must establish the installed CLI version's supported
model/effort combinations, configuration precedence, and substitution behavior.
Record requested model and effort alongside provider-reported resolved values
where available. Do not label an unreported effective value as verified. A
detected mismatch must be surfaced and must not silently yield an accepted
Swarm result. If a setting cannot be applied, block the affected member until
the configuration is corrected; unsupported controls appear as unavailable.

Read-only findings on 2026-09-15:

- Codex's installed `exec --help` exposes `--model` and configuration overrides.
  The existing Hanoi Runner explicitly passes `model_reasoning_effort` but
  hard-codes its model; it is not a per-agent Swarm selector. Official
  [configuration documentation](https://learn.chatgpt.com/docs/config-file/config-reference)
  describes effort controls, and the local
  [App Server contract](https://learn.chatgpt.com/docs/app-server) exposes
  available models and model-specific effort options through `model/list`.
  This is local provider-CLI integration, not a direct model API adapter.
- Installed Claude Code 2.1.112 exposes `--model` and `--effort`; current
  [CLI documentation](https://code.claude.com/docs/en/cli-reference) includes
  newer options that must not be assumed available in that installed version.
  [Model configuration](https://code.claude.com/docs/en/model-config) documents
  aliases, substitutions and supported effort, while
  [environment precedence](https://code.claude.com/docs/en/env-vars) means a
  launch flag alone does not prove the applied setting. Per-worker overrides
  must preserve the user's unrelated global preferences.
- Kiro is not installed locally. Official
  [agent configuration](https://kiro.dev/docs/custom-agents/configuration-reference/#model-field)
  accepts model IDs but can fall back when one is unavailable.
  [Effort settings](https://kiro.dev/docs/models/effort/) include persistence
  in shared CLI settings. The [ACP contract](https://kiro.dev/docs/cli/acp/)
  documents model selection but does not establish effort negotiation and
  acknowledgement. Concurrent effort isolation and reliable configuration
  reporting remain qualification gates for the subscription-backed adapter.

No provider model was invoked, no credentials or global provider settings were
changed, and no adapter was implemented during this investigation.

## Accepted round 3 decisions and follow-ups

The user's round 3 "Accept all" accepted the outstanding recommendations through
Q28, preserving the explicit TUI, Windows/macOS, non-coding, resumability, and
progress-review corrections. The new model/effort requirement is recorded
above. The following are accepted decisions, not open questions.

| ID | Accepted decision |
| --- | --- |
| Q16 | Derive visible acceptance criteria from the user's goal before execution. Agents may propose changes; relaxing accepted criteria requires a Human Participant's Direction. |
| Q17 | Agents request and perform reviews within the existing roster. At least one different Agent Participant reviews each final deliverable against criteria and evidence. Authors cannot self-certify. A one-agent Swarm requires human review. |
| Q18 | Integration is a claimable Work Item with one temporary owner. Eligible agents combine contributions; the resulting deliverable requires review, and relevant input changes require renewed validation of affected material. |
| Q19 | Reassign blocked work to an eligible existing roster member when possible, recording the change and reconciling the prior attempt first. No automatic provider, roster, or API fallback expansion. The receiving member retains its configured model and effort. |
| Q20 | An existing Room preserves the same Swarm for resumption. After a supervisor crash or reboot, recover and reconcile state and require explicit Resume. Worker failures during an otherwise active run can receive bounded recovery after reconciliation. TUI detachment leaves execution running. |
| Q21 | Scheduled or event-triggered Swarm Progress Reviews are claimed by existing roster members. Reviewers may create corrective tasks and revise plans within the accepted goal and permissions. Goal/constraint changes return to the human; repeated unsuccessful corrections surface a blocker. |
| Q22 | Target native Windows provider processes first, with per-provider permission controls and tested descendant cleanup. WSL is an optional later path. Provider sandbox capabilities need not be identical. |
| Q23 | Prove the same application with a sourced report using supplied material and a small code change with test evidence. Exercise mixed providers, steering, stop/resume, and TUI reconnect on Windows and macOS. |
| Q24 | Show supervised agent activity and outputs in the TUI, with Directions and Suggestions entered through Agent Swarm. Defer embedded interactive provider terminals; provider login uses its normal CLI flow. |
| Q25 | Make a review due on a configurable five-minute interval while active and on explicit blockers or accepted goal changes. Keep one outstanding review per scope and avoid a backlog of missed periodic reviews while preserving canonical timer catch-up. |
| Q26 | After the no-progress recovery limit is reached, block affected work and its dependents and surface the issue for human attention; independent work continues. A goal-wide blocker prevents the whole Swarm from advancing. Q34 subsequently sets the configurable default threshold to three unsuccessful corrective attempts. |
| Q27 | Progress reviews take priority for the next available eligible roster member within provider caps. They do not add members or interrupt healthy turns. If no eligible reviewer can run, show a review-capacity blocker. |
| Q28 | Use global per-provider caps initially matching the selected roster, adjustable by the human. Offer optional time or Invocation budgets, with no mandatory one-hour cutoff. Configured budget exhaustion requests Pause and preserves work. |

## Accepted round 4 decisions

The user accepted all Q29-Q35 recommendations. These decisions preserve the
previously accepted constraints and the explicit model/effort requirement.

| ID | Decision | Settled prerequisites | Accepted policy |
| --- | --- | --- | --- |
| Q29 | When a human changes an agent's model or effort during a run | Explicit per-agent settings, Q4, Q12 | Apply the change to that member's next Invocation, record it, and retain Participant identity. Let its current turn finish unless the human explicitly requests interruption. An unavailable choice blocks that member rather than substituting another model or effort. |
| Q30 | How conflicting review verdicts are resolved | Q16-Q18 | Unresolved blocking findings prevent acceptance; a later passing review cannot erase them. Another eligible roster member can examine the dispute, and unresolved disagreement is surfaced to the human. Corrections receive renewed review. |
| Q31 | Where contributions are developed and when original resources are modified | Q9, Q14, Q18 | Use an approved working area per Swarm with separate agent contributions and a clear result location. Agents work on drafts/copies/patches as appropriate. Integration modifies supplied originals only within the goal's existing authorization. Git remains optional. |
| Q32 | Whether Directions can target individual work rather than the whole Swarm | Q3, Q11, Q21 | Support goal-wide and Work Item targeting. An agent-focused shortcut identifies its affected current work; the Direction follows that work across reassignment and applies to relevant dependent work. The TUI shows the target before submission. |
| Q33 | Whether each roster member keeps an ongoing provider conversation | Q2, Q4, Q20, explicit settings | Reuse an explicit private provider session per member per Swarm where supported, refreshing authorized Room context each turn. If the session cannot be resumed, start a fresh one with a recorded handoff and the same selected settings. Provider memory never overrides current Room facts. |
| Q34 | What demonstrates progress and how many failed corrective attempts trigger escalation | Q16, Q21, Q25-Q26 | Track goal-relevant evidence such as useful attributed findings, artifact revisions, resolved blockers, and verified checks. Messages and self-reported activity alone do not reset the failure count. Start with a configurable limit of three unsuccessful corrective attempts for the same unresolved problem; ordinary review runs do not count as failures. |
| Q35 | How multiple Swarms share provider capacity | Q13, Q27-Q28 | Give Swarms equal scheduling priority by default, allow explicit human priority changes, and prioritize reviews within each Swarm's share. Opening a Swarm in the TUI does not silently change its priority. Global caps still apply. |

## Accepted round 5 decisions

The user accepted Q36-Q38. No product questions remain in the design tree.

| ID | Decision | Settled prerequisites | Accepted policy |
| --- | --- | --- | --- |
| Q36 | Shared inputs or outputs change while a Swarm is working | Q18, Q30-Q31 | Preserve the newer resource. Refresh or recompute compatible contributions within existing authorization and repeat affected checks and review. Conflicts that cannot be reconciled block affected work and dependents while independent work continues. Do not silently overwrite newer human or other-Swarm work. |
| Q37 | Changes arrive after goal completion | Q2, Q10, Q20, Q30-Q31 | Completion records the accepted result against identified inputs and criteria. Stop automatic agent work and preserve that record. On explicit reopening, assess changed inputs and produce a newly reviewed result version in the same Room. Late contributions remain attributable but cannot silently change the accepted result. |
| Q38 | Provider-controlled moving model aliases | Explicit model/effort requirement, Q29, Q33 | Prefer explicit versioned provider model IDs. Permit a moving alias only when the human explicitly selects it with its behavior disclosed. Retain requested and provider-reported resolved values where available; surface detected changes or substitutions and block affected execution until resolved rather than silently adopting a different model. Auto-routing is not a default. |

### Consequences already established

- Review evidence must identify the result and the relevant input/criteria
  versions. Changed material requires renewed validation; older contributions
  and verdicts remain attributable rather than being silently rewritten.
- Recovery reconciles incomplete attempts before retry or reassignment. An
  unresolved external effect remains uncertain and blocks affected work;
  replaying Room history is not permission to repeat the external action.
- Returning to or revising the same goal retains its Room. Completing a goal
  does not implicitly archive the Room or erase the accepted result's history.
- Model and effort selection is explicit per roster member. Presets may copy
  chosen values; they do not justify silently choosing defaults or fallback.

## Completion of the interview

The design tree is closed with Q1-Q38 accepted and all explicit user corrections
preserved. [Agent Swarm specification](agent-swarm-spec.md) consolidates the
agreed behavior, architecture boundaries, provider qualification gaps, and
release evidence. The user has confirmed that shared understanding and requested
implementation tickets. Reopen questions only for an actual conflict or changed
requirement.

Provider qualification, TUI framework choice, installation/update packaging,
artifact integrity, and native operating-system validation remain engineering
work to make the eventual agreed specification concrete. Do not turn every
implementation constant into a new product approval question.

## Accepted post-interview JEV decision

On 2026-09-17, after reviewing TypeSafe's launch material and runtime contract,
the user accepted hosted JEV as a narrow exception to the direct-model-API
exclusion and placed it in the Agent Swarm application layer.

- Ship JEV as a built-in but per-Swarm opt-in semantic advisor, disabled by
  default. Codex, Claude Code and Kiro remain the roster's working agents.
- JEV receives a minimized projection at one exact Room Head and returns a
  versioned Swarm Assessment with typed answers and uncertainty. It has no
  Participant identity, Room submission credential, Work Item ownership, tool
  access, review vote, Direction authority or result-acceptance authority.
- Begin in shadow mode. A successful evaluation may later raise operational
  attention or request an ordinary authorized review; it never replaces the
  existing roster-member or human review.
- Pin the requested model and retain the provider-reported resolved version.
  Record policy/input provenance, bound retries and budget, reject stale
  results, minimize remote data, and isolate the API key from every roster
  agent and tool process.
- Missing credentials, depleted credits, rate limits, timeouts and service
  outages disable only the optional assessment path. They cannot block normal
  local coordination or subscription-CLI work.

The boundary and evidence requirements are consolidated in the
[specification](agent-swarm-spec.md) and
[JEV integration research](jev-agent-swarm-integration-research.md).

## Documentation discipline

Record resolved decisions here as answers arrive. Add resolved domain-specific
terms to root CONTEXT.md immediately. Create an ADR only for a settled decision
with a meaningful trade-off, substantial reversal cost, and rationale a future
reader would otherwise miss. Implementation begins after the interview reaches
a shared understanding confirmed by the user.

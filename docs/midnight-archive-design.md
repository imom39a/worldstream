# Midnight Archive: first playable design

Status: Design decisions adopted, 2026-09-09. The portable Pack, independent
browser client, authored scenarios, optional specialist mechanics, four
reviewed Roster Options, private debrief, and session expiry are implemented
and locally qualified. Deployed Host activation, external-provider evidence,
phone acceptance, and human playtesting remain. The user accepted Q1–Q12 and
directed that all remaining grilling questions use the recommended answers.
The [decision record](second-game-design.md) closes that interview.

## Product and experiment

Midnight Archive is a 15–20-minute browser escape adventure for people who
enjoy technology and games. One human explores personally, with zero, one or
two supplied AI Companions. Recover the authentic ledger from a fictional
private archive and get the entire starting crew out. Companions investigate
and perform useful work; the human chooses the mission's consequential
commitments.

The first experiment tests whether companion discoveries, specialist methods
and standing tasks make the human's decisions more interesting. It does not
claim endless replay, proven retention, commercial economics or a general
world engine. Human friends, paid hires, external agents, native mobile apps,
persistent character relationships and creator publishing follow separately.

Deliver this as a second Activity Listing on the common platform, with its
own portable Activity Pack and Activity Client. Keep current Agent Heist
stabilization separate. Extract shared gameplay code only after a mechanic
has proved useful here and has a concrete second use in Heist.

## The world

One WorldStream Room contains the entire archive. These five Locations are
Activity State, not separate Rooms.

~~~mermaid
flowchart LR
    A["Atrium / exit"] --- R["Records room"]
    A --- C["Conservation hall / archivist"]
    R --- C
    R --- P["Plant room"]
    C ---|"Archivist gate"| V["Ledger vault"]
    P ---|"Service hatch"| V
~~~

All routes are bidirectional once opened. The map is known from the brief.
Unopened gates are visibly distinguished from open passages. There are no
moving guards, combat, physics simulation, arbitrary object crafting or
secret sixth Location in this version.

| Location | Useful contents |
| --- | --- |
| Atrium | Arrival, objective briefing and explicit extraction |
| Records room | Intake evidence and a powered catalog verifier |
| Conservation hall | Restoration evidence, a threatened collection and the archivist |
| Plant room | Service-hatch controls and the source's identifying access record |
| Ledger vault | Candidate ledgers with visible distinguishing attributes |

There are three interacting systems: evidence authenticity, access and
limited power. An agreement changes access through explicit conditions.
Power is a shared, discrete reserve. An operation consumes its declared
charge; opening a gate leaves it open for the rest of this first scenario.

### Authentication and access

Authentication and access are independent problems. Players may visit and
combine them in any legal order.

- **Evidence:** combine an intake clue from Records with a restoration clue
  from Conservation, then compare the visible candidate attributes. Thinking
  and arranging already-known evidence are free. The authoring validator must
  establish one genuine candidate, a unique correct intersection, and more
  than one candidate compatible with either source alone.
- **Powered verification:** spend one charge and one turn in Records to obtain
  a verified intake record identifying the original. This is a declared
  instrument result; an LLM cannot invent it.
- **Negotiated access:** accept the archivist's preservation agreement and
  complete the collection work. The conservation gate opens when both recorded
  conditions hold.
- **Technical access:** operate the service hatch in Plant, paying its charge
  cost. This provides an alternative entrance and return path.

The player may carry a candidate without definitive verification, with the
uncertainty shown. Carrying a ledger does not reveal its hidden authenticity.
Changing a carried candidate at the vault costs another recovery action.
Outcome checks use the actual ledger identity, not the player's assertion.
There is no free hidden-answer query or requirement to guess an author's
exact sentence.

### Initial balance defaults

These are the first implementation values, supported only by the limited
route check below. Tune them after playtests and pin the selected values in
each reviewed scenario configuration.

| Operation | Location / requirement | Turns | Power |
| --- | --- | ---: | ---: |
| Move to an adjacent Location | Gate open when applicable | 1 | 0 |
| Discover an evidence source | Records or Conservation | 1 | 0 |
| Use the catalog verifier | Records | 1 | 1 |
| Accept the preservation agreement | Human lead in Conservation | 1 | 0 |
| Prepare the collection | Conservation; not already prepared | 1 | 0 |
| Energize preservation equipment | Conservation; collection prepared | 1 | 1 |
| Open the service hatch | Plant; human or Mira | 1 | 2 |
| Open the service hatch | Plant; Jonah's specialist method | 1 | 1 |
| Perform Mira's field assay | Mira in Vault; two consecutive eligible work steps | 2 | 0 |
| Recover or exchange a candidate ledger | Human lead in Vault | 1 | 0 |
| Protect the source's identifying record | Plant; ledger has been recovered | 1 | 1 |
| Extract | Human lead at Atrium; explicitly acknowledge any crew left behind | 1 | 0 |
| Wait deliberately | Explicit human choice | 1 | 0 |

Start with 16 Activity Turns and three power charges. Use the same core costs
and initial turn budget across the four party configurations. Companions may
make the main objective easier; optional objectives reward their additional
capacity. Do not secretly compensate by adding hazards when a companion is
chosen.

The optional objectives are preserving the collection and protecting the
source's identifying record. The latter is a fictional device operation with
a known cost, not a real-world intrusion exercise. It gives Jonah's
professional priority a concrete consequence.

A temporary solo route model found four feasible standard combinations in
10–12 turns. Completing both optional objectives takes 15 turns using powered
verification plus an agreement, or 16 using evidence plus an agreement. The
[route-check note](midnight-archive-route-check.md) records assumptions and
witnesses. This does not prove discoverability, multi-actor correctness,
difficulty or enjoyment.

## The human and the characters

| Participant | Mechanical contribution | Character priority |
| --- | --- | --- |
| Human lead | All essential solo methods; task limits; binding agreement; choice of ledger; extraction | Chooses the expedition's trade-offs |
| Mira | Ordinary investigation plus a two-step, zero-charge field assay in the vault | Wants claims supported by evidence |
| Jonah | Ordinary investigation plus a one-charge service-hatch method | Wants the source's identity protected |
| Archivist | Offers the preservation agreement and honors its recorded conditions | Wants the collection preserved |

Mira and Jonah are optional Roles, each with maximum cardinality one. They
can inspect sources, share permitted discoveries, move and operate within
their allowed methods and task limits. All essential work has a human
alternative. The human retains the binding agreement, ledger selection and
extraction decisions through the lead Role.

The archivist is initially a Pack-governed character with authored factual
responses and conditional offers. A third LLM Runner is unnecessary to prove
social consequences. Natural dialogue for that character can be added later
without giving it authority to invent access, facts or new bargain terms.

Companions share the main objective. Their priorities affect advice and
optional choices; they do not secretly sabotage the mission. They may
recommend the correct solution when their authorized evidence supports it.
Do not make them artificially ignorant or silently expose the hidden answer
to make them appear capable.

Everyone begins in the Atrium. Companions initially follow the lead under a
disclosed Pack rule. Assigning a task releases that character from following.
A regroup order can later direct a companion, one valid move per committed
turn, through known open routes to the Atrium. This is explicit basic movement,
not a new model, replacement Principal, teleport or invented agent decision.

## A turn and a task

The human chooses one personal move and gives companions goals such as
“investigate the records,” “prepare the service route; spend at most one
charge,” or “analyze the ledgers and report.” The UI always exposes the task,
resource allowance, current plan status and an easy way to change or cancel
it. Optional text can express intent; the recorded task and limits are shown
in structured form.

The Runner may propose a Companion Plan containing at most three typed steps.
Routine known work can continue across turns without fresh model calls. Stop
for a new decision when discoveries invalidate the plan, a condition fails,
the task changes or a limit would be exceeded. Do not execute a speculative
branch based on evidence not yet observed.

Preparation is separate from physical progression:

1. Record the human's chosen move and the current task revisions.
2. Obtain missing companion plans through serialized, bounded opportunities.
   Existing valid plans need no model call.
3. Show readiness, the lead-authorized summary of intended contributions and
   authorized resource costs. Do not expose private step payloads or
   discoveries merely because a plan exists. Resolve known conflicts or
   explicitly defer a contribution.
4. The human commits the Activity Turn, including explicit acknowledgement of
   exactly which starting crew members the prepared extraction would leave
   behind.
5. Apply the personal move and at most one eligible step per companion, then
   show discoveries, changed routes, spent power and remaining turns.

The game must never auto-commit the next turn. Reading, viewing evidence,
editing local drafts and waiting for AI responses do not advance danger.
Canonical planning Actions may still be recorded; an Activity Turn is not a
kernel Transition.

### Resolution rules

- Use the state at the beginning of the turn to validate each step's physical
  prerequisites. Opening a gate and traversing that newly opened gate require
  successive turns; actor ordering must not create a hidden shortcut.
- Reserve scarce resources and unique interactions before applying effects.
  If prepared work conflicts, require a choice or explicit deferral before
  commitment. Do not silently spend power on one actor and strand another.
- Apply accepted independent effects in a fixed documented Role order:
  lead, Mira, Jonah. Roles are resolved from current Core Memberships.
- Record the originating plan/task for each companion effect. These are
  consequences of accepted plans, not fabricated new agent Actions.
- Increment the turn count once. Evaluate extraction and then the turn limit.
  An extraction committed as turn 16 can succeed; unextracted crews do not
  receive an unannounced extra turn.
- Extraction uses post-resolution Locations. Preparation previews that
  extracted set, including eligible companion returns on this turn. A changed
  contribution invalidates the preview and requires preparation again before
  the lead can acknowledge any crew left behind.
- Cancelled, stale, ineligible or out-of-limit steps cannot execute. Missing
  contributions require explicit continuation or an already chosen basic
  movement mode; they never become invented successes.

A plan retains its origin Membership, task revision and registration
provenance. Each selected step is separately fenced to the current preparation
and Activity Turn; advancing a turn does not invalidate eligible remaining
steps. A source-role change or standing change invalidates execution
eligibility. No mutable copy of Core Role ownership is maintained in Activity
State.

## Interaction and information

Use a responsive illustrated map, location scenes, contextual action cards,
an evidence board and compact crew task cards. The first useful action should
be obvious without an explanation of WorldStream or a typed identifier.
Known cost and consequence information belongs next to the action.

All required play is possible through direct controls. Optional conversation
supports advice and task expression; it is not a mandatory chat wall. A typed
request can produce a proposed task/action card, but game consequences still
require its legal recorded action and the normal turn commitment.

Known evidence stays available with its source. Clearly distinguish an
observed fact, a character's recommendation and something still unknown.
Preserve uncertainty without presenting fabricated observations as facts.
Mira's assay and the catalog verifier produce instrument evidence only when
their game actions complete.

Keep task limits, commitments, inventory, important discoveries and plan
progress as structured Activity State. Retain a bounded recent dialogue
window for the authorized audience; full accepted utterances remain in
Canonical History under their original visibility. Private motives or facts
are shared only through permitted disclosures. Companions receive their
authorized projection, never a dump of hidden world truth.

Build the browser client for desktop and phone layouts from the start, using
tap targets, a compact map, and one primary action at a time. A native app,
offline gameplay and push notifications are later work. Re-entry synchronizes
the same Membership and current facts before enabling Actions.

## AI execution and unavailable responses

Use the current provider allowance as the initial ceiling: at most ten
attempts per companion assignment, one in flight, and the existing timeout and
size caps. Plans reduce call count; they do not create additional allowance.
Advice uses that same allowance. Do not dispatch on every websocket frame,
every UI render or every routine movement.

Current House accounting measures the serialized request and reply using
conservative UTF-8 byte units: the per-call ceilings currently correspond to
12,000 input bytes and 1,000 reply-content bytes. Keep plans to three compact
steps and short optional dialogue. Use the supported closed schema subset;
objects, arrays and enums are supported, but schema references and unions are
not. Check the full provider request, including policy and schemas, during
qualification.

A new plan is requested on task creation, meaningful blocking or plan
exhaustion. Use one active planning opportunity at a time so independently
computed replies do not race on the same Room Head. State-changing edits
cancel that opportunity before a replacement is prepared. The client may
keep local drafts while waiting.

Opportunity expiry alone does not trigger another model call. A failed
opportunity stays unavailable until an explicit new request or meaningful new
task/game information. Start the next response window only when its bounded
execution can begin.

Set a 15-second Pack response opportunity for the prototype. This is separate
from the provider's existing maximum call timeout. Expiry closes the game
opportunity without advancing danger; a late provider reply is fenced and its
attempt remains charged. Do not start another call while the old one remains
in flight. Validate this timing against the selected reviewed model route
rather than claiming it is already a good latency experience.

The human can explicitly continue without a missing contribution or use the
disclosed regroup/follow controls. The Pack permits these based on recorded
game/task state and human choice, not a browser's assertion that a provider
failed. No hidden model swap, unlimited retry, new Principal or replacement
assignment is introduced. Basic movement is visibly distinguished from new
AI reasoning. If the allowance cannot support the mission, first revise task
granularity and call triggers; do not silently raise spending caps.

See [ADR 0028](adr/0028-record-bounded-companion-plans-in-activity-state.md).

## Outcomes, mistakes and return

Full success means the actual ledger and the entire starting crew have been
extracted. Track the optional objectives independently. Returning with a
counterfeit, returning without a ledger, or extracting only part of the crew
has a clear partial outcome. Remaining inside when the turn budget is spent
is a failed expedition. Explain the facts that produced the result.

Ordinary mistakes spend visible time or power and can force a new route or
retreat. A failed hypothesis does not require an automatic restart.
Known irreversible risks are shown before commitment. Hidden truth can still
make a choice uncertain; the interface must not leak it while pretending to
warn fairly. Power spent and accepted consequences are not secretly restored.

The debrief shows outcome facts, turns, power, optional objectives and
attributable companion contributions. There is no universal numeric score or
leaderboard. Recommendations are not counted as completed work.

Closing a tab neither advances danger nor frees current hosted capacity.
Use existing same-Membership re-entry. For the limited first hosted preview,
retain active expeditions for a disclosed 24-hour window from start; expiry
ends the session without manufacturing a gameplay loss or success. Implement
that as an explicit Pack terminal expiry with no Outcome, observed by the
existing terminal-evidence/capacity path. Do not infer completion from a
disconnected browser. The expiry policy needs its own replay and deadline-race
checks and must be visible before launch.

The 24-hour expiry applies only while the expedition is nonterminal. Any
terminal transition cancels outstanding response and expiry timers and
preserves any established Outcome.

This deliberately avoids adding resumable Runner suspension in the first
build. The current single-host capacity remains a preview limitation, not a
consumer-scale hosting claim. Durable identity and retained accounting must
survive supported restart; terminating an execution process never resets
allowance.

## Variation and content upkeep

Standard and Low Reserve are now closed authored configurations. Low Reserve
starts with two initial power charges while Standard starts with three; both
keep the same map, rules, and sixteen-turn budget. In the solo route model, powered
verification plus the ordinary service hatch is feasible in Standard and
infeasible in Low Reserve; other legal combinations remain. Both optional
objectives require more careful authentication choices in Low Reserve.

That is a useful constraint variation, not proof of an endlessly replayable
game. A memorized safe route may remain valid. Measure whether players change
decisions and voluntarily retry before investing in a generator.

Use authored, validated candidate/evidence combinations. Once the first
level works, add several consistent configurations with changed power, initial
access or evidence placement, each with legal witness routes. Freeze every
run's configuration at Genesis. Never reroll a hidden fact after a player
discovers it or let a narrator repair an impossible world.

An author release includes rules, scenario data, client compatibility and
conformance evidence. Rule changes get a new Pack Revision; reviewed
configuration changes get new exact launch/listing intent. Existing runs
retain their original rules and inputs. Review new scenarios against state,
privacy, route and resource checks before publishing. Choose a release cadence
from demonstrated authoring and review capacity, not a promise of daily
generated missions.

## Actual integration work

The core state/action model fits. Adding this game is nevertheless more than
placing a new JSON file in the catalog.

| Remaining work | Current source finding |
| --- | --- |
| Deployed model companions | Reviewed Mira/Jonah policies, exact House identities, bounded v4/v5 current-context adapters, failure continuation, and local provider-boundary evidence exist; deployed activation and external-provider latency/quality remain unproved |
| Common-platform activation | The authenticated unlisted Listing, exact dependency gate, four roster choices, and v13 client are locally wired; a deployed Host journey remains |
| Hosted return journey | Formation, private terminal status, expiry, and capacity reuse have composed local evidence; restart/re-entry and phone behavior still need candidate-level deployed acceptance |
| Publication and results | The candidate remains unlisted with public result publication disabled; the exact projector records minimal terminal facts without exposing private clues |
| Player evidence | Scripted conformance proves rules and Replay; it does not establish comprehension, enjoyment, session length, or replay motivation |

Extend the public TypeScript authoring tools backwards compatibly. Keep
legacy source forms readable and preserve retained artifacts. Supply explicit
configuration/action/state/view schemas, optional Role cardinality, start
metadata, bounded offer timing, actual privacy mutations and external-input
golden steps. Do not bypass strict validation or embed Archive into a
game-specific kernel branch to avoid these changes.

Use a Pack-neutral declared Activity Start Contract through the existing
ExternalInput-to-reduce path. Add reviewed Roster Options for solo, Mira,
Jonah and both. Freeze the selected option before claims; the server derives
exact seat/configuration choices. The browser receives no arbitrary Role,
prompt, model or setup authority. This is a versioned formation and
House-selection extension, governed by
[ADR 0027](adr/0027-declare-generic-hosted-starts-and-reviewed-roster-options.md).

Keep named companions free during the experiment and retain exhibition
disclosure and the existing allowance. Each run has new scoped assignments;
a returning character name does not imply cross-run memory. Initially disable
anonymous viewing and public result publication for Archive. Personal results
remain available as a debrief in the re-enterable Activity Client's terminal
Participant Projection. My Games supplies entry and status, not a private
result index: current result storage supports only public publication. Do not
add a private result-index extension in this first build. The exact Result
Projector still interprets a minimal Public Projection for terminal/capacity
evidence without exposing private clues. Richer sharing is a separate reviewed
publication choice.

Source references:
[SDK artifacts](../sdk/typescript-pack/packages/pack-cli/src/artifacts.ts),
[offer normalization](../sdk/typescript-pack/packages/pack-cli/src/component.ts),
[conformance](../sdk/typescript-pack/packages/pack-cli/src/conformance.ts),
[production proof](../sdk/typescript-pack/packages/pack-cli/src/toolchain.ts),
[server catalog](../crates/worldstream-server/src/lib.rs),
[Heist start recognition](../crates/worldstream-core/src/agent_heist_registry.rs),
[hosted contract](../sdk/typescript-hosted-contract/src/index.ts),
[result storage](../supabase/migrations/20260905222000_replay_verified_activity_results.sql),
[My Games result status](../supabase/migrations/20260908190000_my_games_result_status.sql).

## Build sequence and gates

1. **Authoring/start prerequisites and a minimal fixture.** Extend SDK/CLI
   metadata, conformance and the declared start path in the server/Host
   backends without changing the five callback boundary. Prove one
   human, human+Mira, human+Jonah and all three. Preserve Negotiate and retained
   Heist compatibility. Follow the public Pack path in
   [Negotiate](../packs/negotiate/README.md). The current pack:prove receipt
   requires at least two Roles and two private views: prove the full-party
   Pack through that lane and cover every roster through dedicated conformance
   cases. Do not weaken the production proof to make a solo receipt pass.
2. **Local rules slice.** Create the proposed packs/midnight-archive source
   project. Implement Standard, all required solo methods, typed task/plan
   rules, turn resolution, outcomes and deterministic scripted companion
   policies. Qualify replay, privacy, cancelled plans, conflicting resources
   and no-response continuation against the real Component Host.
3. **Playable client.** Create the proposed clients/midnight-archive-web
   project using existing scoped client/session infrastructure. Deliver the
   map, scenes, action cards, evidence board, task controls and debrief.
   Validate a full solo journey and scripted companion journeys in a browser,
   including reset/re-entry and phone layout.
4. **Bounded LLM companions and variation.** Add reviewed policies, compact
   plans and concise dialogue. Exercise actual request/reply size, stale-head
   handling, timeout, cancellation and allowance exhaustion. Add Low Reserve.
   Validate live model usefulness separately from scripted rules correctness.
5. **Second hosted listing.** Complete platform wiring for the declared start
   path and reviewed roster formation. Publish exact Pack/client artifacts,
   conformance, approved bindings, listing, public schema and result projector
   through existing config/activity-clients and config/hosted lanes. Generalize
   catalog and projector registration. Qualify formation, readiness,
   restart/re-entry, terminal retirement, expiry, terminal client debrief and
   repeated games, with public result publication disabled.
6. **Heist application and extraction.** Apply proven objective presentation,
   contextual controls, evidence/disclosure, task/status and debrief patterns
   to a separately scoped Heist improvement. If Heist should execute the
   chosen plan, design that as a new gameplay revision. Extract libraries only
   around an actual second use, not a speculative universal game engine.

Mandatory game evidence includes both authentication methods, both access
methods, all four party configurations, optional-objective routes, wrong
ledger and partial extraction, reachable escape after ordinary setbacks,
invalid actions, resource conflicts, task cancellation, missing/late replies
and no leakage of private clues. Planned steps must stop after task or
participant-eligibility changes. Projection Reset replaces current views;
re-entry does not repeat uncertain actions. Validate actual historical and
current audiences, not a generic dummy privacy field.

Pack check/test/build/inspect and production Component proof are necessary
but do not establish client or hosted qualification. Use the documented
[client release lane](activity-clients.md) and existing hosted recovery,
result and repeat-play gates. No production rollout or paid provider run was
performed during this design pass.

The first playtest gate is qualitative evidence from five target-audience
players: they can state the goal and find the first action unaided; most
complete or knowingly retreat in roughly the target session length; companion
users can name a concrete useful contribution and a decision they retained;
at least two materially different approaches appear; a replay invitation
produces a specific reason to try something different. Record failures and
revise the level before expanding content. Five playtests are directional
evidence, not statistical retention validation.

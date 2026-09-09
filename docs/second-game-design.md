# Second game design interview

Status: Design interview in progress; updated 2026-09-09. The direction below is
accepted; the proposed game and unanswered decisions are not an implementation
specification or an approved release plan.

## Accepted direction

The user wants one small second game on the existing Hosted Activity Platform,
followed by extracting useful shared components from demonstrated gameplay.
The intended audience is people interested in both technology/agents and
games. The experience should support a bounded simulated situation with
multiple ways to achieve a clear objective and agents that make useful
contributions. The user wants candid challenges to weak ideas and a design
interview before implementation.

The user accepted the round 1 recommendations on 2026-09-08:

- The human primarily explores and solves problems personally, with companions
  investigating and helping carry out decisions.
- The first playable supports one human with zero, one or two AI companions.
  It includes a complete solo baseline; human friends and mixed human/AI
  parties follow after this first loop works.
- An ordinary complete first playthrough targets 15–20 minutes. This is not a
  decision to impose a real-time countdown.

The user accepted the round 2 recommendations on 2026-09-08:

- The first world is Midnight Archive: recover an authentic ledger and get the
  crew out of a locked archive.
- Pressure advances through player-committed Activity Turns with a visible
  budget. Reading existing clues and model response time do not spend turns.
- AI Companions execute work within explicit Companion Task limits and bring
  trade-offs outside those limits to the human. Permission already included in
  a task need not be requested again for each routine step.

The user accepted all round 3 recommendations on 2026-09-09:

- The first archive has five Locations and three interacting systems:
  evidence authenticity, access, and limited power.
- The crew cooperates on the main objective. Known professional priorities
  create disagreements and optional trade-offs; hidden betrayal and arbitrary
  sabotage are outside the first experiment.
- An ordinary Activity Turn combines one meaningful personal move with a
  bounded step from each AI Companion's standing task, followed by the human's
  explicit commitment. Task directions persist until changed, completed or
  blocked, and results are shown before the next commitment.

Agent Heist is currently unsatisfactory to the user. This interview must
identify lessons that can improve it, without assuming that its present rules
are the template for the second game. The existing platform stabilization scope
and this gameplay design remain distinct; no release baseline or existing
implementation has been changed by this interview.

## Selected world: Midnight Archive

A short, illustrated escape adventure in a fictional private archive. The
accepted primary mission is to recover an authentic ledger and get the crew
out. A security purge remains a proposed explanation for the turn budget; its
exact consequences and the detailed victory conditions are not yet settled.

The accepted experience centers on personal exploration and problem-solving
with useful companion participation. The loop combines a personal move with
companion work, then a player-committed Activity Turn and visible results. The
proposed mission adds comparing discoveries and negotiating a useful agreement
to open routes. The core hypothesis is that another
participant's discoveries and preferences can make the human's decisions more
interesting while leaving the human active in the world.

Accepted core scope:

- Five Locations with evidence authenticity, access, and limited power as the
  three interacting systems.
- A cooperative crew with known priorities and persistent, bounded task
  directions. Hidden betrayal and arbitrary sabotage are excluded.
- One meaningful personal move and companion task steps per player-committed
  Activity Turn. Exact costs, readiness and conflict handling are not settled.

Remaining candidate details, subject to the interview:

- Five connected locations presented through a small map and illustrated
  scenes, with evidence, inventory, an objective and contextual Actions.
- Two possible AI crew members: Mira, a cryptanalyst who checks evidence, and
  Jonah, a former security consultant with useful local knowledge.
- One non-crew character, an archivist whose preservation priority can affect
  cooperation. Whether this character needs a Runner or bounded authored
  behavior is an open design decision.
- Agreements can change access and resource choices within the accepted
  systems; the conditions and costs still need definition.
- Several achievable approaches through combinations of those rules. A
  technical route and negotiated access are examples, not three hard-coded
  scripts to implement.
- A fixed authored starting situation for initial playtests. Variants follow
  after different approaches actually work. Randomization alone is not evidence
  of replay value.

An illustrative moment: Mira finds conflicting evidence about which ledger is
authentic; Jonah learns that the archivist will grant access after the crew
secures a threatened collection; the player discovers a costly alternate
route. The player must decide which lead to investigate and what to spend.
Those discoveries must arise from game facts and accepted Actions. This is a
design illustration, not a promised sequence that must occur in every run.

The experiment fails its purpose if the human's best move is consistently
"finish the mission for me," if conversations leave the situation unchanged,
or if success depends on guessing a hidden identifier or the author's exact
wording. Detailed success measures will be settled after the intended player
experience is chosen.

## Facts informing the interview

### Heist gameplay

The currently selected Heist 0.3 rules expose clue inspection/disclosure,
exchanges, plan proposals, endorsements/challenges and sealed commitments.
Resolution selects a plan with two matching commitments and scores route,
entry window, tool, extraction and a contribution flag. There is no simulated
execution of the selected heist in this ruleset.

The source contains three fixed answer tuples. Because the components
uniquely identify the tuples, learning them permits inferring the full answer
from one component. That replay limitation is an inference from the fixtures,
not a finding from a player study. Current client forms also expose text
fields for clue identifiers and claim codes. Together these are concrete
reasons to improve the experience beyond presentation alone.

Sources:

- [Heist action definitions and fixtures](../crates/worldstream-core/src/agent_heist_clock_safe.rs).
- [Heist interaction forms](../clients/agent-heist-web/src/AgentHeistClientView.tsx).
- [Platform stabilization baseline](activity-platform-design.md).
- [Dated hosted evidence](hosted-preview-status-2026-09-07.md).

These are source-level findings. No production gameplay or recovery
qualification was performed for this interview. Unshipped Heist 0.4 attention
changes should not be mistaken for a systemic gameplay redesign.

### Architecture and language

The game can use one Room for its entire authoritative situation. Physical
locations within the fiction are Activity State; the word "world" in product
copy does not introduce another WorldStream authority. Existing canonical
terms remain defined only in [CONTEXT.md](../CONTEXT.md).

The existing Pack boundary supports state, Roles, Actions, visibility,
transitions and Outcomes. World generation, solvability checking, character
rules and game balance are additional game/authoring work. LLM execution stays
in external Runners; the Pack validates the consequences of proposed Actions.

Current House Agent fill is an exhibition service with a bounded allowance,
random approved selection, one proposed Action per Invocation and no retained
cross-Invocation conversation. Selected companions, dialogue context and
delegated work require explicit design and qualification against that service's
current scope. Multi-turn task continuity can use recorded task facts in each
authorized Projection; it does not automatically require a separate private
memory system. A kernel rewrite has not been identified as necessary for this
bounded experiment, but implementation and qualification have not been
performed.

Governing references:

- [Activity Pack contract](activity-packs.md).
- [Independent Activity Clients](adr/0017-separate-activity-clients-from-packs-and-studio.md).
- [Hosted Activity Platform ownership](adr/0019-separate-hosted-activity-platform-from-worldstream.md).
- [Formation and fixed roster](adr/0021-use-reviewed-pre-genesis-formation-for-hosted-activity-runs.md).
- [Existing House Agent boundary](adr/0022-use-bounded-openrouter-house-runners-for-exhibition-fill.md).

### Turn feasibility and remaining constraints

A read-only contract check found no kernel blocker for recording participant
intentions in Activity State and resolving their consequences in a later,
bounded reduction. Each submitted intention still belongs to its actual
Membership; a resolving Action must not pretend to be fresh Actions submitted
by other participants. The Pack can associate intentions with their intended
turn and task revision.

A waiting-for-player phase can omit gameplay timers. Bounded model requests
can finish or fail without advancing danger. This is not a new Room pause
status: already-admitted Actions or model work may still complete after a
browser closes. Active-Run and House capacity also remain occupied under
current hosted policy. Idle capacity release and later restoration require a
separate delivery decision.

Concurrent model replies still face strict current-Head admission; the Runner
integration must synchronize and handle stale decisions. It cannot assume
that unrelated-looking actions from the same prior Head will both be accepted.
Turn resolution order, incomplete intentions, cancellation and provider-failure
continuation remain open design decisions. The Pack must not call or wait for
a model inside reduction.

Sources: [Pack application and attention](activity-packs.md),
[separate activation and Action authority](adr/0003-separate-activation-and-action-authority.md),
[re-entry behavior](activity-platform-design.md), and
[hosted formation/capacity policy](adr/0021-use-reviewed-pre-genesis-formation-for-hosted-activity-runs.md).

## Design tree

Each round asks only decisions whose prerequisites are settled. The branches
below are a working map and will change with the user's answers.

| Branch | Prerequisites | Decision work | Status |
| --- | --- | --- | --- |
| A. Human experience | Accepted audience and second-game direction | Human explores and solves with useful companions | Q1 accepted |
| B. First party | Accepted second-game experiment | One human with zero, one or two AI companions; true solo baseline | Q2 accepted |
| C. Session budget | Accepted audience and second-game direction | 15–20-minute ordinary first playthrough | Q3 accepted |
| D1. World premise | A, C | Midnight Archive; recover the authentic ledger and escape with the crew | Q4 accepted |
| D2. Core obstacles | D1, E1, F1 | Five Locations; evidence authenticity, access and limited power | Q7 accepted |
| D3. Mission structure | D2, E2, F2 | Combine authentication and access approaches before extraction | Q10 open |
| E1. Pressure | A, B, C | Player-committed Activity Turns and a visible budget | Q5 accepted |
| E2. Ordinary turn | D1, E1, F1 and turn feasibility check | One meaningful personal move plus companion task steps; human commits | Q9 accepted |
| E3. Costs and recovery | D3, E2, F3, G1 | Exact action costs, solo budget, incomplete turns, interruption and departure behavior | Later round |
| F1. Companion autonomy | A, B | Execute within Companion Task limits; escalate trade-offs outside them | Q6 accepted |
| F2. Social posture | D1, E1, F1 | Cooperative crew with known professional priorities; no hidden betrayal | Q8 accepted |
| F3. Companion advantage | A, B, D2, E2, F2 | Parallel specialist work, costs and accessible solo alternatives | Q11 open |
| F4. Character interaction | D3, F3 | Exact abilities, private knowledge, dialogue, agreements and disagreements | Later round |
| G1. Mistakes and stakes | A, C, D1, E1, E2, F2 | Recovery, risk disclosure and partial outcomes | Q12 open |
| G2. Variation and fairness | D3, E3, F4, G1 | Initial solvability, discoverability, repeat play and dominant strategies | Later round |
| H. Runner service | E2, E3, F4 | Activation, authorized dialogue context, bounded work, cost and provider-failure behavior | Later round |
| I. Sharing with Heist | D3, E3, F4 and source audit | Concrete transferable components and the separate scope of Heist gameplay changes | Later round; source audit complete |
| J. Delivery and validation | D3–I | Client scope, listings, content revisions, idle/resume policy, playtest evidence and smallest buildable slice | Later round |
| K. Shared understanding | All active branches resolved | Confirm the complete design before implementation | Pending |

### Round 1: accepted

The user answered "Accept recommendations."

**Q1 — Player experience.** Accepted: personally exploring and solving
problems with companions investigating and helping carry out decisions. Crew
command and conversation can support this activity, but neither replaces it
as the primary human experience.

**Q2 — First playable party.** Accepted: one human with zero, one or two AI
companions. Solo must be a complete mode, and adding companions must be tested
for actual value. Human friends and mixed human/AI parties are subsequent
scope. This decision does not yet settle character selection, companion
abilities, resource budgets, or solo balancing rules.

**Q3 — Session length.** Accepted: a 15–20-minute target for an ordinary
complete first playthrough. It does not establish a countdown, number of
turns, or limit on reading time.

### Round 2: accepted

The user answered "Accept."

**Q4 — First world and objective.** Accepted: Midnight Archive, a fictional
archive escape mission whose main goal is to recover an authentic ledger and
get the crew out. This settles the premise without approving every
illustrative scene or fixing all outcome rules.

**Q5 — Pressure.** Accepted: player-committed Activity Turns with a visible
budget. Reading known evidence and waiting for a model do not spend turns;
in-world work does. The player deliberately advances the situation. Exact
costs, the number of turns, resolution and failure behavior remain later
decisions.

**Q6 — Companion autonomy.** Accepted: AI Companions perform useful work within
explicit Companion Task limits and bring trade-offs outside those limits to
the human. A general order to find a route does not silently permit spending
the last shared power cell; an explicit allowance to spend that cell can
authorize it. Personality and disagreement remain separate decisions.

These are gameplay decisions. They do not amend the existing House Agent
service contract or authorize implementation before the shared-understanding
checkpoint.

### Round 3: accepted

The user answered "accept all" on 2026-09-09.

**Q7 — Core obstacles.** Accepted: one authored five-Location archive with
evidence authenticity, access, and limited power. Moving guard patrols are
outside this first obstacle mix. This settles the systems to test, not their
exact costs, map connections or proof of solvability.

**Q8 — Social posture.** Accepted: the crew shares the main objective; known
professional priorities shape advice and optional trade-offs. Companions can
disagree without hidden betrayal or arbitrary sabotage. Exact abilities,
private clues and dialogue implementation remain open.

**Q9 — Ordinary turn.** Accepted: the player chooses one meaningful personal
move, companions prepare bounded steps under their standing tasks, and the
player commits the Activity Turn. Task directions persist until changed,
completed or blocked, and results are shown clearly before the next
commitment. Exact order, resource conflicts, missing intentions and whether a
move has a multi-turn cost remain downstream decisions.

### Round 4: open questions

The current frontier is mission structure (D3), the form of companion advantage
(F3), and the treatment of mistakes (G1). These can be decided from the
accepted world, primary objective, turn model and cooperative social posture.
Exact costs, clue content, dialogue and recovery procedures depend on these
decisions.

**Q10 — Mission structure.** Recommendation: make authentication and access
independent problems, each with more than one method, then require recovery of
the actual ledger and crew extraction. Authentication can use connected
evidence or a powered analysis tool. Access can follow a negotiated agreement
or a powered service route. Methods may be combined and tackled in a legal
order chosen by the player; the Pack checks resulting facts rather than a
scripted itinerary. A candidate map has an entry/exit, records room,
conservation hall with the archivist, plant room, and vault. The intended
trade-off is that negotiation costs time or a favor while a technical route
spends scarce power. Exact topology, costs and clue prerequisites require
validation; the four method combinations are a design target, not a proven
balanced solution set. Alternative: a linear escape-room sequence of puzzles.

**Q11 — Companion advantage.** Recommendation: essential jobs remain
achievable solo; specialists contribute parallel work and efficient methods
with explicit costs or conditions. Mira examines and correlates evidence;
Jonah investigates access and prepares route options. They receive their
authorized observations and can make recommendations from them, including a
possible solution when they have enough evidence. Their usefulness must not
depend on access to the hidden answer or deliberately withholding a solution.
More help may make the main mission easier; useful optional objectives can
reward spare capacity. Keep core rules consistent while qualifying suitable
turn budgets for each party size. Alternative: equal per-job abilities, with
companion differences mainly in decision style and parallel execution. Exact
skill effects, budgets and social dialogue are later details.

**Q12 — Mistakes and stakes.** Recommendation: ordinary mistakes consume
visible time/resources and usually permit another approach or retreat. Clearly
signal known irreversible risks, while preserving uncertainty about facts the
party has not learned. Escape without the authentic ledger is a distinct
partial result; loss of the primary objective or failure to extract can end
the mission with an explanation. Do not silently restore spent resources or
reroll outcomes. Alternative: a strict challenge where any important mistake
requires a restart. Provider failures and unavailable companion steps are
operational cases to handle separately, not fictional player mistakes.

Round 4 answers are pending. Recommendations do not count as accepted
decisions.

## Candidate reuse to test

Possible shared components include objective/evidence presentation,
contextual Action controls, disclosure, consequence-bearing agreements,
companion task/status presentation, and an understandable debrief. Shared
platform lifecycle and scoped participation already have their own contracts.

The experiment should provide evidence before a common gameplay library is
extracted. Heist's current fixed phase schedule, majority requirement and
five-check scorer are not assumed to be general-purpose game mechanics.

## Documentation discipline

Record confirmed design decisions in this document as each round resolves.
Add resolved domain terms directly to the root glossary when needed; do not
create a second glossary in this proposal. Round 2 established AI Companion,
Companion Task, Activity Turn and Location as useful domain distinctions;
their definitions are recorded in the root glossary. They are gameplay
concepts and do not add fields or authority to Core Room State.

Create an ADR only for a consequential architectural trade-off that is hard
to reverse and needs its rationale preserved. The working title, initial
location count and playtime target do not need ADRs. This interview has not
yet made a new architectural decision or amended an accepted ADR.

Earlier exploration: [companion activity research](companion-activity-pack-research.md)
and [gameplay precedents](companion-gameplay-precedents-research.md).

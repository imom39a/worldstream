# Second game design decisions

Status: Design interview closed, 2026-09-09. Implementation and qualification
are tracked in IMO-197 through IMO-214.

The user accepted rounds 1–4, then directed: “please use recommended for all
grilling questions.” Remaining branches were resolved using the recommended
answers under that delegation. No further interview or confirmation round is
required for this design.

The complete implementation specification is
[Midnight Archive](midnight-archive-design.md). The
[route-check note](midnight-archive-route-check.md) records the limited
mathematical checks performed during design.

## Accepted direction

Build one small second game on the existing Hosted Activity Platform, then
extract useful shared components from demonstrated gameplay. The audience is
people interested in both technology/agents and games. The game must have a
clear objective, a bounded simulated situation, multiple legal approaches and
companions that make useful contributions.

The user wants candid criticism. Agent Heist is currently unsatisfactory to
them; its current gameplay is not the template for the new game. Existing
Heist stabilization and gameplay redesign remain separate work.

## Directly accepted decisions

| Question | Accepted answer | Accepted |
| --- | --- | --- |
| Q1 — Human experience | Personal exploration and problem-solving with useful companion work | 2026-09-08 |
| Q2 — First party | One human with zero, one or two AI Companions; complete solo baseline; human friends later | 2026-09-08 |
| Q3 — Session target | 15–20-minute ordinary first playthrough; not a countdown requirement | 2026-09-08 |
| Q4 — World | Midnight Archive; recover the authentic ledger and extract the crew | 2026-09-08 |
| Q5 — Pressure | Player-committed Activity Turns with a visible budget; reading and model waits do not spend turns | 2026-09-08 |
| Q6 — Autonomy | Execute within explicit Companion Task limits; escalate trade-offs outside them | 2026-09-08 |
| Q7 — Obstacles | Five Locations; evidence authenticity, access and limited power | 2026-09-09 |
| Q8 — Social posture | Cooperative crew with known priorities; no hidden betrayal or arbitrary sabotage | 2026-09-09 |
| Q9 — Ordinary turn | One meaningful personal move plus companion task steps; standing directions persist | 2026-09-09 |
| Q10 — Mission structure | Combine evidence/powered authentication with agreement/service access, then recover and extract | 2026-09-09 |
| Q11 — Companion value | Parallel work and efficient specialist methods; essential solo alternatives remain | 2026-09-09 |
| Q12 — Mistakes | Recoverable setbacks, visible costs and distinct partial outcomes | 2026-09-09 |

Rounds 1–3 were accepted in text. Round 4's three recommended options were
selected explicitly through the question interface.

## Remaining recommendations applied under delegation

| Decision | Adopted recommendation |
| --- | --- |
| Q13 — Interface and dialogue | Responsive map, scenes, contextual controls, evidence board and task cards; optional text; no required identifier entry or chat wall |
| Q14 — Character methods | Mira has a two-step field assay; Jonah has a cheaper service-hatch method; the lead makes binding agreement, ledger and extraction choices; archivist initially uses authored conditional behavior |
| Q15 — Task continuity | Record a maximum-three-step Companion Plan from one authenticated agent Action; revalidate and apply at most one step per turn; stop for changed facts or limits |
| Q16 — Turn conflicts | Beginning-of-turn physical preconditions; reserve scarce resources; require known conflicts to be resolved/deferred before commitment; fixed effect order |
| Q17 — AI response and cost | Serialized bounded planning, existing ten-attempt ceiling, compact byte-bounded replies, 15-second Pack opportunity; explicit skip/regroup without fabricated success or provider substitution |
| Q18 — Initial balance | Standard starts with 16 turns and three charges; same initial core budget across party sizes; optional preservation and source protection; tune using actual play evidence |
| Q19 — Variation | Standard first, then Low Reserve with two charges; authored consistent configurations and witnessed routes; no claim of endless replay |
| Q20 — Outcomes and return | Full and partial outcomes from actual facts; explicit partial-extraction acknowledgement; re-entry to the same run; disclosed 24-hour preview session expiry without a fabricated gameplay result |
| Q21 — Integration | Extend public authoring metadata/schemas/conformance and the generic hosted start path; add reviewed pre-Genesis roster choices; keep the five callback and authority boundaries |
| Q22 — Release and reuse | Local real-Room/scripted proof, client, capped LLM proof, then second hosted listing; free companions, private terminal-client debrief, no billing or new private result index; extract for Heist only around a demonstrated second use |

These are recommended design choices adopted through delegation, not claims
that the user separately reviewed every technical detail. Initial tuning
values are concrete implementation defaults; they remain subject to the
specified playtest gates and immutable release/version rules.

## Resolved design tree

| Branch | Resolution |
| --- | --- |
| A — Human experience | Q1 |
| B — First party | Q2 |
| C — Session budget | Q3 and Q18 |
| D — World, obstacles and mission structure | Q4, Q7, Q10 and the specified map/action costs |
| E — Pressure, turn resolution and recovery | Q5, Q9, Q16, Q17 and Q20 |
| F — Autonomy, skills and social interaction | Q6, Q8, Q11, Q13–Q15 |
| G — Stakes, fairness and variation | Q12, Q18, Q19 and the playtest/conformance gates |
| H — Runner integration | Q15 and Q17; ADR 0028 |
| I — Application to Heist | Q22 and the explicit separate Heist gameplay scope |
| J — Delivery and validation | Q21–Q22, ADR 0027 and the staged build sequence |
| K — Shared understanding | Remaining recommendations delegated by the user; interview closed |

Implementation failures or playtests can produce new evidence and require
revisions. They are not unresolved interview questions being silently left to
the implementer.

## Facts that changed the plan

### Heist is a planning-and-resolution game today

Current hosted Heist 0.3 exposes clue inspection/disclosure, exchanges, plan
proposals, endorsements/challenges and sealed commitments. Resolution selects
a majority plan and scores route, entry window, tool, extraction and a
contribution flag. The rules do not simulate executing the heist.

Its three fixed answer tuples permit a memorized component to identify the
whole tuple. Current forms also expose raw clue identifiers and claim codes.
These source findings support improving the game beyond visual polish.
They do not constitute a live production qualification or a player study.
Unshipped 0.4 attention changes do not establish the proposed world model.

Sources: [Heist rules](../crates/worldstream-core/src/agent_heist_clock_safe.rs),
[client](../clients/agent-heist-web/src/AgentHeistClientView.tsx),
[stabilization scope](activity-platform-design.md).

### Kernel fit does not mean turnkey authoring or hosting

The five-operation Pack contract supports the world and staged turns.
However, the public authoring path currently emits generic object schemas
and mandatory Roles, has narrow offer/start metadata, and uses
negotiation-shaped conformance assumptions. Hosted start and catalog/projector
wiring also contain Heist-specific restrictions. A second game needs the
bounded tooling and launch changes recorded in the final specification.

The initial assessment was too optimistic if read as “just add a Pack.”
No kernel rewrite was identified, but authoring, server/Host integration,
formation and qualification work are real prerequisites.

Sources: [authoring artifacts](../sdk/typescript-pack/packages/pack-cli/src/artifacts.ts),
[conformance](../sdk/typescript-pack/packages/pack-cli/src/conformance.ts),
[start recognition](../crates/worldstream-core/src/agent_heist_registry.rs),
[hosted launch inputs](../sdk/typescript-hosted-contract/src/index.ts).

### Plans can fit within existing execution boundaries

A single offered Action can carry a compact bounded step list. The Pack can
retain and later apply those intentions without calling an LLM or inventing
new companion Actions. Current House limits still apply; byte limits,
stale-head admission, response deadlines and no-response behavior matter.
Recorded task facts can provide continuity without private cross-invocation
memory.

Sources: [Pack contract](activity-packs.md),
[House model boundary](../crates/worldstream-studio-supervisor/src/house_model.rs),
[House decision](adr/0022-use-bounded-openrouter-house-runners-for-exhibition-fill.md).

## Domain and architecture records

The root [CONTEXT.md](../CONTEXT.md) remains the only glossary. It now
distinguishes AI Companion, Companion Task, Companion Plan, Activity Turn,
Location, Activity Start Contract and Roster Option.

Two consequential integration choices warranted ADRs:

- [ADR 0027 — generic hosted starts and reviewed roster options](adr/0027-declare-generic-hosted-starts-and-reviewed-roster-options.md).
- [ADR 0028 — bounded Companion Plans in Activity State](adr/0028-record-bounded-companion-plans-in-activity-state.md).

These record design decisions, not completed implementation, automatic Host
approval or deployed behavior.

Earlier exploration: [companion activity research](companion-activity-pack-research.md)
and [gameplay precedents](companion-gameplay-precedents-research.md).
